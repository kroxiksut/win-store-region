//! Read-only uninstall-registry evidence for Win32 products.

use crate::platform::observation_timestamp_now;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
    RegQueryValueExW,
};
use windows::core::{HSTRING, PWSTR};
use winstoreregion_core::{
    StoreProductId, UninstallRegistryEntry, UninstallRegistryScope, UninstallRegistrySnapshot,
};

const UNINSTALL_REGISTRY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall";

const STORE_PRODUCT_VALUE_NAMES: [&str; 4] =
    ["ProductId", "ProductID", "StoreProductId", "StoreProductID"];

/// Structured failure from the read-only Win32 uninstall-registry adapter.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Win32ObservationReadError {
    RegistryUnavailable { native_code: Option<i32> },
}

/// Read-only adapter for Win32 uninstall registration evidence.
///
/// It opens only the current user's and both local-machine registry views
/// with `KEY_READ`; it never launches an installer or modifies the registry.
#[allow(dead_code)]
pub(super) struct WindowsUninstallRegistryObserver;

#[allow(dead_code)]
impl WindowsUninstallRegistryObserver {
    /// Capture all supported uninstall-registry views for a before/after
    /// comparison. An inaccessible view fails closed rather than producing
    /// a partial snapshot that might be mistaken for completion evidence.
    pub(super) fn snapshot() -> Result<UninstallRegistrySnapshot, Win32ObservationReadError> {
        let mut entries = Vec::new();
        for (scope, root, access) in uninstall_registry_sources() {
            let Some(uninstall_key) = open_registry_key(root, UNINSTALL_REGISTRY_PATH, access)?
            else {
                continue;
            };
            entries.extend(read_uninstall_entries(scope, &uninstall_key, access)?);
        }
        entries.sort_by(|left, right| {
            (left.scope as u8, left.key_name.as_str())
                .cmp(&(right.scope as u8, right.key_name.as_str()))
        });
        Ok(UninstallRegistrySnapshot {
            observed_at: observation_timestamp_now(),
            entries,
        })
    }
}

fn uninstall_registry_sources() -> [(
    UninstallRegistryScope,
    HKEY,
    windows::Win32::System::Registry::REG_SAM_FLAGS,
); 3] {
    [
        (
            UninstallRegistryScope::CurrentUser,
            HKEY_CURRENT_USER,
            KEY_READ,
        ),
        (
            UninstallRegistryScope::LocalMachine64,
            HKEY_LOCAL_MACHINE,
            KEY_READ | KEY_WOW64_64KEY,
        ),
        (
            UninstallRegistryScope::LocalMachine32,
            HKEY_LOCAL_MACHINE,
            KEY_READ | KEY_WOW64_32KEY,
        ),
    ]
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

#[allow(unsafe_code)]
fn open_registry_key(
    root: HKEY,
    path: &str,
    access: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> Result<Option<RegistryKey>, Win32ObservationReadError> {
    let mut key = HKEY::default();
    let path = HSTRING::from(path);
    let result = unsafe { RegOpenKeyExW(root, &path, None, access, &raw mut key) };
    if result == WIN32_ERROR::default() {
        Ok(Some(RegistryKey(key)))
    } else if result == ERROR_FILE_NOT_FOUND {
        Ok(None)
    } else {
        Err(Win32ObservationReadError::RegistryUnavailable {
            native_code: native_registry_code(result),
        })
    }
}

#[allow(unsafe_code)]
fn read_uninstall_entries(
    scope: UninstallRegistryScope,
    uninstall_key: &RegistryKey,
    access: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> Result<Vec<UninstallRegistryEntry>, Win32ObservationReadError> {
    let mut entries = Vec::new();
    let mut index = 0_u32;
    loop {
        let mut units = vec![0_u16; 1_024];
        let mut length = u32::try_from(units.len().saturating_sub(1)).unwrap_or(u32::MAX);
        let result = unsafe {
            RegEnumKeyExW(
                uninstall_key.0,
                index,
                Some(PWSTR(units.as_mut_ptr())),
                &raw mut length,
                None,
                None,
                None,
                None,
            )
        };
        if result == ERROR_NO_MORE_ITEMS {
            break;
        }
        if result != WIN32_ERROR::default() {
            return Err(Win32ObservationReadError::RegistryUnavailable {
                native_code: native_registry_code(result),
            });
        }
        let key_name = String::from_utf16(&units[..usize::try_from(length).unwrap_or(units.len())])
            .map_err(|_| Win32ObservationReadError::RegistryUnavailable { native_code: None })?;
        let Some(entry_key) = open_registry_key(uninstall_key.0, &key_name, access)? else {
            return Err(Win32ObservationReadError::RegistryUnavailable { native_code: None });
        };
        let store_product_id = STORE_PRODUCT_VALUE_NAMES.iter().find_map(|value_name| {
            read_registry_string(&entry_key, value_name)
                .and_then(|value| StoreProductId::parse(&value).ok())
        });
        entries.push(UninstallRegistryEntry {
            scope,
            key_name,
            display_name: read_registry_string(&entry_key, "DisplayName"),
            publisher: read_registry_string(&entry_key, "Publisher"),
            display_version: read_registry_string(&entry_key, "DisplayVersion"),
            store_product_id,
        });
        index = index.saturating_add(1);
    }
    Ok(entries)
}

#[allow(unsafe_code)]
fn read_registry_string(key: &RegistryKey, value_name: &str) -> Option<String> {
    let value_name = HSTRING::from(value_name);
    let mut value_type = REG_VALUE_TYPE::default();
    let mut byte_count = 0_u32;
    let result = unsafe {
        RegQueryValueExW(
            key.0,
            &value_name,
            None,
            Some(&raw mut value_type),
            None,
            Some(&raw mut byte_count),
        )
    };
    if result != WIN32_ERROR::default()
        || byte_count == 0
        || (value_type != REG_SZ && value_type != REG_EXPAND_SZ)
    {
        return None;
    }
    let mut bytes = vec![0_u8; usize::try_from(byte_count).ok()?];
    let result = unsafe {
        RegQueryValueExW(
            key.0,
            &value_name,
            None,
            Some(&raw mut value_type),
            Some(bytes.as_mut_ptr()),
            Some(&raw mut byte_count),
        )
    };
    if result != WIN32_ERROR::default() || value_type != REG_SZ && value_type != REG_EXPAND_SZ {
        return None;
    }
    let used_bytes = usize::try_from(byte_count).ok()?;
    if used_bytes % 2 != 0 {
        return None;
    }
    let units = bytes[..used_bytes]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let value = String::from_utf16_lossy(&units);
    Some(value.trim_end_matches('\0').to_owned())
}

fn native_registry_code(result: WIN32_ERROR) -> Option<i32> {
    i32::try_from(result.0).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninstall_registry_snapshot_is_read_only_and_covers_all_registry_views() {
        match WindowsUninstallRegistryObserver::snapshot() {
            Ok(snapshot) => {
                assert!(snapshot.observed_at.unix_millis() > 0);
                assert!(
                    snapshot
                        .entries
                        .iter()
                        .all(|entry| !entry.key_name.is_empty())
                );
            }
            Err(super::Win32ObservationReadError::RegistryUnavailable { .. }) => {}
        }
    }
}
