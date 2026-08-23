//! Asking one market whether it offers a product, over `WinHTTP`.
//!
//! The source answers per market rather than per machine, which is what makes a
//! region choice informed instead of blind. This adapter performs one GET and
//! translates the answer; it decides nothing, and a failure of any kind becomes
//! "unknown" rather than a refusal, because a market nobody could reach may
//! well be the one the user needs.
//!
//! `WinHTTP` is used because it is part of Windows: a third-party client would
//! bring its own TLS stack and multiply the size of a portable executable for
//! the sake of one request.

use serde_json::Value;
use std::ffi::c_void;
use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::core::{HSTRING, PCWSTR};
use winstoreregion_core::{
    APPLICATION_NAME, CatalogPackage, DeviceCompatibility, DeviceCompatibilityProber,
    InstallClassification, MarketAnswer, MarketCode, MarketProber, OfferedProduct,
    PackageFamilyName, PackedVersion, StoreProductId, StoreProductLookup,
    evaluate_device_compatibility, offered_version,
};

/// Host serving the `msstore` package manifests.
const MANIFEST_HOST: &str = "storeedgefd.dsx.mp.microsoft.com";

/// Host serving the catalogue entry the Store installer itself consults.
///
/// This is the request Microsoft Store makes before it installs, so its answer
/// is what decides whether a product can be delivered to this device at all.
const CATALOG_HOST: &str = "displaycatalog.mp.microsoft.com";

/// Largest response body this adapter will read.
///
/// A manifest is about 6 KB. The cap exists so a surprising answer cannot grow
/// without bound in a window that is waiting for it.
const MAX_BODY_BYTES: usize = 512 * 1024;

/// Size of one read from the response stream.
const READ_CHUNK_BYTES: usize = 8 * 1024;

/// Timeouts in milliseconds: resolve, connect, send, receive.
///
/// A probe is a convenience. Waiting a long time for one market would make the
/// whole survey feel broken, so an unreachable market is given up on quickly.
const TIMEOUTS: (i32, i32, i32, i32) = (5_000, 5_000, 10_000, 15_000);

/// Market used only to name a product, never to judge its availability.
///
/// The lookup answers who a package family belongs to, and that answer is the
/// same everywhere. Availability is a different question, asked of a different
/// host, with the market that actually matters: this catalogue will answer
/// about a product for a market whose Store does not offer it.
const LOOKUP_MARKET: &str = "US";

/// The installer type this product is delivered by, as the manifest spells it.
const MSSTORE_INSTALLER_TYPE: &str = "msstore";

/// A version the source does not actually know.
const UNKNOWN_VERSION: &str = "Unknown";

/// An open `WinHTTP` handle that closes itself.
struct Handle(*mut c_void);

impl Handle {
    /// Take ownership of a handle, rejecting the null one an API failure returns.
    fn new(raw: *mut c_void) -> Option<Self> {
        (!raw.is_null()).then_some(Self(raw))
    }

    const fn raw(&self) -> *mut c_void {
        self.0
    }
}

