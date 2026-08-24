//! Running one guarded installation off the UI thread.
//!
//! The thread here executes; it decides nothing. Core says which single step is
//! due, this module carries it out against the real region writer, recovery
//! store and backend, feeds the result back into the state machine, and posts
//! every change to the window. Nothing is inferred: the region is always read
//! back, and completion needs the package identity core asked for.

use crate::gui::ids::{WM_APP_INSTALL_PROGRESS, post_boxed};
use crate::gui::install_trace::{OperationTrace, operation_never_started};
use crate::platform::packaged::WindowsPackagedObserver;
use crate::platform::region::{Win32RegionReader, Win32RegionWriter};
use crate::platform::storage::{Win32JournalStore, Win32RecoveryStore};
use crate::platform::winget::backend::{WinGetComBackend, stated_failure};
use crate::platform::winget::resolver::WinGetComResolver;
use crate::platform::{WindowsRuntimeGuard, observation_timestamp_now};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::HWND;
use winstoreregion_core::{
    Diagnostic, DiagnosticCode, DurableOperationState, ExpectedPackagedIdentity, GeoId,
    InstallBackend, InstallHandle, InstallKind, InstallObservation, InstallPhase, InstallProgress,
    InstallRequest, InstallationOperationEvent, InstallationOperationMachine,
    InstallationOperationState, JournalProduct, JournalRecord, JournalResult, JournalSubject,
    MarketCode, ObservationElapsedMillis, OperationFacts, OperationId, OperationIdGenerator,
    OperationStatePersistence, OperationStep, OperationTimeoutPolicy, PackageSnapshot,
    PackagedObservation, PendingRestore, PendingRestoreGuard, RecoveryStore, RecoveryStoreError,
    RegionReader, RegionSwitchError, RegionSwitchOutcome, RegionWriter, ResolveRequest,
    ResolveRequestId, ResolvedProduct, StoreProductId, UtcTimestamp, evaluate_packaged_observation,
    install_supported_in_v0_1, next_step, resolve_product, switch_region,
};

/// How often the running install is polled for a new observation.
///
/// The backend emits progress roughly every 110 ms; polling faster would only
/// repeat what the UI already shows.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Backend identity written into the recovery record.
const BACKEND_NAME: &str = "winget-com-msstore";

/// How many times putting the original region back is attempted.
///
/// Restoring the region is the whole promise of this application, so one
/// refused write or one read-back that disagreed is not accepted as the answer.
const RESTORE_ATTEMPTS: u32 = 3;

/// Pause between two restoration attempts.
const RESTORE_RETRY_PAUSE: Duration = Duration::from_millis(300);

/// What the window learns about a running or finished installation.
pub(super) struct InstallationUpdate {
    /// Where the operation stands now.
    pub(super) state: InstallationOperationState,
    /// Phase reported by the backend, when one has arrived.
    pub(super) phase: Option<InstallPhase>,
    /// Percentage reported by the backend, when it reports one.
    pub(super) progress: Option<InstallProgress>,
    /// Whether the original region is already back.
    pub(super) region_restored_early: bool,
    /// Set once, when the operation can go no further.
    pub(super) failure: Option<Diagnostic>,
    /// The product as the catalogue described it under the temporary region.
    pub(super) resolved: Option<ResolvedProduct>,
    /// The deadline elapsed and only the user can say how this ends.
    ///
    /// The thread cannot wait for that answer: it would hold the recovery guard
    /// for as long as the dialog is open. It reports the question instead and
    /// ends, leaving the published record for whoever answers.
    pub(super) awaiting_user_decision: bool,
}

impl InstallationUpdate {
    /// An update that reports only where the operation stands.
    fn state(state: InstallationOperationState, region_restored_early: bool) -> Self {
        Self {
            state,
            phase: None,
            progress: None,
            region_restored_early,
            failure: None,
            resolved: None,
            awaiting_user_decision: false,
        }
    }
}

/// Everything the thread needs to run one operation.
///
/// A resolved product is deliberately absent. The catalogue is regional at
/// search time, so the product this operation is about can only be resolved
/// once the temporary region is in force.
pub(super) struct InstallationRequest {
    /// The resolve this operation will answer once the region is in force.
    ///
    /// It is opened before the operation starts so the window can match the
    /// card that arrives later against the request it is showing.
    pub(super) resolve: ResolveRequest,
    /// Region to hold while the source is asked for the product.
    pub(super) temporary_region: GeoId,
}

/// Start one guarded installation on its own thread.
pub(super) fn start_installation(window: HWND, request: InstallationRequest) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let post = |update: InstallationUpdate| post_update(window_handle, update);
        run(&request, &post);
    });
}

/// What the package manager answered about an installation a record names.
///
/// A plain yes/no hid three different situations behind one `false`, and the
/// log could not tell them apart afterwards: a record nothing can be rebuilt
/// from, a package manager that would not answer, and a straight "that
/// installation is not running".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ResumeProbe {
    /// The installation is still in flight and can be taken over.
    InFlight,
    /// The package manager answered, and this installation is not among its own.
    NotInFlight,
    /// The package manager could not be asked at all.
    BackendUnavailable,
    /// The record does not describe an installation this application can resume.
    RecordNotUsable,
}

impl ResumeProbe {
    /// A stable name for the diagnostic log.
    pub(super) const fn as_token(self) -> &'static str {
        match self {
            Self::InFlight => "in_flight",
            Self::NotInFlight => "not_in_flight",
            Self::BackendUnavailable => "backend_unavailable",
            Self::RecordNotUsable => "record_not_usable",
        }
    }
}

/// What a restart could learn about the installation a record names.
pub(super) struct ResumeAnswer {
    /// What the package manager said about the installation itself.
    pub(super) probe: ResumeProbe,
    /// Whether the package identity the record names is installed now.
    ///
    /// Only meaningful once the installation is no longer running, and only
    /// when the record names an identity at all.
    pub(super) package_present: bool,
}

