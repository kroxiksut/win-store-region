//! Windows Home Location values and the guarded region switch.

/// Microsoft's own instructions for changing the Store country or region.
///
/// This application automates exactly the setting that page describes, and the
/// page is the vendor documenting that the change is a supported thing to do.
/// The address stays in English in every language: the localized variants of
/// this article redirect, and an English link that works is worth more than a
/// translated one that moves.
/// Where this project lives.
///
/// A constant rather than a parameter, for the same reason the Microsoft pages
/// below are: no caller may decide where the user is sent.
/// Who wrote and maintains this application.
///
/// A constant for the same reason the addresses are: what the About dialog says
/// about authorship is not something a caller gets to decide.
pub const PROJECT_AUTHOR: &str = "kroxiksut";

/// The licence this application is distributed under.
///
/// Named in the interface rather than only in a file, because a user who has
/// only the executable still has the right to know it.
pub const PROJECT_LICENSE: &str = "GPL-3.0-or-later";

pub const PROJECT_REPOSITORY: &str = "https://github.com/kroxiksut/win-store-region";

pub const MICROSOFT_REGION_DOCUMENTATION: &str = "https://support.microsoft.com/en-us/account-billing/change-your-country-or-region-in-microsoft-store-5895e006-34f4-10f7-16b1-999e40adb048";

/// Title of that article, quoted as Microsoft titles it.
pub const MICROSOFT_REGION_DOCUMENTATION_TITLE: &str =
    "Change your country or region in Microsoft Store";

/// A Windows geographical location identifier used by recovery and read-back.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GeoId(i32);

impl GeoId {
    /// Create a `GeoId` from a positive Windows geographical location value.
    #[must_use]
    pub const fn new(value: i32) -> Option<Self> {
        if value > 0 { Some(Self(value)) } else { None }
    }

    /// Return the Windows value represented by this identifier.
    #[must_use]
    pub const fn value(self) -> i32 {
        self.0
    }

    /// Classify the value returned by `GetUserGeoID`.
    ///
    /// # Errors
    ///
    /// Returns [`RegionReadError::GeoIdNotAvailable`] for the Windows sentinel
    /// and [`RegionReadError::InvalidGeoId`] for all other invalid values.
    pub const fn from_windows_user_value(value: i32) -> Result<Self, RegionReadError> {
        if value == -1 {
            return Err(RegionReadError::GeoIdNotAvailable);
        }

        match Self::new(value) {
            Some(geo_id) => Ok(geo_id),
            None => Err(RegionReadError::InvalidGeoId(value)),
        }
    }
}

/// A validated ISO 3166-1 alpha-2 country or territory code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsoAlpha2(String);

impl IsoAlpha2 {
    /// Validate and normalise an ISO alpha-2 value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            return None;
        }

        Some(Self(value.to_ascii_uppercase()))
    }

    /// Return the normalised two-letter code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Human-readable information about a Windows geographical location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Region {
    /// Canonical identifier used by safety decisions.
    pub geo_id: GeoId,
    /// Name shown to the user; never used in restore decisions.
    pub display_name: String,
    /// Optional flag hint, independent from the name and `GeoId`.
    pub iso_alpha2: Option<IsoAlpha2>,
}

impl Region {
    /// Build a region only when a usable display name is available.
    ///
    /// # Errors
    ///
    /// Returns [`RegionBuildError::MissingDisplayName`] when the platform did
    /// not provide a human-readable location name.
    pub fn new(
        geo_id: GeoId,
        display_name: impl Into<String>,
        iso_alpha2: Option<IsoAlpha2>,
    ) -> Result<Self, RegionBuildError> {
        let display_name = display_name.into();
        if display_name.trim().is_empty() {
            return Err(RegionBuildError::MissingDisplayName);
        }

        Ok(Self {
            geo_id,
            display_name,
            iso_alpha2,
        })
    }
}

/// Canonical two-letter market code passed to the selected Store backend.
///
/// The value is deliberately distinct from a URL query parameter: the market
/// always comes from the temporary region selected by the application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarketCode(String);

