//! User-facing classification of failures that already happened.
//!
//! This module diagnoses nothing on its own: it turns a structured error that
//! some layer already produced into one closed cause plus the technical detail
//! that belongs in a details area. It never reaches the network, never changes
//! a region, and never relaxes a safety gate.
//!
//! Localized text lives in the presentation layer. Core hands out a code so
//! that a wording change is never a domain change.

use crate::input::StoreInputError;
use crate::install::InstallBackendError;
use crate::recovery::{RecoveryStoreError, StartupRecoveryError};
use crate::region::{OperationId, RegionReadError, RegionWriteError};
use crate::resolve::{ResolveError, ResolveFailure};
use crate::source::HandoffRefusal;
use crate::store_page::StorePageOpenError;

/// How much attention a diagnostic needs.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DiagnosticSeverity {
    /// The user should know, but nothing is at risk.
    Notice,
    /// The requested step did not happen.
    Failure,
    /// Recovery is required or the Windows region may not be the original one.
    Unsafe,
}

/// What the user may reasonably do next.
///
/// The list exists so the window never has to re-derive which buttons a failure
/// leaves meaningful, and never offers an action that cannot work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticAction {
    /// Correct the application input and resolve again.
    CorrectInput,
    /// Choose a different temporary region.
    ChooseAnotherRegion,
    /// Open the product page in Microsoft Store; this is not an installation.
    OpenStorePage,
    /// Resolve pending recovery before anything else.
    ResolveRecovery,
    /// Keep waiting for the current observation.
    ContinueWaiting,
    /// Retry the same step unchanged.
    Retry,
    /// Copy the technical block for a report.
    CopyDetails,
}