/// Ask whether the installation a record names is still going.
///
/// A COM-driven install keeps running after the client that asked for it dies,
/// so a restart must ask before treating a record as an abandoned operation.
/// `false` means nothing is in flight, which is not the same as failure: the
/// install may equally have finished meanwhile.
///
/// This activates an out-of-process package manager and asks it over the
/// network, so it belongs on a worker thread. The answer cannot be handed to
/// the thread that will follow the install either: COM objects belong to the
/// apartment that created them, and that thread connects again for itself.
pub(super) fn probe_resumable_install(pending: &PendingRestore) -> ResumeAnswer {
    let answer = |probe| ResumeAnswer {
        probe,
        package_present: false,
    };
    let Some(request) = resumed_request(pending) else {
        return answer(ResumeProbe::RecordNotUsable);
    };
    let install_request = InstallRequest::for_resumed_install(
        request.resolve.product_id.clone(),
        request.resolve.market.clone(),
    );
    let Ok(_runtime) = WindowsRuntimeGuard::initialize() else {
        return answer(ResumeProbe::BackendUnavailable);
    };
    let Ok(backend) = WinGetComBackend::connect() else {
        return answer(ResumeProbe::BackendUnavailable);
    };
    let probe = match backend.resume(&install_request) {
        Ok(Some(_)) => ResumeProbe::InFlight,
        Ok(None) => ResumeProbe::NotInFlight,
        Err(_) => ResumeProbe::BackendUnavailable,
    };
    ResumeAnswer {
        probe,
        // Asked under the same runtime, and only once there is nothing left to
        // take over: while an installation runs, its package may appear at any
        // moment and the answer would say nothing.
        package_present: probe == ResumeProbe::NotInFlight && recorded_package_present(pending),
    }
}

/// Whether the package identity a record names is installed right now.
///
/// This is evidence about the recorded operation, not a general observation.
/// The record is written before the installation starts, and an operation
/// refuses to start at all when the package is already there — so an identity
/// present now appeared while that operation was running.
fn recorded_package_present(pending: &PendingRestore) -> bool {
    let Some(expected) = pending_identity(pending) else {
        return false;
    };
    WindowsPackagedObserver::snapshot().is_ok_and(|installed| installed.contains(&expected))
}

/// Take over an installation that outlived the process which started it.
///
/// Returns whether a thread was started for it. Only the record has to be
/// usable here: whether the installation is still in flight was answered by
/// [`installation_is_resumable`], and the thread asks again for itself.
pub(super) fn follow_resumed_installation(window: HWND, pending: &PendingRestore) -> bool {
    let Some(request) = resumed_request(pending) else {
        return false;
    };
    let window_handle = window.0 as usize;
    let pending = pending.clone();
    thread::spawn(move || {
        let post = |update: InstallationUpdate| post_update(window_handle, update);
        follow_resumed_install(&request, &pending, &post);
    });
    true
}

/// Follow an install already in flight, reporting through `post`.
///
/// Connects for itself: the objects belong to whichever apartment created them.
fn follow_resumed_install(
    request: &InstallationRequest,
    pending: &PendingRestore,
    post: &dyn Fn(InstallationUpdate),
) {
    let Ok(_runtime) = WindowsRuntimeGuard::initialize() else {
        resumed_install_not_followed(pending, post, DiagnosticCode::BackendUnavailable);
        return;
    };
    let Ok(backend) = WinGetComBackend::connect() else {
        resumed_install_not_followed(pending, post, DiagnosticCode::BackendUnavailable);
        return;
    };
    let Ok(store) = Win32RecoveryStore::for_current_user() else {
        resumed_install_not_followed(pending, post, DiagnosticCode::RecoveryRecordUnreadable);
        return;
    };
    let install_request = InstallRequest::for_resumed_install(
        request.resolve.product_id.clone(),
        request.resolve.market.clone(),
    );
    // The window was told an installation was resumed after a synchronous probe
    // found one. It can be gone by the time this thread asks again, and then
    // this run has no evidence of any kind about how it ended.
    let Ok(Some(handle)) = backend.resume(&install_request) else {
        resumed_install_not_followed(pending, post, DiagnosticCode::CompletionUncertain);
        return;
    };
    let already_restored = pending.state() == DurableOperationState::RegionRestoredEarly;
    let guard = PendingRestoreGuard::new(store, pending.clone());
    let mut operation = Operation::resumed(request, post, pending, already_restored);
    operation.observe(&backend, &handle, guard);
}

/// Report a taken-over installation this run cannot follow after all.
///
/// The record is left exactly as it was found: nothing here learnt how the
/// installation ended. The window is told, because it has already announced a
/// resumed installation and would otherwise show it running forever. The user
/// is asked only while the temporary region is still in force — that is the
/// part still worth deciding in this session.
fn resumed_install_not_followed(
    pending: &PendingRestore,
    post: &dyn Fn(InstallationUpdate),
    code: DiagnosticCode,
) {
    let region_restored_early = pending.state() == DurableOperationState::RegionRestoredEarly;
    post(InstallationUpdate {
        state: InstallationOperationState::CompletionUncertain,
        phase: None,
        progress: None,
        region_restored_early,
        failure: Some(Diagnostic::new(code)),
        resolved: None,
        awaiting_user_decision: !region_restored_early,
    });
}

/// Rebuild the operation input from a recovery record.
fn resumed_request(pending: &PendingRestore) -> Option<InstallationRequest> {
    let product_id = StoreProductId::parse(pending.product_id()).ok()?;
    let region = Win32RegionReader::region_for_geo_id(pending.temporary_region()).ok()?;
    let market = MarketCode::from_region(&region).ok()?;
    Some(InstallationRequest {
        resolve: ResolveRequest {
            request_id: ResolveRequestId::new(1),
            product_id,
            market,
        },
        temporary_region: pending.temporary_region(),
    })
}

/// Post one update to the window, discarding it if the window has gone.
fn post_update(window_handle: usize, update: InstallationUpdate) {
    post_boxed(window_handle, WM_APP_INSTALL_PROGRESS, update);
}

/// Execute the operation, reporting every state change through `post`.
fn run(request: &InstallationRequest, post: &dyn Fn(InstallationUpdate)) {
    let Ok(_runtime) = WindowsRuntimeGuard::initialize() else {
        let diagnostic = Diagnostic::new(winstoreregion_core::DiagnosticCode::BackendUnavailable);
        operation_never_started(&diagnostic);
        post(failure(diagnostic));
        return;
    };
    let backend = match WinGetComBackend::connect() {
        Ok(backend) => backend,
        Err(error) => {
            let diagnostic = Diagnostic::with_detail(
                winstoreregion_core::DiagnosticCode::BackendUnavailable,
                winstoreregion_core::TechnicalDetail {
                    variant: "WinGetUnavailable",
                    native_code: Some(error.native_code()),
                    operation_id: None,
                },
            );
            operation_never_started(&diagnostic);
            post(failure(diagnostic));
            return;
        }
    };

    let mut operation = Operation::new(request, post);
    operation.execute(&backend);
}