impl MarketCode {
    /// Parse and normalise a backend market code.
    ///
    /// # Errors
    ///
    /// Returns [`MarketCodeError::Invalid`] unless `value` is exactly two ASCII
    /// letters. The currently evaluated `msstore` backend uses this form.
    pub fn parse(value: &str) -> Result<Self, MarketCodeError> {
        let value = value.trim();
        if value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            return Err(MarketCodeError::Invalid);
        }
        Ok(Self(value.to_ascii_uppercase()))
    }

    /// Derive the backend market from the selected temporary region.
    ///
    /// # Errors
    ///
    /// Returns a structured error before any resolver call when Windows did not
    /// supply a usable alpha-2 code for the selected region.
    pub fn from_region(region: &Region) -> Result<Self, MarketResolutionError> {
        region
            .iso_alpha2
            .as_ref()
            .ok_or(MarketResolutionError::RegionHasNoMarket)
            .and_then(|code| Self::parse(code.as_str()).map_err(MarketResolutionError::Invalid))
    }

    /// Return the normalised code required by the backend.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a raw backend market code is unusable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarketCodeError {
    /// The code is not exactly two ASCII letters.
    Invalid,
}

/// Why the selected temporary region cannot determine a resolver market.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarketResolutionError {
    /// Windows did not provide an alpha-2 country or territory code.
    RegionHasNoMarket,
    /// The platform-provided code was not in the canonical backend form.
    Invalid(MarketCodeError),
}

/// The region data is incomplete for a GUI view model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionBuildError {
    /// Windows returned no usable friendly name.
    MissingDisplayName,
}

/// Structured reason why the current Windows region could not be read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionReadError {
    /// Windows has no configured user `GeoId`.
    GeoIdNotAvailable,
    /// Windows returned a `GeoId` outside the valid domain.
    InvalidGeoId(i32),
    /// A required NLS lookup failed.
    LookupFailed(RegionField),
    /// A required NLS string was not valid Unicode.
    InvalidUnicode(RegionField),
    /// Windows did not provide a usable friendly name.
    MissingDisplayName,
}

/// A component of [`Region`] that a platform lookup can fail to provide.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionField {
    /// User-facing friendly name.
    DisplayName,
}

/// Read-only boundary for the current Windows Home Location.
pub trait RegionReader {
    /// Read the current region without changing any user setting.
    ///
    /// # Errors
    ///
    /// Returns [`RegionReadError`] when Windows cannot provide a complete
    /// safety identifier and display model.
    fn current_region(&self) -> Result<Region, RegionReadError>;
}

/// An identifier that links a pending recovery record to one region change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationId(String);

impl OperationId {
    /// Validate a non-empty operation identifier supplied by the platform.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.trim().is_empty()).then_some(Self(value))
    }

    /// Return the operation identifier for persistence and diagnostics.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Supplies an operation identifier after the original region has been read.
pub trait OperationIdGenerator {
    /// Create the identifier for the next guarded operation.
    fn next_operation_id(&mut self) -> OperationId;
}

/// Data that must be durable before Windows Home Location is changed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedRegionChange {
    /// Identifier of the guarded region-changing operation.
    pub operation_id: OperationId,
    /// Region read before attempting the temporary change.
    pub original_region: GeoId,
    /// Requested temporary region.
    pub target_region: GeoId,
}

/// Persistence boundary required by a guarded temporary region change.
///
/// This boundary prevents the region writer from being invoked before a
/// concrete [`RecoveryStore`] confirms a durable prepared record.
pub trait RegionChangeGuard {
    /// Failure type reported by the durable recovery store.
    type Error;

    /// Persist a recovery record before Windows Home Location changes.
    ///
    /// # Errors
    ///
    /// Returns the concrete store error when the record was not made durable.
    fn save_prepared(&mut self, change: &PreparedRegionChange) -> Result<(), Self::Error>;

    /// Clear a prepared record only after read-back confirms the original region.
    ///
    /// # Errors
    ///
    /// Returns the concrete store error when the record could not be cleared.
    fn clear_after_original_read_back(
        &mut self,
        change: &PreparedRegionChange,
    ) -> Result<(), Self::Error>;
}

