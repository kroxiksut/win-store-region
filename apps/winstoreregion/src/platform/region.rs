//! Reading Windows Home Location and its display data.

use windows::Win32::Foundation::LPARAM;
use windows::Win32::Globalization::{
    EnumSystemGeoNames, GEO_FRIENDLYNAME, GEO_ID, GEO_ISO2, GEOCLASS_NATION, GetGeoInfoEx,
    GetGeoInfoW, GetUserGeoID, SYSGEOTYPE, SetUserGeoID,
};
use windows::core::{BOOL, HSTRING, PCWSTR};
use winstoreregion_core::{
    GeoId, IsoAlpha2, Region, RegionField, RegionReadError, RegionReader, RegionWriteError,
    RegionWriter,
};

/// Read-only Win32 adapter for the current Windows Home Location.
pub(crate) struct Win32RegionReader;

impl Win32RegionReader {
    /// Resolve a user-selected `GeoId` through Windows NLS without changing it.
    pub(crate) fn region_for_geo_id(geo_id: GeoId) -> Result<Region, RegionReadError> {
        let display_name = read_required_geo_string(geo_id, GEO_FRIENDLYNAME)?;
        let iso_alpha2 = read_optional_geo_string(geo_id, GEO_ISO2)
            .ok()
            .flatten()
            .and_then(|value| IsoAlpha2::parse(&value));
        Region::new(geo_id, display_name, iso_alpha2)
            .map_err(|_| RegionReadError::MissingDisplayName)
    }

    /// Build the selectable region list from Windows NLS.  The values are
    /// display data only: the method never reads or writes the user's Home
    /// Location.
    #[allow(unsafe_code)]
    pub(crate) fn available_national_regions() -> Vec<Region> {
        let mut names: Vec<String> = Vec::new();
        let context = LPARAM((&raw mut names) as isize);
        let _ = unsafe {
            EnumSystemGeoNames(GEOCLASS_NATION.0 as u32, Some(collect_geo_name), context)
        };

        let mut regions = names
            .into_iter()
            .filter_map(|name| geo_id_from_geo_name(&name))
            .filter_map(|geo_id| Self::region_for_geo_id(geo_id).ok())
            .collect::<Vec<_>>();
        regions.sort_by(|left, right| {
            left.display_name
                .cmp(&right.display_name)
                .then_with(|| left.geo_id.value().cmp(&right.geo_id.value()))
        });
        regions.dedup_by(|left, right| left.geo_id == right.geo_id);
        regions
    }
}

impl RegionReader for Win32RegionReader {
    #[allow(unsafe_code)]
    fn current_region(&self) -> Result<Region, RegionReadError> {
        let raw_geo_id = unsafe { GetUserGeoID(GEOCLASS_NATION) };
        let geo_id = GeoId::from_windows_user_value(raw_geo_id)?;
        Self::region_for_geo_id(geo_id)
    }
}

/// The single writer of Windows Home Location.
///
/// `SetUserGeoID` is the primary API by decision of block 4.2: it takes the
/// canonical `GeoId` directly, whereas `SetUserGeoName` needs an ISO alpha-2
/// code, which this product treats as optional presentation data.
///
/// Measured on the designated test VM: an unacceptable target returns `FALSE`
/// with `ERROR_INVALID_PARAMETER` and leaves the region unchanged, so a failed
/// write never produces a partial state. `GetLastError` after a *successful*
/// call holds a stale value, which is why nothing here inspects it on success
/// and why core treats this result as diagnostic only, trusting the read-back.
pub(crate) struct Win32RegionWriter;

impl RegionWriter for Win32RegionWriter {
    #[allow(unsafe_code)]
    fn set_region(&self, target: GeoId) -> Result<(), RegionWriteError> {
        match unsafe { SetUserGeoID(target.value()) } {
            Ok(()) => Ok(()),
            Err(error) => Err(region_write_error(error.code().0)),
        }
    }
}

/// Classify a failed write, separating a refused target from any other failure.
fn region_write_error(native_code: i32) -> RegionWriteError {
    // HRESULT_FROM_WIN32(ERROR_INVALID_PARAMETER): the target was not accepted.
    if native_code == -2_147_024_809 {
        RegionWriteError::Rejected
    } else {
        RegionWriteError::PlatformFailure(native_code)
    }
}