/// Closed set of causes a user can be shown.
///
/// A new cause is a deliberate addition, not a formatted string. Variants whose
/// producer does not exist yet are marked below; they stay in the set so the
/// presentation layer is written once, not twice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticCode {
    /// The typed application source is not a Product ID, URL, or Store URI.
    StoreInputNotUnderstood,
    /// The exact product identifier resolved to nothing.
    ProductNotFound,
    /// The product exists but is not offered in the requested market.
    ProductUnavailableInRegion,
    /// No product resolver could be reached.
    ResolverUnavailable,
    /// The requested market is not accepted by the resolver.
    UnsupportedMarket,
    /// The resolver answered, but not about the requested product.
    ResolverAnswerMismatch,
    /// Windows did not report the current region.
    RegionUnavailable,
    /// The region did not change to the requested value.
    RegionUnchanged,
    /// Windows or policy refused the region change.
    RegionChangeRejected,
    /// The active region belongs to neither the original nor the temporary one.
    RegionConflict,
    /// The recovery record could not be read or validated safely.
    RecoveryRecordUnreadable,
    /// Restoration did not end with the original region confirmed.
    RestoreFailed,
    /// The operation history could not be read or replaced safely.
    JournalUnavailable,
    /// The selected file could not be inspected.
    InstallerFileUnreadable,
    /// The selected file is not a Microsoft Store installer stub.
    InstallerFileNotAStoreStub,
    /// The chosen file is not one existing regular `.exe`.
    InstallerFileNotSupported,
    /// The file may not be executed: its signature or signer failed the gate.
    ///
    /// This is the refusal of the one place in the product that runs foreign
    /// code, so it is its own cause rather than a shade of "unreadable".
    InstallerFileNotAdmitted,
    /// Windows refused to start the verified installer.
    InstallerLaunchRejected,
    /// Windows refused to open the Store product page.
    StorePageLaunchRejected,
    /// The clipboard was not available.
    ClipboardUnavailable,
    /// No installation backend is available for this product.
    BackendUnavailable,
    /// The backend refused or failed the exact install request.
    BackendFailed,
    /// The backend accepted the request but completion is not proven.
    CompletionUncertain,
    /// `WinGet` is not present. Produced once block 12 defines detection.
    WinGetMissing,
    /// The `msstore` source is not reachable. Awaits block 12.
    MsStoreSourceUnavailable,
    /// The `msstore` source terms were not accepted, so nothing ran.
    ///
    /// Measured on the test VM: every operation stops with `-1978335162`
    /// before doing any work. This is not the same as an unreachable source
    /// and no download fixes it.
    MsStoreSourceTermsNotAccepted,
    /// Microsoft Store is not available. Awaits block 13.
    MicrosoftStoreUnavailable,
    /// No network connectivity. Awaits blocks 12 and 13.
    NetworkUnavailable,
    /// A Microsoft Account sign-in is required. Awaits block 13.
    MicrosoftAccountRequired,
    /// Microsoft Store refused without a machine-readable cause.
    ///
    /// Account and address restrictions are indistinguishable from here, so
    /// this cause states the refusal and lists hypotheses in the details rather
    /// than asserting one. Awaits block 13.
    StoreRefusedWithoutReason,
    /// The source rejected the install because the temporary region was not in force.
    ///
    /// Measured as `0x8A150010`. It is deliberately separate from
    /// [`DiagnosticCode::ProductUnavailableInRegion`]: the product does exist in
    /// the chosen region, and repeating the operation can succeed.
    TemporaryRegionNotApplied,
    /// The source had nothing under this identifier when the install was asked for.
    ///
    /// Deliberately separate from [`DiagnosticCode::TemporaryRegionNotApplied`]:
    /// the region was confirmed by read-back and the catalogue answered under
    /// it moments earlier, so blaming the region would be a false statement
    /// about the one fact this operation proved. A product withdrawn between
    /// the resolve and the request looks exactly like this.
    ProductNoLongerOffered,
    /// The same product is already being installed.
    ///
    /// Measured as `0x80070652`. The install in flight may belong to another
    /// application, so this is a refusal to start a second one, not a failure.
    ProductAlreadyInstalling,
    /// The product is already installed, so this operation had nothing to do.
    ///
    /// Distinct from a failure: the machine is in the state the user wanted.
    /// It is also distinct from a completed installation, because this
    /// operation did not perform one.
    ProductAlreadyInstalled,
    /// The operation attempted a step that does not follow from its state.
    ///
    /// This is an internal sequencing fault, not a machine or network problem.
    /// It is surfaced rather than hidden, because the alternative is an
    /// operation that silently does nothing.
    OperationSequenceViolated,
    /// This version does not install products delivered this way.
    ///
    /// The product resolved and the backend works; only its installer type is
    /// out of scope, and nothing the user does on this machine changes that.
    InstallKindNotSupported,
    /// Microsoft Store will not deliver this product to this device.
    ///
    /// Measured as `0x803FB104`: the Store log said `Availability not allowed on
    /// DeviceFamily = Windows.Desktop` and the catalogue declared every package
    /// of that product under `Windows.Xbox` only. The region is not the
    /// obstacle and no region can become one, so this is
    /// deliberately separate from
    /// [`DiagnosticCode::ProductUnavailableInRegion`].
    ProductNotOnThisDevice,
    /// The backend finished with a failure code nothing here can name.
    ///
    /// Distinct from [`DiagnosticCode::CompletionUncertain`], which claims only
    /// that the outcome is unknown. Here the backend stated that it failed; the
    /// number is carried in the technical detail because inventing a meaning
    /// for it would be worse than showing it.
    BackendReportedUnknownFailure,
}

