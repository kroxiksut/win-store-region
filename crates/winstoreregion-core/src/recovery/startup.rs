//! Conflict-safe classification and execution of recovery.

use crate::recovery::record::{DurableOperationState, PendingRestore};
use crate::recovery::store::{RecoveryStore, RecoveryStoreError};
use crate::region::{GeoId, RegionReadError, RegionReader, RegionWriteError, RegionWriter};

/// Safe next step selected from a validated pending record and a fresh read-back.
///
/// This classification never changes Windows Home Location. It gives the caller
/// enough information to either offer restoration or block automatic action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingRecoveryDisposition {
    /// The original region is already active and only verified record cleanup remains.
    OriginalRegionAlreadyActive,
    /// The temporary region is active, so restoring the recorded original is safe to offer.
    RestoreOriginalRegion {
        /// Recorded original region to use as the writer target.
        original_region: GeoId,
        /// Current region observed during the recovery read-back.
        temporary_region: GeoId,
    },
    /// A region outside this operation is active; the user must choose what to do.
    ExternalRegionConflict {
        original_region: GeoId,
        temporary_region: GeoId,
        current_region: GeoId,
    },
}

/// Explicit user choice offered by recovery when the active region is not original.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingRecoveryChoice {
    /// Keep the current Windows region and preserve the recovery record.
    KeepCurrentRegion,
    /// Request restoration of the recorded original region.
    RestoreOriginalRegion,
}

/// Next recovery action that the platform/UI layer is allowed to perform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingRecoveryAction {
    /// The original region is active; a verified record cleanup may proceed.
    ClearVerifiedRecord,
    /// Leave Windows unchanged and retain the record for a later explicit decision.
    KeepPendingRecord,
    /// Re-read the current region before writing the recorded original region.
    ///
    /// The writer must not run unless the fresh value still equals
    /// `expected_current_region`.
    RestoreOriginalAfterReadBack {
        expected_current_region: GeoId,
        original_region: GeoId,
    },
}

/// A recovery action was requested for a state that does not permit it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingRecoveryChoiceError {
    /// The original region is already active and needs no user choice.
    OriginalRegionAlreadyActive,
}

/// Result of inspecting pending recovery at application startup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartupRecoveryOutcome {
    /// No published recovery record exists, so a new operation is not blocked by recovery.
    NoPendingRecord,
    /// The original region was already active and the validated record was removed.
    VerifiedRecordCleaned { pending: PendingRestore },
    /// A valid recovery record requires an explicit user decision before a new operation.
    UserDecisionRequired {
        pending: PendingRestore,
        current_region: GeoId,
        disposition: PendingRecoveryDisposition,
    },
    /// The original region is active, but the operation that recorded it has no
    /// outcome yet.
    ///
    /// The region alone cannot tell these apart from a finished operation, and
    /// reading them as one was how a restart lost a running installation: the
    /// record was cleaned, nothing reconnected, and no history entry was ever
    /// written. The caller must first try to take the operation over, and only
    /// then decide what its record deserves.
    OperationWithoutOutcome {
        pending: PendingRestore,
        current_region: GeoId,
    },
}

/// A storage or Windows-read failure while inspecting startup recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupRecoveryError {
    /// The recovery record could not be safely read, validated, or cleaned.
    Store(RecoveryStoreError),
    /// Windows did not provide the current safety identifier.
    Region(RegionReadError),
}

/// Whether startup recovery leaves a new guarded operation admissible.
///
/// Only two outcomes admit one: no published record, and a record that was
/// removed after read-back confirmed the original region. Every other outcome,
/// including a failure to read recovery at all, keeps a new operation blocked.
/// The caller must not weaken this into "unknown means allowed".
#[must_use]
pub const fn startup_recovery_admits_new_operation(
    recovery: &Result<StartupRecoveryOutcome, StartupRecoveryError>,
) -> bool {
    matches!(
        recovery,
        Ok(StartupRecoveryOutcome::NoPendingRecord
            | StartupRecoveryOutcome::VerifiedRecordCleaned { .. })
    )
}

