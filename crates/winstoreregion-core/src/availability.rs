//! Where a product is offered, established before any region changes.
//!
//! This is a preliminary reference, not permission to install. A guarded
//! operation still resolves the product under the temporary region it actually
//! holds, and nothing here may stand in for that. The value of the reference is
//! that a user picks a region knowing where the application exists, instead of
//! choosing blindly from every nation Windows knows.
//!
//! Core owns the question and the arithmetic. It does not know what performs a
//! probe, and it never learns that a network exists.

use crate::input::StoreProductId;
use crate::install::{InstallClassification, InstallKind};
use crate::observe::PackageFamilyName;
use crate::region::MarketCode;

/// Markets asked first, in the order they are asked.
///
/// The set is curated rather than complete: the endpoint answers one market per
/// request, so a full sweep costs one request per nation. The order puts the
/// markets a Store product is most often offered in at the front, so an answer
/// usually arrives before the sweep ends.
pub const CURATED_MARKETS: [&str; 40] = [
    "US", "GB", "CA", "AU", "NZ", "IE", "DE", "FR", "IT", "ES", "NL", "BE", "AT", "CH", "SE", "NO",
    "DK", "FI", "PL", "CZ", "PT", "GR", "HU", "RO", "TR", "UA", "RU", "JP", "KR", "TW", "HK", "SG",
    "MY", "TH", "ID", "PH", "VN", "IN", "BR", "MX",
];

/// What one market answered about one product.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarketAvailability {
    /// The source offers this product in this market.
    Offered,
    /// The source answered, and does not offer this product here.
    NotOffered,
    /// No answer was obtained, so nothing is known about this market.
    ///
    /// Deliberately distinct from [`Self::NotOffered`]: a market that could not
    /// be reached may well be the one the user needs, and hiding it behind a
    /// refusal would make the reference lie.
    Unknown,
}

/// Product facts a manifest carries, before any installation is contemplated.
///
/// This is not a [`crate::resolve::ResolvedProduct`] and cannot become one: it
/// describes what a market offers, not a product an operation resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfferedProduct {
    pub product_id: StoreProductId,
    pub market: MarketCode,
    pub display_name: String,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub package_family_name: Option<PackageFamilyName>,
    pub install_classification: InstallClassification,
}

impl OfferedProduct {
    /// Whether v0.1 could install what this market offers.
    ///
    /// The answer is about the delivery mechanism only. It never admits an
    /// installation on its own: the operation classifies the product again from
    /// its own resolve.
    #[must_use]
    pub const fn is_installable_kind(&self) -> bool {
        matches!(self.install_classification.kind(), InstallKind::Packaged)
    }
}

/// One market's complete answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarketAnswer {
    pub market: MarketCode,
    pub availability: MarketAvailability,
    /// Present only for a market that offers the product.
    pub offered: Option<OfferedProduct>,
}

impl MarketAnswer {
    /// An answer that names a market without knowing anything about it.
    #[must_use]
    pub const fn unknown(market: MarketCode) -> Self {
        Self {
            market,
            availability: MarketAvailability::Unknown,
            offered: None,
        }
    }

    /// An answer from a market that does not offer the product.
    #[must_use]
    pub const fn not_offered(market: MarketCode) -> Self {
        Self {
            market,
            availability: MarketAvailability::NotOffered,
            offered: None,
        }
    }

    /// An answer from a market that offers the product.
    #[must_use]
    pub fn offered(offered: OfferedProduct) -> Self {
        Self {
            market: offered.market.clone(),
            availability: MarketAvailability::Offered,
            offered: Some(offered),
        }
    }
}

/// How much of the world a survey has covered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurveyScope {
    /// Only the curated set was asked, so the answer is incomplete by design.
    Curated,
    /// Every market the machine knows was asked.
    Complete,
}

/// What is known about one product across markets.
///
/// The survey belongs to exactly one product. A different Product ID is a
/// different question, and its answers never inherit these.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailabilitySurvey {
    product_id: StoreProductId,
    scope: SurveyScope,
    answers: Vec<MarketAnswer>,
}

