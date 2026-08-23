//! Adapter for the `WinGet` COM API and its `msstore` catalogue.
//!
//! This is the same engine `winget.exe` drives, reached through its typed
//! interfaces instead of its localised console output. Activation is plain COM:
//! the classes are packaged out-of-process servers registered by the App
//! Installer manifest, not `WinRT`-activatable types, so `RoActivateInstance`
//! cannot reach them and must not be used.
//!
//! Everything here is narrow platform work. Which product may be installed, and
//! what counts as completion, stay in core.

// Nothing calls the backend yet: the GUI reaches it once the operation state
// machine drives an install, and the adapter is proven by its own tests until
// then.
#[allow(dead_code)]
pub(crate) mod backend;
mod bindings;
pub(crate) mod metadata;
pub(crate) mod resolver;

use bindings::{
    CatalogPackage, ConnectResultStatus, FindPackagesOptions, FindPackagesResultStatus,
    PackageCatalog, PackageCatalogReference, PackageFieldMatchOption, PackageManager,
    PackageMatchField, PackageMatchFilter,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::core::{GUID, HSTRING, Result as WindowsResult};

/// `PackageManager` server declared by the App Installer package manifest.
const CLSID_PACKAGE_MANAGER: GUID = GUID::from_u128(0xC53A4F16_787E_42A4_B304_29EFFB4BF597);

/// `FindPackagesOptions` server.
const CLSID_FIND_PACKAGES_OPTIONS: GUID = GUID::from_u128(0x572DED96_9C60_4526_8F92_EE7D91D38C1A);

/// `PackageMatchFilter` server.
const CLSID_PACKAGE_MATCH_FILTER: GUID = GUID::from_u128(0xD02C9DAF_99DC_429C_B503_4E504E4AB000);

/// `InstallOptions` server.
const CLSID_INSTALL_OPTIONS: GUID = GUID::from_u128(0x1095F097_EB96_453B_B4E6_1613637F3B14);

/// Catalogue name of the Microsoft Store source.
const MSSTORE_CATALOG: &str = "msstore";

/// Why the `WinGet` COM API could not be used.
///
/// Each variant is a different conversation with the user, so they stay apart:
/// a missing server is a machine problem, an unreachable source is a service
/// problem, and a refused connection is neither.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WinGetUnavailable {
    /// The COM server could not be created; App Installer is absent or broken.
    ServerUnavailable(i32),
    /// The interface could not be marshalled, which means the metadata is missing.
    MetadataMissing(i32),
    /// The `msstore` catalogue is registered but did not connect.
    SourceUnavailable(i32),
}

impl WinGetUnavailable {
    /// The native code, kept for the diagnostic details and the log.
    pub(crate) const fn native_code(self) -> i32 {
        match self {
            Self::ServerUnavailable(code)
            | Self::MetadataMissing(code)
            | Self::SourceUnavailable(code) => code,
        }
    }

    /// Classify an activation failure by what it tells the user.
    fn from_activation(error: &windows::core::Error) -> Self {
        // Windows reports a missing proxy registration when the interface
        // metadata is not beside the executable. It is a solvable placement
        // problem rather than a broken installation, so it is named separately.
        const RO_E_METADATA_NAME_NOT_FOUND: i32 = -2_147_009_196;
        let code = error.code().0;
        if code == RO_E_METADATA_NAME_NOT_FOUND {
            Self::MetadataMissing(code)
        } else {
            Self::ServerUnavailable(code)
        }
    }
}

/// A live connection to the `msstore` catalogue.
///
/// Held by one worker thread. The COM objects are apartment-bound, so this must
/// not cross threads; the caller keeps it inside the work it started.
pub(crate) struct WinGetSession {
    #[allow(dead_code)]
    manager: PackageManager,
    #[allow(dead_code)]
    catalog_reference: PackageCatalogReference,
    catalog: PackageCatalog,
}

impl WinGetSession {
    /// Activate the package manager and connect to the Microsoft Store source.
    ///
    /// The metadata is put in place first: the copy only takes effect for
    /// activations that follow it, and there is exactly one activation here.
    ///
    /// # Errors
    ///
    /// Returns the structured reason the API is unusable, having changed
    /// nothing on the machine.
    #[allow(unsafe_code)]
    pub(crate) fn connect() -> Result<Self, WinGetUnavailable> {
        let placement = metadata::place_metadata_beside_executable();
        let manager: PackageManager =
            unsafe { CoCreateInstance(&CLSID_PACKAGE_MANAGER, None, CLSCTX_ALL) }
                .map_err(|error| WinGetUnavailable::from_activation(&error))?;

        let catalog_reference = manager
            .GetPackageCatalogByName(&HSTRING::from(MSSTORE_CATALOG))
            .map_err(|error| {
                if placement == metadata::MetadataPlacement::Present {
                    WinGetUnavailable::from_activation(&error)
                } else {
                    WinGetUnavailable::MetadataMissing(error.code().0)
                }
            })?;

        // Without accepting the source agreements the source performs nothing at
        // all. The acceptance travels with the call because it cannot be
        // established read-only beforehand.
        catalog_reference
            .SetAcceptSourceAgreements(true)
            .map_err(|error| WinGetUnavailable::SourceUnavailable(error.code().0))?;
        let connect_result = catalog_reference
            .Connect()
            .map_err(|error| WinGetUnavailable::SourceUnavailable(error.code().0))?;
        let status = connect_result
            .Status()
            .map_err(|error| WinGetUnavailable::SourceUnavailable(error.code().0))?;
        if status != ConnectResultStatus::Ok {
            let extended = connect_result
                .ExtendedErrorCode()
                .map_or(status.0, |code| code.0);
            return Err(WinGetUnavailable::SourceUnavailable(extended));
        }
        let catalog = connect_result
            .PackageCatalog()
            .map_err(|error| WinGetUnavailable::SourceUnavailable(error.code().0))?;

        Ok(Self {
            manager,
            catalog_reference,
            catalog,
        })
    }