impl DiagnosticCode {
    /// Stable machine-facing name, used by the diagnostic log and by details.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProductAlreadyInstalled => "product_already_installed",
            Self::OperationSequenceViolated => "operation_sequence_violated",
            Self::TemporaryRegionNotApplied => "temporary_region_not_applied",
            Self::ProductNoLongerOffered => "product_no_longer_offered",
            Self::ProductAlreadyInstalling => "product_already_installing",
            Self::InstallKindNotSupported => "install_kind_not_supported",
            Self::ProductNotOnThisDevice => "product_not_on_this_device",
            Self::BackendReportedUnknownFailure => "backend_reported_unknown_failure",
            Self::StoreInputNotUnderstood => "store_input_not_understood",
            Self::ProductNotFound => "product_not_found",
            Self::ProductUnavailableInRegion => "product_unavailable_in_region",
            Self::ResolverUnavailable => "resolver_unavailable",
            Self::UnsupportedMarket => "unsupported_market",
            Self::ResolverAnswerMismatch => "resolver_answer_mismatch",
            Self::RegionUnavailable => "region_unavailable",
            Self::RegionUnchanged => "region_unchanged",
            Self::RegionChangeRejected => "region_change_rejected",
            Self::RegionConflict => "region_conflict",
            Self::RecoveryRecordUnreadable => "recovery_record_unreadable",
            Self::RestoreFailed => "restore_failed",
            Self::JournalUnavailable => "journal_unavailable",
            Self::InstallerFileUnreadable => "installer_file_unreadable",
            Self::InstallerFileNotAStoreStub => "installer_file_not_a_store_stub",
            Self::InstallerFileNotSupported => "installer_file_not_supported",
            Self::InstallerFileNotAdmitted => "installer_file_not_admitted",
            Self::InstallerLaunchRejected => "installer_launch_rejected",
            Self::StorePageLaunchRejected => "store_page_launch_rejected",
            Self::ClipboardUnavailable => "clipboard_unavailable",
            Self::BackendUnavailable => "backend_unavailable",
            Self::BackendFailed => "backend_failed",
            Self::CompletionUncertain => "completion_uncertain",
            Self::WinGetMissing => "winget_missing",
            Self::MsStoreSourceUnavailable => "msstore_source_unavailable",
            Self::MsStoreSourceTermsNotAccepted => "msstore_source_terms_not_accepted",
            Self::MicrosoftStoreUnavailable => "microsoft_store_unavailable",
            Self::NetworkUnavailable => "network_unavailable",
            Self::MicrosoftAccountRequired => "microsoft_account_required",
            Self::StoreRefusedWithoutReason => "store_refused_without_reason",
        }
    }

    /// How much attention this cause needs.
    #[must_use]
    pub const fn severity(self) -> DiagnosticSeverity {
        match self {
            Self::RegionConflict
            | Self::RecoveryRecordUnreadable
            | Self::RestoreFailed
            | Self::CompletionUncertain => DiagnosticSeverity::Unsafe,
            Self::StorePageLaunchRejected | Self::ClipboardUnavailable => {
                DiagnosticSeverity::Notice
            }
            _ => DiagnosticSeverity::Failure,
        }
    }

    /// Actions that remain meaningful after this cause.
    ///
    /// `CopyDetails` is available everywhere and is therefore not repeated
    /// here; the presentation layer always offers it beside the details.
    #[must_use]
    pub const fn permitted_actions(self) -> &'static [DiagnosticAction] {
        match self {
            Self::StoreInputNotUnderstood
            | Self::ProductNotFound
            | Self::ResolverAnswerMismatch
            | Self::InstallerFileNotAStoreStub
            | Self::InstallerFileNotSupported
            | Self::InstallerFileNotAdmitted
            | Self::InstallerFileUnreadable => &[DiagnosticAction::CorrectInput],
            Self::ProductUnavailableInRegion | Self::UnsupportedMarket => {
                &[DiagnosticAction::ChooseAnotherRegion]
            }
            // The region was right; only its timing was wrong, so repeating the
            // operation is the fix rather than choosing somewhere else.
            Self::TemporaryRegionNotApplied | Self::ProductAlreadyInstalling => {
                &[DiagnosticAction::Retry]
            }
            // Nothing about the input or the region was wrong, so the two
            // things left are to ask again and to ask somewhere else.
            Self::ProductNoLongerOffered => &[
                DiagnosticAction::Retry,
                DiagnosticAction::ChooseAnotherRegion,
            ],

            Self::RegionUnchanged | Self::RegionChangeRejected => &[
                DiagnosticAction::ChooseAnotherRegion,
                DiagnosticAction::Retry,
            ],
            Self::RegionConflict | Self::RecoveryRecordUnreadable | Self::RestoreFailed => {
                &[DiagnosticAction::ResolveRecovery]
            }
            Self::CompletionUncertain => &[
                DiagnosticAction::ContinueWaiting,
                DiagnosticAction::ResolveRecovery,
            ],
            // Neither case is a fault and neither has anything to retry: the
            // Store page is the only thing left worth offering.
            // Nothing on this machine changes the answer, so the page is all
            // that is left: it is where the device requirements are written.
            Self::ProductNotOnThisDevice
            | Self::ProductAlreadyInstalled
            | Self::InstallKindNotSupported
            | Self::BackendUnavailable
            | Self::WinGetMissing
            | Self::MsStoreSourceUnavailable
            | Self::MicrosoftStoreUnavailable
            | Self::StoreRefusedWithoutReason
            | Self::MicrosoftAccountRequired => &[DiagnosticAction::OpenStorePage],
            Self::OperationSequenceViolated
            | Self::ResolverUnavailable
            | Self::RegionUnavailable
            | Self::JournalUnavailable
            | Self::BackendFailed
            | Self::BackendReportedUnknownFailure
            | Self::NetworkUnavailable
            | Self::StorePageLaunchRejected
            | Self::InstallerLaunchRejected
            | Self::ClipboardUnavailable
            // Accepting the terms is what unblocks this; no page or download can.
            | Self::MsStoreSourceTermsNotAccepted => &[DiagnosticAction::Retry],
        }
    }
}

