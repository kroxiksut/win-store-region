//! Startup recovery inspection and its user-facing notice.

use crate::gui::diagnostic::{diagnostic_details, diagnostic_message};
use crate::gui::state::ModalScope;
use crate::gui::strings::{Language, fill};
use crate::platform::region::{Win32RegionReader, Win32RegionWriter};
use crate::platform::storage::Win32RecoveryStore;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONWARNING, MB_OK, MB_YESNO, MessageBoxW,
};
use windows::core::HSTRING;
use winstoreregion_core::{
    APPLICATION_NAME, Diagnostic, DiagnosticCode, GeoId, PendingRecoveryAction,
    PendingRecoveryChoice, PendingRecoveryDisposition, PendingRecoveryExecution,
    PendingRecoveryExecutionError, PendingRestore, STORE_INSTALLER_HANDOFF_BACKEND,
    StartupRecoveryError, StartupRecoveryOutcome, choose_pending_recovery_action,
    execute_pending_recovery_action, inspect_startup_recovery,
};

pub(super) fn inspect_gui_startup_recovery()
-> std::result::Result<StartupRecoveryOutcome, StartupRecoveryError> {
    let mut store = Win32RecoveryStore::for_current_user().map_err(StartupRecoveryError::Store)?;
    // Housekeeping only; a failure here must not hide or block recovery.
    let _ = store.cleanup_temporary_files();
    inspect_startup_recovery(&mut store, &Win32RegionReader)
}

/// How a recovery record names what its operation was about.
///
/// A handoff record has no Product ID to show: it names the file that was
/// handed to Microsoft Store and the digest of its bytes. Calling that a
/// Product ID would be wrong, and the user could not act on it.
fn recovery_subject_line(language: Language, pending: &PendingRestore) -> String {
    if pending.backend() == STORE_INSTALLER_HANDOFF_BACKEND {
        return fill(
            language.strings().recovery_subject_installer_file,
            &[("value", pending.product_id())],
        );
    }
    fill(
        language.strings().card_product_id,
        &[("id", pending.product_id())],
    )
}

pub(super) fn recovery_startup_notice(
    language: Language,
    recovery: &std::result::Result<StartupRecoveryOutcome, StartupRecoveryError>,
) -> Option<String> {
    match recovery {
        Ok(StartupRecoveryOutcome::NoPendingRecord) => None,
        Ok(StartupRecoveryOutcome::VerifiedRecordCleaned { pending }) => Some(fill(
            language.strings().recovery_verified_record_cleaned,
            &[("id", pending.product_id())],
        )),
        Ok(StartupRecoveryOutcome::UserDecisionRequired {
            pending,
            current_region,
            disposition,
        }) => {
            let strings = language.strings();
            let situation = match disposition {
                PendingRecoveryDisposition::RestoreOriginalRegion { .. } => {
                    strings.recovery_situation_temporary_region_active
                }
                PendingRecoveryDisposition::ExternalRegionConflict { .. } => {
                    strings.recovery_situation_external_region_conflict
                }
                PendingRecoveryDisposition::OriginalRegionAlreadyActive => {
                    strings.recovery_situation_original_region_active
                }
            };
            Some(recovery_notice(
                language,
                pending,
                *current_region,
                situation,
            ))
        }
        Ok(StartupRecoveryOutcome::OperationWithoutOutcome {
            pending,
            current_region,
        }) => Some(recovery_notice(
            language,
            pending,
            *current_region,
            language.strings().recovery_situation_outcome_unknown,
        )),
        Err(error) => {
            let diagnostic = Diagnostic::from(*error);
            let blocked = language.strings().recovery_startup_blocked;
            Some(format!(
                "{} {blocked}

{}",
                diagnostic_message(language, diagnostic.code()),
                diagnostic_details(language, &diagnostic),
            ))
        }
    }
}

/// The full startup notice about one record, naming only what was written down.
fn recovery_notice(
    language: Language,
    pending: &PendingRestore,
    current_region: GeoId,
    situation: &str,
) -> String {
    let subject = recovery_subject_line(language, pending);
    fill(
        language.strings().recovery_required,
        &[
            ("situation", situation),
            ("subject", &subject),
            ("started", pending.started_at_utc().as_str()),
            ("backend", pending.backend()),
            ("original", &pending.original_region().value().to_string()),
            ("temporary", &pending.temporary_region().value().to_string()),
            ("current", &current_region.value().to_string()),
            ("state", &format!("{:?}", pending.state())),
        ],
    )
}