/// Classify a validated pending record using a newly read current `GeoId`.
///
/// The original region takes precedence if both saved values are equal: there
/// is then nothing to restore, and only verified cleanup is permitted.
#[must_use]
pub fn classify_pending_recovery(
    pending: &PendingRestore,
    current_region: GeoId,
) -> PendingRecoveryDisposition {
    if current_region == pending.original_region() {
        PendingRecoveryDisposition::OriginalRegionAlreadyActive
    } else if current_region == pending.temporary_region() {
        PendingRecoveryDisposition::RestoreOriginalRegion {
            original_region: pending.original_region(),
            temporary_region: pending.temporary_region(),
        }
    } else {
        PendingRecoveryDisposition::ExternalRegionConflict {
            original_region: pending.original_region(),
            temporary_region: pending.temporary_region(),
            current_region,
        }
    }
}

/// Translate an explicit recovery choice into an action without changing Windows.
///
/// # Errors
///
/// Returns [`PendingRecoveryChoiceError::OriginalRegionAlreadyActive`] when a
/// user choice is requested even though cleanup is the only permitted action.
pub fn choose_pending_recovery_action(
    disposition: PendingRecoveryDisposition,
    choice: PendingRecoveryChoice,
) -> Result<PendingRecoveryAction, PendingRecoveryChoiceError> {
    match disposition {
        PendingRecoveryDisposition::OriginalRegionAlreadyActive => {
            Err(PendingRecoveryChoiceError::OriginalRegionAlreadyActive)
        }
        PendingRecoveryDisposition::RestoreOriginalRegion {
            original_region,
            temporary_region,
        } => match choice {
            PendingRecoveryChoice::KeepCurrentRegion => {
                Ok(PendingRecoveryAction::KeepPendingRecord)
            }
            PendingRecoveryChoice::RestoreOriginalRegion => {
                Ok(PendingRecoveryAction::RestoreOriginalAfterReadBack {
                    expected_current_region: temporary_region,
                    original_region,
                })
            }
        },
        PendingRecoveryDisposition::ExternalRegionConflict {
            original_region,
            current_region,
            ..
        } => match choice {
            PendingRecoveryChoice::KeepCurrentRegion => {
                Ok(PendingRecoveryAction::KeepPendingRecord)
            }
            PendingRecoveryChoice::RestoreOriginalRegion => {
                Ok(PendingRecoveryAction::RestoreOriginalAfterReadBack {
                    expected_current_region: current_region,
                    original_region,
                })
            }
        },
    }
}

/// Outcome of applying one explicit pending-recovery action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PendingRecoveryExecution {
    /// The validated record was removed because read-back found the original region.
    VerifiedRecordCleared,
    /// The user chose to leave Windows unchanged and retain recovery state.
    PendingRecordRetained,
    /// A fresh read-back no longer matched the region that the explicit action
    /// authorised for restoration, so no writer call was made.
    RestoreConflict { current_region: GeoId },
    /// Read-back confirmed the original region after a writer attempt and the
    /// verified record was removed.
    Restored {
        writer_result: Result<(), RegionWriteError>,
    },
}

/// Failure while carrying out a conflict-safe explicit recovery action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingRecoveryExecutionError {
    /// Windows did not provide a mandatory pre- or post-write read-back.
    Region(RegionReadError),
    /// The durable pending record could not be updated or cleared safely.
    Store(RecoveryStoreError),
    /// The requested restoration target does not belong to this pending record.
    ActionDoesNotMatchPending,
}

