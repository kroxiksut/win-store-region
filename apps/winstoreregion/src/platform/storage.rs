//! Atomic per-user files for the recovery record and the journal.

use serde_json::{Value, json};
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows::core::PCWSTR;
use winstoreregion_core::{
    InstalledStoreApplication, JOURNAL_FILE_NAME, JournalFormatError, JournalRecord,
    MarketAvailability, MarketCode, PENDING_RESTORE_FILE_NAME, PackageFamilyName, PendingRestore,
    RecoveryStore, RecoveryStoreError, StoreProductId, UpdateCandidate, journal_records_from_json,
    journal_records_to_json,
};

#[allow(dead_code)]
static NEXT_TEMPORARY_FILE_ID: AtomicU64 = AtomicU64::new(0);

/// Win32 storage adapter for one published `pending-restore.json` file.
#[allow(dead_code)]
pub(crate) struct Win32RecoveryStore {
    pub(crate) directory: PathBuf,
}

#[allow(dead_code)]
impl Win32RecoveryStore {
    /// Create a store rooted at the per-user `WinStoreRegion` data directory.
    pub(crate) fn for_current_user() -> Result<Self, RecoveryStoreError> {
        let directory = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|base| base.join(winstoreregion_core::LOCAL_DATA_DIRECTORY))
            .ok_or(RecoveryStoreError::Read { os_error: None })?;
        Ok(Self { directory })
    }

    #[cfg(test)]
    pub(crate) fn for_test(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub(crate) fn published_path(&self) -> PathBuf {
        self.directory.join(PENDING_RESTORE_FILE_NAME)
    }

    pub(crate) fn create_temporary_file(&self) -> Result<(PathBuf, File), RecoveryStoreError> {
        fs::create_dir_all(&self.directory).map_err(|error| write_error(&error))?;
        for _ in 0..64 {
            let unique = NEXT_TEMPORARY_FILE_ID.fetch_add(1, Ordering::Relaxed);
            let path = self.directory.join(format!(
                ".pending-restore-{}-{unique}.tmp",
                std::process::id()
            ));
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(file) => return Ok((path, file)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(write_error(&error)),
            }
        }

        Err(RecoveryStoreError::Write { os_error: None })
    }

    /// Remove orphaned publish attempts left by a crash.
    ///
    /// A temporary file is never the published record, so this can only
    /// reclaim space and must never gate recovery.
    pub(crate) fn cleanup_temporary_files(&self) -> Result<(), RecoveryStoreError> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(cleanup_error(&error)),
        };
        for entry in entries {
            let entry = entry.map_err(|error| cleanup_error(&error))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".pending-restore-") && name.ends_with(".tmp") {
                fs::remove_file(entry.path()).map_err(|error| cleanup_error(&error))?;
            }
        }
        Ok(())
    }
}

#[allow(dead_code)]
impl RecoveryStore for Win32RecoveryStore {
    fn publish(&mut self, pending: &PendingRestore) -> Result<(), RecoveryStoreError> {
        if let Some(existing) = self.load()? {
            if !existing.has_same_recovery_identity(pending) {
                return Err(RecoveryStoreError::Verification);
            }
        }
        let bytes = pending.to_json_bytes()?;
        let (temporary_path, mut temporary_file) = self.create_temporary_file()?;
        let write_result = temporary_file
            .write_all(&bytes)
            .and_then(|()| temporary_file.flush())
            .and_then(|()| temporary_file.sync_all());
        drop(temporary_file);
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary_path);
            return Err(write_error(&error));
        }

        let published_path = self.published_path();
        if let Err(error) = move_file_write_through(&temporary_path, &published_path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }

        let published = self.load()?.ok_or(RecoveryStoreError::Verification)?;
        if published != *pending {
            return Err(RecoveryStoreError::Verification);
        }
        Ok(())
    }

    fn load(&self) -> Result<Option<PendingRestore>, RecoveryStoreError> {
        let bytes = match fs::read(self.published_path()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(read_error(&error)),
        };
        PendingRestore::from_json_bytes(&bytes).map(Some)
    }

    fn clear_verified(&mut self, pending: &PendingRestore) -> Result<(), RecoveryStoreError> {
        let published = self.load()?.ok_or(RecoveryStoreError::Verification)?;
        if published != *pending {
            return Err(RecoveryStoreError::Verification);
        }
        fs::remove_file(self.published_path()).map_err(|error| cleanup_error(&error))
    }
}

