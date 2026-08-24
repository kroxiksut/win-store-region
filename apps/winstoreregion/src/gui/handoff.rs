//! Running one Store-installer handoff off the UI thread.
//!
//! A handoff is not an installation and never claims to be one. A Microsoft
//! Store installer carries no readable Product ID, so this operation cannot
//! name a product, cannot observe progress and cannot prove completion. What it
//! can do is exactly what the user needs: hold the temporary region under the
//! full recovery protocol, start the signed installer, and leave the region in
//! the user's hands until they say the work in Microsoft Store is done.
//!
//! Every safety rule still applies. The record is durable before Windows is
//! touched, the region is confirmed by read-back before anything is executed,
//! and the file is re-verified against the digest the user confirmed.

use crate::gui::diagnostic::stub_diagnostic;
use crate::gui::ids::{WM_APP_HANDOFF_PROGRESS, post_boxed};
use crate::gui::install::{append_journal_record, now_utc};
use crate::platform::diagnostic_log::record;
use crate::platform::handoff::launch_admitted_installer;
use crate::platform::observation_timestamp_now;
use crate::platform::region::{Win32RegionReader, Win32RegionWriter};
use crate::platform::storage::Win32RecoveryStore;
use crate::platform::stub::inspect_installer_stub;
use std::path::PathBuf;
use std::thread;
use windows::Win32::Foundation::HWND;
use winstoreregion_core::{
    Diagnostic, DiagnosticCode, DurableOperationState, GeoId, HandoffFileIdentity, JournalRecord,
    JournalResult, JournalSubject, LogEvent, LogEventCode, LogLevel, OperationId,
    OperationIdGenerator, PendingRecoveryAction, PendingRecoveryChoice, PendingRecoveryDisposition,
    PendingRecoveryExecution, PendingRestore, PendingRestoreGuard, RecoveryStore,
    RecoveryStoreError, RegionReader, RegionSwitchError, RegionSwitchOutcome, RegionWriteError,
    RegionWriter, STORE_INSTALLER_HANDOFF_BACKEND, TechnicalDetail, UtcTimestamp,
    admit_confirmed_installer_handoff, choose_pending_recovery_action, classify_pending_recovery,
    execute_pending_recovery_action, switch_region,
};

/// Where a handoff stands, as far as the window is concerned.
///
/// There is deliberately no "installing" and no "installed": this operation
/// observes nothing after the launch, and a stage that implied otherwise would
/// be the one lie this whole path exists to avoid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HandoffStage {
    /// The recovery record is being published and the region switched.
    SwitchingRegion,
    /// The installer was started under the confirmed temporary region.
    HandedOff,
    /// The user asked for the original region back.
    RestoringRegion,
    /// Read-back confirmed the original region and the record is gone.
    Restored,
    /// The operation stopped with Windows holding the original region.
    ///
    /// Either nothing was ever written, or it was written and put back. Both
    /// mean the same thing to the window: nothing is pending.
    FailedRegionSafe,
    /// The operation stopped and a recovery record is still published.
    FailedWithTemporaryRegion,
}

impl HandoffStage {
    /// Whether a pending recovery record belongs to this stage.
    pub(super) const fn requires_recovery(self) -> bool {
        matches!(
            self,
            Self::HandedOff | Self::RestoringRegion | Self::FailedWithTemporaryRegion
        )
    }

    /// Whether the region can still be put back from the window.
    pub(super) const fn offers_region_restore(self) -> bool {
        matches!(self, Self::HandedOff | Self::FailedWithTemporaryRegion)
    }

    /// Whether this handoff still stands in the way of a new operation.
    pub(super) const fn blocks_new_operation(self) -> bool {
        !matches!(self, Self::Restored | Self::FailedRegionSafe)
    }
}

/// Everything the thread needs to hand one file over.
pub(super) struct HandoffRequest {
    /// The file the user selected, exactly as it was inspected.
    pub(super) path: PathBuf,
    /// Digest shown in the confirmation, re-checked before anything is run.
    pub(super) confirmed_sha256: String,
    /// Region to switch to and hold while Microsoft Store works.
    pub(super) temporary_region: GeoId,
}