impl AvailabilitySurvey {
    /// Begin a survey of one product over the curated set.
    #[must_use]
    pub const fn new(product_id: StoreProductId) -> Self {
        Self {
            product_id,
            scope: SurveyScope::Curated,
            answers: Vec::new(),
        }
    }

    /// The product this survey answers about.
    #[must_use]
    pub const fn product_id(&self) -> &StoreProductId {
        &self.product_id
    }

    /// Whether this survey still answers about `product_id`.
    ///
    /// A survey that does not is stale, and a caller replaces it rather than
    /// adding to it: answers from another product are not evidence here.
    #[must_use]
    pub fn answers_about(&self, product_id: &StoreProductId) -> bool {
        &self.product_id == product_id
    }

    /// How much of the world this survey has covered.
    #[must_use]
    pub const fn scope(&self) -> SurveyScope {
        self.scope
    }

    /// Declare that every known market has now been asked.
    pub const fn widen_to_complete(&mut self) {
        self.scope = SurveyScope::Complete;
    }

    /// Record one answer, replacing any earlier answer about the same market.
    ///
    /// Replacing matters for a market that was unreachable once: a later, real
    /// answer must win over the `Unknown` left by a failed attempt.
    pub fn record(&mut self, answer: MarketAnswer) {
        match self
            .answers
            .iter_mut()
            .find(|existing| existing.market == answer.market)
        {
            Some(existing) => *existing = answer,
            None => self.answers.push(answer),
        }
    }

    /// Every answer gathered so far, in the order the markets were asked.
    #[must_use]
    pub fn answers(&self) -> &[MarketAnswer] {
        &self.answers
    }

    /// The answer for one market, when it has been asked.
    #[must_use]
    pub fn answer_for(&self, market: &MarketCode) -> Option<&MarketAnswer> {
        self.answers.iter().find(|answer| &answer.market == market)
    }

    /// Markets that offer the product, in the order they were asked.
    #[must_use]
    pub fn offering_markets(&self) -> Vec<MarketCode> {
        self.answers
            .iter()
            .filter(|answer| answer.availability == MarketAvailability::Offered)
            .map(|answer| answer.market.clone())
            .collect()
    }

    /// How many asked markets produced no answer at all.
    #[must_use]
    pub fn unknown_count(&self) -> usize {
        self.answers
            .iter()
            .filter(|answer| answer.availability == MarketAvailability::Unknown)
            .count()
    }

    /// Whether the survey covers everything it could, with nothing unresolved.
    ///
    /// Only a complete, fully answered survey may claim that a market missing
    /// from [`Self::offering_markets`] does not offer the product.
    #[must_use]
    pub fn is_conclusive(&self) -> bool {
        self.scope == SurveyScope::Complete && self.unknown_count() == 0
    }

    /// Markets from `candidates` this survey has not answered yet.
    ///
    /// A market whose answer is `Unknown` is offered again: nothing was learned
    /// about it, and the second step is exactly the chance to learn it.
    #[must_use]
    pub fn unasked(&self, candidates: &[MarketCode]) -> Vec<MarketCode> {
        candidates
            .iter()
            .filter(|candidate| {
                self.answer_for(candidate)
                    .is_none_or(|answer| answer.availability == MarketAvailability::Unknown)
            })
            .cloned()
            .collect()
    }
}

/// The curated set as market codes, skipping anything malformed.
///
/// The constant is written by hand, so a typo would otherwise become a silent
/// request for a market that cannot exist.
#[must_use]
pub fn curated_markets() -> Vec<MarketCode> {
    CURATED_MARKETS
        .iter()
        .filter_map(|code| MarketCode::parse(code).ok())
        .collect()
}

/// Something that can ask one market about one product.
///
/// The contract says nothing about how the question travels. Core does not know
/// that a network is involved, and a test implementation is a plain function.
pub trait MarketProber {
    /// Ask one market, answering `Unknown` rather than failing.
    fn probe(&self, product_id: &StoreProductId, market: &MarketCode) -> MarketAnswer;
}