/// Ask the user what to do about a pending recovery record.
///
/// Only two answers exist. "Continue observing" from block 6 is deliberately
/// absent: it is meaningful only when a backend can resume an observation, and
/// no backend is connected. Offering it would promise a capability we lack.
#[allow(unsafe_code)]
pub(super) unsafe fn ask_pending_recovery(
    window: HWND,
    language: Language,
    situation: &str,
) -> PendingRecoveryChoice {
    let message = HSTRING::from(fill(
        language.strings().recovery_ask_pending_recovery,
        &[("situation", situation)],
    ));
    let title = HSTRING::from(APPLICATION_NAME);
    let _modal = ModalScope::enter();
    if unsafe { MessageBoxW(Some(window), &message, &title, MB_YESNO | MB_ICONWARNING) } == IDYES {
        PendingRecoveryChoice::RestoreOriginalRegion
    } else {
        PendingRecoveryChoice::KeepCurrentRegion
    }
}

/// Carry out the user's recovery choice through the core protocol.
///
/// Core owns every safety rule here: a fresh read-back before the write, the
/// durable `RestoringRegion` milestone, and removal of the record only after
/// the original region is confirmed. This function only supplies the adapters.
pub(super) fn execute_gui_recovery(
    pending: &PendingRestore,
    disposition: PendingRecoveryDisposition,
    choice: PendingRecoveryChoice,
) -> Result<PendingRecoveryExecution, PendingRecoveryExecutionError> {
    let action = choose_pending_recovery_action(disposition, choice)
        .map_err(|_| PendingRecoveryExecutionError::ActionDoesNotMatchPending)?;
    let mut store =
        Win32RecoveryStore::for_current_user().map_err(PendingRecoveryExecutionError::Store)?;
    execute_pending_recovery_action(
        action,
        pending,
        &mut store,
        &Win32RegionReader,
        &Win32RegionWriter,
    )
}

/// Remove a record whose operation is over and whose region is confirmed back.
///
/// No user choice exists here: the region already matches the recorded original
/// one, so there is nothing to offer and nothing to write. Core still re-reads
/// the region and refuses to clear a record that does not match it.
pub(super) fn clear_verified_recovery_record(
    pending: &PendingRestore,
) -> Result<PendingRecoveryExecution, PendingRecoveryExecutionError> {
    let mut store =
        Win32RecoveryStore::for_current_user().map_err(PendingRecoveryExecutionError::Store)?;
    execute_pending_recovery_action(
        PendingRecoveryAction::ClearVerifiedRecord,
        pending,
        &mut store,
        &Win32RegionReader,
        &Win32RegionWriter,
    )
}

/// Report the outcome of a recovery action without overstating it.
pub(super) fn recovery_execution_status(
    language: Language,
    result: &Result<PendingRecoveryExecution, PendingRecoveryExecutionError>,
) -> String {
    let strings = language.strings();
    match result {
        Ok(PendingRecoveryExecution::Restored { .. }) => strings.recovery_restored.to_owned(),
        Ok(PendingRecoveryExecution::VerifiedRecordCleared) => {
            strings.recovery_verified_record_cleared.to_owned()
        }
        Ok(PendingRecoveryExecution::PendingRecordRetained) => {
            strings.recovery_pending_record_retained.to_owned()
        }
        Ok(PendingRecoveryExecution::RestoreConflict { current_region }) => fill(
            strings.recovery_restore_conflict,
            &[("geo_id", &current_region.value().to_string())],
        ),
        Err(_) => {
            let unchanged = strings.recovery_recovery_execution_status;
            format!(
                "{} {unchanged}",
                diagnostic_message(language, DiagnosticCode::RecoveryRecordUnreadable)
            )
        }
    }
}