/// A failure update for an operation that never reached its own state.
fn failure(diagnostic: Diagnostic) -> InstallationUpdate {
    InstallationUpdate {
        state: InstallationOperationState::FailedBeforeRegionChange,
        phase: None,
        progress: None,
        region_restored_early: false,
        failure: Some(diagnostic),
        resolved: None,
        awaiting_user_decision: false,
    }
}

/// One guarded operation and the facts gathered while it runs.
struct Operation<'a> {
    request: &'a InstallationRequest,
    post: &'a dyn Fn(InstallationUpdate),
    /// Identity shared by this operation's record and its journal entry.
    identity: OperationId,
    /// The product as the catalogue described it under the temporary region.
    product: Option<ResolvedProduct>,
    /// Package identity to confirm, taken from the card or a recovery record.
    expected: Option<ExpectedPackagedIdentity>,
    /// The record this operation took over, when it did not start the install.
    taken_over: Option<PendingRestore>,
    /// Packages present before the install was requested.
    baseline: Option<PackageSnapshot>,
    machine: InstallationOperationMachine,
    policy: OperationTimeoutPolicy,
    trace: OperationTrace,
    /// Whether this operation has already published a record or changed the region.
    ///
    /// A failure before that point changed nothing on the machine and belongs in
    /// the diagnostic log only; after it, the history owes the user an entry.
    touched_machine: bool,
    /// Whether this operation has already written its one history entry.
    outcome_written: bool,
    /// Whether the backend's own result codes have already been recorded.
    backend_exit_recorded: bool,
    started: Instant,
    last_observation_at: Instant,
    facts: OperationFacts,
    phase: Option<InstallPhase>,
    progress: Option<InstallProgress>,
}

impl<'a> Operation<'a> {
    fn new(request: &'a InstallationRequest, post: &'a dyn Fn(InstallationUpdate)) -> Self {
        let now = Instant::now();
        let identity = SingleOperationId::from_clock().next_operation_id();
        Self {
            request,
            post,
            trace: OperationTrace::new(identity.clone()),
            identity,
            product: None,
            expected: None,
            taken_over: None,
            baseline: None,
            machine: InstallationOperationMachine::new(),
            policy: OperationTimeoutPolicy::default(),
            touched_machine: false,
            outcome_written: false,
            backend_exit_recorded: false,
            started: now,
            last_observation_at: now,
            facts: OperationFacts::at_request(),
            phase: None,
            progress: None,
        }
    }

    /// An operation that is already installing, taken over after a restart.
    ///
    /// The events replayed here describe what the previous run actually did, so
    /// the machine ends up where that run left it rather than at the start.
    fn resumed(
        request: &'a InstallationRequest,
        post: &'a dyn Fn(InstallationUpdate),
        pending: &PendingRestore,
        region_restored_early: bool,
    ) -> Self {
        let mut operation = Self::new(request, post);
        for event in [
            InstallationOperationEvent::BeginResolvingProduct,
            InstallationOperationEvent::ProductResolved,
            InstallationOperationEvent::BeginPreparing,
            InstallationOperationEvent::BeginSavingRecoveryState,
            InstallationOperationEvent::RecoveryStateSaved,
            InstallationOperationEvent::RegionSwitchFinished(RegionSwitchOutcome::RegionSwitched),
            InstallationOperationEvent::BeginRefreshingStoreContext,
            InstallationOperationEvent::BeginInstall,
            InstallationOperationEvent::InstallRequestAccepted,
            InstallationOperationEvent::ObservationStarted,
        ] {
            operation.advance(event);
        }
        operation.facts.resolved_under_temporary_region = true;
        operation.expected = pending_identity(pending);
        operation.taken_over = Some(pending.clone());
        // The record and the temporary region are already in place; this run
        // inherits them, so it also inherits the duty to report an outcome.
        operation.touched_machine = true;
        operation.trace.started(
            &request.resolve.product_id,
            &request.resolve.market,
            request.temporary_region,
            true,
        );
        // The install began before this process existed, so anything present
        // now was either put there by that install or was already there; the
        // snapshot cannot tell them apart and must not pretend otherwise.
        operation.baseline = Some(PackageSnapshot {
            observed_at: observation_timestamp_now(),
            entries: Vec::new(),
        });
        if region_restored_early {
            operation.advance(InstallationOperationEvent::Observation(
                InstallObservation::Installing {
                    phase: InstallPhase::Installing,
                    progress: None,
                },
            ));
            operation.advance(InstallationOperationEvent::EarlyRegionRestoreConfirmed);
            operation.facts.region_restored_early = true;
        }
        operation
    }

    /// Run the whole operation to a state that needs no further step.
    fn execute(&mut self, backend: &WinGetComBackend) {
        self.trace.started(
            &self.request.resolve.product_id,
            &self.request.resolve.market,
            self.request.temporary_region,
            false,
        );
        let Some(mut guard) = self.switch_region() else {
            return;
        };
        let Some(product) = self.resolve_under_temporary_region(&mut guard) else {
            if self.restore(&mut guard, false) {
                clear_record(guard);
            }
            return;
        };
        let install_request = InstallRequest::from_resolved_product(&product);
        let Some(handle) = self.start_install(backend, &install_request) else {
            // The install never started, so the temporary region has no reason
            // to stay a moment longer.
            if self.restore(&mut guard, false) {
                clear_record(guard);
            }
            return;
        };
        self.observe(backend, &handle, guard);
    }

    /// Put the original region back, recording the attempt whichever way it went.
    fn restore(&self, guard: &mut PendingRestoreGuard<Win32RecoveryStore>, early: bool) -> bool {
        let original = guard.pending().original_region();
        let restored = restore_region(guard, early);
        self.trace.region_restore(original, early, restored);
        restored
    }