/// Device families a Windows desktop can actually receive a package from.
///
/// `Windows.Universal` belongs here beside `Windows.Desktop`: the catalogue
/// lists both for products that install on a PC, and treating a universal
/// package as another device's would refuse an installation that works. Everything else — `Windows.Xbox`, `Windows.Windows8x`
/// and the rest — is a device this machine is not, and telling those apart adds
/// nothing, because the answer to the user is the same.
pub const DESKTOP_DEVICE_FAMILIES: [&str; 2] = ["Windows.Desktop", "Windows.Universal"];

/// Whether a catalogue package targets something this desktop can receive.
#[must_use]
pub fn is_desktop_family(platform: &str) -> bool {
    DESKTOP_DEVICE_FAMILIES.contains(&platform)
}

/// One package as the catalogue describes it, reduced to what decides delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogPackage {
    /// Device family the package targets, verbatim from the catalogue.
    pub platform: String,
    /// Lowest version of that family the package accepts, packed as the
    /// catalogue packs it: four sixteen-bit fields, most significant first.
    pub min_version: u64,
    /// How the catalogue itself ranks this package against the others.
    ///
    /// The ordering signal that matters. A product can offer `1.4.3.0` and
    /// `2021.917.2041.0` at once, where the second is the older release under a
    /// numbering scheme the publisher has since abandoned. The rank tells them
    /// apart; comparing the numbers does not.
    pub rank: i64,
    /// Version of the package itself, as the catalogue spells it.
    ///
    /// Absent when the catalogue named no package version. It is shown, never
    /// compared with the installed one: the two can belong to different
    /// numbering schemes, and then order between them means nothing.
    pub version: Option<String>,
}

/// A Windows version packed the way the Store catalogue packs it.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PackedVersion(u64);

impl PackedVersion {
    /// Build one from the four fields Windows reports.
    #[must_use]
    pub const fn from_parts(major: u16, minor: u16, build: u16, revision: u16) -> Self {
        Self((major as u64) << 48 | (minor as u64) << 32 | (build as u64) << 16 | (revision as u64))
    }

    /// Take a value the catalogue already packed.
    #[must_use]
    pub const fn from_packed(packed: u64) -> Self {
        Self(packed)
    }

    /// The four fields again, for a message a person has to read.
    #[must_use]
    pub const fn parts(self) -> (u16, u16, u16, u16) {
        #[allow(clippy::cast_possible_truncation)]
        (
            (self.0 >> 48) as u16,
            (self.0 >> 32) as u16,
            (self.0 >> 16) as u16,
            self.0 as u16,
        )
    }
}

/// Whether Microsoft Store can deliver a product to this machine at all.
///
/// This is not availability: a product can be offered in a market and still be
/// undeliverable to this machine. The two questions are kept apart because
/// their remedies differ — one is solved by choosing another region, the other
/// by nothing the user can do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceCompatibility {
    /// At least one package targets Windows desktop and accepts this version.
    Deliverable,
    /// Packages exist, and none of them is for Windows desktop.
    OtherDeviceOnly,
    /// Desktop packages exist and every one of them wants a newer Windows.
    NewerWindowsRequired {
        /// The lowest version any desktop package accepts.
        required: PackedVersion,
    },
    /// Nothing was learned, so nothing may be refused on this ground.
    ///
    /// Deliberately distinct from every refusal above: silence from the
    /// catalogue is the ordinary consequence of a network hiccup, and blocking
    /// an installation on it would turn every offline moment into a wall.
    Unknown,
}

impl DeviceCompatibility {
    /// Whether this verdict is a reason to refuse before touching the region.
    ///
    /// Only a stated refusal counts. [`Self::Unknown`] never blocks.
    #[must_use]
    pub const fn blocks_installation(self) -> bool {
        matches!(
            self,
            Self::OtherDeviceOnly | Self::NewerWindowsRequired { .. }
        )
    }
}