/// Technical evidence for the details area, never for the primary message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TechnicalDetail {
    /// Name of the internal error variant this diagnostic came from.
    pub variant: &'static str,
    /// Native Windows or backend code, when one exists.
    pub native_code: Option<i32>,
    /// Guarded operation this failure belongs to, when one is active.
    pub operation_id: Option<OperationId>,
}

/// One classified failure ready to be shown.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    code: DiagnosticCode,
    technical: Option<TechnicalDetail>,
}

impl Diagnostic {
    /// Classify a failure that carries no further technical evidence.
    #[must_use]
    pub const fn new(code: DiagnosticCode) -> Self {
        Self {
            code,
            technical: None,
        }
    }

    /// Classify a failure and keep its technical evidence.
    #[must_use]
    pub const fn with_detail(code: DiagnosticCode, technical: TechnicalDetail) -> Self {
        Self {
            code,
            technical: Some(technical),
        }
    }

    /// Attach the guarded operation this failure belongs to.
    #[must_use]
    pub fn for_operation(mut self, operation_id: OperationId) -> Self {
        match self.technical.as_mut() {
            Some(technical) => technical.operation_id = Some(operation_id),
            None => {
                self.technical = Some(TechnicalDetail {
                    variant: self.code.as_str(),
                    native_code: None,
                    operation_id: Some(operation_id),
                });
            }
        }
        self
    }

    /// Return the classified cause.
    #[must_use]
    pub const fn code(&self) -> DiagnosticCode {
        self.code
    }

    /// Return the technical evidence, if any was preserved.
    #[must_use]
    pub const fn technical(&self) -> Option<&TechnicalDetail> {
        self.technical.as_ref()
    }

    /// Render the copyable technical block.
    ///
    /// This is never the primary message: it exists for a bug report.
    #[must_use]
    pub fn technical_block(&self) -> String {
        use std::fmt::Write as _;

        let mut block = format!("code={}", self.code.as_str());
        if let Some(technical) = &self.technical {
            let _ = write!(block, "\nvariant={}", technical.variant);
            if let Some(native_code) = technical.native_code {
                let _ = write!(block, "\nnative=0x{native_code:08x} ({native_code})");
            }
            if let Some(operation_id) = &technical.operation_id {
                let _ = write!(block, "\noperation={}", operation_id.as_str());
            }
        }
        block
    }
}