/// Local failures while reading or replacing the operation journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JournalStoreError {
    Read { os_error: Option<i32> },
    Write { os_error: Option<i32> },
    Publish { os_error: Option<i32> },
    Verification,
    Format(JournalFormatError),
    MissingRecord,
}

/// Name of the file the last updates scan is remembered in.
const UPDATES_SCAN_FILE_NAME: &str = "updates-scan.json";

/// Remember the last updates scan, so reopening the window costs no network.
///
/// A convenience and nothing more: the scan is a picture of the catalogue at
/// one moment, and the window says when it was taken rather than pretending it
/// is current. A failure to write is silent — losing a cache costs one button
/// press, and reporting it as an error would be reporting a non-problem.
pub(crate) fn save_updates_scan(market: &MarketCode, taken: &str, candidates: &[UpdateCandidate]) {
    let Some(path) = updates_scan_path() else {
        return;
    };
    let rows: Vec<Value> = candidates
        .iter()
        .map(|candidate| {
            json!({
                "product_id": candidate.product_id.as_ref().map(StoreProductId::as_str),
                "family_name": candidate.application.family_name.as_str(),
                "display_name": candidate.application.display_name,
                "installed_version": candidate.application.installed_version,
                "offered_version": candidate.offered_version,
            })
        })
        .collect();
    let document = json!({
        "version": 1,
        "market": market.as_str(),
        "taken": taken,
        "candidates": rows,
    });
    let Ok(bytes) = serde_json::to_vec_pretty(&document) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, bytes);
}

/// What the last scan found, when it was taken, and for which market.
///
/// Anything unreadable, of another version, or from another market is treated
/// as absent: a remembered answer about a different region would be worse than
/// no answer at all.
pub(crate) fn load_updates_scan(market: &MarketCode) -> Option<(String, Vec<UpdateCandidate>)> {
    let path = updates_scan_path()?;
    let bytes = fs::read(path).ok()?;
    let document: Value = serde_json::from_slice(&bytes).ok()?;
    if document.get("version").and_then(Value::as_i64) != Some(1) {
        return None;
    }
    if document.get("market").and_then(Value::as_str) != Some(market.as_str()) {
        return None;
    }
    let taken = document.get("taken").and_then(Value::as_str)?.to_owned();
    let rows = document.get("candidates").and_then(Value::as_array)?;
    let mut candidates = Vec::new();
    for row in rows {
        let family_name =
            PackageFamilyName::parse(row.get("family_name").and_then(Value::as_str)?.to_owned())?;
        candidates.push(UpdateCandidate {
            application: InstalledStoreApplication {
                family_name,
                display_name: row.get("display_name").and_then(Value::as_str)?.to_owned(),
                installed_version: row
                    .get("installed_version")
                    .and_then(Value::as_str)?
                    .to_owned(),
            },
            product_id: row
                .get("product_id")
                .and_then(Value::as_str)
                .and_then(|value| StoreProductId::parse(value).ok()),
            // The file only ever holds entries that were worth listing, and
            // those are exactly the ones both of these describe.
            availability: MarketAvailability::NotOffered,
            offered_version: row
                .get("offered_version")
                .and_then(Value::as_str)
                .map(str::to_owned),
            offered_elsewhere: true,
        });
    }
    Some((taken, candidates))
}