    /// The package manager, for callers that start or resume an installation.
    #[allow(dead_code)]
    pub(crate) const fn manager(&self) -> &PackageManager {
        &self.manager
    }

    /// The connected catalogue reference, needed to re-attach to an install.
    #[allow(dead_code)]
    pub(crate) const fn catalog_reference(&self) -> &PackageCatalogReference {
        &self.catalog_reference
    }

    /// Find exactly one product by its Store identifier.
    ///
    /// `Ok(None)` is a real answer, not a failure: under a home region where the
    /// product is not offered, the catalogue reports success with no matches.
    ///
    /// # Errors
    ///
    /// Returns the catalogue's own failure code when the search itself failed.
    pub(crate) fn find_exact(&self, product_id: &str) -> Result<Option<CatalogPackage>, i32> {
        find_exact_in(&self.catalog, product_id)
    }
}

/// Search one catalogue for an exact product identifier.
#[allow(unsafe_code)]
fn find_exact_in(
    catalog: &PackageCatalog,
    product_id: &str,
) -> Result<Option<CatalogPackage>, i32> {
    let native = |error: windows::core::Error| error.code().0;
    let options: FindPackagesOptions =
        unsafe { CoCreateInstance(&CLSID_FIND_PACKAGES_OPTIONS, None, CLSCTX_ALL) }
            .map_err(native)?;
    let filter: PackageMatchFilter =
        unsafe { CoCreateInstance(&CLSID_PACKAGE_MATCH_FILTER, None, CLSCTX_ALL) }
            .map_err(native)?;
    filter.SetField(PackageMatchField::Id).map_err(native)?;
    filter
        .SetOption(PackageFieldMatchOption::EqualsCaseInsensitive)
        .map_err(native)?;
    filter
        .SetValue(&HSTRING::from(product_id))
        .map_err(native)?;
    options
        .Filters()
        .map_err(native)?
        .Append(&filter)
        .map_err(native)?;

    let found = catalog.FindPackages(&options).map_err(native)?;
    let status = found.Status().map_err(native)?;
    if status != FindPackagesResultStatus::Ok {
        let extended = found.ExtendedErrorCode().map_or(status.0, |code| code.0);
        return Err(extended);
    }
    let matches = found.Matches().map_err(native)?;
    if matches.Size().map_err(native)? == 0 {
        return Ok(None);
    }
    matches
        .GetAt(0)
        .and_then(|matched| matched.CatalogPackage())
        .map(Some)
        .map_err(native)
}

/// Read a string property, treating a failure as an absent value.
fn optional_text(value: WindowsResult<HSTRING>) -> Option<String> {
    value
        .ok()
        .map(|text| text.to_string_lossy())
        .filter(|text| {
            // `Unknown` is what the Store source reports for a version it does not
            // publish. Showing it as a version would be worse than showing none.
            !text.trim().is_empty() && text != "Unknown"
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_unavailability_keeps_its_native_code_for_the_details() {
        assert_eq!(WinGetUnavailable::ServerUnavailable(-1).native_code(), -1);
        assert_eq!(WinGetUnavailable::MetadataMissing(-2).native_code(), -2);
        assert_eq!(WinGetUnavailable::SourceUnavailable(-3).native_code(), -3);
    }

    #[test]
    fn a_missing_proxy_registration_is_reported_as_missing_metadata() {
        let missing_proxy =
            windows::core::Error::from_hresult(windows::core::HRESULT(-2_147_009_196));
        assert!(matches!(
            WinGetUnavailable::from_activation(&missing_proxy),
            WinGetUnavailable::MetadataMissing(_)
        ));

        // Class not registered, which is what a machine without a working App
        // Installer reports.
        let class_not_registered =
            windows::core::Error::from_hresult(windows::core::HRESULT(-2_147_221_164));
        assert!(matches!(
            WinGetUnavailable::from_activation(&class_not_registered),
            WinGetUnavailable::ServerUnavailable(_)
        ));
    }

    #[test]
    fn an_unpublished_version_is_absent_rather_than_the_word_unknown() {
        assert_eq!(optional_text(Ok(HSTRING::from("Unknown"))), None);
        assert_eq!(optional_text(Ok(HSTRING::from("   "))), None);
        assert_eq!(
            optional_text(Ok(HSTRING::from("1.2.3"))),
            Some("1.2.3".to_owned())
        );
    }
}
