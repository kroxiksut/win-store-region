//! Read-only packaged-application and deployment-event evidence.

use crate::platform::{WindowsRuntimeGuard, observation_timestamp_now};
use windows::ApplicationModel::{Package, PackageSignatureKind};
use windows::Management::Deployment::PackageManager;
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS};
use windows::Win32::System::EventLog::{
    EVT_HANDLE, EvtClose, EvtNext, EvtQuery, EvtQueryChannelPath, EvtQueryReverseDirection,
    EvtRender, EvtRenderEventXml,
};
use windows::core::{HRESULT, HSTRING};
use winstoreregion_core::{
    AppxDeploymentEvent, InstalledStoreApplication, ObservationTimestamp, PackageFamilyName,
    PackageSnapshot, PackageSnapshotEntry, PackageStatusSnapshot, StoreProductId,
};

/// Read-only failures from Windows package and `AppX` deployment observation.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackagedObservationReadError {
    PackageManagerUnavailable { native_code: Option<i32> },
    PackageDataUnavailable { native_code: Option<i32> },
}

/// Read-only adapter for packaged Store evidence.
///
/// It never asks Windows to deploy, register, update, or remove a package.
#[allow(dead_code)]
pub(crate) struct WindowsPackagedObserver;

#[allow(dead_code)]
impl WindowsPackagedObserver {
    /// Capture the current user's packaged-app baseline or follow-up state.
    pub(crate) fn snapshot() -> Result<PackageSnapshot, PackagedObservationReadError> {
        let _runtime = WindowsRuntimeGuard::initialize().map_err(|error| {
            PackagedObservationReadError::PackageManagerUnavailable {
                native_code: Some(error.code().0),
            }
        })?;
        let manager = PackageManager::new().map_err(|error| {
            PackagedObservationReadError::PackageManagerUnavailable {
                native_code: Some(error.code().0),
            }
        })?;
        // Enumerating every user's packages requires administrator rights,
        // which this product does not ask for. The current user's own packages
        // are what the observation is about anyway.
        let packages = manager
            .FindPackagesByUserSecurityId(&HSTRING::new())
            .map_err(
                |error| PackagedObservationReadError::PackageDataUnavailable {
                    native_code: Some(error.code().0),
                },
            )?;
        let mut entries = Vec::new();
        for package in packages {
            if let Ok(entry) = snapshot_entry(&package) {
                entries.push(entry);
            }
        }
        Ok(PackageSnapshot {
            observed_at: observation_timestamp_now(),
            entries,
        })
    }

    /// Read recent `AppX` deployment events that carry both product and
    /// package-family identity. Events without both facts are intentionally
    /// discarded because they cannot prove completion for one Store product.
    #[allow(dead_code)]
    pub(super) fn recent_appx_events(
        limit: usize,
    ) -> Result<Vec<AppxDeploymentEvent>, PackagedObservationReadError> {
        read_appx_deployment_events(limit)
    }
}

/// Every Store application installed for this user, as the updates tab needs it.
///
/// Filtered to what a person could plausibly reinstall from Microsoft Store:
/// signed by the Store, not a framework, and carrying a display name. System
/// and developer-signed packages are deliberately absent — nothing here can
/// offer to reinstall those, and listing them would be an invitation to try.
///
/// The version is the installed one. It is not compared with anything here:
/// core decides what a version means.
pub(crate) fn installed_store_applications()
-> Result<Vec<InstalledStoreApplication>, PackagedObservationReadError> {
    let _runtime = WindowsRuntimeGuard::initialize().map_err(|error| {
        PackagedObservationReadError::PackageManagerUnavailable {
            native_code: Some(error.code().0),
        }
    })?;
    let manager = PackageManager::new().map_err(|error| {
        PackagedObservationReadError::PackageManagerUnavailable {
            native_code: Some(error.code().0),
        }
    })?;
    let packages = manager
        .FindPackagesByUserSecurityId(&HSTRING::new())
        .map_err(
            |error| PackagedObservationReadError::PackageDataUnavailable {
                native_code: Some(error.code().0),
            },
        )?;
    let mut applications = Vec::new();
    for package in packages {
        if let Some(application) = store_application(&package) {
            applications.push(application);
        }
    }
    applications.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
    });
    Ok(applications)
}

/// Reduce one package to the facts the updates tab uses, or skip it.
///
/// Every failure is a skip rather than an error: one unreadable package must
/// not cost the user the whole list.
fn store_application(package: &Package) -> Option<InstalledStoreApplication> {
    if package.IsFramework().ok()? {
        return None;
    }
    if package.SignatureKind().ok()? != PackageSignatureKind::Store {
        return None;
    }
    let identity = package.Id().ok()?;
    let family_name = PackageFamilyName::parse(identity.FamilyName().ok()?.to_string())?;
    let version = identity.Version().ok()?;
    let display_name = package
        .DisplayName()
        .ok()
        .map(|name| name.to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| family_name.as_str().to_owned());
    Some(InstalledStoreApplication {
        family_name,
        display_name,
        installed_version: format!(
            "{}.{}.{}.{}",
            version.Major, version.Minor, version.Build, version.Revision
        ),
    })
}

