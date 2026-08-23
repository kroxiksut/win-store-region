//! Installation model, backend contract, and backend selection.

use crate::input::StoreProductId;
use crate::region::MarketCode;
use crate::resolve::ResolvedProduct;
use serde::{Deserialize, Serialize};

/// Product installation model established by a structured source.
///
/// `Unknown` is the conservative default. It must not be replaced merely
/// because a package identity field is absent: Store products may be Win32,
/// packaged, or not yet classifiable by the selected backend.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    /// The resolver did not establish a reliable installation model.
    Unknown,
    /// A packaged application with a positively observed package identity.
    Packaged,
    /// A Win32 application explicitly reported as such by the Store backend.
    Win32,
}

/// Structured fact that permits a non-unknown [`InstallKind`] classification.
///
/// This is deliberately not inferred from `PackageFullName`: lack of that
/// field is not evidence that a Store application is Win32.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKindEvidence {
    /// The platform positively observed a packaged application identity.
    PackagedIdentityObserved,
    /// The structured Store backend explicitly reported a Win32 installer.
    Win32ReportedByBackend,
    /// The structured Store backend reported the `MSStore` installer type.
    ///
    /// This arrives before installation and names the delivery mechanism, not a
    /// package that already exists. It is the only evidence v0.1 accepts for an
    /// install, because it is the only one whose completion can be proven.
    MsStoreInstallerTypeReported,
}

/// A classified installation model together with the evidence behind it.
///
/// The fields are private so callers cannot construct `Packaged` or `Win32`
/// without their matching proof.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstallClassification {
    kind: InstallKind,
    evidence: Option<InstallKindEvidence>,
}

impl Default for InstallClassification {
    fn default() -> Self {
        Self::unknown()
    }
}

impl InstallClassification {
    /// Construct the safe default used before a backend supplies proof.
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            kind: InstallKind::Unknown,
            evidence: None,
        }
    }

    /// Construct a packaged classification from a positively observed identity.
    #[must_use]
    pub const fn packaged_from_observed_identity() -> Self {
        Self {
            kind: InstallKind::Packaged,
            evidence: Some(InstallKindEvidence::PackagedIdentityObserved),
        }
    }

    /// Construct a packaged classification from a reported `MSStore` installer type.
    #[must_use]
    pub const fn packaged_from_msstore_installer_type() -> Self {
        Self {
            kind: InstallKind::Packaged,
            evidence: Some(InstallKindEvidence::MsStoreInstallerTypeReported),
        }
    }

    /// Construct a Win32 classification from an explicit structured backend report.
    #[must_use]
    pub const fn win32_from_backend_report() -> Self {
        Self {
            kind: InstallKind::Win32,
            evidence: Some(InstallKindEvidence::Win32ReportedByBackend),
        }
    }

    /// Return the model selected from the available proof.
    #[must_use]
    pub const fn kind(self) -> InstallKind {
        self.kind
    }

    /// Return the evidence that justified a non-unknown model.
    #[must_use]
    pub const fn evidence(self) -> Option<InstallKindEvidence> {
        self.evidence
    }
}

/// Whether v0.1 may install a product with this classification.
///
/// Only a packaged product whose `MSStore` installer type was reported
/// qualifies. Everything else, including a packaged identity observed by other
/// means, leaves no way to prove completion, and an install nobody can verify
/// must not be offered.
#[must_use]
pub const fn install_supported_in_v0_1(classification: InstallClassification) -> bool {
    matches!(
        classification.evidence(),
        Some(InstallKindEvidence::MsStoreInstallerTypeReported)
    )
}

/// Which specialised completion observer may be used after an install request.
///
/// `CompletionUncertain` is intentional: it prevents an `AppX` observer from
/// being silently applied to an unknown or Win32 product.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallObservationRoute {
    /// Use the packaged-app observer only.
    Packaged,
    /// Use the Win32 observer only.
    Win32,
    /// Do not infer completion; retain recovery state for an explicit decision.
    CompletionUncertain,
}