/// Decide what the catalogue's package list means for this machine.
///
/// An empty list is [`DeviceCompatibility::Unknown`], not a refusal: a product
/// whose packages could not be read says nothing about where it can run.
#[must_use]
pub fn evaluate_device_compatibility(
    packages: &[CatalogPackage],
    device: PackedVersion,
) -> DeviceCompatibility {
    if packages.is_empty() {
        return DeviceCompatibility::Unknown;
    }
    let desktop: Vec<&CatalogPackage> = packages
        .iter()
        .filter(|package| is_desktop_family(&package.platform))
        .collect();
    let Some(lowest) = desktop
        .iter()
        .map(|package| PackedVersion::from_packed(package.min_version))
        .min()
    else {
        return DeviceCompatibility::OtherDeviceOnly;
    };
    if lowest > device {
        return DeviceCompatibility::NewerWindowsRequired { required: lowest };
    }
    DeviceCompatibility::Deliverable
}

/// One Store application already installed on this machine.
///
/// Deliberately free of any verdict. What it means for updates is decided by
/// [`UpdateCandidate`], which needs an answer from the catalogue this type has
/// never seen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledStoreApplication {
    pub family_name: PackageFamilyName,
    pub display_name: String,
    /// Version Windows reports for the installed package.
    pub installed_version: String,
}

/// What this application can say about updating one installed product.
///
/// `winget` cannot update a Store product at all — it answers "no applicable update" because
/// the `msstore` source reports the version as `Unknown`. So this type does not
/// claim that an update exists. It reports the one thing that is provable and
/// actionable: whether Microsoft Store serves this product in the region the
/// machine is in right now. A product it does not serve here will never be
/// updated here, whatever version either side happens to hold.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateCandidate {
    pub application: InstalledStoreApplication,
    /// Store identifier, when the catalogue recognised the package family.
    pub product_id: Option<StoreProductId>,
    /// What the current market answered about this product.
    pub availability: MarketAvailability,
    /// Version the catalogue offers for this device, when it names one.
    ///
    /// Shown beside the installed version so the user can see both. Whether one
    /// is newer than the other is not claimed here.
    pub offered_version: Option<String>,
    /// Whether the source offers this product in the reference market at all.
    ///
    /// Without this, a refusal here cannot be called regional. Most products a
    /// market refuses are refused by every market: they are simply absent from
    /// this source, as first-party Windows applications are, and their region
    /// was never the obstacle. Listing those tells the user to fix something
    /// that is not broken.
    pub offered_elsewhere: bool,
}

impl UpdateCandidate {
    /// Whether Microsoft Store refuses to serve this product in this region.
    ///
    /// Only a stated refusal counts. Silence is not a refusal here for the same
    /// reason it is not one anywhere else in this product: an unanswered market
    /// may be the one that works.
    #[must_use]
    pub fn store_refuses_here(&self) -> bool {
        self.availability == MarketAvailability::NotOffered
    }

    /// Whether the refusal here is about the region and not about the source.
    ///
    /// Both halves are needed: refused in this market, and offered in one this
    /// application knows works. Anything else is a product this source does not
    /// carry, which no region changes.
    #[must_use]
    pub fn is_regional_refusal(&self) -> bool {
        self.store_refuses_here() && self.offered_elsewhere
    }

    /// Whether the installed version is exactly the one the catalogue offers.
    ///
    /// String equality and nothing cleverer: two versions that read the same
    /// are the same, and two that do not may still be incomparable, which is
    /// why no order is inferred here.
    #[must_use]
    pub fn versions_match(&self) -> bool {
        self.offered_version
            .as_ref()
            .is_some_and(|offered| offered == &self.application.installed_version)
    }