fn snapshot_entry(package: &Package) -> Result<PackageSnapshotEntry, PackagedObservationReadError> {
    let identity = package
        .Id()
        .map_err(|_| PackagedObservationReadError::PackageDataUnavailable { native_code: None })?;
    let family_name = PackageFamilyName::parse(
        identity
            .FamilyName()
            .map_err(|_| PackagedObservationReadError::PackageDataUnavailable {
                native_code: None,
            })?
            .to_string(),
    )
    .ok_or(PackagedObservationReadError::PackageDataUnavailable { native_code: None })?;
    let full_name = identity
        .FullName()
        .map_err(|_| PackagedObservationReadError::PackageDataUnavailable { native_code: None })?
        .to_string();
    let product_id = identity
        .ProductId()
        .ok()
        .and_then(|value| StoreProductId::parse(&value.to_string()).ok());
    Ok(PackageSnapshotEntry {
        product_id,
        family_name,
        full_name,
        status: package_status_snapshot(package),
    })
}

fn package_status_snapshot(package: &Package) -> Option<PackageStatusSnapshot> {
    let status = package.Status().ok()?;
    Some(PackageStatusSnapshot {
        verify_is_ok: status.VerifyIsOK().ok()?,
        not_available: status.NotAvailable().ok()?,
        package_offline: status.PackageOffline().ok()?,
        data_offline: status.DataOffline().ok()?,
        disabled: status.Disabled().ok()?,
        needs_remediation: status.NeedsRemediation().ok()?,
        license_issue: status.LicenseIssue().ok()?,
        modified: status.Modified().ok()?,
        tampered: status.Tampered().ok()?,
        dependency_issue: status.DependencyIssue().ok()?,
        servicing: status.Servicing().ok()?,
        deployment_in_progress: status.DeploymentInProgress().ok()?,
        is_partially_staged: status.IsPartiallyStaged().ok()?,
    })
}

struct EventHandle(EVT_HANDLE);

impl Drop for EventHandle {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        let _ = unsafe { EvtClose(self.0) };
    }
}

#[allow(unsafe_code)]
fn read_appx_deployment_events(
    limit: usize,
) -> Result<Vec<AppxDeploymentEvent>, PackagedObservationReadError> {
    let channel = HSTRING::from("Microsoft-Windows-AppXDeploymentServer/Operational");
    let query = HSTRING::from("*");
    let query = unsafe {
        EvtQuery(
            None,
            &channel,
            &query,
            EvtQueryChannelPath.0 | EvtQueryReverseDirection.0,
        )
    }
    .map_err(|_| PackagedObservationReadError::PackageDataUnavailable { native_code: None })?;
    let _query = EventHandle(query);
    let mut events = Vec::new();

    while events.len() < limit {
        let mut raw_event = [0_isize; 1];
        let mut returned = 0_u32;
        match unsafe { EvtNext(query, &mut raw_event, 0, 0, &raw mut returned) } {
            Ok(()) if returned == 1 => {
                let event = EventHandle(EVT_HANDLE(raw_event[0]));
                let xml = render_event_xml(event.0)?;
                if let Some(event) = appx_event_from_xml(&xml) {
                    events.push(event);
                }
            }
            Ok(()) => break,
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NO_MORE_ITEMS.0) => {
                break;
            }
            Err(_) => {
                return Err(PackagedObservationReadError::PackageDataUnavailable {
                    native_code: None,
                });
            }
        }
    }

    Ok(events)
}

#[allow(unsafe_code)]
fn render_event_xml(event: EVT_HANDLE) -> Result<String, PackagedObservationReadError> {
    let mut bytes_needed = 0_u32;
    let mut property_count = 0_u32;
    match unsafe {
        EvtRender(
            None,
            event,
            EvtRenderEventXml.0,
            0,
            None,
            &raw mut bytes_needed,
            &raw mut property_count,
        )
    } {
        Ok(()) => return Ok(String::new()),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0) => {}
        Err(_) => {
            return Err(PackagedObservationReadError::PackageDataUnavailable { native_code: None });
        }
    }
    let word_count = usize::try_from(bytes_needed)
        .ok()
        .and_then(|bytes| bytes.checked_add(1))
        .map(|bytes| bytes / 2)
        .ok_or(PackagedObservationReadError::PackageDataUnavailable { native_code: None })?;
    let mut buffer = vec![0_u16; word_count];
    unsafe {
        EvtRender(
            None,
            event,
            EvtRenderEventXml.0,
            bytes_needed,
            Some(buffer.as_mut_ptr().cast()),
            &raw mut bytes_needed,
            &raw mut property_count,
        )
    }
    .map_err(|_| PackagedObservationReadError::PackageDataUnavailable { native_code: None })?;
    let units = usize::try_from(bytes_needed)
        .ok()
        .map(|bytes| bytes / 2)
        .ok_or(PackagedObservationReadError::PackageDataUnavailable { native_code: None })?;
    Ok(String::from_utf16_lossy(&buffer[..units]))
}