    /// Hold the temporary region under the full recovery protocol.
    ///
    /// Every failure here is reported rather than swallowed: the user is owed
    /// the reason a region did not change, and a silent return would leave the
    /// window claiming an operation that never began.
    fn switch_region(&mut self) -> Option<PendingRestoreGuard<Win32RecoveryStore>> {
        self.advance(InstallationOperationEvent::BeginResolvingProduct);
        self.advance(InstallationOperationEvent::ProductResolved);
        self.advance(InstallationOperationEvent::BeginPreparing);
        self.advance(InstallationOperationEvent::BeginSavingRecoveryState);

        let reader = Win32RegionReader;
        let writer = Win32RegionWriter;
        let store = match Win32RecoveryStore::for_current_user() {
            Ok(store) => store,
            Err(error) => {
                self.fail(Diagnostic::from(error));
                return None;
            }
        };
        let original = match RegionReader::current_region(&reader) {
            Ok(region) => region.geo_id,
            Err(error) => {
                self.fail(Diagnostic::from(error));
                return None;
            }
        };
        let mut ids = SingleOperationId(self.identity.clone());
        let prepared = now_utc().and_then(|started_at| {
            PendingRestore::prepared(
                ids.next_operation_id(),
                started_at,
                original,
                self.request.temporary_region,
                self.request.resolve.product_id.as_str(),
                BACKEND_NAME,
            )
            .ok()
        });
        let Some(pending) = prepared else {
            self.fail(Diagnostic::new(DiagnosticCode::RecoveryRecordUnreadable));
            return None;
        };
        let mut guard = PendingRestoreGuard::new(store, pending);

        // `switch_region` publishes the record through the guard before it
        // touches Windows, so the machine learns of it in the same order.
        self.advance(InstallationOperationEvent::RecoveryStateSaved);
        // From here the operation owns a durable record and is about to write a
        // region, so it owes the user a history entry however it ends.
        self.touched_machine = true;
        self.trace
            .recovery_record_written(original, self.request.temporary_region);
        let report = match switch_region(
            self.request.temporary_region,
            &reader,
            &writer,
            &mut ids,
            &mut guard,
        ) {
            Ok(report) => report,
            Err(error) => {
                self.trace
                    .region_switch_failed(self.request.temporary_region, &error);
                // A read-back that never answered is the one failure here where
                // the write has probably already landed. Every other one either
                // precedes the writer or follows a read-back that found the
                // original region, so only this one owes a rollback.
                if matches!(error, RegionSwitchError::ReadBack(_)) {
                    self.advance(InstallationOperationEvent::FailWithTemporaryRegion);
                    self.advance(InstallationOperationEvent::BeginRestoringRegion);
                    if self.restore(&mut guard, false) {
                        self.advance(InstallationOperationEvent::OriginalRegionRestored);
                        clear_record(guard);
                    } else {
                        self.advance(InstallationOperationEvent::RestoreFailed);
                    }
                }
                self.fail(switch_diagnostic(&error));
                return None;
            }
        };
        self.trace.region_switched(
            original,
            self.request.temporary_region,
            report.writer_result,
            report.outcome,
        );
        self.advance(InstallationOperationEvent::RegionSwitchFinished(
            report.outcome,
        ));
        match report.outcome {
            // Only a read-back-confirmed temporary region may reach the backend.
            RegionSwitchOutcome::RegionSwitched => Some(guard),
            RegionSwitchOutcome::OriginalRegionPreserved => {
                self.fail(Diagnostic::new(DiagnosticCode::RegionUnchanged));
                None
            }
            RegionSwitchOutcome::RegionConflict { .. } => {
                self.fail(Diagnostic::new(DiagnosticCode::RegionConflict));
                None
            }
        }
    }

    /// Resolve the product now that the temporary region is confirmed.
    ///
    /// A product missing here is not a failure of the application: it means the
    /// chosen region does not offer it either, and that is worth saying plainly.
    fn resolve_under_temporary_region(
        &mut self,
        guard: &mut PendingRestoreGuard<Win32RecoveryStore>,
    ) -> Option<ResolvedProduct> {
        self.advance(InstallationOperationEvent::BeginRefreshingStoreContext);
        if next_step(self.machine.state(), self.facts, self.policy)
            != OperationStep::ResolveUnderTemporaryRegion
        {
            return None;
        }
        let resolved = match resolve_product(&WinGetComResolver, &self.request.resolve) {
            Ok(product) => product,
            Err(failure) => {
                self.trace.product_not_resolved(
                    &self.request.resolve.product_id,
                    &self.request.resolve.market,
                );
                self.advance(InstallationOperationEvent::FailWithTemporaryRegion);
                self.fail(Diagnostic::from(&failure));
                return None;
            }
        };
        self.trace.product_resolved(&resolved);
        if !install_supported_in_v0_1(resolved.install_classification) {
            self.advance(InstallationOperationEvent::FailWithTemporaryRegion);
            self.fail(Diagnostic::new(DiagnosticCode::InstallKindNotSupported));
            return None;
        }
        self.product = Some(resolved.clone());
        self.expected = expected_identity(&resolved);
        // A restart can only prove completion if the record names the identity
        // this operation is waiting for, so it is written before the install.
        if let Some(expected) = self.expected.as_ref() {
            // Both halves matter: the identity says what to look for, and the
            // classification is what makes it a provable packaged identity.
            let updated = guard
                .pending()
                .with_install_classification(resolved.install_classification)
                .with_package_family_name(expected.family_name().clone());
            // A refused write is not fatal — the installation can still run and
            // report its own outcome — but it decides how a restart will read
            // this operation, so it is never lost silently.
            self.trace
                .completion_identity_recorded(guard.replace_record(updated).is_ok());
        }
        // Taken before the request so a package that was already there cannot
        // be mistaken for the one this operation installed.
        self.baseline = WindowsPackagedObserver::snapshot().ok();
        if self.already_installed() {
            self.advance(InstallationOperationEvent::FailWithTemporaryRegion);
            self.fail(Diagnostic::new(DiagnosticCode::ProductAlreadyInstalled));
            return None;
        }
        self.facts.resolved_under_temporary_region = true;
        (self.post)(InstallationUpdate {
            resolved: Some(resolved.clone()),
            ..InstallationUpdate::state(self.machine.state(), self.facts.region_restored_early)
        });
        Some(resolved)
    }