/// What the window learns about a running or finished handoff.
pub(super) struct HandoffUpdate {
    pub(super) stage: HandoffStage,
    /// Set once, when the operation can go no further.
    pub(super) failure: Option<Diagnostic>,
    /// The region the operation will put back, once it is known.
    pub(super) original_region: Option<GeoId>,
    /// The file as it was admitted, once the gate has passed it.
    pub(super) file: Option<HandoffFileIdentity>,
}

impl HandoffUpdate {
    const fn stage(stage: HandoffStage) -> Self {
        Self {
            stage,
            failure: None,
            original_region: None,
            file: None,
        }
    }

    /// A failure that left Windows holding the original region.
    fn safe_failure(diagnostic: Diagnostic) -> Self {
        Self {
            stage: HandoffStage::FailedRegionSafe,
            failure: Some(diagnostic),
            original_region: None,
            file: None,
        }
    }

    /// A failure that left a recovery record for the region behind.
    fn pending_failure(diagnostic: Diagnostic, original_region: GeoId) -> Self {
        Self {
            stage: HandoffStage::FailedWithTemporaryRegion,
            failure: Some(diagnostic),
            original_region: Some(original_region),
            file: None,
        }
    }
}

/// Start one handoff on its own thread.
pub(super) fn start_handoff(window: HWND, request: HandoffRequest) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let post = |update: HandoffUpdate| post_update(window_handle, update);
        run(&request, &post);
    });
}

/// Put the original region back for whatever handoff left a record.
///
/// The record is the authority, not the window: it names the original region,
/// and core owns the conflict-safe protocol that writes it. Nothing here
/// decides anything a restart would decide differently.
pub(super) fn start_region_restore(window: HWND) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let post = |update: HandoffUpdate| post_update(window_handle, update);
        post(HandoffUpdate::stage(HandoffStage::RestoringRegion));
        restore(&post);
    });
}

/// Post one update to the window, discarding it if the window has gone.
fn post_update(window_handle: usize, update: HandoffUpdate) {
    post_boxed(window_handle, WM_APP_HANDOFF_PROGRESS, update);
}