fn appx_event_from_xml(xml: &str) -> Option<AppxDeploymentEvent> {
    let observed_at = parse_event_system_time(xml)?;
    let product_id = xml_data_value(xml, "ProductId")
        .or_else(|| xml_data_value(xml, "StoreProductId"))
        .and_then(|value| StoreProductId::parse(value).ok())?;
    let family_name = xml_data_value(xml, "PackageFamilyName")
        .or_else(|| xml_attribute_value(xml, "PackageFamilyName"))
        .and_then(PackageFamilyName::parse)
        .or_else(|| {
            xml_data_value(xml, "PackageFullName")
                .or_else(|| xml_attribute_value(xml, "PackageFullName"))
                .and_then(package_family_from_full_name)
        })?;
    Some(AppxDeploymentEvent {
        observed_at,
        product_id: Some(product_id),
        family_name: Some(family_name),
    })
}

fn xml_data_value<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("<Data Name=\"{name}\">");
    let after = xml.split_once(&marker)?.1;
    after.split_once("</Data>").map(|(value, _)| value.trim())
}

fn xml_attribute_value<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("{name}=\"");
    let after = xml.split_once(&marker)?.1;
    after.split_once('"').map(|(value, _)| value)
}

fn package_family_from_full_name(full_name: &str) -> Option<PackageFamilyName> {
    let mut parts = full_name.split('_');
    let name = parts.next()?;
    let publisher_id = full_name.rsplit('_').next()?;
    PackageFamilyName::parse(format!("{name}_{publisher_id}"))
}

fn parse_event_system_time(xml: &str) -> Option<ObservationTimestamp> {
    let value = xml_attribute_value(xml, "SystemTime")?;
    let date_time = value.strip_suffix('Z')?;
    let (date, time) = date_time.split_once('T')?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i64>().ok()?;
    let month = date_parts.next()?.parse::<u32>().ok()?;
    let day = date_parts.next()?.parse::<u32>().ok()?;
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut clock_parts = clock.split(':');
    let hour = clock_parts.next()?.parse::<u64>().ok()?;
    let minute = clock_parts.next()?.parse::<u64>().ok()?;
    let second = clock_parts.next()?.parse::<u64>().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let fraction = fraction.as_bytes();
    if !fraction.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let milliseconds = fraction
        .iter()
        .take(3)
        .fold(0_u64, |value, digit| value * 10 + u64::from(*digit - b'0'))
        * match fraction.len() {
            0 => 1_000,
            1 => 100,
            2 => 10,
            _ => 1,
        };
    let days = days_since_unix_epoch(year, month, day)?;
    let seconds = u64::try_from(days)
        .ok()?
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second)?;
    Some(ObservationTimestamp::from_unix_millis(
        seconds.checked_mul(1_000)?.checked_add(milliseconds)?,
    ))
}

fn days_since_unix_epoch(year: i64, month: u32, day: u32) -> Option<i64> {
    let month = i64::from(month);
    let day = i64::from(day);
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let month_index = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    (days >= 0).then_some(days)
}

#[cfg(test)]
mod tests {
    use super::*;

    use winstoreregion_core::{PackageFamilyName, StoreProductId};

    #[test]
    fn package_snapshot_is_read_only_and_surfaces_unavailability_structurally() {
        match WindowsPackagedObserver::snapshot() {
            Ok(snapshot) => {
                assert!(snapshot.observed_at.unix_millis() > 0);
                assert!(snapshot.entries.iter().all(|entry| {
                    !entry.family_name.as_str().is_empty() && !entry.full_name.is_empty()
                }));
            }
            Err(
                super::PackagedObservationReadError::PackageManagerUnavailable { .. }
                | super::PackagedObservationReadError::PackageDataUnavailable { .. },
            ) => {}
        }
    }

    #[test]
    fn appx_event_parser_requires_product_and_package_identity() {
        let xml = r#"<Event><System><TimeCreated SystemTime="2026-08-16T12:34:56.789Z" /></System><EventData><Data Name="ProductId">9WZDNCRFJ3PZ</Data><Data Name="PackageFullName">Contoso.App_1.0.0.0_x64__1234567890abc</Data></EventData></Event>"#;
        let event = appx_event_from_xml(xml).expect("fully identified deployment event");

        assert_eq!(
            event.product_id.as_ref().map(StoreProductId::as_str),
            Some("9WZDNCRFJ3PZ")
        );
        assert_eq!(
            event.family_name.as_ref().map(PackageFamilyName::as_str),
            Some("Contoso.App_1234567890abc")
        );
        assert!(appx_event_from_xml("<Event />").is_none());
    }
}