/// Narrow platform boundary for one attempt to change Windows Home Location.
pub trait RegionWriter {
    /// Request that Windows Home Location be changed to `target`.
    ///
    /// The return value is diagnostic only. The core always reads the region
    /// back and never treats this result as proof that Windows changed it.
    ///
    /// # Errors
    ///
    /// Returns a structured platform result when Windows did not accept the
    /// requested change.
    fn set_region(&self, target: GeoId) -> Result<(), RegionWriteError>;
}

/// Structured result of one platform write attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionWriteError {
    /// Windows or a policy refused the requested region change.
    Rejected,
    /// The selected platform API failed with its native error code.
    PlatformFailure(i32),
}

/// Classified result after the mandatory read-back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionSwitchOutcome {
    /// Read-back confirmed the requested temporary region; recovery remains pending.
    RegionSwitched,
    /// Read-back confirmed the original region; the pending record was cleared.
    OriginalRegionPreserved,
    /// Another region is active; core must keep recovery state and avoid a write.
    RegionConflict {
        /// Region read after the attempted write.
        current_region: GeoId,
    },
}

/// Evidence and state for a completed region-switch attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegionSwitchReport {
    /// Recovery data persisted before attempting the write.
    pub change: PreparedRegionChange,
    /// Diagnostic result from the selected writer API.
    pub writer_result: Result<(), RegionWriteError>,
    /// Result classified from the mandatory read-back.
    pub outcome: RegionSwitchOutcome,
}

/// Failure that prevents core from classifying a region-switch attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegionSwitchError<GuardError> {
    /// The original region was unavailable before the operation began.
    ReadOriginal(RegionReadError),
    /// Recovery state was not durable, so no platform write was attempted.
    PendingSave(GuardError),
    /// The region could not be read after the writer was attempted.
    ReadBack(RegionReadError),
    /// Read-back confirmed the original region, but cleanup of pending state failed.
    PendingCleanup(GuardError),
}