impl Drop for Handle {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // Nothing can be done about a failed close, and an operation must not
        // end differently because a diagnostic handle misbehaved.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

/// One `WinHTTP` session used for market probes.
///
/// A session is deliberately not shared between threads: each worker opens its
/// own, so no handle is used from two places at once.
pub(crate) struct WinHttpMarketProber {
    session: Handle,
}

impl WinHttpMarketProber {
    /// Open a session, or report that `WinHTTP` is unavailable.
    #[allow(unsafe_code)]
    pub(crate) fn open() -> Option<Self> {
        let agent = HSTRING::from(APPLICATION_NAME);
        let session = Handle::new(unsafe {
            WinHttpOpen(
                PCWSTR(agent.as_ptr()),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        })?;
        let (resolve, connect, send, receive) = TIMEOUTS;
        unsafe { WinHttpSetTimeouts(session.raw(), resolve, connect, send, receive) }.ok()?;
        Some(Self { session })
    }

    /// Perform one GET and return its status code with the body it carried.
    #[allow(unsafe_code)]
    fn get(&self, host: &str, path: &str) -> Option<(u32, Vec<u8>)> {
        let host = HSTRING::from(host);
        let connection = Handle::new(unsafe {
            WinHttpConnect(
                self.session.raw(),
                PCWSTR(host.as_ptr()),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            )
        })?;

        let verb = HSTRING::from("GET");
        let object = HSTRING::from(path);
        let request = Handle::new(unsafe {
            WinHttpOpenRequest(
                connection.raw(),
                PCWSTR(verb.as_ptr()),
                PCWSTR(object.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_OPEN_REQUEST_FLAGS(WINHTTP_FLAG_SECURE.0),
            )
        })?;

        unsafe { WinHttpSendRequest(request.raw(), None, None, 0, 0, 0) }.ok()?;
        unsafe { WinHttpReceiveResponse(request.raw(), std::ptr::null_mut()) }.ok()?;

        let mut status: u32 = 0;
        let mut status_size = u32::try_from(size_of::<u32>()).ok()?;
        unsafe {
            WinHttpQueryHeaders(
                request.raw(),
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(std::ptr::from_mut(&mut status).cast::<c_void>()),
                &raw mut status_size,
                std::ptr::null_mut(),
            )
        }
        .ok()?;

        Some((status, read_body(&request)))
    }
}

impl MarketProber for WinHttpMarketProber {
    fn probe(&self, product_id: &StoreProductId, market: &MarketCode) -> MarketAnswer {
        let path = format!(
            "/v9.0/packageManifests/{}?Market={}",
            product_id.as_str(),
            market.as_str()
        );
        let Some((status, body)) = self.get(MANIFEST_HOST, &path) else {
            return MarketAnswer::unknown(market.clone());
        };
        match status {
            200 => offered_product(product_id, market, &body).map_or_else(
                // The market answered, but not with something this application
                // can describe. Claiming a refusal would be a different fact.
                || MarketAnswer::unknown(market.clone()),
                MarketAnswer::offered,
            ),
            404 => MarketAnswer::not_offered(market.clone()),
            _ => MarketAnswer::unknown(market.clone()),
        }
    }
}

impl DeviceCompatibilityProber for WinHttpMarketProber {
    fn compatibility(
        &self,
        product_id: &StoreProductId,
        market: &MarketCode,
        device: PackedVersion,
    ) -> DeviceCompatibility {
        // `InstallAgent` is the template Microsoft Store itself asks for, so
        // the packages read here are the ones delivery is decided from.
        let path = format!(
            "/v7.0/products/{}/?fieldsTemplate=InstallAgent&market={}&languages=neutral",
            product_id.as_str(),
            market.as_str()
        );
        let Some((200, body)) = self.get(CATALOG_HOST, &path) else {
            return DeviceCompatibility::Unknown;
        };
        evaluate_device_compatibility(&catalog_packages(&body), device)
    }
}

impl StoreProductLookup for WinHttpMarketProber {
    fn product_for_family(&self, family_name: &PackageFamilyName) -> Option<StoreProductId> {
        // A package family name is letters, digits, dots and underscores, so it
        // needs no escaping; nothing user-typed reaches this path in any case.
        let path = format!(
            "/v7.0/products/lookup?alternateId=PackageFamilyName&value={}&market={}&languages=neutral&fieldsTemplate=InstallAgent",
            family_name.as_str(),
            LOOKUP_MARKET
        );
        let (200, body) = self.get(CATALOG_HOST, &path)? else {
            return None;
        };
        let root = serde_json::from_slice::<Value>(&body).ok()?;
        let identifier = root
            .get("Products")
            .and_then(Value::as_array)?
            .first()?
            .get("ProductId")
            .and_then(Value::as_str)?;
        StoreProductId::parse(identifier).ok()
    }
}

impl WinHttpMarketProber {
    /// The version this catalogue would deliver to this device, if it names one.
    ///
    /// Asked of the same host and template the delivery check uses, so the
    /// packages read here are the ones an installation would receive.
    pub(crate) fn offered_version(
        &self,
        product_id: &StoreProductId,
        market: &MarketCode,
        device: PackedVersion,
    ) -> Option<String> {
        let path = format!(
            "/v7.0/products/{}/?fieldsTemplate=InstallAgent&market={}&languages=neutral",
            product_id.as_str(),
            market.as_str()
        );
        let (200, body) = self.get(CATALOG_HOST, &path)? else {
            return None;
        };
        offered_version(&catalog_packages(&body), device)
    }
}

/// Pull every package the catalogue lists, reduced to what decides delivery.
///
/// Anything unreadable yields an empty list, which core turns into `Unknown`.
/// This function never decides; it only reports what the answer contained.
fn catalog_packages(body: &[u8]) -> Vec<CatalogPackage> {
    let Ok(root) = serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };
    let mut packages = Vec::new();
    let skus = root
        .get("Product")
        .and_then(|product| product.get("DisplaySkuAvailabilities"))
        .and_then(Value::as_array);
    for sku in skus.into_iter().flatten() {
        let listed = sku
            .get("Sku")
            .and_then(|sku| sku.get("Properties"))
            .and_then(|properties| properties.get("Packages"))
            .and_then(Value::as_array);
        for package in listed.into_iter().flatten() {
            let dependencies = package
                .get("PlatformDependencies")
                .and_then(Value::as_array);
            for dependency in dependencies.into_iter().flatten() {
                let Some(platform) = dependency.get("PlatformName").and_then(Value::as_str) else {
                    continue;
                };
                // The catalogue has been seen writing the packed version both
                // as a number and as a string; neither form is an error.
                let min_version = dependency
                    .get("MinVersion")
                    .and_then(|value| {
                        value
                            .as_u64()
                            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
                    })
                    .unwrap_or(0);
                packages.push(CatalogPackage {
                    platform: platform.to_owned(),
                    min_version,
                    // The catalogue's own ordering of its packages. Absent
                    // means last: a package the catalogue did not rank cannot
                    // outrank one it did.
                    rank: package
                        .get("PackageRank")
                        .and_then(Value::as_i64)
                        .unwrap_or(i64::MIN),
                    version: package_version(package),
                });
            }
        }
    }
    packages
}

/// The version a machine would end up with after installing this package.
///
/// Not the version in `PackageFullName`: that names the **bundle**, and a
/// bundle is numbered independently of what it contains.
///
/// The bundled packages are listed inside `PlatformDependencyXmlBlob`, which is
/// JSON despite its name. The bundle's own version is the fallback for a
/// package that is not a bundle at all.
fn package_version(package: &Value) -> Option<String> {
    let bundle_version = package
        .get("PackageFullName")
        .and_then(Value::as_str)
        .and_then(version_of_full_name);
    let Some(blob) = package
        .get("PlatformDependencyXmlBlob")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
    else {
        return bundle_version;
    };
    let bundled = blob
        .get("content.bundledPackages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    // Architecture-neutral first, then the one this build runs as. Anything
    // else belongs to a machine that is not this one.
    bundled
        .iter()
        .filter_map(Value::as_str)
        .find(|full_name| {
            matches!(
                architecture_of_full_name(full_name),
                Some("neutral" | "x64")
            )
        })
        .and_then(version_of_full_name)
        .or(bundle_version)
}

/// The version segment of `Name_Version_Architecture__PublisherId`.
fn version_of_full_name(full_name: &str) -> Option<String> {
    full_name.split('_').nth(1).map(str::to_owned)
}

/// The architecture segment of the same shape.
fn architecture_of_full_name(full_name: &str) -> Option<&str> {
    full_name.split('_').nth(2)
}

/// The version of Windows this machine is actually running.
///
/// Read from the registry rather than from `GetVersionExW`, which reports what
/// the executable's manifest claims compatibility with instead of the truth.
/// An unreadable value becomes zero in its field: a version that reads lower
/// than reality can only make this product more permissive, never less.
#[allow(unsafe_code)]
pub(crate) fn current_windows_version() -> PackedVersion {
    let key = HSTRING::from(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let number = |name: &str| -> u32 {
        let name = HSTRING::from(name);
        let mut value: u32 = 0;
        let mut size = u32::try_from(size_of::<u32>()).unwrap_or(4);
        let read = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some(std::ptr::from_mut(&mut value).cast::<c_void>()),
                Some(&raw mut size),
            )
        };
        if read.is_ok() { value } else { 0 }
    };
    // `CurrentBuildNumber` is a string; the numeric `CurrentBuild` beside it is
    // absent on some builds, so the string is parsed instead.
    let build = string_value(&key, "CurrentBuildNumber")
        .and_then(|text| text.parse::<u16>().ok())
        .unwrap_or(0);
    let major = u16::try_from(number("CurrentMajorVersionNumber")).unwrap_or(0);
    let minor = u16::try_from(number("CurrentMinorVersionNumber")).unwrap_or(0);
    let revision = u16::try_from(number("UBR")).unwrap_or(0);
    PackedVersion::from_parts(major, minor, build, revision)
}

/// Read one string value, or nothing if it is absent or unreadable.
#[allow(unsafe_code)]
fn string_value(key: &HSTRING, name: &str) -> Option<String> {
    let name = HSTRING::from(name);
    let mut size: u32 = 0;
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&raw mut size),
        )
    }
    .ok()
    .ok()?;
    let mut buffer = vec![0u16; (size as usize / 2) + 1];
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            Some(&raw mut size),
        )
    }
    .ok()
    .ok()?;
    let end = buffer.iter().position(|unit| *unit == 0)?;
    Some(String::from_utf16_lossy(&buffer[..end]))
}