    /// Ask the backend to install, reporting a refusal as the operation's end.
    fn start_install(
        &mut self,
        backend: &WinGetComBackend,
        request: &InstallRequest,
    ) -> Option<InstallHandle> {
        if next_step(self.machine.state(), self.facts, self.policy) != OperationStep::StartInstall {
            return None;
        }
        self.advance(InstallationOperationEvent::BeginInstall);
        self.trace
            .backend_selected(BACKEND_NAME, request.install_classification().kind());
        match backend.start_install(request) {
            Ok(handle) => {
                self.trace.backend_accepted(backend.native_detail());
                self.advance(InstallationOperationEvent::InstallRequestAccepted);
                self.advance(InstallationOperationEvent::ObservationStarted);
                self.started = Instant::now();
                self.last_observation_at = self.started;
                Some(handle)
            }
            Err(error) => {
                // The domain error names the cause; only the backend still holds
                // the number that says which one it actually was.
                self.trace.backend_refused(backend.native_detail());
                self.backend_exit_recorded = true;
                self.fail(Diagnostic::from(error));
                None
            }
        }
    }

    /// Follow the install, performing whichever step core says is due.
    fn observe(
        &mut self,
        backend: &WinGetComBackend,
        handle: &InstallHandle,
        mut guard: PendingRestoreGuard<Win32RecoveryStore>,
    ) {
        loop {
            match backend.observe(handle) {
                Ok(observation) => {
                    // The backend only holds result codes once its operation is
                    // over, and it holds them for one operation, so they are
                    // taken the first time it stops reporting progress.
                    let mut stated = None;
                    if !matches!(observation, InstallObservation::Installing { .. })
                        && !self.backend_exit_recorded
                    {
                        let detail = backend.native_detail();
                        self.trace.backend_finished(detail);
                        self.backend_exit_recorded = true;
                        stated = stated_failure(detail);
                    }
                    // A backend that stated a failure has answered the question
                    // core would otherwise have to leave open. Calling that
                    // "completion could not be confirmed" reads as "it probably
                    // worked", and the region would sit switched until the user
                    // resolved an outcome that is already known.
                    if let Some(code) = stated {
                        self.advance(InstallationOperationEvent::FailWithTemporaryRegion);
                        self.fail(Diagnostic::new(code));
                        if self.facts.region_restored_early || self.restore(&mut guard, false) {
                            clear_record(guard);
                        }
                        return;
                    }
                    if observation == InstallObservation::Completed {
                        self.record(self.confirmed_completion());
                    } else {
                        self.record(observation);
                    }
                }
                Err(error) => {
                    self.advance(InstallationOperationEvent::CompletionBecameUncertain);
                    self.fail(Diagnostic::from(error));
                    return;
                }
            }

            match next_step(self.machine.state(), self.facts, self.policy) {
                OperationStep::RestoreRegionEarly => {
                    if self.restore(&mut guard, true) {
                        self.facts.region_restored_early = true;
                        self.advance(InstallationOperationEvent::EarlyRegionRestoreConfirmed);
                    }
                }
                OperationStep::RestoreRegion => {
                    self.advance(InstallationOperationEvent::BeginRestoringRegion);
                    if self.restore(&mut guard, false) {
                        self.advance(InstallationOperationEvent::OriginalRegionRestored);
                        self.advance(InstallationOperationEvent::Complete);
                        clear_record(guard);
                        let result = self.journal_result();
                        self.write_journal_entry(result);
                    } else {
                        // Every attempt to write the original region failed.
                        // The record stays, because the next start is the only
                        // thing that can still put the region right, and the
                        // user is told now instead of watching a step that will
                        // never finish.
                        self.advance(InstallationOperationEvent::RestoreFailed);
                        self.write_journal_entry(JournalResult::RecoveryRequired);
                        self.fail(Diagnostic::new(DiagnosticCode::RestoreFailed));
                    }
                    return;
                }
                OperationStep::Finish => {
                    self.advance(InstallationOperationEvent::Complete);
                    clear_record(guard);
                    let result = self.journal_result();
                    self.write_journal_entry(result);
                    return;
                }
                OperationStep::AskUserAfterTimeout => {
                    // A deadline never restores a region; it hands the decision
                    // to the user and leaves the record in place. An operation
                    // that is already uncertain has nothing left to announce.
                    if self.machine.state() != InstallationOperationState::CompletionUncertain {
                        self.advance(InstallationOperationEvent::ObservationTimedOut);
                    }
                    self.write_journal_entry(JournalResult::CompletionUncertain);
                    // Asking is the whole point of this step, and the window is
                    // the only part of the application that can ask. Ending
                    // without saying so is what left the window showing an
                    // operation nobody could finish.
                    let region_is_temporary = !self.facts.region_restored_early;
                    (self.post)(InstallationUpdate {
                        awaiting_user_decision: region_is_temporary,
                        // With the region already back there is nothing left to
                        // decide in this session: the record survives for the
                        // next start, and the window is told the operation is
                        // over instead of being left showing it running.
                        failure: (!region_is_temporary)
                            .then(|| Diagnostic::new(DiagnosticCode::CompletionUncertain)),
                        ..InstallationUpdate::state(
                            self.machine.state(),
                            self.facts.region_restored_early,
                        )
                    });
                    return;
                }
                OperationStep::KeepObserving | OperationStep::Wait => thread::sleep(POLL_INTERVAL),
                // Both belong to earlier stages; reaching one here would mean
                // the operation went backwards, so it stops instead.
                OperationStep::StartInstall | OperationStep::ResolveUnderTemporaryRegion => return,
            }
        }
    }

    /// Whether the expected package is already present before anything ran.
    ///
    /// Installing over it would produce no evidence this operation could claim,
    /// and the user is better served by being told the truth immediately.
    fn already_installed(&self) -> bool {
        let Some(expected) = self.expected.as_ref() else {
            return false;
        };
        self.baseline
            .as_ref()
            .is_some_and(|baseline| baseline.contains(expected))
    }