/// Attempt one temporary region change under the recovery safety contract.
///
/// The writer is called exactly once, only after `guard` confirms the prepared
/// record. A read-back follows both writer success and writer failure. No
/// fallback writer is accepted by this operation.
///
/// # Errors
///
/// Returns an error when the original region cannot be read, the pending state
/// is not durable, read-back fails, or safe cleanup fails. A writer error alone
/// is preserved in [`RegionSwitchReport`] and does not suppress read-back.
pub fn switch_region<R, W, I, G>(
    target_region: GeoId,
    reader: &R,
    writer: &W,
    operation_ids: &mut I,
    guard: &mut G,
) -> Result<RegionSwitchReport, RegionSwitchError<G::Error>>
where
    R: RegionReader,
    W: RegionWriter,
    I: OperationIdGenerator,
    G: RegionChangeGuard,
{
    let original_region = reader
        .current_region()
        .map_err(RegionSwitchError::ReadOriginal)?
        .geo_id;
    let change = PreparedRegionChange {
        operation_id: operation_ids.next_operation_id(),
        original_region,
        target_region,
    };
    guard
        .save_prepared(&change)
        .map_err(RegionSwitchError::PendingSave)?;

    let writer_result = writer.set_region(target_region);
    let current_region = reader
        .current_region()
        .map_err(RegionSwitchError::ReadBack)?
        .geo_id;
    let outcome = if current_region == target_region {
        RegionSwitchOutcome::RegionSwitched
    } else if current_region == original_region {
        guard
            .clear_after_original_read_back(&change)
            .map_err(RegionSwitchError::PendingCleanup)?;
        RegionSwitchOutcome::OriginalRegionPreserved
    } else {
        RegionSwitchOutcome::RegionConflict { current_region }
    };

    Ok(RegionSwitchReport {
        change,
        writer_result,
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::RecoveryStore;
    use crate::recovery::store::PendingRestoreGuard;
    use crate::test_support::{
        MemoryRecoveryStore, SequenceReader, TestGuard, TestGuardError, TestOperationIds,
        TestWriter, geo_id, pending_restore_for,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn market_is_normalised_from_the_selected_region_not_from_the_source_url() {
        let region = Region::new(geo_id(840), "United States", IsoAlpha2::parse("us"))
            .expect("complete region");
        assert_eq!(
            MarketCode::from_region(&region)
                .expect("region market")
                .as_str(),
            "US"
        );
        let incomplete = Region::new(geo_id(840), "United States", None)
            .expect("display name is sufficient for a region view");
        assert_eq!(
            MarketCode::from_region(&incomplete),
            Err(MarketResolutionError::RegionHasNoMarket)
        );
    }

    #[test]
    fn the_vendor_documentation_link_is_one_fixed_microsoft_address() {
        // A page this product points at must be Microsoft's own and reached
        // over TLS; anything else would send a user somewhere on our word.
        assert!(MICROSOFT_REGION_DOCUMENTATION.starts_with("https://support.microsoft.com/"));
        // The repository address is shipped, so it is checked like any other:
        // it must be this project's own page and it must be https.
        assert_eq!(
            PROJECT_REPOSITORY,
            "https://github.com/kroxiksut/win-store-region"
        );
        assert!(!MICROSOFT_REGION_DOCUMENTATION_TITLE.trim().is_empty());
    }

    #[test]
    fn geo_id_rejects_missing_and_negative_values() {
        assert_eq!(GeoId::new(244).map(GeoId::value), Some(244));
        assert_eq!(GeoId::new(0), None);
        assert_eq!(GeoId::new(-1), None);
        assert_eq!(
            GeoId::from_windows_user_value(-1),
            Err(super::RegionReadError::GeoIdNotAvailable)
        );
    }

    #[test]
    fn iso_alpha_two_is_optional_presentation_data() {
        assert_eq!(
            IsoAlpha2::parse("us").as_ref().map(IsoAlpha2::as_str),
            Some("US")
        );
        assert_eq!(IsoAlpha2::parse("840"), None);
        assert_eq!(IsoAlpha2::parse(""), None);
    }

    #[test]
    fn region_requires_a_human_readable_name() {
        let geo_id = GeoId::new(244).expect("positive GeoId");
        assert_eq!(
            Region::new(geo_id, "   ", None),
            Err(RegionBuildError::MissingDisplayName)
        );
    }

    #[test]
    fn region_switch_keeps_pending_when_read_back_reaches_target() {
        let original = geo_id(244);
        let target = geo_id(840);
        let events = Rc::new(RefCell::new(Vec::new()));
        let reader = SequenceReader::from_geo_ids([original, target]);
        let writer = TestWriter {
            result: Ok(()),
            events: Rc::clone(&events),
        };
        let mut operation_ids = TestOperationIds;
        let mut guard = TestGuard {
            save_result: Ok(()),
            cleanup_result: Ok(()),
            events: Rc::clone(&events),
        };

        let report = switch_region(target, &reader, &writer, &mut operation_ids, &mut guard)
            .expect("target read-back is a classified result");

        assert_eq!(report.change.original_region, original);
        assert_eq!(report.change.target_region, target);
        assert_eq!(report.outcome, RegionSwitchOutcome::RegionSwitched);
        assert_eq!(events.borrow().as_slice(), ["save", "write"]);
    }

    #[test]
    fn region_switch_uses_the_recovery_store_guard_for_both_durable_outcomes() {
        let original = geo_id(244);
        let target = geo_id(840);
        let events = Rc::new(RefCell::new(Vec::new()));

        let reader = SequenceReader::from_geo_ids([original, target]);
        let writer = TestWriter {
            result: Ok(()),
            events: Rc::clone(&events),
        };
        let mut operation_ids = TestOperationIds;
        let pending = pending_restore_for("test-operation");
        let mut guard = PendingRestoreGuard::new(MemoryRecoveryStore::default(), pending.clone());

        let report = switch_region(target, &reader, &writer, &mut operation_ids, &mut guard)
            .expect("target read-back is a classified result");

        assert_eq!(report.outcome, RegionSwitchOutcome::RegionSwitched);
        assert_eq!(guard.pending(), &pending);
        assert_eq!(guard.into_store().load(), Ok(Some(pending)));
        assert_eq!(events.borrow().as_slice(), ["write"]);

        let reader = SequenceReader::from_geo_ids([original, original]);
        let writer = TestWriter {
            result: Err(RegionWriteError::Rejected),
            events,
        };
        let mut operation_ids = TestOperationIds;
        let pending = pending_restore_for("test-operation");
        let mut guard = PendingRestoreGuard::new(MemoryRecoveryStore::default(), pending);

        let report = switch_region(target, &reader, &writer, &mut operation_ids, &mut guard)
            .expect("original read-back is a classified result");

        assert_eq!(report.outcome, RegionSwitchOutcome::OriginalRegionPreserved);
        assert_eq!(guard.into_store().load(), Ok(None));
    }

    #[test]
    fn region_switch_cleans_pending_only_after_original_read_back() {
        let original = geo_id(244);
        let target = geo_id(840);
        let events = Rc::new(RefCell::new(Vec::new()));
        let reader = SequenceReader::from_geo_ids([original, original]);
        let writer = TestWriter {
            result: Err(RegionWriteError::Rejected),
            events: Rc::clone(&events),
        };
        let mut operation_ids = TestOperationIds;
        let mut guard = TestGuard {
            save_result: Ok(()),
            cleanup_result: Ok(()),
            events: Rc::clone(&events),
        };

        let report = switch_region(target, &reader, &writer, &mut operation_ids, &mut guard)
            .expect("original read-back is a classified result");

        assert_eq!(report.writer_result, Err(RegionWriteError::Rejected));
        assert_eq!(report.outcome, RegionSwitchOutcome::OriginalRegionPreserved);
        assert_eq!(events.borrow().as_slice(), ["save", "write", "cleanup"]);
    }

    #[test]
    fn region_switch_preserves_pending_for_an_external_region_conflict() {
        let original = geo_id(244);
        let target = geo_id(840);
        let externally_changed = geo_id(276);
        let events = Rc::new(RefCell::new(Vec::new()));
        let reader = SequenceReader::from_geo_ids([original, externally_changed]);
        let writer = TestWriter {
            result: Ok(()),
            events: Rc::clone(&events),
        };
        let mut operation_ids = TestOperationIds;
        let mut guard = TestGuard {
            save_result: Ok(()),
            cleanup_result: Ok(()),
            events: Rc::clone(&events),
        };

        let report = switch_region(target, &reader, &writer, &mut operation_ids, &mut guard)
            .expect("conflict read-back is a classified result");

        assert_eq!(
            report.outcome,
            RegionSwitchOutcome::RegionConflict {
                current_region: externally_changed,
            }
        );
        assert_eq!(events.borrow().as_slice(), ["save", "write"]);
    }

    #[test]
    fn region_switch_does_not_write_without_durable_pending_state() {
        let original = geo_id(244);
        let target = geo_id(840);
        let events = Rc::new(RefCell::new(Vec::new()));
        let reader = SequenceReader::from_geo_ids([original]);
        let writer = TestWriter {
            result: Ok(()),
            events: Rc::clone(&events),
        };
        let mut operation_ids = TestOperationIds;
        let mut guard = TestGuard {
            save_result: Err(TestGuardError::Save),
            cleanup_result: Ok(()),
            events: Rc::clone(&events),
        };

        let result = switch_region(target, &reader, &writer, &mut operation_ids, &mut guard);

        assert_eq!(
            result,
            Err(RegionSwitchError::PendingSave(TestGuardError::Save))
        );
        assert_eq!(events.borrow().as_slice(), ["save"]);
    }

    #[test]
    fn region_switch_reports_failed_pending_cleanup() {
        let original = geo_id(244);
        let target = geo_id(840);
        let events = Rc::new(RefCell::new(Vec::new()));
        let reader = SequenceReader::from_geo_ids([original, original]);
        let writer = TestWriter {
            result: Ok(()),
            events: Rc::clone(&events),
        };
        let mut operation_ids = TestOperationIds;
        let mut guard = TestGuard {
            save_result: Ok(()),
            cleanup_result: Err(TestGuardError::Cleanup),
            events: Rc::clone(&events),
        };

        let result = switch_region(target, &reader, &writer, &mut operation_ids, &mut guard);

        assert_eq!(
            result,
            Err(RegionSwitchError::PendingCleanup(TestGuardError::Cleanup))
        );
        assert_eq!(events.borrow().as_slice(), ["save", "write", "cleanup"]);
    }
}