/// Select the observer family allowed by an evidence-backed classification.
#[must_use]
pub const fn install_observation_route(
    classification: InstallClassification,
) -> InstallObservationRoute {
    match classification.kind() {
        InstallKind::Packaged => InstallObservationRoute::Packaged,
        InstallKind::Win32 => InstallObservationRoute::Win32,
        InstallKind::Unknown => InstallObservationRoute::CompletionUncertain,
    }
}

/// Stable identity of an installation backend candidate.
///
/// The value is domain data rather than a GUI choice. Core selects a compatible
/// backend; presentation only renders the structured result of that decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallBackendKind {
    /// Structured Microsoft Store installation through the `WinGet` COM API.
    WingetComMsStore,
    /// Microsoft Store web-installer path, pending lifecycle experiments.
    StoreWebInstaller,
    /// PDP page opening only; it is never selected for an install request.
    StorePdpFallback,
}

/// What a backend can safely offer for one resolved product.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallBackendCapability {
    /// The backend can start an install and provides the stated observer family.
    CanInstall {
        observation_route: InstallObservationRoute,
    },
    /// The backend can only open a Store PDP page, not request installation.
    StorePageOnly,
    /// The backend reached the product but this version does not install its type.
    ///
    /// Distinct from [`InstallBackendCapability::Unavailable`]: nothing is
    /// broken, the product resolved, and only the delivery mechanism is out of
    /// scope. The user is told that, not that the backend failed.
    UnsupportedInstallKind,
    /// The backend cannot safely operate in the current environment.
    Unavailable,
}

/// Install input already validated by the product resolver.
///
/// It intentionally contains no `GeoId`, region writer, recovery store, or GUI
/// state. Those concerns remain owned by the operation state machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallRequest {
    product_id: StoreProductId,
    market: MarketCode,
    install_classification: InstallClassification,
}

impl InstallRequest {
    /// Build backend input from an exact, already-resolved Store product.
    #[must_use]
    pub fn from_resolved_product(product: &ResolvedProduct) -> Self {
        Self {
            product_id: product.product_id.clone(),
            market: product.market.clone(),
            install_classification: product.install_classification,
        }
    }

    /// Build backend input for an install that is already running.
    ///
    /// Used when taking over an operation after a restart, where the card is
    /// not at hand but the record names the product. The classification is the
    /// one such an operation must have had to start at all.
    #[must_use]
    pub fn for_resumed_install(product_id: StoreProductId, market: MarketCode) -> Self {
        Self {
            product_id,
            market,
            install_classification: InstallClassification::packaged_from_msstore_installer_type(),
        }
    }

    /// Return the exact Store product identity to install.
    #[must_use]
    pub fn product_id(&self) -> &StoreProductId {
        &self.product_id
    }

    /// Return the market confirmed by the resolver.
    #[must_use]
    pub fn market(&self) -> &MarketCode {
        &self.market
    }

    /// Return the evidence-backed model used to select an observer.
    #[must_use]
    pub const fn install_classification(&self) -> InstallClassification {
        self.install_classification
    }
}

/// Opaque backend-owned attempt identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallAttemptId(String);

impl InstallAttemptId {
    /// Build a non-empty backend attempt identity.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.trim().is_empty()).then_some(Self(value))
    }

    /// Return the backend-owned identity for adapter correlation only.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A started install bound to the backend that created it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallHandle {
    backend: InstallBackendKind,
    attempt_id: InstallAttemptId,
}

impl InstallHandle {
    /// Build a handle returned by a backend after it accepted an install request.
    #[must_use]
    pub const fn new(backend: InstallBackendKind, attempt_id: InstallAttemptId) -> Self {
        Self {
            backend,
            attempt_id,
        }
    }

    /// Return the backend authorised to observe or detach this attempt.
    #[must_use]
    pub const fn backend(&self) -> InstallBackendKind {
        self.backend
    }

    /// Return the opaque adapter correlation value.
    #[must_use]
    pub fn attempt_id(&self) -> &InstallAttemptId {
        &self.attempt_id
    }
}

/// Determinate installation progress supplied by a backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstallProgress(u8);

impl InstallProgress {
    /// Construct a valid percentage in the inclusive range 0 through 100.
    #[must_use]
    pub const fn new(percent: u8) -> Option<Self> {
        if percent <= 100 {
            Some(Self(percent))
        } else {
            None
        }
    }