    /// Turn a successful backend status into a confirmed completion, or not.
    ///
    /// The catalogue never reports a Store product as installed, so the package
    /// identity is the only proof available. Without it the operation is
    /// uncertain, which is a state the user resolves rather than a claim of
    /// success.
    fn confirmed_completion(&self) -> InstallObservation {
        let (Some(expected), Some(baseline)) = (self.expected.as_ref(), self.baseline.as_ref())
        else {
            return InstallObservation::CompletionUncertain;
        };
        let Ok(follow_up) = WindowsPackagedObserver::snapshot() else {
            return InstallObservation::CompletionUncertain;
        };
        match evaluate_packaged_observation(expected, baseline, &follow_up, &[]) {
            PackagedObservation::Completed { .. } => InstallObservation::Completed,
            // `AlreadyPresent` is not this operation's doing, and `Installing`
            // means the identity has not appeared yet.
            PackagedObservation::AlreadyPresent | PackagedObservation::Installing => {
                InstallObservation::CompletionUncertain
            }
        }
    }

    /// Write this operation's outcome into the journal, once.
    ///
    /// The region recorded is the one the product was found and installed
    /// under, which is the fact a later reader needs.
    fn write_journal_entry(&mut self, result: JournalResult) {
        if self.outcome_written {
            return;
        }
        self.outcome_written = true;
        self.trace.finished(result);
        let written = self
            .journal_record(result)
            .is_some_and(append_journal_record);
        self.trace.journal_entry(result, written);
    }

    /// Build the history entry describing how this operation ended.
    ///
    /// The card is the best source. A taken-over operation has none: it never
    /// resolved the product, because the region it would need is no longer in
    /// force. A failure before the card exists has none either. In both cases
    /// the entry names the product by its identifier, which is better history
    /// than no entry at all.
    fn journal_record(&self, result: JournalResult) -> Option<JournalRecord> {
        if let Some(product) = self.product.as_ref() {
            let subject = JournalProduct::new(
                product.product_id.clone(),
                product.package_family_name.clone(),
                product.install_classification.kind(),
                product.display_name.clone(),
                product.publisher.clone(),
                product.version.clone(),
            )
            .ok()?;
            return JournalRecord::new(
                self.identity.clone(),
                JournalSubject::Product(subject),
                self.request.temporary_region,
                now_utc()?,
                BACKEND_NAME,
                result,
            )
            .ok();
        }
        let (product_id, family_name, kind, region) = match self.taken_over.as_ref() {
            Some(pending) => (
                StoreProductId::parse(pending.product_id()).ok()?,
                pending.package_family_name().cloned(),
                pending.install_classification().kind(),
                pending.temporary_region(),
            ),
            None => (
                self.request.resolve.product_id.clone(),
                None,
                InstallKind::Unknown,
                self.request.temporary_region,
            ),
        };
        let subject = JournalProduct::new(
            product_id.clone(),
            family_name,
            kind,
            product_id.as_str().to_owned(),
            None,
            None,
        )
        .ok()?;
        JournalRecord::new(
            self.identity.clone(),
            JournalSubject::Product(subject),
            region,
            now_utc()?,
            BACKEND_NAME,
            result,
        )
        .ok()
    }

    /// Fold one observation into the facts core reasons about.
    fn record(&mut self, observation: InstallObservation) {
        if let InstallObservation::Installing { phase, progress } = observation {
            self.phase = Some(phase);
            if progress.is_some() {
                self.progress = progress;
            }
            self.facts.progress_seen = true;
        }
        if Some(observation) != self.facts.last_observation {
            self.last_observation_at = Instant::now();
            // Only changes are recorded: the backend is polled four times a
            // second, and a log of identical lines hides the one that matters.
            self.trace.observation(observation);
        }
        self.facts.last_observation = Some(observation);
        self.facts.since_request = elapsed_millis(self.started);
        self.facts.silence = elapsed_millis(self.last_observation_at);
        self.advance(InstallationOperationEvent::Observation(observation));
    }

    /// Apply one event and report the resulting state to the window.
    ///
    /// A refused event is reported, never swallowed: it would otherwise leave
    /// the operation running through steps the machine never entered.
    fn advance(&mut self, event: InstallationOperationEvent) {
        let mut persistence = NoDurableState;
        if self.machine.apply(event, &mut persistence).is_err() {
            self.fail(Diagnostic::new(DiagnosticCode::OperationSequenceViolated));
            return;
        }
        (self.post)(InstallationUpdate {
            phase: self.phase,
            progress: self.progress,
            ..InstallationUpdate::state(self.machine.state(), self.facts.region_restored_early)
        });
    }

    /// The journal outcome that matches how the operation ended.
    fn journal_result(&self) -> JournalResult {
        match self.facts.last_observation {
            Some(InstallObservation::Completed) => JournalResult::Installed,
            Some(InstallObservation::CompletionUncertain) | None => {
                JournalResult::CompletionUncertain
            }
            Some(InstallObservation::Installing { .. }) => JournalResult::RecoveryRequired,
        }
    }

    /// Report a failure without claiming anything about the installation.
    ///
    /// A failure that never touched the machine belongs in the diagnostic log
    /// only. Once a recovery record exists or a region was written, the
    /// operation happened, and the history says so rather than losing it.
    fn fail(&mut self, diagnostic: Diagnostic) {
        self.trace.diagnostic(&diagnostic);
        if self.touched_machine {
            self.write_journal_entry(JournalResult::Failed);
        }
        (self.post)(InstallationUpdate {
            failure: Some(diagnostic),
            ..InstallationUpdate::state(self.machine.state(), self.facts.region_restored_early)
        });
    }
}

/// Append one entry to the operation history, reporting whether it landed.
///
/// Shared with the handoff path: both operations own the same history file and
/// must reach it the same way.
pub(super) fn append_journal_record(record: JournalRecord) -> bool {
    let Ok(store) = Win32JournalStore::for_current_user() else {
        return false;
    };
    let Ok(mut records) = store.load() else {
        return false;
    };
    records.push(record);
    store.replace(&records).is_ok()
}