/// Execute an already-authorised recovery action under the recovery invariant.
///
/// The writer is called only after a fresh read-back matches the action's
/// expected region. Before the write, `RestoringRegion` is made durable. The
/// record stays pending for every conflict or error and is removed only after a
/// post-write read-back confirms the recorded original region.
///
/// # Errors
///
/// Returns an error without clearing the pending record when a mandatory read
/// or durable store operation fails.
pub fn execute_pending_recovery_action<S, R, W>(
    action: PendingRecoveryAction,
    pending: &PendingRestore,
    store: &mut S,
    reader: &R,
    writer: &W,
) -> Result<PendingRecoveryExecution, PendingRecoveryExecutionError>
where
    S: RecoveryStore,
    R: RegionReader,
    W: RegionWriter,
{
    match action {
        PendingRecoveryAction::KeepPendingRecord => {
            Ok(PendingRecoveryExecution::PendingRecordRetained)
        }
        PendingRecoveryAction::ClearVerifiedRecord => {
            let current_region = reader
                .current_region()
                .map(|region| region.geo_id)
                .map_err(PendingRecoveryExecutionError::Region)?;
            if current_region != pending.original_region() {
                return Ok(PendingRecoveryExecution::RestoreConflict { current_region });
            }
            store
                .clear_verified(pending)
                .map_err(PendingRecoveryExecutionError::Store)?;
            Ok(PendingRecoveryExecution::VerifiedRecordCleared)
        }
        PendingRecoveryAction::RestoreOriginalAfterReadBack {
            expected_current_region,
            original_region,
        } => {
            if original_region != pending.original_region() {
                return Err(PendingRecoveryExecutionError::ActionDoesNotMatchPending);
            }
            let current_region = reader
                .current_region()
                .map(|region| region.geo_id)
                .map_err(PendingRecoveryExecutionError::Region)?;
            if current_region != expected_current_region {
                return Ok(PendingRecoveryExecution::RestoreConflict { current_region });
            }
            let restoring = pending.with_state(DurableOperationState::RestoringRegion);
            store
                .publish(&restoring)
                .map_err(PendingRecoveryExecutionError::Store)?;
            let writer_result = writer.set_region(original_region);
            let current_region = reader
                .current_region()
                .map(|region| region.geo_id)
                .map_err(PendingRecoveryExecutionError::Region)?;
            if current_region != original_region {
                return Ok(PendingRecoveryExecution::RestoreConflict { current_region });
            }
            let restored = restoring.with_state(DurableOperationState::Restored);
            store
                .publish(&restored)
                .map_err(PendingRecoveryExecutionError::Store)?;
            store
                .clear_verified(&restored)
                .map_err(PendingRecoveryExecutionError::Store)?;
            Ok(PendingRecoveryExecution::Restored { writer_result })
        }
    }
}