/// Read the response body up to the cap, stopping at the first failure.
#[allow(unsafe_code)]
fn read_body(request: &Handle) -> Vec<u8> {
    let mut body = Vec::new();
    let mut chunk = vec![0_u8; READ_CHUNK_BYTES];
    loop {
        let mut read: u32 = 0;
        let Ok(chunk_size) = u32::try_from(chunk.len()) else {
            return body;
        };
        if unsafe {
            WinHttpReadData(
                request.raw(),
                chunk.as_mut_ptr().cast::<c_void>(),
                chunk_size,
                &raw mut read,
            )
        }
        .is_err()
        {
            return body;
        }
        let read = read as usize;
        if read == 0 {
            return body;
        }
        body.extend_from_slice(&chunk[..read]);
        if body.len() >= MAX_BODY_BYTES {
            return body;
        }
    }
}

/// Turn one manifest into the product facts the market offers.
///
/// Returns nothing unless the manifest names this exact product and describes
/// an installer: a body that says something else is not evidence about the
/// product that was asked for.
fn offered_product(
    product_id: &StoreProductId,
    market: &MarketCode,
    body: &[u8],
) -> Option<OfferedProduct> {
    let manifest: Value = serde_json::from_slice(body).ok()?;
    let data = manifest.get("Data")?;
    if !data
        .get("PackageIdentifier")
        .and_then(Value::as_str)
        .is_some_and(|identifier| identifier.eq_ignore_ascii_case(product_id.as_str()))
    {
        return None;
    }
    let version = data.get("Versions")?.as_array()?.first()?;
    let locale = version.get("DefaultLocale")?;
    let display_name = text(locale.get("PackageName"))?;

    let installer = version
        .get("Installers")?
        .as_array()?
        .iter()
        .find(|installer| {
            installer
                .get("InstallerType")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.eq_ignore_ascii_case(MSSTORE_INSTALLER_TYPE))
        });

    Some(OfferedProduct {
        product_id: product_id.clone(),
        market: market.clone(),
        display_name,
        publisher: text(locale.get("Publisher")),
        description: text(locale.get("ShortDescription")),
        // The source reports a literal "Unknown" for Store products, which is
        // an absent version rather than one.
        version: text(version.get("PackageVersion"))
            .filter(|value| !value.eq_ignore_ascii_case(UNKNOWN_VERSION)),
        package_family_name: installer
            .and_then(|installer| text(installer.get("PackageFamilyName")))
            .and_then(PackageFamilyName::parse),
        // The classification comes from the reported installer type, which is
        // the only evidence v0.1 accepts, and from nothing else.
        install_classification: if installer.is_some() {
            InstallClassification::packaged_from_msstore_installer_type()
        } else {
            InstallClassification::unknown()
        },
    })
}