    /// Return the backend-reported percentage.
    #[must_use]
    pub const fn percent(self) -> u8 {
        self.0
    }
}

/// Phase of an install that the backend distinguishes.
///
/// Byte counts are deliberately absent: for `MSStore` products the download is
/// performed by Microsoft Store, and the backend reports zero bytes throughout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallPhase {
    /// The backend accepted the request and work has not been reported yet.
    Queued,
    /// Installation work is in progress.
    Installing,
    /// Installation finished and the backend is completing post-install work.
    PostInstall,
}

/// Latest fact obtained while observing an accepted installation request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallObservation {
    /// The request is still active; progress is absent when the backend has none.
    Installing {
        phase: InstallPhase,
        progress: Option<InstallProgress>,
    },
    /// The selected observer produced enough evidence for completion.
    Completed,
    /// The backend cannot prove whether completion happened.
    CompletionUncertain,
}

/// Outcome of asking a backend to detach from or cancel an accepted request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetachOrCancelOutcome {
    /// The backend confirmed cancellation.
    Cancelled,
    /// Observation was detached; the underlying install may still continue.
    Detached,
    /// The backend does not implement reliable cancellation or detachment.
    NotSupported,
}

/// Structured backend failure, kept independent of platform-native text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallBackendError {
    /// The backend is not available in the current Windows environment.
    Unavailable,
    /// The backend rejected this exact install request.
    RequestRejected,
    /// The temporary region was not in force long enough for the source to accept it.
    ///
    /// Separate from a product that is genuinely unavailable in the region: here
    /// the operation itself was mistimed, and repeating it can succeed.
    TemporaryRegionNotApplied,
    /// The same product is already being installed, by this application or another.
    AlreadyInstalling,
    /// The handle belongs to another backend or is otherwise invalid.
    InvalidHandle,
    /// Observation could not produce a reliable result.
    ObservationFailed,
}

/// Platform-independent backend contract.
///
/// Implementations start and observe one request only. They cannot receive a
/// region writer, recovery store, or presentation object, so recovery ownership
/// remains in core.
pub trait InstallBackend {
    /// Return this implementation's stable identity.
    fn kind(&self) -> InstallBackendKind;

    /// Report whether this backend can safely handle the exact resolved product.
    fn capability(&self, request: &InstallRequest) -> InstallBackendCapability;

    /// Ask the backend to start the exact resolved product installation.
    ///
    /// # Errors
    ///
    /// Returns a structured rejection or availability failure without changing
    /// the Windows region.
    fn start_install(&self, request: &InstallRequest)
    -> Result<InstallHandle, InstallBackendError>;

    /// Look for an install of this product that is still running.
    ///
    /// A COM-driven install outlives the process that started it, so a restart
    /// after a crash must ask this before deciding the operation was abandoned.
    /// `Ok(None)` means no install is in flight; it is not evidence that the
    /// previous attempt failed, because it may equally have finished.
    ///
    /// # Errors
    ///
    /// Returns a structured availability failure when the backend cannot answer.
    fn resume(
        &self,
        request: &InstallRequest,
    ) -> Result<Option<InstallHandle>, InstallBackendError>;

    /// Obtain current completion evidence for a handle created by this backend.
    ///
    /// # Errors
    ///
    /// Returns a structured observer or handle failure without inferring
    /// installation completion.
    fn observe(&self, handle: &InstallHandle) -> Result<InstallObservation, InstallBackendError>;

    /// Cancel when supported or otherwise detach without claiming completion.
    ///
    /// # Errors
    ///
    /// Returns a structured handle failure when this backend did not create it.
    fn detach_or_cancel(
        &self,
        handle: &InstallHandle,
    ) -> Result<DetachOrCancelOutcome, InstallBackendError>;
}

/// How core should select an installation backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallBackendSelectionPolicy {
    /// Use core's deterministic production priority.
    Automatic,
    /// Request a named candidate in debug builds without bypassing capability checks.
    DebugForce(InstallBackendKind),
}