/// Where the remembered scan lives.
fn updates_scan_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| {
        PathBuf::from(base)
            .join(winstoreregion_core::LOCAL_DATA_DIRECTORY)
            .join(UPDATES_SCAN_FILE_NAME)
    })
}

/// Win32 filesystem adapter for the user-visible `journal.json` history.
pub(crate) struct Win32JournalStore {
    pub(crate) directory: PathBuf,
}

impl Win32JournalStore {
    /// Open the journal below the current user's local application data.
    pub(crate) fn for_current_user() -> Result<Self, JournalStoreError> {
        let directory = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|base| base.join(winstoreregion_core::LOCAL_DATA_DIRECTORY))
            .ok_or(JournalStoreError::Read { os_error: None })?;
        Ok(Self { directory })
    }

    #[cfg(test)]
    pub(crate) fn for_test(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub(crate) fn published_path(&self) -> PathBuf {
        self.directory.join(JOURNAL_FILE_NAME)
    }

    pub(crate) fn create_temporary_file(&self) -> Result<(PathBuf, File), JournalStoreError> {
        fs::create_dir_all(&self.directory).map_err(|error| JournalStoreError::Write {
            os_error: error.raw_os_error(),
        })?;
        for _ in 0..64 {
            let unique = NEXT_TEMPORARY_FILE_ID.fetch_add(1, Ordering::Relaxed);
            let path = self
                .directory
                .join(format!(".journal-{}-{unique}.tmp", std::process::id()));
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(file) => return Ok((path, file)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(JournalStoreError::Write {
                        os_error: error.raw_os_error(),
                    });
                }
            }
        }
        Err(JournalStoreError::Write { os_error: None })
    }

    /// Read all records without mutating malformed or unsupported history.
    pub(crate) fn load(&self) -> Result<Vec<JournalRecord>, JournalStoreError> {
        let bytes = match fs::read(self.published_path()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(JournalStoreError::Read {
                    os_error: error.raw_os_error(),
                });
            }
        };
        journal_records_from_json(&bytes).map_err(JournalStoreError::Format)
    }

    /// Replace the complete validated history atomically and verify it by reading it back.
    pub(crate) fn replace(&self, records: &[JournalRecord]) -> Result<(), JournalStoreError> {
        // A corrupt/unknown history is evidence, not disposable state. Read it
        // first so a later write can never silently erase that evidence.
        let _ = self.load()?;
        let bytes = journal_records_to_json(records).map_err(JournalStoreError::Format)?;
        let (temporary_path, mut temporary_file) = self.create_temporary_file()?;
        let write_result = temporary_file
            .write_all(&bytes)
            .and_then(|()| temporary_file.write_all(b"\n"))
            .and_then(|()| temporary_file.flush())
            .and_then(|()| temporary_file.sync_all());
        drop(temporary_file);
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary_path);
            return Err(JournalStoreError::Write {
                os_error: error.raw_os_error(),
            });
        }

        let published_path = self.published_path();
        if let Err(error) = move_journal_file_write_through(&temporary_path, &published_path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }
        (self.load()? == records)
            .then_some(())
            .ok_or(JournalStoreError::Verification)
    }

    /// Delete only a record that exactly matches the current validated history.
    pub(crate) fn delete(&self, record: &JournalRecord) -> Result<(), JournalStoreError> {
        let mut records = self.load()?;
        let index = records
            .iter()
            .position(|candidate| candidate == record)
            .ok_or(JournalStoreError::MissingRecord)?;
        records.remove(index);
        self.replace(&records)
    }
}

#[allow(unsafe_code)]
fn move_journal_file_write_through(
    source: &Path,
    destination: &Path,
) -> Result<(), JournalStoreError> {
    let source = wide_path(source);
    let destination = wide_path(destination);
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|_| JournalStoreError::Publish {
        os_error: std::io::Error::last_os_error().raw_os_error(),
    })
}

#[allow(dead_code)]
fn write_error(error: &std::io::Error) -> RecoveryStoreError {
    RecoveryStoreError::Write {
        os_error: error.raw_os_error(),
    }
}