/// A trimmed, non-empty string from an optional JSON value.
fn text(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use winstoreregion_core::InstallKind;

    fn product() -> StoreProductId {
        StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID")
    }

    fn market() -> MarketCode {
        MarketCode::parse("US").expect("valid market")
    }

    const COMPLETE_MANIFEST: &str = r#"{
        "Data": {
            "PackageIdentifier": "9WZDNCRFJ3L1",
            "Versions": [{
                "PackageVersion": "Unknown",
                "DefaultLocale": {
                    "Publisher": "Hulu, LLC",
                    "PackageName": "Hulu",
                    "ShortDescription": "Watch full seasons."
                },
                "Installers": [{
                    "InstallerType": "msstore",
                    "PackageFamilyName": "HuluLLC.HuluPlus_fphbd361v8tya"
                }]
            }]
        }
    }"#;

    #[test]
    fn a_complete_manifest_becomes_the_offer_its_market_makes() {
        let offered = offered_product(&product(), &market(), COMPLETE_MANIFEST.as_bytes())
            .expect("a manifest about this product");

        assert_eq!(offered.display_name, "Hulu");
        assert_eq!(offered.publisher.as_deref(), Some("Hulu, LLC"));
        assert_eq!(
            offered
                .package_family_name
                .as_ref()
                .map(PackageFamilyName::as_str),
            Some("HuluLLC.HuluPlus_fphbd361v8tya")
        );
        assert!(offered.is_installable_kind());
        assert_eq!(
            offered.version, None,
            "the source reports a literal Unknown, which is not a version"
        );
    }

    #[test]
    fn a_manifest_about_another_product_is_not_evidence_about_this_one() {
        let other = StoreProductId::parse("9N1SV6841F0B").expect("valid Product ID");

        assert!(offered_product(&other, &market(), COMPLETE_MANIFEST.as_bytes()).is_none());
    }

    #[test]
    fn a_manifest_without_an_msstore_installer_stays_unclassified() {
        let manifest = COMPLETE_MANIFEST.replace("\"msstore\"", "\"exe\"");
        let offered = offered_product(&product(), &market(), manifest.as_bytes())
            .expect("a manifest about this product");

        assert_eq!(offered.install_classification.kind(), InstallKind::Unknown);
        assert!(!offered.is_installable_kind());
        assert_eq!(offered.package_family_name, None);
    }

    #[test]
    fn neither_rubbish_nor_a_truncated_manifest_produces_an_offer() {
        assert!(offered_product(&product(), &market(), b"not json at all").is_none());
        assert!(
            offered_product(
                &product(),
                &market(),
                br#"{"Data":{"PackageIdentifier":"9WZDNCRFJ3L1"}}"#
            )
            .is_none()
        );
    }

    /// Asks the live source about one product in two markets. Network test.
    #[test]
    #[ignore = "reaches the Microsoft Store source over the network"]
    fn the_live_catalogue_refuses_an_xbox_only_product_and_admits_a_desktop_one() {
        let prober = WinHttpMarketProber::open().expect("WinHTTP is part of Windows");
        let device = current_windows_version();
        let us = MarketCode::parse("US").expect("valid market");

        // One product built for Xbox only, and one ordinary desktop one.
        let xbox_only = StoreProductId::parse("9PL67R4P9PG5").expect("valid product id");
        let desktop = StoreProductId::parse("9WZDNCRFJ3L1").expect("valid product id");

        assert_eq!(
            prober.compatibility(&xbox_only, &us, device),
            DeviceCompatibility::OtherDeviceOnly
        );
        assert_eq!(
            prober.compatibility(&desktop, &us, device),
            DeviceCompatibility::Deliverable
        );
    }

    #[test]
    fn this_machine_reports_a_version_the_catalogue_could_compare() {
        let version = current_windows_version();
        let (major, _, build, _) = version.parts();
        assert!(major >= 10, "unexpected major version {major}");
        assert!(build > 0, "the build number was not read");
    }

    #[test]
    fn a_catalogue_answer_nobody_can_read_refuses_nothing() {
        assert!(catalog_packages(b"not json at all").is_empty());
        assert!(catalog_packages(br#"{"Product":{}}"#).is_empty());
    }

    #[test]
    fn the_version_read_is_the_one_that_lands_on_the_machine_not_the_bundles() {
        //  bundle 2026.709.1617.0, installed
        // package 1.2026.190.0. Reading the bundle called a current
        // application out of date.
        let blob = serde_json::json!({
            "content.bundledPackages": [
                "OpenAI.ChatGPT-Desktop_1.2026.190.0_arm64__2p2nqsd0c76g0",
                "OpenAI.ChatGPT-Desktop_1.2026.190.0_x64__2p2nqsd0c76g0",
            ]
        })
        .to_string();
        let package = serde_json::json!({
            "PackageFullName": "OpenAI.ChatGPT-Desktop_2026.709.1617.0_neutral_~_2p2nqsd0c76g0",
            "PlatformDependencyXmlBlob": blob,
        });
        assert_eq!(package_version(&package), Some("1.2026.190.0".to_owned()));
    }

    #[test]
    fn a_package_that_is_not_a_bundle_still_reports_its_own_version() {
        let package = serde_json::json!({
            "PackageFullName": "Publisher.App_1.4.3.0_neutral_~_hash",
        });
        assert_eq!(package_version(&package), Some("1.4.3.0".to_owned()));
    }

    #[test]
    fn a_bundle_holding_only_another_architecture_falls_back_to_its_own_name() {
        let blob = serde_json::json!({
            "content.bundledPackages": ["Publisher.App_9.9.9.9_arm64__hash"]
        })
        .to_string();
        let package = serde_json::json!({
            "PackageFullName": "Publisher.App_1.4.3.0_neutral_~_hash",
            "PlatformDependencyXmlBlob": blob,
        });
        assert_eq!(package_version(&package), Some("1.4.3.0".to_owned()));
    }

    #[test]
    fn packages_are_read_from_the_shape_the_catalogue_actually_returns() {
        let body = br#"{"Product":{"DisplaySkuAvailabilities":[{"Sku":{"Properties":{"Packages":[
            {"PlatformDependencies":[{"PlatformName":"Windows.Xbox","MinVersion":2814750931222528}]},
            {"PlatformDependencies":[{"PlatformName":"Windows.Desktop","MinVersion":"2814750931222528"}]}
        ]}}}]}}"#;
        let packages = catalog_packages(body);
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].platform, "Windows.Xbox");
        // The string form is read as the same number as the numeric one.
        assert_eq!(packages[1].platform, "Windows.Desktop");
        assert_eq!(packages[0].min_version, packages[1].min_version);
    }

    #[test]
    #[ignore = "reaches the live Microsoft Store catalogue"]
    fn the_live_source_answers_per_market_and_not_per_machine() {
        let prober = WinHttpMarketProber::open().expect("WinHTTP is part of Windows");

        let offered = prober.probe(&product(), &market());
        let refused = prober.probe(&product(), &MarketCode::parse("RU").expect("valid market"));

        assert_eq!(
            offered.availability,
            winstoreregion_core::MarketAvailability::Offered
        );
        assert_eq!(
            refused.availability,
            winstoreregion_core::MarketAvailability::NotOffered
        );
    }
}