    /// Whether this entry is worth putting in front of the user.
    ///
    /// It has to be a regional refusal, it has to be something this window can
    /// act on, and it must not already hold exactly what the catalogue offers.
    /// A product whose versions match has nothing to gain from a region change,
    /// however unserved it is here.
    #[must_use]
    pub fn is_worth_listing(&self) -> bool {
        self.is_regional_refusal() && self.is_actionable() && !self.versions_match()
    }

    /// Whether this window can offer to do anything about it.
    ///
    /// Without a Product ID there is no operation to start: every path this
    /// application has — install, region search, Store installer — is addressed
    /// by Product ID and by nothing else.
    #[must_use]
    pub const fn is_actionable(&self) -> bool {
        self.product_id.is_some()
    }
}

/// The version the catalogue would deliver to this device, if it names one.
///
/// Only packages this device could actually receive are considered, so the
/// answer matches what an installation here would get. Among those the highest
/// version wins, compared field by field as numbers — that ordering is safe
/// because these are versions of one product from one source. It is deliberately
/// **not** compared with the installed version; see [`CatalogPackage::version`].
#[must_use]
pub fn offered_version(packages: &[CatalogPackage], device: PackedVersion) -> Option<String> {
    packages
        .iter()
        .filter(|package| is_desktop_family(&package.platform))
        .filter(|package| PackedVersion::from_packed(package.min_version) <= device)
        .filter(|package| package.version.is_some())
        .max_by_key(|package| package.rank)
        .and_then(|package| package.version.clone())
}

/// Ask the catalogue what it would deliver to this device.
///
/// Separate from [`MarketProber`] because it answers a different question and
/// may be answered by a different source. Core still knows nothing about how
/// the question travels.
pub trait DeviceCompatibilityProber {
    /// Ask about one product in one market, answering `Unknown` rather than
    /// failing.
    fn compatibility(
        &self,
        product_id: &StoreProductId,
        market: &MarketCode,
        device: PackedVersion,
    ) -> DeviceCompatibility;
}

/// Recognise an installed package as a Store product.
///
/// Separate from every other question because it is answered by a different
/// request and can fail on its own: a package family the catalogue does not
/// know is not an error, it is an application this product cannot act on.
pub trait StoreProductLookup {
    /// Name the product one installed package family belongs to, if any.
    fn product_for_family(&self, family_name: &PackageFamilyName) -> Option<StoreProductId>;
}

#[cfg(test)]
mod device_tests {
    use super::*;

    /// A Windows 10 22H2 desktop, as the catalogue sees one.
    fn this_machine() -> PackedVersion {
        PackedVersion::from_parts(10, 0, 19045, 5965)
    }

    fn package(platform: &str, min: PackedVersion) -> CatalogPackage {
        CatalogPackage {
            platform: platform.to_owned(),
            min_version: min.0,
            rank: 0,
            version: None,
        }
    }

    #[test]
    fn a_product_built_only_for_another_device_is_refused_before_any_region_changes() {
        // A product whose every package targets Xbox.
        let xbox = PackedVersion::from_parts(10, 0, 14393, 0);
        let packages = [package("Windows.Xbox", xbox), package("Windows.Xbox", xbox)];
        assert_eq!(
            evaluate_device_compatibility(&packages, this_machine()),
            DeviceCompatibility::OtherDeviceOnly
        );
        assert!(DeviceCompatibility::OtherDeviceOnly.blocks_installation());
    }

    #[test]
    fn one_desktop_package_among_others_is_enough_to_deliver() {
        let packages = [
            package("Windows.Xbox", PackedVersion::from_parts(10, 0, 14393, 0)),
            package(
                "Windows.Desktop",
                PackedVersion::from_parts(10, 0, 17763, 0),
            ),
        ];
        assert_eq!(
            evaluate_device_compatibility(&packages, this_machine()),
            DeviceCompatibility::Deliverable
        );
    }

    #[test]
    fn a_desktop_package_wanting_a_newer_windows_names_the_version_it_wants() {
        let required = PackedVersion::from_parts(10, 0, 22000, 0);
        let packages = [package("Windows.Desktop", required)];
        assert_eq!(
            evaluate_device_compatibility(&packages, this_machine()),
            DeviceCompatibility::NewerWindowsRequired { required }
        );
        assert_eq!(required.parts(), (10, 0, 22000, 0));
    }