#[allow(dead_code)]
fn read_error(error: &std::io::Error) -> RecoveryStoreError {
    RecoveryStoreError::Read {
        os_error: error.raw_os_error(),
    }
}

#[allow(dead_code)]
fn cleanup_error(error: &std::io::Error) -> RecoveryStoreError {
    RecoveryStoreError::Cleanup {
        os_error: error.raw_os_error(),
    }
}

#[allow(dead_code, unsafe_code)]
fn move_file_write_through(source: &Path, destination: &Path) -> Result<(), RecoveryStoreError> {
    let source = wide_path(source);
    let destination = wide_path(destination);
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|_| RecoveryStoreError::Publish {
        os_error: std::io::Error::last_os_error().raw_os_error(),
    })
}

#[allow(dead_code)]
pub(super) fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use winstoreregion_core::{
        DurableOperationState, GeoId, InstallKind, JournalFormatError, JournalProduct,
        JournalRecord, JournalResult, JournalSubject, OperationId, PENDING_RESTORE_FILE_NAME,
        PackageFamilyName, PendingRestore, RecoverySchemaError, RecoveryStoreError, StoreProductId,
        UtcTimestamp,
    };

    static NEXT_TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    fn test_directory() -> PathBuf {
        let unique = NEXT_TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        PathBuf::from(r"C:\temp\WinStoreRegion\recovery-store-tests")
            .join(format!("{}-{unique}", std::process::id()))
    }

    fn pending_restore() -> PendingRestore {
        PendingRestore::prepared(
            OperationId::new("test-operation").expect("non-empty operation identifier"),
            UtcTimestamp::parse("2026-08-15T12:34:56Z").expect("valid UTC timestamp"),
            GeoId::new(244).expect("positive original GeoId"),
            GeoId::new(840).expect("positive temporary GeoId"),
            "9WZDNCRFJ3PZ",
            "test-backend",
        )
        .expect("complete pending restore")
    }

    fn journal_record() -> JournalRecord {
        let product = JournalProduct::new(
            StoreProductId::parse("9WZDNCRFJ3PZ").expect("valid product ID"),
            Some(PackageFamilyName::parse("Contoso.App_1234567890abc").expect("family name")),
            InstallKind::Packaged,
            "Contoso App",
            Some("Contoso".to_owned()),
            Some("1.0.0".to_owned()),
        )
        .expect("complete journal product");
        JournalRecord::new(
            OperationId::new("journal-operation").expect("non-empty operation identifier"),
            JournalSubject::Product(product),
            GeoId::new(244).expect("positive GeoId"),
            UtcTimestamp::parse("2026-08-17T12:34:56Z").expect("valid timestamp"),
            "test-backend",
            JournalResult::Installed,
        )
        .expect("complete journal record")
    }

    fn remove_test_directory(directory: &PathBuf) {
        if directory.exists() {
            fs::remove_dir_all(directory).expect("remove isolated recovery-store test directory");
        }
    }

    #[test]
    fn recovery_store_publishes_and_replaces_complete_records() {
        let directory = test_directory();
        let mut store = Win32RecoveryStore::for_test(directory.clone());
        let prepared = pending_restore();

        store.publish(&prepared).expect("initial publish");
        assert_eq!(store.load(), Ok(Some(prepared.clone())));

        let switched = prepared.with_state(DurableOperationState::RegionSwitched);
        store.publish(&switched).expect("state replacement");
        assert_eq!(store.load(), Ok(Some(switched.clone())));
        let names = fs::read_dir(&directory)
            .expect("test directory")
            .map(|entry| entry.expect("directory entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![PENDING_RESTORE_FILE_NAME]);

        store.clear_verified(&switched).expect("verified cleanup");
        assert_eq!(store.load(), Ok(None));
        remove_test_directory(&directory);
    }

    #[test]
    fn recovery_store_keeps_malformed_and_unknown_schema_records() {
        let directory = test_directory();
        fs::create_dir_all(&directory).expect("test directory");
        let path = directory.join(PENDING_RESTORE_FILE_NAME);
        fs::write(&path, br#"{"schema_version":1}"#).expect("incomplete record");
        let mut store = Win32RecoveryStore::for_test(directory.clone());
        let pending = pending_restore();

        assert_eq!(
            store.load(),
            Err(RecoveryStoreError::Schema(
                RecoverySchemaError::IncompleteRecord
            ))
        );
        assert!(path.exists(), "unsafe record must remain for recovery UI");
        assert_eq!(
            store.publish(&pending),
            Err(RecoveryStoreError::Schema(
                RecoverySchemaError::IncompleteRecord
            ))
        );
        assert_eq!(
            fs::read(&path).expect("unchanged incomplete record"),
            br#"{"schema_version":1}"#
        );

        fs::write(
            &path,
            br#"{"schema_version":2,"operation_id":"x","started_at_utc":"2026-08-15T12:34:56Z","original_region":244,"temporary_region":840,"product_id":"p","backend":"b","state":"prepared"}"#,
        )
        .expect("unknown schema record");
        assert_eq!(
            store.load(),
            Err(RecoveryStoreError::Schema(
                RecoverySchemaError::UnknownSchema(2)
            ))
        );
        assert!(path.exists(), "unknown schema must remain for recovery UI");
        assert_eq!(
            store.publish(&pending),
            Err(RecoveryStoreError::Schema(
                RecoverySchemaError::UnknownSchema(2)
            ))
        );
        remove_test_directory(&directory);
    }

    #[test]
    fn recovery_store_cleans_only_temporary_files() {
        let directory = test_directory();
        fs::create_dir_all(&directory).expect("test directory");
        let temporary = directory.join(".pending-restore-test.tmp");
        fs::write(&temporary, "temporary").expect("temporary record");
        let store = Win32RecoveryStore::for_test(directory.clone());

        store
            .cleanup_temporary_files()
            .expect("safe temporary cleanup");

        assert!(!temporary.exists());
        assert!(!directory.join(PENDING_RESTORE_FILE_NAME).exists());
        remove_test_directory(&directory);
    }

    #[test]
    fn recovery_store_uses_the_current_users_local_data_directory() {
        assert!(Win32RecoveryStore::for_current_user().is_ok());
    }

    #[test]
    fn journal_store_round_trips_indented_history_and_deletes_only_the_selected_record() {
        let directory = test_directory();
        let store = Win32JournalStore::for_test(directory.clone());
        let record = journal_record();

        store
            .replace(std::slice::from_ref(&record))
            .expect("publish journal history");
        let contents = fs::read_to_string(directory.join(winstoreregion_core::JOURNAL_FILE_NAME))
            .expect("read indented journal");
        assert!(
            contents.contains("\n  {"),
            "journal must stay human-readable"
        );
        assert_eq!(store.load(), Ok(vec![record.clone()]));

        store.delete(&record).expect("delete exact journal record");
        assert_eq!(store.load(), Ok(Vec::new()));
        remove_test_directory(&directory);
    }

    #[test]
    fn journal_store_preserves_malformed_history_instead_of_overwriting_it() {
        let directory = test_directory();
        fs::create_dir_all(&directory).expect("test directory");
        let path = directory.join(winstoreregion_core::JOURNAL_FILE_NAME);
        let malformed = br#"[{"schema_version":1}]"#;
        fs::write(&path, malformed).expect("write malformed journal");
        let store = Win32JournalStore::for_test(directory.clone());

        assert_eq!(
            store.load(),
            Err(super::JournalStoreError::Format(
                JournalFormatError::MalformedJson
            ))
        );
        assert!(store.replace(&[journal_record()]).is_err());
        assert_eq!(fs::read(&path).expect("unchanged journal"), malformed);
        remove_test_directory(&directory);
    }
}