/// Write the history entry an installation left without an outcome never wrote.
///
/// The record names the product, the region it was installed under and the
/// method used. The result is `Installed` only when the identity the record was
/// waiting for is present now — proof this operation finished — and otherwise
/// says the completion was never confirmed, which is the whole of what a record
/// without evidence supports.
pub(super) fn record_operation_without_outcome(pending: &PendingRestore, package_present: bool) {
    let Ok(product_id) = StoreProductId::parse(pending.product_id()) else {
        return;
    };
    let Ok(subject) = JournalProduct::new(
        product_id.clone(),
        pending.package_family_name().cloned(),
        pending.install_classification().kind(),
        product_id.as_str().to_owned(),
        None,
        None,
    ) else {
        return;
    };
    let Some(written_at) = now_utc() else {
        return;
    };
    let Ok(record) = JournalRecord::new(
        pending.operation_id().clone(),
        JournalSubject::Product(subject),
        pending.temporary_region(),
        written_at,
        pending.backend(),
        if package_present {
            JournalResult::Installed
        } else {
            JournalResult::CompletionUncertain
        },
    ) else {
        return;
    };
    let _ = append_journal_record(record);
}

/// Name the cause of a region switch that never completed.
fn switch_diagnostic(error: &RegionSwitchError<RecoveryStoreError>) -> Diagnostic {
    match error {
        RegionSwitchError::ReadOriginal(read) | RegionSwitchError::ReadBack(read) => {
            Diagnostic::from(*read)
        }
        RegionSwitchError::PendingSave(store) | RegionSwitchError::PendingCleanup(store) => {
            Diagnostic::from(*store)
        }
    }
}

/// Restore the original region, confirm it by read-back, and record the fact.
///
/// The durable milestone is written only after the read-back agrees, so a
/// record can never claim a region the machine does not have.
fn restore_region(guard: &mut PendingRestoreGuard<Win32RecoveryStore>, early: bool) -> bool {
    let reader = Win32RegionReader;
    let writer = Win32RegionWriter;
    let original = guard.pending().original_region();
    if !write_original_region(&reader, &writer, original) {
        return false;
    }
    let milestone = if early {
        DurableOperationState::RegionRestoredEarly
    } else {
        DurableOperationState::Restored
    };
    guard.update_state(milestone).is_ok()
}

/// Write the original region until a read-back agrees, or give up saying so.
///
/// The attempts are repeated rather than reported once: a region write is
/// refused for reasons that pass, and the alternative to trying again is
/// leaving the machine on a region the user never chose.
fn write_original_region(
    reader: &Win32RegionReader,
    writer: &Win32RegionWriter,
    original: GeoId,
) -> bool {
    for attempt in 0..RESTORE_ATTEMPTS {
        if attempt > 0 {
            thread::sleep(RESTORE_RETRY_PAUSE);
        }
        if RegionWriter::set_region(writer, original).is_err() {
            continue;
        }
        if reader_original_region(reader) == Some(original) {
            return true;
        }
    }
    false
}

/// Clear the recovery record once the operation has an outcome.
///
/// Only a record whose original region is confirmed may go; the store verifies
/// what it clears, so a mismatch leaves the record in place.
fn clear_record(guard: PendingRestoreGuard<Win32RecoveryStore>) {
    let pending = guard.pending().clone();
    let mut store = guard.into_store();
    let _ = store.clear_verified(&pending);
}

/// The identity a resolved card names, when it names one.
fn expected_identity(product: &ResolvedProduct) -> Option<ExpectedPackagedIdentity> {
    ExpectedPackagedIdentity::new(
        product.install_classification,
        product.product_id.clone(),
        product.package_family_name.clone()?,
    )
    .ok()
}

/// The identity a recovery record names, when the operation got that far.
fn pending_identity(pending: &PendingRestore) -> Option<ExpectedPackagedIdentity> {
    let product_id = StoreProductId::parse(pending.product_id()).ok()?;
    ExpectedPackagedIdentity::new(
        pending.install_classification(),
        product_id,
        pending.package_family_name()?.clone(),
    )
    .ok()
}

/// Milliseconds since an instant, saturating rather than panicking.
fn elapsed_millis(since: Instant) -> ObservationElapsedMillis {
    ObservationElapsedMillis::new(u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX))
}

/// Current region, or `None` when Windows cannot answer.
fn reader_original_region(reader: &Win32RegionReader) -> Option<GeoId> {
    use winstoreregion_core::RegionReader;
    reader.current_region().ok().map(|region| region.geo_id)
}

/// Current UTC instant, or nothing when the clock cannot be read.
///
/// Shared with the handoff path, so one operation cannot stamp its history
/// differently from the other. A clock that cannot answer yields nothing
/// rather than a substitute: the value goes into a durable record and a history
/// entry, and a stamp of 1970 reads as fact to everything that opens them
/// later.
pub(super) fn now_utc() -> Option<UtcTimestamp> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| UtcTimestamp::from_unix_seconds(elapsed.as_secs()))
}

/// The single identifier of one guarded operation.
///
/// It hands out the same value every time on purpose. The recovery record is
/// written before the switch and must name the very change the switch then
/// prepares; a fresh identifier per call would make the two disagree and the
/// store would refuse the record.
struct SingleOperationId(OperationId);

impl SingleOperationId {
    /// Derive one identifier from the clock.
    fn from_clock() -> Self {
        let stamp = observation_timestamp_now().unix_millis();
        Self(OperationId::new(format!("op-{stamp}")).unwrap_or_else(|| {
            OperationId::new("op").expect("a constant identifier is never empty")
        }))
    }
}

impl OperationIdGenerator for SingleOperationId {
    fn next_operation_id(&mut self) -> OperationId {
        self.0.clone()
    }
}

/// The recovery record is published by the region guard, not by the machine.
///
/// The machine's milestones would otherwise be written twice, and the guard is
/// the component that verifies what it wrote.
struct NoDurableState;

impl OperationStatePersistence for NoDurableState {
    type Error = std::convert::Infallible;