#[allow(unsafe_code)]
unsafe extern "system" fn collect_geo_name(name: PCWSTR, data: LPARAM) -> BOOL {
    if name.is_null() || data.0 == 0 {
        return BOOL(0);
    }
    let mut length = 0_usize;
    while length < 256 && unsafe { *name.0.add(length) } != 0 {
        length += 1;
    }
    if length == 0 || length == 256 {
        return BOOL(1);
    }
    let value = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(name.0, length)) };
    let names = unsafe { &mut *(data.0 as *mut Vec<String>) };
    names.push(value);
    BOOL(1)
}

#[allow(unsafe_code)]
fn geo_id_from_geo_name(name: &str) -> Option<GeoId> {
    let name = HSTRING::from(name);
    let required = unsafe { GetGeoInfoEx(&name, GEO_ID, None) };
    if required <= 1 {
        return None;
    }
    let mut value = vec![0_u16; usize::try_from(required).ok()?];
    if unsafe { GetGeoInfoEx(&name, GEO_ID, Some(&mut value)) } == 0 {
        return None;
    }
    let end = value
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
        .parse()
        .ok()
        .and_then(GeoId::new)
}

#[allow(unsafe_code)]
fn read_required_geo_string(
    geo_id: GeoId,
    geo_type: SYSGEOTYPE,
) -> Result<String, RegionReadError> {
    read_geo_string(geo_id, geo_type)?
        .ok_or(RegionReadError::LookupFailed(RegionField::DisplayName))
}

#[allow(unsafe_code)]
fn read_optional_geo_string(
    geo_id: GeoId,
    geo_type: SYSGEOTYPE,
) -> Result<Option<String>, RegionReadError> {
    read_geo_string(geo_id, geo_type)
}

#[allow(unsafe_code)]
fn read_geo_string(geo_id: GeoId, geo_type: SYSGEOTYPE) -> Result<Option<String>, RegionReadError> {
    let required = unsafe { GetGeoInfoW(geo_id.value(), geo_type, None, 0) };
    if required == 0 {
        return Ok(None);
    }
    let length = usize::try_from(required)
        .map_err(|_| RegionReadError::LookupFailed(RegionField::DisplayName))?;
    let mut buffer = vec![0_u16; length];
    let read = unsafe { GetGeoInfoW(geo_id.value(), geo_type, Some(&mut buffer), 0) };
    if read == 0 {
        return Ok(None);
    }
    let end = buffer
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(buffer.len());
    String::from_utf16(&buffer[..end])
        .map(Some)
        .map_err(|_| RegionReadError::InvalidUnicode(RegionField::DisplayName))
}

#[cfg(test)]
mod tests {
    use super::*;
    use winstoreregion_core::GeoId;

    #[test]
    fn a_refused_target_is_told_apart_from_any_other_write_failure() {
        // Measured on the designated test VM: an unacceptable GeoId fails with
        // ERROR_INVALID_PARAMETER and leaves the region untouched. windows-rs
        // reports it as HRESULT_FROM_WIN32(0x57).
        const HRESULT_FROM_WIN32_INVALID_PARAMETER: i32 = -2_147_024_809;
        assert_eq!(
            HRESULT_FROM_WIN32_INVALID_PARAMETER,
            i32::from_ne_bytes(0x8007_0057_u32.to_ne_bytes()),
            "the constant must stay the HRESULT form of ERROR_INVALID_PARAMETER"
        );
        assert_eq!(
            region_write_error(HRESULT_FROM_WIN32_INVALID_PARAMETER),
            RegionWriteError::Rejected
        );
        assert_eq!(
            region_write_error(-2_147_467_259),
            RegionWriteError::PlatformFailure(-2_147_467_259),
            "an unfamiliar failure must keep its native code instead of being flattened"
        );
    }

    #[test]
    fn reads_the_current_windows_region_without_changing_it() {
        let region = Win32RegionReader.current_region();

        assert!(region.is_ok(), "Windows should provide the current GeoId");
    }

    #[test]
    fn selected_temporary_geoid_is_read_through_windows_nls_without_changing_it() {
        let region = Win32RegionReader::region_for_geo_id(
            GeoId::new(244).expect("United States GeoId is positive"),
        )
        .expect("Windows NLS should resolve a selected GeoId");
        assert_eq!(region.geo_id.value(), 244);
        assert_eq!(
            region
                .iso_alpha2
                .as_ref()
                .map(winstoreregion_core::IsoAlpha2::as_str),
            Some("US")
        );
    }
}