/// Inspect recovery before admitting a new region-changing operation.
///
/// A valid record is removed only after a fresh read-back confirms the original
/// region *and* its own milestone says the operation is over. Any other current
/// region preserves the record and requires an explicit user decision.
///
/// # Errors
///
/// Returns a structured failure without changing the Windows region. Callers
/// must block a new operation until that failure is shown in recovery UI.
pub fn inspect_startup_recovery<S, R>(
    store: &mut S,
    reader: &R,
) -> Result<StartupRecoveryOutcome, StartupRecoveryError>
where
    S: RecoveryStore,
    R: RegionReader,
{
    let Some(pending) = store.load().map_err(StartupRecoveryError::Store)? else {
        return Ok(StartupRecoveryOutcome::NoPendingRecord);
    };
    let current_region = reader
        .current_region()
        .map(|region| region.geo_id)
        .map_err(StartupRecoveryError::Region)?;
    let disposition = classify_pending_recovery(&pending, current_region);
    if disposition == PendingRecoveryDisposition::OriginalRegionAlreadyActive {
        // The region is the only thing the classification looks at, and it is
        // not enough: a record whose operation is still running keeps the safe
        // region and its duty to report an outcome at the same time.
        if pending.state().outcome_is_still_open() {
            return Ok(StartupRecoveryOutcome::OperationWithoutOutcome {
                pending,
                current_region,
            });
        }
        store
            .clear_verified(&pending)
            .map_err(StartupRecoveryError::Store)?;
        return Ok(StartupRecoveryOutcome::VerifiedRecordCleaned { pending });
    }

    Ok(StartupRecoveryOutcome::UserDecisionRequired {
        pending,
        current_region,
        disposition,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::RecoveryStore;
    use crate::recovery::record::DurableOperationState;
    use crate::region::RegionWriteError;
    use crate::test_support::{
        MemoryRecoveryStore, SequenceReader, TestWriter, geo_id, pending_restore_for_test,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn pending_recovery_classification_never_overwrites_an_external_region() {
        let pending = pending_restore_for_test();

        assert_eq!(
            classify_pending_recovery(&pending, pending.original_region()),
            PendingRecoveryDisposition::OriginalRegionAlreadyActive
        );
        assert_eq!(
            classify_pending_recovery(&pending, pending.temporary_region()),
            PendingRecoveryDisposition::RestoreOriginalRegion {
                original_region: pending.original_region(),
                temporary_region: pending.temporary_region(),
            }
        );
        assert_eq!(
            classify_pending_recovery(&pending, geo_id(276)),
            PendingRecoveryDisposition::ExternalRegionConflict {
                original_region: pending.original_region(),
                temporary_region: pending.temporary_region(),
                current_region: geo_id(276),
            }
        );
    }

    #[test]
    fn recovery_choice_keeps_the_record_or_requires_a_fresh_read_back() {
        let pending = pending_restore_for_test();
        let temporary = classify_pending_recovery(&pending, pending.temporary_region());
        let conflict = classify_pending_recovery(&pending, geo_id(276));

        assert_eq!(
            choose_pending_recovery_action(temporary, PendingRecoveryChoice::KeepCurrentRegion),
            Ok(PendingRecoveryAction::KeepPendingRecord)
        );
        assert_eq!(
            choose_pending_recovery_action(temporary, PendingRecoveryChoice::RestoreOriginalRegion),
            Ok(PendingRecoveryAction::RestoreOriginalAfterReadBack {
                expected_current_region: pending.temporary_region(),
                original_region: pending.original_region(),
            })
        );
        assert_eq!(
            choose_pending_recovery_action(conflict, PendingRecoveryChoice::RestoreOriginalRegion),
            Ok(PendingRecoveryAction::RestoreOriginalAfterReadBack {
                expected_current_region: geo_id(276),
                original_region: pending.original_region(),
            })
        );
        assert_eq!(
            choose_pending_recovery_action(
                PendingRecoveryDisposition::OriginalRegionAlreadyActive,
                PendingRecoveryChoice::KeepCurrentRegion,
            ),
            Err(PendingRecoveryChoiceError::OriginalRegionAlreadyActive)
        );
    }

    #[test]
    fn startup_recovery_cleans_only_an_already_restored_record() {
        let pending = pending_restore_for_test();
        let mut store = MemoryRecoveryStore {
            pending: Some(pending.clone()),
        };
        let reader = SequenceReader::from_geo_ids([pending.original_region()]);

        assert_eq!(
            inspect_startup_recovery(&mut store, &reader),
            Ok(StartupRecoveryOutcome::VerifiedRecordCleaned {
                pending: pending.clone(),
            })
        );
        assert_eq!(store.load(), Ok(None));

        let mut store = MemoryRecoveryStore {
            pending: Some(pending.clone()),
        };
        let reader = SequenceReader::from_geo_ids([pending.temporary_region()]);
        assert_eq!(
            inspect_startup_recovery(&mut store, &reader),
            Ok(StartupRecoveryOutcome::UserDecisionRequired {
                pending: pending.clone(),
                current_region: pending.temporary_region(),
                disposition: PendingRecoveryDisposition::RestoreOriginalRegion {
                    original_region: pending.original_region(),
                    temporary_region: pending.temporary_region(),
                },
            })
        );
        assert_eq!(store.load(), Ok(Some(pending.clone())));

        let current_region = geo_id(276);
        let mut store = MemoryRecoveryStore {
            pending: Some(pending.clone()),
        };
        let reader = SequenceReader::from_geo_ids([current_region]);
        assert_eq!(
            inspect_startup_recovery(&mut store, &reader),
            Ok(StartupRecoveryOutcome::UserDecisionRequired {
                pending: pending.clone(),
                current_region,
                disposition: PendingRecoveryDisposition::ExternalRegionConflict {
                    original_region: pending.original_region(),
                    temporary_region: pending.temporary_region(),
                    current_region,
                },
            })
        );
        assert_eq!(store.load(), Ok(Some(pending)));
    }

    #[test]
    fn a_running_installation_survives_a_restart_that_finds_the_safe_region() {
        // The region was put back early, so it matches the original one while
        // the installation is still going. Clearing the record here is what
        // used to lose the operation: nothing reconnected to it afterwards.
        let pending =
            pending_restore_for_test().with_state(DurableOperationState::RegionRestoredEarly);
        let mut store = MemoryRecoveryStore {
            pending: Some(pending.clone()),
        };
        let reader = SequenceReader::from_geo_ids([pending.original_region()]);

        let outcome = inspect_startup_recovery(&mut store, &reader);

        assert_eq!(
            outcome,
            Ok(StartupRecoveryOutcome::OperationWithoutOutcome {
                pending: pending.clone(),
                current_region: pending.original_region(),
            })
        );
        assert_eq!(store.load(), Ok(Some(pending)));
        assert!(
            !startup_recovery_admits_new_operation(&outcome),
            "an operation without an outcome must keep the next one blocked"
        );
    }

    #[test]
    fn explicit_recovery_rechecks_before_restore_and_keeps_pending_until_verified() {
        let pending =
            pending_restore_for_test().with_state(DurableOperationState::CompletionUncertain);
        let action = PendingRecoveryAction::RestoreOriginalAfterReadBack {
            expected_current_region: pending.temporary_region(),
            original_region: pending.original_region(),
        };
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut store = MemoryRecoveryStore {
            pending: Some(pending.clone()),
        };
        let result = execute_pending_recovery_action(
            action,
            &pending,
            &mut store,
            &SequenceReader::from_geo_ids([pending.temporary_region(), pending.original_region()]),
            &TestWriter {
                result: Ok(()),
                events: Rc::clone(&events),
            },
        )
        .expect("matching read-backs allow verified recovery");
        assert_eq!(
            result,
            PendingRecoveryExecution::Restored {
                writer_result: Ok(())
            }
        );
        assert_eq!(&*events.borrow(), &["write"]);
        assert_eq!(
            store.pending, None,
            "only verified restoration clears pending"
        );

        let mut store = MemoryRecoveryStore {
            pending: Some(pending.clone()),
        };
        let events = Rc::new(RefCell::new(Vec::new()));
        let conflict = execute_pending_recovery_action(
            action,
            &pending,
            &mut store,
            &SequenceReader::from_geo_ids([geo_id(276)]),
            &TestWriter {
                result: Ok(()),
                events: Rc::clone(&events),
            },
        )
        .expect("changed region is a conflict, not a write");
        assert_eq!(
            conflict,
            PendingRecoveryExecution::RestoreConflict {
                current_region: geo_id(276)
            }
        );
        assert!(events.borrow().is_empty());
        assert_eq!(store.pending, Some(pending.clone()));

        let mut store = MemoryRecoveryStore {
            pending: Some(pending.clone()),
        };
        let after_write_conflict = execute_pending_recovery_action(
            action,
            &pending,
            &mut store,
            &SequenceReader::from_geo_ids([pending.temporary_region(), geo_id(276)]),
            &TestWriter {
                result: Err(RegionWriteError::Rejected),
                events: Rc::new(RefCell::new(Vec::new())),
            },
        )
        .expect("post-write read-back classifies conflict without cleanup");
        assert_eq!(
            after_write_conflict,
            PendingRecoveryExecution::RestoreConflict {
                current_region: geo_id(276)
            }
        );
        assert_eq!(
            store.pending,
            Some(pending.with_state(DurableOperationState::RestoringRegion)),
            "a failed or conflicting restore remains recoverable"
        );

        let mismatched_action = PendingRecoveryAction::RestoreOriginalAfterReadBack {
            expected_current_region: pending.temporary_region(),
            original_region: geo_id(840),
        };
        assert_eq!(
            execute_pending_recovery_action(
                mismatched_action,
                &pending,
                &mut store,
                &SequenceReader::from_geo_ids([]),
                &TestWriter {
                    result: Ok(()),
                    events: Rc::new(RefCell::new(Vec::new())),
                },
            ),
            Err(PendingRecoveryExecutionError::ActionDoesNotMatchPending)
        );
    }

    #[test]
    fn only_a_resolved_recovery_admits_a_new_operation() {
        let pending = pending_restore_for_test();
        assert!(startup_recovery_admits_new_operation(&Ok(
            StartupRecoveryOutcome::NoPendingRecord
        )));
        assert!(startup_recovery_admits_new_operation(&Ok(
            StartupRecoveryOutcome::VerifiedRecordCleaned {
                pending: pending.clone(),
            }
        )));

        for disposition in [
            PendingRecoveryDisposition::RestoreOriginalRegion {
                original_region: pending.original_region(),
                temporary_region: pending.temporary_region(),
            },
            PendingRecoveryDisposition::ExternalRegionConflict {
                original_region: pending.original_region(),
                temporary_region: pending.temporary_region(),
                current_region: geo_id(276),
            },
        ] {
            assert!(!startup_recovery_admits_new_operation(&Ok(
                StartupRecoveryOutcome::UserDecisionRequired {
                    pending: pending.clone(),
                    current_region: geo_id(276),
                    disposition,
                }
            )));
        }

        // An unreadable record must block, never fall through to "allowed".
        assert!(!startup_recovery_admits_new_operation(&Err(
            StartupRecoveryError::Store(RecoveryStoreError::Verification)
        )));
        assert!(!startup_recovery_admits_new_operation(&Err(
            StartupRecoveryError::Region(RegionReadError::GeoIdNotAvailable)
        )));
    }
}