    #[test]
    fn the_lowest_desktop_requirement_decides_not_the_first_one_listed() {
        let packages = [
            package(
                "Windows.Desktop",
                PackedVersion::from_parts(10, 0, 22000, 0),
            ),
            package(
                "Windows.Desktop",
                PackedVersion::from_parts(10, 0, 17763, 0),
            ),
        ];
        assert_eq!(
            evaluate_device_compatibility(&packages, this_machine()),
            DeviceCompatibility::Deliverable
        );
    }

    #[test]
    fn silence_from_the_catalogue_never_blocks_an_installation() {
        assert_eq!(
            evaluate_device_compatibility(&[], this_machine()),
            DeviceCompatibility::Unknown
        );
        assert!(!DeviceCompatibility::Unknown.blocks_installation());
        assert!(!DeviceCompatibility::Deliverable.blocks_installation());
    }

    fn installed(name: &str) -> InstalledStoreApplication {
        InstalledStoreApplication {
            family_name: PackageFamilyName::parse(format!("{name}_2p2nqsd0c76g0"))
                .expect("valid family name"),
            display_name: name.to_owned(),
            installed_version: "1.2026.190.0".to_owned(),
        }
    }

    fn candidate(availability: MarketAvailability, known: bool) -> UpdateCandidate {
        UpdateCandidate {
            application: installed("OpenAI.ChatGPT-Desktop"),
            product_id: known
                .then(|| StoreProductId::parse("9NT1R1C2HH7J").expect("valid Product ID")),
            availability,
            offered_version: Some("2026.709.1617.0".to_owned()),
            offered_elsewhere: true,
        }
    }

    #[test]
    fn the_offered_version_is_the_one_the_catalogue_ranks_highest() {
        // The installed version is
        // 1.4.3.0, and it is the highest-ranked desktop package — while
        // 2021.917.2041.0 is the highest *number* and an older release.
        let device = this_machine();
        let packages = vec![
            CatalogPackage {
                platform: "Windows.Desktop".to_owned(),
                min_version: PackedVersion::from_parts(10, 0, 19003, 0).0,
                rank: 30010,
                version: Some("2021.917.2041.0".to_owned()),
            },
            CatalogPackage {
                platform: "Windows.Desktop".to_owned(),
                min_version: PackedVersion::from_parts(10, 0, 19041, 0).0,
                rank: 30030,
                version: Some("1.4.3.0".to_owned()),
            },
            CatalogPackage {
                platform: "Windows.Desktop".to_owned(),
                min_version: PackedVersion::from_parts(10, 0, 16299, 0).0,
                rank: 30021,
                version: Some("1.4.2.0".to_owned()),
            },
            CatalogPackage {
                platform: "Windows.Xbox".to_owned(),
                min_version: PackedVersion::from_parts(10, 0, 14393, 0).0,
                rank: 30042,
                version: Some("1.10.5.70".to_owned()),
            },
        ];
        assert_eq!(
            offered_version(&packages, device),
            Some("1.4.3.0".to_owned())
        );
        assert_eq!(offered_version(&[], device), None);
    }

    #[test]
    fn a_package_this_windows_is_too_old_for_is_never_offered() {
        let device = this_machine();
        let packages = vec![CatalogPackage {
            platform: "Windows.Desktop".to_owned(),
            min_version: PackedVersion::from_parts(10, 0, 22000, 0).0,
            rank: 99999,
            version: Some("3.0.0.0".to_owned()),
        }];
        assert_eq!(offered_version(&packages, device), None);
    }