/// Selection failure that presentation can localise without platform details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallBackendSelectionError {
    /// No registered backend can safely install the resolved product.
    NoCompatibleBackend,
    /// A backend reached the product, but this version does not install its type.
    InstallKindNotSupported,
    /// A release build rejected a debug-only override request.
    DebugOverrideUnavailable,
    /// The forced candidate exists but cannot safely install this request.
    ForcedBackendNotCapable,
}

/// Core-selected backend, not a GUI-owned implementation detail.
pub struct SelectedInstallBackend<'a> {
    backend: &'a dyn InstallBackend,
}

impl<'a> SelectedInstallBackend<'a> {
    /// Return the selected backend for the operation orchestrator only.
    #[must_use]
    pub fn backend(&self) -> &'a dyn InstallBackend {
        self.backend
    }
}

/// Select the first compatible backend in core-owned deterministic priority.
///
/// A debug force changes selection order only. It never turns an unavailable,
/// PDP-only, or observer-mismatched backend into an installation backend.
///
/// # Errors
///
/// Returns [`InstallBackendSelectionError`] when no candidate is compatible or
/// a debug override is unavailable or unsafe.
pub fn select_install_backend<'a>(
    backends: &'a [&'a dyn InstallBackend],
    request: &InstallRequest,
    policy: InstallBackendSelectionPolicy,
) -> Result<SelectedInstallBackend<'a>, InstallBackendSelectionError> {
    let wanted_route = install_observation_route(request.install_classification());
    let is_compatible = |backend: &&dyn InstallBackend| {
        matches!(
            backend.capability(request),
            InstallBackendCapability::CanInstall { observation_route } if observation_route == wanted_route
        )
    };
    let find_by_kind = |kind| {
        backends
            .iter()
            .copied()
            .find(|backend| backend.kind() == kind)
    };

    let unsupported_kind = || {
        backends.iter().any(|backend| {
            matches!(
                backend.capability(request),
                InstallBackendCapability::UnsupportedInstallKind
            )
        })
    };

    match policy {
        InstallBackendSelectionPolicy::Automatic => [
            InstallBackendKind::WingetComMsStore,
            InstallBackendKind::StoreWebInstaller,
        ]
        .into_iter()
        .filter_map(find_by_kind)
        .find(is_compatible)
        .map(|backend| SelectedInstallBackend { backend })
        .ok_or_else(|| {
            if unsupported_kind() {
                InstallBackendSelectionError::InstallKindNotSupported
            } else {
                InstallBackendSelectionError::NoCompatibleBackend
            }
        }),
        InstallBackendSelectionPolicy::DebugForce(kind) => {
            if !cfg!(debug_assertions) {
                return Err(InstallBackendSelectionError::DebugOverrideUnavailable);
            }
            let Some(backend) = find_by_kind(kind) else {
                return Err(InstallBackendSelectionError::ForcedBackendNotCapable);
            };
            is_compatible(&backend)
                .then_some(SelectedInstallBackend { backend })
                .ok_or(InstallBackendSelectionError::ForcedBackendNotCapable)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeInstallBackend, install_request_for};

    #[test]
    fn install_kind_requires_matching_evidence_and_routes_observation_separately() {
        let unknown = InstallClassification::unknown();
        let packaged = InstallClassification::packaged_from_observed_identity();
        let msstore = InstallClassification::packaged_from_msstore_installer_type();
        let win32 = InstallClassification::win32_from_backend_report();

        assert_eq!(unknown.kind(), InstallKind::Unknown);
        assert_eq!(unknown.evidence(), None);
        assert_eq!(
            install_observation_route(unknown),
            InstallObservationRoute::CompletionUncertain
        );
        assert_eq!(packaged.kind(), InstallKind::Packaged);
        assert_eq!(
            packaged.evidence(),
            Some(InstallKindEvidence::PackagedIdentityObserved)
        );
        assert_eq!(
            install_observation_route(packaged),
            InstallObservationRoute::Packaged
        );
        assert_eq!(win32.kind(), InstallKind::Win32);
        assert_eq!(
            win32.evidence(),
            Some(InstallKindEvidence::Win32ReportedByBackend)
        );
        assert_eq!(
            install_observation_route(win32),
            InstallObservationRoute::Win32
        );
        assert_eq!(msstore.kind(), InstallKind::Packaged);
        assert_eq!(
            install_observation_route(msstore),
            InstallObservationRoute::Packaged
        );
    }

    #[test]
    fn only_a_reported_msstore_installer_type_may_be_installed_in_v0_1() {
        assert!(install_supported_in_v0_1(
            InstallClassification::packaged_from_msstore_installer_type()
        ));
        for out_of_scope in [
            InstallClassification::unknown(),
            InstallClassification::packaged_from_observed_identity(),
            InstallClassification::win32_from_backend_report(),
        ] {
            assert!(
                !install_supported_in_v0_1(out_of_scope),
                "{out_of_scope:?} has no way to prove completion"
            );
        }
    }

    #[test]
    fn an_unsupported_installer_type_is_reported_as_itself_not_as_a_missing_backend() {
        let request = install_request_for(InstallClassification::win32_from_backend_report());
        let winget = FakeInstallBackend {
            kind: InstallBackendKind::WingetComMsStore,
            capability: InstallBackendCapability::UnsupportedInstallKind,
            observation: InstallObservation::CompletionUncertain,
            detach_outcome: DetachOrCancelOutcome::NotSupported,
            resumable: false,
        };
        let backends: [&dyn InstallBackend; 1] = [&winget];

        assert_eq!(
            select_install_backend(
                &backends,
                &request,
                InstallBackendSelectionPolicy::Automatic
            )
            .err(),
            Some(InstallBackendSelectionError::InstallKindNotSupported)
        );
    }

    #[test]
    fn a_backend_can_report_an_install_that_outlived_the_process() {
        let request =
            install_request_for(InstallClassification::packaged_from_msstore_installer_type());
        let running = FakeInstallBackend {
            kind: InstallBackendKind::WingetComMsStore,
            capability: InstallBackendCapability::CanInstall {
                observation_route: InstallObservationRoute::Packaged,
            },
            observation: InstallObservation::Installing {
                phase: InstallPhase::Installing,
                progress: InstallProgress::new(40),
            },
            detach_outcome: DetachOrCancelOutcome::Detached,
            resumable: true,
        };
        let finished = FakeInstallBackend {
            kind: InstallBackendKind::WingetComMsStore,
            capability: InstallBackendCapability::CanInstall {
                observation_route: InstallObservationRoute::Packaged,
            },
            observation: InstallObservation::Completed,
            detach_outcome: DetachOrCancelOutcome::Detached,
            resumable: false,
        };

        let handle = running
            .resume(&request)
            .expect("the backend can answer")
            .expect("an install is still in flight");
        assert_eq!(handle.backend(), InstallBackendKind::WingetComMsStore);
        assert_eq!(
            finished.resume(&request).expect("the backend can answer"),
            None,
            "no install in flight is not evidence of failure"
        );
    }

    #[test]
    fn core_selects_a_compatible_backend_without_exposing_backend_details_to_gui() {
        let request = install_request_for(InstallClassification::packaged_from_observed_identity());
        let web = FakeInstallBackend {
            kind: InstallBackendKind::StoreWebInstaller,
            capability: InstallBackendCapability::CanInstall {
                observation_route: InstallObservationRoute::Packaged,
            },
            observation: InstallObservation::Installing {
                phase: InstallPhase::Installing,
                progress: InstallProgress::new(50),
            },
            detach_outcome: DetachOrCancelOutcome::Detached,
            resumable: false,
        };
        let winget = FakeInstallBackend {
            kind: InstallBackendKind::WingetComMsStore,
            capability: InstallBackendCapability::CanInstall {
                observation_route: InstallObservationRoute::Packaged,
            },
            observation: InstallObservation::Completed,
            detach_outcome: DetachOrCancelOutcome::Cancelled,
            resumable: false,
        };
        let pdp = FakeInstallBackend {
            kind: InstallBackendKind::StorePdpFallback,
            capability: InstallBackendCapability::StorePageOnly,
            observation: InstallObservation::CompletionUncertain,
            detach_outcome: DetachOrCancelOutcome::NotSupported,
            resumable: false,
        };
        let backends: [&dyn InstallBackend; 3] = [&web, &pdp, &winget];

        let selected = select_install_backend(
            &backends,
            &request,
            InstallBackendSelectionPolicy::Automatic,
        )
        .expect("core-owned priority selects compatible WinGet candidate");
        assert_eq!(
            selected.backend().kind(),
            InstallBackendKind::WingetComMsStore
        );

        let handle = selected
            .backend()
            .start_install(&request)
            .expect("selected backend accepts exact request");
        assert_eq!(handle.backend(), InstallBackendKind::WingetComMsStore);
        assert_eq!(handle.attempt_id().as_str(), "fake-attempt");
        assert_eq!(
            selected.backend().observe(&handle),
            Ok(InstallObservation::Completed)
        );
        assert_eq!(
            selected.backend().detach_or_cancel(&handle),
            Ok(DetachOrCancelOutcome::Cancelled)
        );
    }

    #[test]
    fn selection_never_turns_pdp_or_a_wrong_observer_into_install_capability() {
        let request = install_request_for(InstallClassification::packaged_from_observed_identity());
        let wrong_observer = FakeInstallBackend {
            kind: InstallBackendKind::WingetComMsStore,
            capability: InstallBackendCapability::CanInstall {
                observation_route: InstallObservationRoute::Win32,
            },
            observation: InstallObservation::CompletionUncertain,
            detach_outcome: DetachOrCancelOutcome::NotSupported,
            resumable: false,
        };
        let pdp = FakeInstallBackend {
            kind: InstallBackendKind::StorePdpFallback,
            capability: InstallBackendCapability::StorePageOnly,
            observation: InstallObservation::CompletionUncertain,
            detach_outcome: DetachOrCancelOutcome::NotSupported,
            resumable: false,
        };
        let backends: [&dyn InstallBackend; 2] = [&wrong_observer, &pdp];

        assert_eq!(
            select_install_backend(
                &backends,
                &request,
                InstallBackendSelectionPolicy::Automatic,
            )
            .err(),
            Some(InstallBackendSelectionError::NoCompatibleBackend)
        );
        // A release build refuses the override outright; a debug build accepts
        // the request and then still refuses the incapable candidate.
        let expected = if cfg!(debug_assertions) {
            InstallBackendSelectionError::ForcedBackendNotCapable
        } else {
            InstallBackendSelectionError::DebugOverrideUnavailable
        };
        assert_eq!(
            select_install_backend(
                &backends,
                &request,
                InstallBackendSelectionPolicy::DebugForce(InstallBackendKind::StorePdpFallback),
            )
            .err(),
            Some(expected)
        );
    }

    #[test]
    fn debug_force_changes_priority_but_not_capability_requirements() {
        let request = install_request_for(InstallClassification::packaged_from_observed_identity());
        let winget = FakeInstallBackend {
            kind: InstallBackendKind::WingetComMsStore,
            capability: InstallBackendCapability::CanInstall {
                observation_route: InstallObservationRoute::Packaged,
            },
            observation: InstallObservation::Completed,
            detach_outcome: DetachOrCancelOutcome::Cancelled,
            resumable: false,
        };
        let web = FakeInstallBackend {
            kind: InstallBackendKind::StoreWebInstaller,
            capability: InstallBackendCapability::CanInstall {
                observation_route: InstallObservationRoute::Packaged,
            },
            observation: InstallObservation::Installing {
                phase: InstallPhase::Queued,
                progress: None,
            },
            detach_outcome: DetachOrCancelOutcome::Detached,
            resumable: false,
        };
        let backends: [&dyn InstallBackend; 2] = [&winget, &web];
        let result = select_install_backend(
            &backends,
            &request,
            InstallBackendSelectionPolicy::DebugForce(InstallBackendKind::StoreWebInstaller),
        );

        if cfg!(debug_assertions) {
            assert_eq!(
                result
                    .expect("debug force keeps capability check")
                    .backend()
                    .kind(),
                InstallBackendKind::StoreWebInstaller
            );
        } else {
            assert_eq!(
                result.err(),
                Some(InstallBackendSelectionError::DebugOverrideUnavailable)
            );
        }
    }
}