    fn persist_state(
        &mut self,
        _state: winstoreregion_core::DurableOperationState,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use winstoreregion_core::{MarketCode, RecoveryStore, ResolveRequestId, StoreProductId};

    #[test]
    fn a_failure_before_the_region_changes_says_so_in_its_state() {
        let update = failure(Diagnostic::new(DiagnosticCode::BackendUnavailable));
        assert_eq!(
            update.state,
            InstallationOperationState::FailedBeforeRegionChange
        );
        assert!(!update.state.requires_recovery());
        assert!(update.failure.is_some());
    }

    #[test]
    fn a_failure_before_a_card_exists_still_names_the_product_in_its_history_entry() {
        let request = InstallationRequest {
            resolve: ResolveRequest {
                request_id: ResolveRequestId::new(1),
                product_id: StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID"),
                market: MarketCode::parse("US").expect("valid market"),
            },
            temporary_region: GeoId::new(244).expect("positive GeoId"),
        };
        let post = |_update: InstallationUpdate| {};
        let operation = Operation::new(&request, &post);

        let record = operation
            .journal_record(JournalResult::Failed)
            .expect("an entry the history can hold");

        let product = record
            .subject()
            .product()
            .expect("an installation entry is about a product");
        assert_eq!(product.product_id().as_str(), "9WZDNCRFJ3L1");
        assert_eq!(product.name(), "9WZDNCRFJ3L1");
        assert_eq!(product.install_kind(), InstallKind::Unknown);
        assert_eq!(record.region(), request.temporary_region);
        assert_eq!(record.result(), JournalResult::Failed);
    }

    /// Reports an application that is already installed instead of installing
    /// it again. Run after the guarded-installation test, on a test machine.
    #[test]
    #[ignore = "changes the Windows region"]
    fn an_application_that_is_already_installed_is_reported_not_reinstalled() {
        let reports: Mutex<Vec<InstallationUpdate>> = Mutex::new(Vec::new());
        let request = InstallationRequest {
            resolve: ResolveRequest {
                request_id: ResolveRequestId::new(1),
                product_id: StoreProductId::parse("9NT1R1C2HH7J").expect("valid identifier"),
                market: MarketCode::parse("US").expect("valid market"),
            },
            temporary_region: GeoId::new(244).expect("valid GeoId"),
        };
        let reader = Win32RegionReader;
        let before = reader_original_region(&reader).expect("the machine reports its region");
        {
            let _runtime = WindowsRuntimeGuard::initialize().expect("the runtime starts");
            match WindowsPackagedObserver::snapshot() {
                Ok(snapshot) => println!("baseline packages: {}", snapshot.entries.len()),
                Err(error) => println!("baseline unavailable: {error:?}"),
            }
        }

        run(&request, &|update| {
            reports.lock().expect("no poisoned lock").push(update);
        });

        let reports = reports.into_inner().expect("no poisoned lock");
        for report in &reports {
            println!(
                "state={:?} failure={:?} resolved={:?}",
                report.state,
                report.failure.as_ref().map(Diagnostic::code),
                report
                    .resolved
                    .as_ref()
                    .map(|product| &product.display_name),
            );
        }
        let causes: Vec<_> = reports
            .iter()
            .filter_map(|report| report.failure.as_ref().map(Diagnostic::code))
            .collect();
        assert!(
            causes.contains(&DiagnosticCode::ProductAlreadyInstalled),
            "an installed product must be named as such: {causes:?}"
        );
        assert_eq!(
            reader_original_region(&reader),
            Some(before),
            "the original region must come back"
        );
    }

    /// Takes over an install left running by another process. Start one with
    /// the guarded-installation test in a process that is then killed.
    #[test]
    #[ignore = "requires an install left in flight by another process"]
    fn an_install_left_by_another_process_is_taken_over() {
        let store = Win32RecoveryStore::for_current_user().expect("a recovery store");
        let pending = RecoveryStore::load(&store)
            .expect("the record can be read")
            .expect("a previous run must have left a record");
        let request = resumed_request(&pending).expect("the record names a product and region");

        let reports: Mutex<Vec<InstallationUpdate>> = Mutex::new(Vec::new());
        follow_resumed_install(&request, &pending, &|update| {
            reports.lock().expect("no poisoned lock").push(update);
        });

        let reports = reports.into_inner().expect("no poisoned lock");
        let states: Vec<_> = reports.iter().map(|report| report.state).collect();
        for report in &reports {
            println!(
                "state={:?} phase={:?} progress={:?} early={} failure={:?}",
                report.state,
                report.phase,
                report.progress,
                report.region_restored_early,
                report.failure.as_ref().map(Diagnostic::code),
            );
        }
        assert!(
            !states.is_empty(),
            "an install in flight must produce observations"
        );
        assert!(
            states.contains(&InstallationOperationState::Completed),
            "the resumed operation must reach an end: {states:?}"
        );
    }

    /// Installs a real application under a temporary region, so it runs only
    /// where that is allowed: `cargo test -- --ignored guarded_installation`.
    #[test]
    #[ignore = "changes the Windows region and installs an application"]
    fn a_guarded_installation_switches_installs_and_restores() {
        let reports: Mutex<Vec<InstallationUpdate>> = Mutex::new(Vec::new());
        let request = InstallationRequest {
            resolve: ResolveRequest {
                request_id: ResolveRequestId::new(1),
                // Offered in 244 and absent from 203, which is the case this
                // application exists for.
                product_id: StoreProductId::parse("9NT1R1C2HH7J").expect("valid identifier"),
                market: MarketCode::parse("US").expect("valid market"),
            },
            temporary_region: GeoId::new(244).expect("valid GeoId"),
        };
        let reader = Win32RegionReader;
        let before = reader_original_region(&reader).expect("the machine reports its region");

        run(&request, &|update| {
            reports.lock().expect("no poisoned lock").push(update);
        });

        let reports = reports.into_inner().expect("no poisoned lock");
        let states: Vec<_> = reports.iter().map(|report| report.state).collect();
        for report in &reports {
            println!(
                "state={:?} phase={:?} progress={:?} early={} failure={:?} resolved={:?}",
                report.state,
                report.phase,
                report.progress,
                report.region_restored_early,
                report.failure.as_ref().map(Diagnostic::code),
                report
                    .resolved
                    .as_ref()
                    .map(|product| &product.display_name),
            );
        }
        assert!(
            states.contains(&InstallationOperationState::RegionSwitched),
            "the temporary region must be confirmed by read-back: {states:?}"
        );
        assert!(
            reports.iter().any(|report| report.resolved.is_some()),
            "the product must be resolved under the temporary region"
        );
        assert!(
            states.contains(&InstallationOperationState::Completed),
            "the operation must finish: {states:?}"
        );
        assert_eq!(
            reader_original_region(&reader),
            Some(before),
            "the original region must come back"
        );
    }
}