/// Carry out one handoff, reporting every step through `post`.
#[allow(clippy::too_many_lines)]
fn run(request: &HandoffRequest, post: &dyn Fn(HandoffUpdate)) {
    let identity = handoff_operation_id();
    post(HandoffUpdate::stage(HandoffStage::SwitchingRegion));

    // The file is inspected again here rather than trusted from the window: the
    // confirmation happened before the region changed, and this is the moment
    // that actually executes something.
    let inspection = match inspect_installer_stub(&request.path) {
        Ok(inspection) => inspection,
        Err(error) => {
            let diagnostic = stub_diagnostic(error);
            report(&identity, &diagnostic);
            post(HandoffUpdate::safe_failure(diagnostic));
            return;
        }
    };
    let file = match admit_confirmed_installer_handoff(&inspection, &request.confirmed_sha256) {
        Ok(file) => file,
        Err(refusal) => {
            let diagnostic = Diagnostic::from(refusal);
            report(&identity, &diagnostic);
            post(HandoffUpdate::safe_failure(diagnostic));
            return;
        }
    };

    let reader = Win32RegionReader;
    let writer = Win32RegionWriter;
    let store = match Win32RecoveryStore::for_current_user() {
        Ok(store) => store,
        Err(error) => {
            let diagnostic = Diagnostic::from(error);
            report(&identity, &diagnostic);
            post(HandoffUpdate::safe_failure(diagnostic));
            return;
        }
    };
    let original = match RegionReader::current_region(&reader) {
        Ok(region) => region.geo_id,
        Err(error) => {
            let diagnostic = Diagnostic::from(error);
            report(&identity, &diagnostic);
            post(HandoffUpdate::safe_failure(diagnostic));
            return;
        }
    };
    record(
        &LogEvent::new(LogLevel::Info, LogEventCode::OperationStarted)
            .for_operation(identity.clone())
            .with_token("backend", STORE_INSTALLER_HANDOFF_BACKEND)
            .with_file_identity(file.file_name(), file.sha256())
            .with_geo_id("to", request.temporary_region),
    );

    // The record names the file because there is no product identifier to name.
    let prepared = now_utc().and_then(|started_at| {
        PendingRestore::prepared(
            identity.clone(),
            started_at,
            original,
            request.temporary_region,
            file.record_text(),
            STORE_INSTALLER_HANDOFF_BACKEND,
        )
        .ok()
    });
    let Some(pending) = prepared else {
        let diagnostic = Diagnostic::new(DiagnosticCode::RecoveryRecordUnreadable);
        report(&identity, &diagnostic);
        post(HandoffUpdate::safe_failure(diagnostic));
        return;
    };
    let mut guard = PendingRestoreGuard::new(store, pending);
    let mut ids = SingleOperationId(identity.clone());
    let switch = switch_region(
        request.temporary_region,
        &reader,
        &writer,
        &mut ids,
        &mut guard,
    );
    record(
        &LogEvent::new(LogLevel::Info, LogEventCode::RecoveryRecordWritten)
            .for_operation(identity.clone())
            .with_geo_id("from", original)
            .with_geo_id("to", request.temporary_region),
    );
    let switched = match switch {
        Ok(report) => report,
        Err(error) => {
            let diagnostic = switch_diagnostic(&error);
            record(
                &LogEvent::new(LogLevel::Error, LogEventCode::RegionSwitchAttempted)
                    .for_operation(identity.clone())
                    .with_geo_id("to", request.temporary_region)
                    .with_token("outcome", "not_classified"),
            );
            report(&identity, &diagnostic);
            // The record survives on purpose: a switch that could not even be
            // classified may have left Windows somewhere else.
            write_handoff_entry(
                &identity,
                &file,
                request.temporary_region,
                JournalResult::Failed,
            );
            post(HandoffUpdate::pending_failure(diagnostic, original));
            return;
        }
    };
    record(&region_switch_event(
        &identity,
        original,
        request.temporary_region,
        switched.writer_result,
        switched.outcome,
    ));
    match switched.outcome {
        RegionSwitchOutcome::RegionSwitched => {}
        RegionSwitchOutcome::OriginalRegionPreserved => {
            // `switch_region` already cleared the record: nothing changed, so
            // there is no history to write and nothing left pending.
            let diagnostic = Diagnostic::new(DiagnosticCode::RegionUnchanged);
            report(&identity, &diagnostic);
            post(HandoffUpdate::safe_failure(diagnostic));
            return;
        }
        RegionSwitchOutcome::RegionConflict { .. } => {
            let diagnostic = Diagnostic::new(DiagnosticCode::RegionConflict);
            report(&identity, &diagnostic);
            write_handoff_entry(
                &identity,
                &file,
                request.temporary_region,
                JournalResult::Failed,
            );
            post(HandoffUpdate::pending_failure(diagnostic, original));
            return;
        }
    }
    // The milestone is durable before the launch, so a restart that finds this
    // record knows the region was actually written.
    let _ = guard.update_state(DurableOperationState::RegionSwitched);

    // The bytes were admitted before the region changed, and changing a region
    // is not instant. What runs has to be what was admitted, not whatever the
    // path names now, so the file answers for itself once more with nothing
    // between the answer and the launch.
    let file = match inspect_installer_stub(&request.path)
        .map_err(stub_diagnostic)
        .and_then(|inspection| {
            admit_confirmed_installer_handoff(&inspection, &request.confirmed_sha256)
                .map_err(Diagnostic::from)
        }) {
        Ok(file) => file,
        Err(diagnostic) => {
            report(&identity, &diagnostic);
            post(HandoffUpdate::pending_failure(diagnostic, original));
            return;
        }
    };

    let Err(error) = launch_admitted_installer(&request.path) else {
        record(
            &LogEvent::new(LogLevel::Info, LogEventCode::HandoffLaunchAttempted)
                .for_operation(identity)
                .with_file_identity(file.file_name(), file.sha256())
                .with_geo_id("region", request.temporary_region)
                .with_token("outcome", "started"),
        );
        // The record stays published. The region goes back when the user says
        // Microsoft Store is finished, not when this thread ends.
        post(HandoffUpdate {
            stage: HandoffStage::HandedOff,
            failure: None,
            original_region: Some(original),
            file: Some(file),
        });
        return;
    };
    record(
        &LogEvent::new(LogLevel::Error, LogEventCode::HandoffLaunchAttempted)
            .for_operation(identity.clone())
            .with_file_identity(file.file_name(), file.sha256())
            .with_code("native", error.native_code())
            .with_token("outcome", "rejected"),
    );
    let diagnostic = Diagnostic::with_detail(
        DiagnosticCode::InstallerLaunchRejected,
        TechnicalDetail {
            variant: "HandoffLaunchError::LaunchRejected",
            native_code: Some(error.native_code()),
            operation_id: None,
        },
    );
    report(&identity, &diagnostic);
    // Nothing was started, so the temporary region has no reason to stay a
    // moment longer.
    let restored = restore_region_now(&mut guard, &identity);
    write_handoff_entry(
        &identity,
        &file,
        request.temporary_region,
        JournalResult::Failed,
    );
    if restored {
        clear_record(guard);
        post(HandoffUpdate::safe_failure(diagnostic));
    } else {
        post(HandoffUpdate::pending_failure(diagnostic, original));
    }
}