impl From<StoreInputError> for Diagnostic {
    fn from(error: StoreInputError) -> Self {
        let variant = match error {
            StoreInputError::Empty => "StoreInputError::Empty",
            StoreInputError::InvalidProductId => "StoreInputError::InvalidProductId",
            StoreInputError::MissingProductId => "StoreInputError::MissingProductId",
            StoreInputError::UnsupportedLocation => "StoreInputError::UnsupportedLocation",
        };
        Self::with_detail(
            DiagnosticCode::StoreInputNotUnderstood,
            detail(variant, None),
        )
    }
}

fn detail(variant: &'static str, native_code: Option<i32>) -> TechnicalDetail {
    TechnicalDetail {
        variant,
        native_code,
        operation_id: None,
    }
}

impl From<&ResolveFailure> for Diagnostic {
    fn from(failure: &ResolveFailure) -> Self {
        let code = match failure.error {
            ResolveError::ProductNotFoundInMarket => DiagnosticCode::ProductUnavailableInRegion,
            // A cancelled resolve leaves the catalogue just as unreachable.
            ResolveError::ResolverUnavailable | ResolveError::Cancelled => {
                DiagnosticCode::ResolverUnavailable
            }
            ResolveError::SourceUnavailable => DiagnosticCode::MsStoreSourceUnavailable,
            ResolveError::UnsupportedMarket => DiagnosticCode::UnsupportedMarket,
            ResolveError::NetworkUnavailable => DiagnosticCode::NetworkUnavailable,
            ResolveError::PolicyDenied => DiagnosticCode::StoreRefusedWithoutReason,
            ResolveError::InvalidResponse | ResolveError::ProductIdMismatch => {
                DiagnosticCode::ResolverAnswerMismatch
            }
        };
        Self::with_detail(
            code,
            detail(
                resolve_variant(failure.error),
                failure
                    .diagnostic
                    .as_ref()
                    .and_then(|diagnostic| diagnostic.native_code),
            ),
        )
    }
}

const fn resolve_variant(error: ResolveError) -> &'static str {
    match error {
        ResolveError::ResolverUnavailable => "ResolveError::ResolverUnavailable",
        ResolveError::SourceUnavailable => "ResolveError::SourceUnavailable",
        ResolveError::UnsupportedMarket => "ResolveError::UnsupportedMarket",
        ResolveError::ProductNotFoundInMarket => "ResolveError::ProductNotFoundInMarket",
        ResolveError::NetworkUnavailable => "ResolveError::NetworkUnavailable",
        ResolveError::PolicyDenied => "ResolveError::PolicyDenied",
        ResolveError::InvalidResponse => "ResolveError::InvalidResponse",
        ResolveError::ProductIdMismatch => "ResolveError::ProductIdMismatch",
        ResolveError::Cancelled => "ResolveError::Cancelled",
    }
}

impl From<RegionReadError> for Diagnostic {
    fn from(error: RegionReadError) -> Self {
        let (variant, native_code) = match error {
            RegionReadError::GeoIdNotAvailable => ("RegionReadError::GeoIdNotAvailable", None),
            RegionReadError::InvalidGeoId(value) => ("RegionReadError::InvalidGeoId", Some(value)),
            RegionReadError::LookupFailed(_) => ("RegionReadError::LookupFailed", None),
            RegionReadError::InvalidUnicode(_) => ("RegionReadError::InvalidUnicode", None),
            RegionReadError::MissingDisplayName => ("RegionReadError::MissingDisplayName", None),
        };
        Self::with_detail(
            DiagnosticCode::RegionUnavailable,
            detail(variant, native_code),
        )
    }
}