    #[test]
    fn a_universal_package_counts_as_one_this_desktop_can_receive() {
        // the catalogue lists `Windows.Universal` beside
        // `Windows.Desktop` for products that install on a PC.
        let device = this_machine();
        let packages = vec![CatalogPackage {
            platform: "Windows.Universal".to_owned(),
            min_version: PackedVersion::from_parts(10, 0, 17763, 0).0,
            rank: 30015,
            version: Some("2.2631.102.0".to_owned()),
        }];
        assert_eq!(
            evaluate_device_compatibility(&packages, device),
            DeviceCompatibility::Deliverable
        );
        assert_eq!(
            offered_version(&packages, device),
            Some("2.2631.102.0".to_owned())
        );
    }

    #[test]
    fn only_a_stated_refusal_marks_a_product_as_unserved_here() {
        assert!(candidate(MarketAvailability::NotOffered, true).store_refuses_here());
        // Silence is not a refusal, here as everywhere else in this product.
        assert!(!candidate(MarketAvailability::Unknown, true).store_refuses_here());
        assert!(!candidate(MarketAvailability::Offered, true).store_refuses_here());
    }

    #[test]
    fn a_refusal_every_market_makes_is_not_about_the_region() {
        // A product this source does not carry answers the same way in every
        // market, and its region was never the obstacle.
        let mut absent = candidate(MarketAvailability::NotOffered, true);
        absent.offered_elsewhere = false;
        assert!(absent.store_refuses_here());
        assert!(!absent.is_regional_refusal());
        assert!(!absent.is_worth_listing());

        // ChatGPT: refused in RU, offered in US. This one is about the region.
        let regional = candidate(MarketAvailability::NotOffered, true);
        assert!(regional.is_regional_refusal());
        assert!(regional.is_worth_listing());
    }

    #[test]
    fn a_product_that_already_holds_the_offered_version_is_not_listed() {
        let mut current = candidate(MarketAvailability::NotOffered, true);
        current.offered_version = Some(current.application.installed_version.clone());
        assert!(current.is_regional_refusal());
        assert!(current.versions_match());
        assert!(!current.is_worth_listing());
    }

    #[test]
    fn an_application_the_catalogue_does_not_recognise_offers_no_action() {
        // Every path this product has is addressed by Product ID, so without
        // one there is nothing a window could start.
        assert!(!candidate(MarketAvailability::NotOffered, false).is_actionable());
        assert!(candidate(MarketAvailability::NotOffered, true).is_actionable());
    }