/// Put the original region back through the core recovery protocol.
fn restore(post: &dyn Fn(HandoffUpdate)) {
    let mut store = match Win32RecoveryStore::for_current_user() {
        Ok(store) => store,
        Err(error) => {
            post(HandoffUpdate::safe_failure(Diagnostic::from(error)));
            return;
        }
    };
    let pending = match RecoveryStore::load(&store) {
        Ok(Some(pending)) => pending,
        // Nothing is published, so nothing needs restoring: the region Windows
        // has is already the one the window shows.
        Ok(None) => {
            post(HandoffUpdate::stage(HandoffStage::Restored));
            return;
        }
        Err(error) => {
            post(HandoffUpdate::safe_failure(Diagnostic::from(error)));
            return;
        }
    };
    let identity = pending.operation_id().clone();
    let original = pending.original_region();
    let current = match RegionReader::current_region(&Win32RegionReader) {
        Ok(region) => region.geo_id,
        Err(error) => {
            post(HandoffUpdate::pending_failure(
                Diagnostic::from(error),
                original,
            ));
            return;
        }
    };
    let action = match classify_pending_recovery(&pending, current) {
        PendingRecoveryDisposition::OriginalRegionAlreadyActive => {
            PendingRecoveryAction::ClearVerifiedRecord
        }
        disposition => {
            let Ok(action) = choose_pending_recovery_action(
                disposition,
                PendingRecoveryChoice::RestoreOriginalRegion,
            ) else {
                post(HandoffUpdate::pending_failure(
                    Diagnostic::new(DiagnosticCode::RecoveryRecordUnreadable),
                    original,
                ));
                return;
            };
            action
        }
    };
    let outcome = execute_pending_recovery_action(
        action,
        &pending,
        &mut store,
        &Win32RegionReader,
        &Win32RegionWriter,
    );
    let restored = matches!(
        outcome,
        Ok(PendingRecoveryExecution::Restored { .. }
            | PendingRecoveryExecution::VerifiedRecordCleared)
    );
    record(&restore_event(&identity, original, restored));
    if !restored {
        // Core preserves the record in every one of these cases, so the next
        // start still offers the region back.
        post(HandoffUpdate::pending_failure(
            Diagnostic::new(DiagnosticCode::RestoreFailed),
            original,
        ));
        return;
    }
    // Only now is there an outcome worth keeping: the region is the original
    // one and the installer had its chance.
    if let Some(file) = handoff_file_of(&pending) {
        write_handoff_entry(
            &identity,
            &file,
            pending.temporary_region(),
            JournalResult::HandedOffToInstaller,
        );
    }
    post(HandoffUpdate {
        stage: HandoffStage::Restored,
        failure: None,
        original_region: Some(original),
        file: None,
    });
}