impl From<RegionWriteError> for Diagnostic {
    fn from(error: RegionWriteError) -> Self {
        let (variant, native_code) = match error {
            RegionWriteError::Rejected => ("RegionWriteError::Rejected", None),
            RegionWriteError::PlatformFailure(code) => {
                ("RegionWriteError::PlatformFailure", Some(code))
            }
        };
        Self::with_detail(
            DiagnosticCode::RegionChangeRejected,
            detail(variant, native_code),
        )
    }
}

impl From<RecoveryStoreError> for Diagnostic {
    fn from(error: RecoveryStoreError) -> Self {
        let (variant, native_code) = match error {
            RecoveryStoreError::Read { os_error } => ("RecoveryStoreError::Read", os_error),
            RecoveryStoreError::Write { os_error } => ("RecoveryStoreError::Write", os_error),
            RecoveryStoreError::Publish { os_error } => ("RecoveryStoreError::Publish", os_error),
            RecoveryStoreError::Cleanup { os_error } => ("RecoveryStoreError::Cleanup", os_error),
            RecoveryStoreError::Serialization => ("RecoveryStoreError::Serialization", None),
            RecoveryStoreError::Verification => ("RecoveryStoreError::Verification", None),
            RecoveryStoreError::RecordMismatch => ("RecoveryStoreError::RecordMismatch", None),
            RecoveryStoreError::Schema(_) => ("RecoveryStoreError::Schema", None),
        };
        Self::with_detail(
            DiagnosticCode::RecoveryRecordUnreadable,
            detail(variant, native_code),
        )
    }
}

impl From<StartupRecoveryError> for Diagnostic {
    fn from(error: StartupRecoveryError) -> Self {
        match error {
            StartupRecoveryError::Store(store) => Self::from(store),
            StartupRecoveryError::Region(region) => {
                let mut diagnostic = Self::from(region);
                diagnostic.code = DiagnosticCode::RecoveryRecordUnreadable;
                diagnostic
            }
        }
    }
}

impl From<StorePageOpenError> for Diagnostic {
    fn from(error: StorePageOpenError) -> Self {
        let variant = match error {
            StorePageOpenError::LaunchRejected => "StorePageOpenError::LaunchRejected",
        };
        Self::with_detail(
            DiagnosticCode::StorePageLaunchRejected,
            detail(variant, None),
        )
    }
}

impl From<HandoffRefusal> for Diagnostic {
    fn from(refusal: HandoffRefusal) -> Self {
        let variant = match refusal {
            HandoffRefusal::SignatureNotTrusted => "HandoffRefusal::SignatureNotTrusted",
            HandoffRefusal::SignerUnavailable => "HandoffRefusal::SignerUnavailable",
            HandoffRefusal::SignerOutsideStorePolicy => "HandoffRefusal::SignerOutsideStorePolicy",
            HandoffRefusal::FileChangedDuringInspection => {
                "HandoffRefusal::FileChangedDuringInspection"
            }
            HandoffRefusal::FileIdentityIncomplete => "HandoffRefusal::FileIdentityIncomplete",
            HandoffRefusal::FileDiffersFromConfirmed => "HandoffRefusal::FileDiffersFromConfirmed",
        };
        Self::with_detail(
            DiagnosticCode::InstallerFileNotAdmitted,
            detail(variant, None),
        )
    }
}

