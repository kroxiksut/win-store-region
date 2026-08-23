//! Product resolution through the `WinGet` COM catalogue.
//!
//! The catalogue answers with typed fields, so nothing here parses text. Two
//! properties of that answer shape the rules below. The catalogue is regional
//! at search time: under a home region where a product is not offered it
//! reports success with no matches, which is an answer about availability
//! rather than a failure. And it never reports whether a Store product is
//! already installed, so no code here pretends to know that.

use super::{WinGetSession, WinGetUnavailable, optional_text};
use crate::platform::WindowsRuntimeGuard;
use crate::platform::winget::bindings::{
    CatalogPackage, InstallOptions, PackageInstallerType, PackageVersionInfo,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use winstoreregion_core::{
    InstallClassification, PackageFamilyName, ProductResolver, ResolveError, ResolveFailure,
    ResolveRequest, ResolverDiagnostic, ResolverProduct, ResolverSource,
};

/// Resolver backed by the `msstore` catalogue of the `WinGet` COM API.
pub(crate) struct WinGetComResolver;

impl ProductResolver for WinGetComResolver {
    fn resolve(&self, request: &ResolveRequest) -> Result<ResolverProduct, ResolveFailure> {
        let _runtime = WindowsRuntimeGuard::initialize()
            .map_err(|error| failure(ResolveError::ResolverUnavailable, Some(error.code().0)))?;
        let session = WinGetSession::connect().map_err(unavailable_to_failure)?;
        let package = session
            .find_exact(request.product_id.as_str())
            .map_err(|code| failure(ResolveError::InvalidResponse, Some(code)))?
            .ok_or_else(|| failure(ResolveError::ProductNotFoundInMarket, None))?;

        product_from_package(request, &package)
    }
}

/// Build the resolver's answer from the catalogue entry.
fn product_from_package(
    request: &ResolveRequest,
    package: &CatalogPackage,
) -> Result<ResolverProduct, ResolveFailure> {
    let identifier =
        optional_text(package.Id()).ok_or_else(|| failure(ResolveError::InvalidResponse, None))?;
    if !identifier.eq_ignore_ascii_case(request.product_id.as_str()) {
        return Err(failure(ResolveError::ProductIdMismatch, None));
    }
    let display_name = optional_text(package.Name())
        .ok_or_else(|| failure(ResolveError::InvalidResponse, None))?;
    let version_info = package.DefaultInstallVersion().ok();

    Ok(ResolverProduct {
        request_id: request.request_id,
        product_id: request.product_id.clone(),
        market: request.market.clone(),
        // The catalogue applies the machine's home region itself; a product it
        // returns is one it offers here.
        market_applied: true,
        display_name,
        package_family_name: version_info.as_ref().and_then(first_package_family),
        publisher: version_info
            .as_ref()
            .and_then(|version| optional_text(version.Publisher())),
        icon: None,
        install_classification: classification_of(version_info.as_ref()),
        version: version_info
            .as_ref()
            .and_then(|version| optional_text(version.Version())),
        size_bytes: None,
        resolver_source: ResolverSource::WinGetMsStore,
    })
}

/// The first package family the catalogue lists for this version.
///
/// Store products name exactly one; anything else is treated as unnamed rather
/// than guessed at, because a wrong identity would prove the wrong install.
fn first_package_family(version: &PackageVersionInfo) -> Option<PackageFamilyName> {
    let families = version.PackageFamilyNames().ok()?;
    if families.Size().ok()? != 1 {
        return None;
    }
    PackageFamilyName::parse(families.GetAt(0).ok()?.to_string_lossy())
}

/// Classify the delivery mechanism the catalogue reported for this product.
///
/// The installer type is an enumeration supplied before installation, which is
/// what makes the classification provable rather than guessed. Anything the
/// catalogue does not state stays `Unknown`.
#[allow(unsafe_code)]
fn classification_of(version: Option<&PackageVersionInfo>) -> InstallClassification {
    let Some(version) = version else {
        return InstallClassification::unknown();
    };
    let Ok(options) = (unsafe {
        CoCreateInstance::<_, InstallOptions>(&super::CLSID_INSTALL_OPTIONS, None, CLSCTX_ALL)
    }) else {
        return InstallClassification::unknown();
    };
    let Ok(installer) = version.GetApplicableInstaller(&options) else {
        return InstallClassification::unknown();
    };
    match installer.InstallerType() {
        Ok(PackageInstallerType::MSStore) => {
            InstallClassification::packaged_from_msstore_installer_type()
        }
        Ok(PackageInstallerType::Msix) => InstallClassification::packaged_from_observed_identity(),
        Ok(
            PackageInstallerType::Exe
            | PackageInstallerType::Msi
            | PackageInstallerType::Inno
            | PackageInstallerType::Nullsoft
            | PackageInstallerType::Wix
            | PackageInstallerType::Burn
            | PackageInstallerType::Portable,
        ) => InstallClassification::win32_from_backend_report(),
        _ => InstallClassification::unknown(),
    }
}

/// Translate an unusable API into the resolver's own vocabulary.
fn unavailable_to_failure(error: WinGetUnavailable) -> ResolveFailure {
    let cause = match error {
        WinGetUnavailable::ServerUnavailable(_) | WinGetUnavailable::MetadataMissing(_) => {
            ResolveError::ResolverUnavailable
        }
        WinGetUnavailable::SourceUnavailable(_) => ResolveError::SourceUnavailable,
    };
    failure(cause, Some(error.native_code()))
}

/// A structured failure that carries the native code but no native text.
const fn failure(error: ResolveError, native_code: Option<i32>) -> ResolveFailure {
    ResolveFailure {
        error,
        diagnostic: Some(ResolverDiagnostic { native_code }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winstoreregion_core::{InstallKind, install_supported_in_v0_1};

    #[test]
    fn only_the_msstore_installer_type_yields_an_installable_classification() {
        let msstore = InstallClassification::packaged_from_msstore_installer_type();
        assert_eq!(msstore.kind(), InstallKind::Packaged);
        assert!(install_supported_in_v0_1(msstore));

        // An `Exe` product from the same source resolves and is shown, but this
        // version cannot prove its completion, so it must not be installable.
        assert!(!install_supported_in_v0_1(
            InstallClassification::win32_from_backend_report()
        ));
    }

    /// Reaches the live Microsoft Store catalogue, so it is not part of the
    /// ordinary run: `cargo test -- --ignored resolves_a_real_product`.
    #[test]
    #[ignore = "requires network and the msstore source"]
    fn resolves_a_real_product_from_the_live_catalogue() {
        use winstoreregion_core::{MarketCode, ResolveRequest, ResolveRequestId, StoreProductId};

        let request = ResolveRequest {
            request_id: ResolveRequestId::new(1),
            product_id: StoreProductId::parse("9NZVDKPMR9RD").expect("valid product identifier"),
            market: MarketCode::parse("US").expect("valid market"),
        };
        let product = WinGetComResolver
            .resolve(&request)
            .expect("the live catalogue answers for a globally available product");

        assert_eq!(product.product_id.as_str(), "9NZVDKPMR9RD");
        assert!(!product.display_name.trim().is_empty());
        assert!(
            install_supported_in_v0_1(product.install_classification),
            "this product is delivered by Microsoft Store and must be installable"
        );
    }

    #[test]
    fn an_unavailable_source_is_not_reported_as_an_unavailable_resolver() {
        assert_eq!(
            unavailable_to_failure(WinGetUnavailable::SourceUnavailable(-5)).error,
            ResolveError::SourceUnavailable
        );
        assert_eq!(
            unavailable_to_failure(WinGetUnavailable::MetadataMissing(-6)).error,
            ResolveError::ResolverUnavailable
        );
        assert_eq!(
            unavailable_to_failure(WinGetUnavailable::ServerUnavailable(-7))
                .diagnostic
                .and_then(|diagnostic| diagnostic.native_code),
            Some(-7)
        );
    }
}