/// Record a handoff whose region came back through startup recovery.
///
/// The region can be restored two ways: the button in this window, and the
/// dialog the next start offers for a record the previous run left behind. The
/// history owes the user the same entry either way, so the second route writes
/// it here rather than leaving a handoff with no trace at all.
///
/// A record left by the installation backend is not a handoff and is ignored.
pub(super) fn record_restored_handoff(pending: &PendingRestore) {
    let Some(file) = handoff_file_of(pending) else {
        return;
    };
    write_handoff_entry(
        pending.operation_id(),
        &file,
        pending.temporary_region(),
        JournalResult::HandedOffToInstaller,
    );
}

/// The file a handoff record names, when the record belongs to a handoff.
///
/// A record left by the installation backend names a Product ID instead, and
/// must never be turned into a file entry.
fn handoff_file_of(pending: &PendingRestore) -> Option<HandoffFileIdentity> {
    if pending.backend() != STORE_INSTALLER_HANDOFF_BACKEND {
        return None;
    }
    let (name, sha256) = pending.product_id().rsplit_once(" sha256:")?;
    HandoffFileIdentity::new(name, sha256)
}

/// Build this handoff's one history entry, naming the file and nothing else.
///
/// The entry can only ever be about a file: there is no product identity in
/// this operation, and the backend name says which path produced it.
fn handoff_record(
    identity: &OperationId,
    file: &HandoffFileIdentity,
    region: GeoId,
    result: JournalResult,
    at: UtcTimestamp,
) -> Option<JournalRecord> {
    JournalRecord::new(
        identity.clone(),
        JournalSubject::InstallerFile(file.clone()),
        region,
        at,
        STORE_INSTALLER_HANDOFF_BACKEND,
        result,
    )
    .ok()
}

/// Write this handoff's one history entry.
fn write_handoff_entry(
    identity: &OperationId,
    file: &HandoffFileIdentity,
    region: GeoId,
    result: JournalResult,
) {
    let written = now_utc()
        .and_then(|at| handoff_record(identity, file, region, result, at))
        .is_some_and(append_journal_record);
    record(
        &LogEvent::new(
            if written {
                LogLevel::Info
            } else {
                LogLevel::Warning
            },
            LogEventCode::JournalEntryWritten,
        )
        .for_operation(identity.clone())
        .with_token("result", result_token(result))
        .with_flag("written", written),
    );
}

/// Stable machine-facing name of one history outcome.
const fn result_token(result: JournalResult) -> &'static str {
    match result {
        JournalResult::Installed => "installed",
        JournalResult::HandedOffToInstaller => "handed_off_to_installer",
        JournalResult::CompletionUncertain => "completion_uncertain",
        JournalResult::RecoveryRequired => "recovery_required",
        JournalResult::Failed => "failed",
    }
}

/// Write the original region back and confirm it by read-back.
fn restore_region_now(
    guard: &mut PendingRestoreGuard<Win32RecoveryStore>,
    identity: &OperationId,
) -> bool {
    let original = guard.pending().original_region();
    let restored = RegionWriter::set_region(&Win32RegionWriter, original).is_ok()
        && RegionReader::current_region(&Win32RegionReader)
            .is_ok_and(|region| region.geo_id == original)
        && guard.update_state(DurableOperationState::Restored).is_ok();
    record(&restore_event(identity, original, restored));
    restored
}

/// One log line for an attempt to put the original region back.
fn restore_event(identity: &OperationId, original: GeoId, restored: bool) -> LogEvent {
    LogEvent::new(
        if restored {
            LogLevel::Info
        } else {
            LogLevel::Error
        },
        LogEventCode::RegionRestoreAttempted,
    )
    .for_operation(identity.clone())
    .with_geo_id("to", original)
    .with_flag("early", false)
    .with_token("outcome", if restored { "restored" } else { "failed" })
}