impl From<InstallBackendError> for Diagnostic {
    fn from(error: InstallBackendError) -> Self {
        let (code, variant, native_code) = match error {
            InstallBackendError::Unavailable => (
                DiagnosticCode::BackendUnavailable,
                "InstallBackendError::Unavailable",
                None,
            ),
            InstallBackendError::RequestRejected => (
                DiagnosticCode::BackendFailed,
                "InstallBackendError::RequestRejected",
                None,
            ),
            InstallBackendError::TemporaryRegionNotApplied => (
                DiagnosticCode::TemporaryRegionNotApplied,
                "InstallBackendError::TemporaryRegionNotApplied",
                None,
            ),
            InstallBackendError::PackageNotOffered => (
                DiagnosticCode::ProductNoLongerOffered,
                "InstallBackendError::PackageNotOffered",
                None,
            ),
            InstallBackendError::AlreadyInstalling => (
                DiagnosticCode::ProductAlreadyInstalling,
                "InstallBackendError::AlreadyInstalling",
                None,
            ),
            InstallBackendError::InvalidHandle => (
                DiagnosticCode::BackendFailed,
                "InstallBackendError::InvalidHandle",
                None,
            ),
            InstallBackendError::ObservationFailed => (
                DiagnosticCode::CompletionUncertain,
                "InstallBackendError::ObservationFailed",
                None,
            ),
        };
        Self::with_detail(code, detail(variant, native_code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::ResolverDiagnostic;

    #[test]
    fn every_resolver_failure_becomes_a_cause_that_keeps_its_native_code() {
        for error in [
            ResolveError::ResolverUnavailable,
            ResolveError::SourceUnavailable,
            ResolveError::UnsupportedMarket,
            ResolveError::ProductNotFoundInMarket,
            ResolveError::NetworkUnavailable,
            ResolveError::PolicyDenied,
            ResolveError::InvalidResponse,
            ResolveError::ProductIdMismatch,
            ResolveError::Cancelled,
        ] {
            let failure = ResolveFailure {
                error,
                diagnostic: Some(ResolverDiagnostic {
                    native_code: Some(-2_147_024_809),
                }),
            };
            let diagnostic = Diagnostic::from(&failure);
            let technical = diagnostic
                .technical()
                .expect("technical detail is preserved");
            assert_eq!(technical.native_code, Some(-2_147_024_809));
            assert!(
                technical.variant.starts_with("ResolveError::"),
                "the details must name the internal variant"
            );
        }
    }

    #[test]
    fn a_primary_cause_never_carries_technical_text() {
        let diagnostic = Diagnostic::from(RegionReadError::InvalidGeoId(5));
        assert_eq!(diagnostic.code(), DiagnosticCode::RegionUnavailable);
        // The code is a token, not a sentence: no user wording lives in core.
        assert!(!diagnostic.code().as_str().contains(' '));
        assert!(
            diagnostic
                .technical_block()
                .contains("native=0x00000005 (5)")
        );
    }

    #[test]
    fn unsafe_causes_offer_recovery_and_never_a_plain_retry() {
        for code in [
            DiagnosticCode::RegionConflict,
            DiagnosticCode::RecoveryRecordUnreadable,
            DiagnosticCode::RestoreFailed,
        ] {
            assert_eq!(code.severity(), DiagnosticSeverity::Unsafe);
            assert!(
                code.permitted_actions()
                    .contains(&DiagnosticAction::ResolveRecovery)
            );
            assert!(
                !code.permitted_actions().contains(&DiagnosticAction::Retry),
                "an unsafe state must not offer a blind retry"
            );
        }
    }

    #[test]
    fn a_region_read_failure_during_recovery_is_reported_as_a_recovery_problem() {
        let diagnostic = Diagnostic::from(StartupRecoveryError::Region(
            RegionReadError::GeoIdNotAvailable,
        ));
        assert_eq!(diagnostic.code(), DiagnosticCode::RecoveryRecordUnreadable);
        assert_eq!(diagnostic.code().severity(), DiagnosticSeverity::Unsafe);
    }

    #[test]
    fn an_operation_identifier_reaches_the_details_even_without_native_evidence() {
        let operation_id = OperationId::new("op-7").expect("non-empty operation identifier");
        let diagnostic = Diagnostic::new(DiagnosticCode::CompletionUncertain)
            .for_operation(operation_id.clone());
        let block = diagnostic.technical_block();
        assert!(block.contains("code=completion_uncertain"));
        assert!(block.contains("operation=op-7"));
    }
}