#[allow(unsafe_code)]
pub(super) unsafe fn show_recovery_notice(message: &str) {
    let title = HSTRING::from(APPLICATION_NAME);
    let message = HSTRING::from(message);
    let _modal = ModalScope::enter();
    let _ = unsafe { MessageBoxW(None, &message, &title, MB_OK | MB_ICONWARNING) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::strings::Language;
    use winstoreregion_core::{
        DurableOperationState, GeoId, OperationId, PendingRecoveryDisposition, PendingRestore,
        StartupRecoveryOutcome, UtcTimestamp,
    };

    #[test]
    fn every_recovery_outcome_is_reported_without_overstating_it() {
        let restored = recovery_execution_status(
            Language::Russian,
            &Ok(PendingRecoveryExecution::Restored {
                writer_result: Ok(()),
            }),
        );
        assert!(restored.contains("подтверждён"));

        let retained = recovery_execution_status(
            Language::Russian,
            &Ok(PendingRecoveryExecution::PendingRecordRetained),
        );
        assert!(
            retained.contains("заблокирована"),
            "keeping the region must say the gate stays closed"
        );

        let conflict = recovery_execution_status(
            Language::English,
            &Ok(PendingRecoveryExecution::RestoreConflict {
                current_region: GeoId::new(276).expect("positive GeoId"),
            }),
        );
        assert!(conflict.contains("276"));
        assert!(
            conflict.contains("Nothing was changed"),
            "a conflict must state that nothing happened"
        );

        let failed = recovery_execution_status(
            Language::English,
            &Err(PendingRecoveryExecutionError::Region(
                winstoreregion_core::RegionReadError::GeoIdNotAvailable,
            )),
        );
        assert!(failed.contains("preserved"));
        assert!(
            !failed.contains("RegionReadError"),
            "a failure must not leak the internal variant into user text"
        );
    }

    #[test]
    fn recovery_notice_shows_the_record_and_blocks_a_new_installation() {
        let pending = PendingRestore::prepared(
            OperationId::new("recovery-notice").expect("non-empty operation identifier"),
            UtcTimestamp::parse("2026-08-16T12:34:56Z").expect("valid UTC timestamp"),
            GeoId::new(203).expect("positive original GeoId"),
            GeoId::new(244).expect("positive temporary GeoId"),
            "9WZDNCRFJ3PZ",
            "test-backend",
        )
        .expect("complete pending record")
        .with_state(DurableOperationState::CompletionUncertain);
        let notice = recovery_startup_notice(
            Language::Russian,
            &Ok(StartupRecoveryOutcome::UserDecisionRequired {
                pending,
                current_region: GeoId::new(244).expect("positive current GeoId"),
                disposition: PendingRecoveryDisposition::RestoreOriginalRegion {
                    original_region: GeoId::new(203).expect("positive original GeoId"),
                    temporary_region: GeoId::new(244).expect("positive temporary GeoId"),
                },
            }),
        )
        .expect("pending recovery must show a blocking notice");

        assert!(notice.contains("Product ID: 9WZDNCRFJ3PZ"));
        // The record's own numbers have to be on screen: a notice naming no
        // region leaves the user nothing to check Windows against. The labels
        // beside them are translated text and are deliberately not pinned here.
        assert!(notice.contains("203"), "the original region is not named");
        assert!(notice.contains("244"), "the temporary region is not named");
        // The closing sentence is what says a new installation cannot start.
        let blocking = Language::Russian
            .strings()
            .recovery_required
            .lines()
            .next_back()
            .expect("the notice template ends with the blocking sentence");
        assert!(notice.contains(blocking));
    }

    #[test]
    fn a_record_left_without_an_outcome_says_so_instead_of_naming_a_region_problem() {
        let pending = PendingRestore::prepared(
            OperationId::new("outcome-unknown").expect("non-empty operation identifier"),
            UtcTimestamp::parse("2026-08-23T20:15:00Z").expect("valid UTC timestamp"),
            GeoId::new(203).expect("positive original GeoId"),
            GeoId::new(244).expect("positive temporary GeoId"),
            "9WZDNCRFJ3PZ",
            "test-backend",
        )
        .expect("complete pending record")
        .with_state(DurableOperationState::RegionRestoredEarly);
        let notice = recovery_startup_notice(
            Language::Russian,
            &Ok(StartupRecoveryOutcome::OperationWithoutOutcome {
                pending,
                current_region: GeoId::new(203).expect("positive current GeoId"),
            }),
        )
        .expect("a record without an outcome must show a blocking notice");

        assert!(
            notice.contains(
                Language::Russian
                    .strings()
                    .recovery_situation_outcome_unknown
            )
        );
        // The region is back, so nothing may hint that it is the problem.
        assert!(
            !notice.contains(
                Language::Russian
                    .strings()
                    .recovery_situation_temporary_region_active
            ),
            "the temporary region is not what is wrong here"
        );
        assert!(notice.contains("Product ID: 9WZDNCRFJ3PZ"));
    }

    #[test]
    fn a_handoff_record_is_named_by_its_file_and_never_as_a_product_id() {
        let pending = PendingRestore::prepared(
            OperationId::new("handoff-notice").expect("non-empty operation identifier"),
            UtcTimestamp::parse("2026-08-21T09:00:00Z").expect("valid UTC timestamp"),
            GeoId::new(203).expect("positive original GeoId"),
            GeoId::new(244).expect("positive temporary GeoId"),
            "StoreInstaller.exe sha256:abc123",
            STORE_INSTALLER_HANDOFF_BACKEND,
        )
        .expect("complete pending record");
        let notice = recovery_startup_notice(
            Language::Russian,
            &Ok(StartupRecoveryOutcome::UserDecisionRequired {
                pending,
                current_region: GeoId::new(244).expect("positive current GeoId"),
                disposition: PendingRecoveryDisposition::RestoreOriginalRegion {
                    original_region: GeoId::new(203).expect("positive original GeoId"),
                    temporary_region: GeoId::new(244).expect("positive temporary GeoId"),
                },
            }),
        )
        .expect("pending recovery must show a blocking notice");

        assert!(notice.contains("Файл установщика: StoreInstaller.exe sha256:abc123"));
        assert!(
            !notice.contains("Product ID"),
            "a handoff record has no product identifier and must not claim one"
        );
    }
}