/// Clear the recovery record once the original region is confirmed.
fn clear_record(guard: PendingRestoreGuard<Win32RecoveryStore>) {
    let pending = guard.pending().clone();
    let mut store = guard.into_store();
    let _ = store.clear_verified(&pending);
}

/// Record the failure the user was shown.
fn report(identity: &OperationId, diagnostic: &Diagnostic) {
    let event = LogEvent::new(LogLevel::Error, LogEventCode::DiagnosticReported)
        .for_operation(identity.clone())
        .with_token("code", diagnostic.code().as_str());
    let event = match diagnostic.technical() {
        Some(technical) => {
            let event = event.with_token("variant", technical.variant);
            match technical.native_code {
                Some(code) => event.with_code("native", code),
                None => event,
            }
        }
        None => event,
    };
    record(&event);
}

/// One log line for a region switch that reached its mandatory read-back.
fn region_switch_event(
    identity: &OperationId,
    original: GeoId,
    target: GeoId,
    writer_result: Result<(), RegionWriteError>,
    outcome: RegionSwitchOutcome,
) -> LogEvent {
    let event = LogEvent::new(
        match outcome {
            RegionSwitchOutcome::RegionSwitched => LogLevel::Info,
            RegionSwitchOutcome::OriginalRegionPreserved => LogLevel::Warning,
            RegionSwitchOutcome::RegionConflict { .. } => LogLevel::Error,
        },
        LogEventCode::RegionSwitchAttempted,
    )
    .for_operation(identity.clone())
    .with_geo_id("from", original)
    .with_geo_id("to", target)
    .with_token(
        "outcome",
        match outcome {
            RegionSwitchOutcome::RegionSwitched => "region_switched",
            RegionSwitchOutcome::OriginalRegionPreserved => "original_region_preserved",
            RegionSwitchOutcome::RegionConflict { .. } => "region_conflict",
        },
    );
    let event = match writer_result {
        Ok(()) => event.with_token("writer", "accepted"),
        Err(RegionWriteError::Rejected) => event.with_token("writer", "rejected"),
        Err(RegionWriteError::PlatformFailure(code)) => event
            .with_token("writer", "platform_failure")
            .with_code("native", code),
    };
    match outcome {
        RegionSwitchOutcome::RegionConflict { current_region } => {
            event.with_geo_id("read_back", current_region)
        }
        RegionSwitchOutcome::RegionSwitched => event.with_geo_id("read_back", target),
        RegionSwitchOutcome::OriginalRegionPreserved => event.with_geo_id("read_back", original),
    }
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

/// The single identifier of one handoff.
///
/// It hands out the same value every time for the same reason the installation
/// path does: the record is written before the switch and must name the very
/// change the switch then prepares.
struct SingleOperationId(OperationId);

impl OperationIdGenerator for SingleOperationId {
    fn next_operation_id(&mut self) -> OperationId {
        self.0.clone()
    }
}

/// Derive one identifier from the clock, marked as a handoff in the log.
fn handoff_operation_id() -> OperationId {
    let stamp = observation_timestamp_now().unix_millis();
    OperationId::new(format!("handoff-{stamp}")).unwrap_or_else(|| {
        OperationId::new("handoff").expect("a constant identifier is never empty")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_for(product_text: &str, backend: &str) -> PendingRestore {
        PendingRestore::prepared(
            OperationId::new("handoff-test").expect("non-empty identifier"),
            UtcTimestamp::parse("2026-08-21T09:00:00Z").expect("valid timestamp"),
            GeoId::new(203).expect("positive GeoId"),
            GeoId::new(244).expect("positive GeoId"),
            product_text,
            backend,
        )
        .expect("complete pending record")
    }

    #[test]
    fn only_a_handoff_record_yields_a_file_and_an_install_record_never_does() {
        let handoff = pending_for(
            "StoreInstaller.exe sha256:abc123",
            STORE_INSTALLER_HANDOFF_BACKEND,
        );
        let file = handoff_file_of(&handoff).expect("a handoff record names its file");
        assert_eq!(file.file_name(), "StoreInstaller.exe");
        assert_eq!(file.sha256(), "abc123");

        // A record left by the installation backend names a Product ID, and
        // reading it as a file name would invent history that never happened.
        assert!(handoff_file_of(&pending_for("9WZDNCRFJ3L1", "winget-com-msstore")).is_none());
        // Even under the handoff backend, text that is not the recorded form
        // yields nothing rather than half an identity.
        assert!(
            handoff_file_of(&pending_for(
                "9WZDNCRFJ3L1",
                STORE_INSTALLER_HANDOFF_BACKEND
            ))
            .is_none()
        );
    }

    #[test]
    fn a_stage_offers_a_restore_only_where_a_record_can_still_exist() {
        assert!(HandoffStage::HandedOff.requires_recovery());
        assert!(HandoffStage::HandedOff.offers_region_restore());
        assert!(HandoffStage::HandedOff.blocks_new_operation());

        // A failure that left the region temporary keeps both the button and
        // the block; one that ended with the original region keeps neither.
        assert!(HandoffStage::FailedWithTemporaryRegion.offers_region_restore());
        assert!(HandoffStage::FailedWithTemporaryRegion.blocks_new_operation());
        assert!(!HandoffStage::FailedRegionSafe.offers_region_restore());
        assert!(!HandoffStage::FailedRegionSafe.blocks_new_operation());

        assert!(!HandoffStage::SwitchingRegion.offers_region_restore());
        assert!(HandoffStage::SwitchingRegion.blocks_new_operation());
        assert!(!HandoffStage::Restored.blocks_new_operation());
        assert!(!HandoffStage::Restored.requires_recovery());
    }

    #[test]
    fn the_history_entry_of_a_handoff_says_handed_over_and_names_only_the_file() {
        let file = HandoffFileIdentity::new("StoreInstaller.exe", "abc123").expect("identity");
        let entry = handoff_record(
            &OperationId::new("handoff-1").expect("non-empty identifier"),
            &file,
            GeoId::new(244).expect("positive GeoId"),
            JournalResult::HandedOffToInstaller,
            UtcTimestamp::parse("2026-08-21T10:00:00Z").expect("valid timestamp"),
        )
        .expect("an entry the history can hold");

        assert_eq!(entry.result(), JournalResult::HandedOffToInstaller);
        assert_eq!(entry.backend(), STORE_INSTALLER_HANDOFF_BACKEND);
        assert_eq!(entry.display_name(), "StoreInstaller.exe");
        // The outcome is deliberately not an installation, and the entry has no
        // product to be about.
        assert_ne!(entry.result(), JournalResult::Installed);
        assert!(entry.product_id().is_none());
        assert_eq!(
            entry
                .subject()
                .installer_file()
                .map(HandoffFileIdentity::sha256),
            Some("abc123")
        );
    }

    #[test]
    fn nothing_is_executed_before_the_region_switch_is_confirmed_by_read_back() {
        // A structural barrier rather than a live run: this operation writes a
        // Windows setting and starts a process, so the ordering is asserted on
        // the code that does it.
        let source = include_str!("handoff.rs");
        // Split so the needles do not match themselves.
        let call = ["launch_admitted_", "installer(&request.path)"].concat();
        let confirmation = ["RegionSwitchOutcome::RegionSwitched", " => {}"].concat();
        let launch = source.find(&call).expect("the operation launches the file");
        let confirmed = source
            .find(&confirmation)
            .expect("the operation matches on the confirmed read-back");
        assert!(
            confirmed < launch,
            "the file must not be started before read-back confirms the region"
        );
        assert_eq!(
            source.matches(&call).count(),
            1,
            "one launch site keeps the gate in one place"
        );
    }

    #[test]
    fn every_history_outcome_has_its_own_machine_facing_name() {
        let tokens = [
            result_token(JournalResult::Installed),
            result_token(JournalResult::HandedOffToInstaller),
            result_token(JournalResult::CompletionUncertain),
            result_token(JournalResult::RecoveryRequired),
            result_token(JournalResult::Failed),
        ];
        for (index, token) in tokens.iter().enumerate() {
            assert!(!tokens[index + 1..].contains(token), "{token} is reused");
        }
    }
}