    #[test]
    fn the_packed_form_matches_what_the_store_log_printed() {
        // The form the Store log prints a device version in.
        assert_eq!(this_machine().0, 2_814_751_015_245_645);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product() -> StoreProductId {
        StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID")
    }

    fn market(code: &str) -> MarketCode {
        MarketCode::parse(code).expect("valid market")
    }

    fn offered_in(code: &str) -> MarketAnswer {
        MarketAnswer::offered(OfferedProduct {
            product_id: product(),
            market: market(code),
            display_name: "Hulu".to_owned(),
            publisher: Some("Hulu, LLC".to_owned()),
            description: None,
            version: None,
            package_family_name: None,
            install_classification: InstallClassification::packaged_from_msstore_installer_type(),
        })
    }

    /// Answers from a fixed table, and nothing for anything else.
    struct FakeProber {
        offering: Vec<&'static str>,
        silent: Vec<&'static str>,
    }

    impl MarketProber for FakeProber {
        fn probe(&self, product_id: &StoreProductId, market: &MarketCode) -> MarketAnswer {
            if self.silent.contains(&market.as_str()) {
                return MarketAnswer::unknown(market.clone());
            }
            if self.offering.contains(&market.as_str()) {
                return MarketAnswer::offered(OfferedProduct {
                    product_id: product_id.clone(),
                    market: market.clone(),
                    display_name: "Tubi".to_owned(),
                    publisher: None,
                    description: None,
                    version: None,
                    package_family_name: None,
                    install_classification:
                        InstallClassification::packaged_from_msstore_installer_type(),
                });
            }
            MarketAnswer::not_offered(market.clone())
        }
    }

    #[test]
    fn a_survey_over_the_curated_set_separates_offers_refusals_and_silence() {
        let prober = FakeProber {
            offering: vec!["US", "GB", "CA"],
            silent: vec!["JP"],
        };
        let mut survey = AvailabilitySurvey::new(product());
        for market in curated_markets() {
            survey.record(prober.probe(&product(), &market));
        }

        assert_eq!(
            survey
                .offering_markets()
                .iter()
                .map(MarketCode::as_str)
                .collect::<Vec<_>>(),
            vec!["US", "GB", "CA"]
        );
        assert_eq!(survey.unknown_count(), 1);
        // Asked as widely as this pass goes, and still not conclusive: the set
        // is curated, and one market never answered.
        assert!(!survey.is_conclusive());
        assert_eq!(
            survey.unasked(&curated_markets()),
            vec![MarketCode::parse("JP").expect("valid market")]
        );
    }

    #[test]
    fn the_curated_set_is_forty_valid_markets_without_repetition() {
        let markets = curated_markets();
        let mut codes: Vec<_> = markets.iter().map(MarketCode::as_str).collect();
        codes.sort_unstable();
        let unique = codes.len();
        codes.dedup();

        assert_eq!(markets.len(), CURATED_MARKETS.len());
        assert_eq!(codes.len(), unique, "a market must not be asked twice");
        assert_eq!(markets.first().map(MarketCode::as_str), Some("US"));
    }

    #[test]
    fn an_unreachable_market_is_never_reported_as_refusing_the_product() {
        let mut survey = AvailabilitySurvey::new(product());
        survey.record(offered_in("US"));
        survey.record(MarketAnswer::not_offered(market("RU")));
        survey.record(MarketAnswer::unknown(market("DE")));

        assert_eq!(
            survey
                .offering_markets()
                .iter()
                .map(MarketCode::as_str)
                .collect::<Vec<_>>(),
            vec!["US"]
        );
        assert_eq!(survey.unknown_count(), 1);
        assert!(
            !survey.is_conclusive(),
            "an unanswered market leaves the survey open"
        );
        assert_eq!(
            survey.unasked(&[market("DE"), market("RU"), market("JP")]),
            vec![market("DE"), market("JP")],
            "an unknown market is asked again; a refusal is not"
        );
    }

    #[test]
    fn a_later_answer_replaces_the_silence_of_an_earlier_attempt() {
        let mut survey = AvailabilitySurvey::new(product());
        survey.record(MarketAnswer::unknown(market("GB")));
        survey.record(offered_in("GB"));

        assert_eq!(survey.answers().len(), 1);
        assert_eq!(survey.unknown_count(), 0);
        assert_eq!(
            survey
                .answer_for(&market("GB"))
                .map(|answer| answer.availability),
            Some(MarketAvailability::Offered)
        );
    }

    #[test]
    fn only_a_complete_and_fully_answered_survey_may_claim_to_be_conclusive() {
        let mut survey = AvailabilitySurvey::new(product());
        survey.record(offered_in("US"));
        survey.record(MarketAnswer::not_offered(market("RU")));

        assert_eq!(survey.scope(), SurveyScope::Curated);
        assert!(!survey.is_conclusive());

        survey.widen_to_complete();
        assert!(survey.is_conclusive());
    }

    #[test]
    fn a_survey_answers_about_one_product_and_says_so_when_asked_about_another() {
        let survey = AvailabilitySurvey::new(product());
        let other = StoreProductId::parse("9N1SV6841F0B").expect("valid Product ID");

        assert!(survey.answers_about(&product()));
        assert!(!survey.answers_about(&other));
    }

    #[test]
    fn an_offer_describes_delivery_and_never_admits_an_installation_by_itself() {
        let MarketAnswer {
            offered: Some(offered),
            ..
        } = offered_in("US")
        else {
            panic!("an offered market carries the product it offers");
        };

        assert!(offered.is_installable_kind());
        assert_eq!(
            offered.install_classification,
            InstallClassification::packaged_from_msstore_installer_type()
        );
    }
}
