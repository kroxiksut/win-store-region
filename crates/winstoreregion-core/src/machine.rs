//! Core state machine for one guarded installation operation.

use crate::install::InstallObservation;
use crate::observe::timeout::TimeoutResolution;
use crate::recovery::record::DurableOperationState;
use crate::region::RegionSwitchOutcome;

/// Durable-state sink used by the operation state machine.
///
/// Only the recovery adapter owns the actual file. The state machine asks it to
/// persist a selected milestone before exposing that milestone to callers.
pub trait OperationStatePersistence {
    /// Failure returned by the underlying durable storage adapter.
    type Error;

    /// Publish the supplied recovery milestone.
    ///
    /// # Errors
    ///
    /// Returns the underlying persistence failure without changing operation
    /// state.
    fn persist_state(&mut self, state: DurableOperationState) -> Result<(), Self::Error>;
}

/// States of one guarded Store-installation operation.
///
/// PDP page opening is intentionally absent: it is a non-install fallback
/// action represented by [`StoreInstallLifecycleEvent::StorePageOpened`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InstallationOperationState {
    /// No guarded operation is active.
    #[default]
    Idle,
    /// Exact product metadata is being resolved.
    ResolvingProduct,
    /// Product identity and market were confirmed.
    ProductResolved,
    /// Core is preparing the guarded operation.
    Preparing,
    /// The complete recovery record is about to be published.
    SavingRecoveryState,
    /// Windows Home Location is being switched and read back.
    SwitchingRegion,
    /// Read-back confirmed the selected temporary `GeoId`.
    RegionSwitched,
    /// Backend context is being refreshed under the confirmed temporary region.
    RefreshingStoreContext,
    /// Core is about to ask the selected backend to install.
    StartingInstall,
    /// The backend accepted the exact install request.
    InstallRequested,
    /// The selected observer began reporting evidence for that request.
    InstallObserved,
    /// Installation is still active according to the observer.
    Installing,
    /// The original region is back while the installation continues.
    ///
    /// Recovery is still required: the region is safe, but the operation has no
    /// outcome yet, so its record must survive until one exists.
    RegionRestoredEarly,
    /// Completion or cancellation cannot be proven automatically.
    CompletionUncertain,
    /// Completion evidence was confirmed.
    Installed,
    /// Original region restoration is being attempted and will be read back.
    RestoringRegion,
    /// Read-back confirmed the original region; durable cleanup may now finish.
    Restored,
    /// Recovery record cleanup completed and the operation is terminal.
    Completed,
    /// The operation failed before a temporary region was confirmed.
    FailedBeforeRegionChange,
    /// The operation failed while recovery remains required.
    FailedWithTemporaryRegion,
    /// The current region conflicts with the guarded operation.
    RestoreConflict,
    /// Restoration did not confirm the original region.
    RestoreFailed,
    /// The user cancelled before a temporary region was confirmed.
    Cancelled,
}

impl InstallationOperationState {
    /// Whether recovery state must remain available for this state.
    ///
    /// `SwitchingRegion` is in the list because the record is published before
    /// the writer runs, not after it: from the moment this state is entered a
    /// region write may already have landed, and a failure here must not be
    /// allowed to declare "nothing was changed".
    #[must_use]
    pub const fn requires_recovery(self) -> bool {
        matches!(
            self,
            Self::SwitchingRegion
                | Self::RegionSwitched
                | Self::RefreshingStoreContext
                | Self::StartingInstall
                | Self::InstallRequested
                | Self::InstallObserved
                | Self::Installing
                | Self::RegionRestoredEarly
                | Self::CompletionUncertain
                | Self::Installed
                | Self::RestoringRegion
                | Self::Restored
                | Self::FailedWithTemporaryRegion
                | Self::RestoreConflict
                | Self::RestoreFailed
        )
    }

    /// Whether this state admits an exact backend start request.
    #[must_use]
    pub const fn allows_install_start(self) -> bool {
        matches!(self, Self::RefreshingStoreContext)
    }

    /// Whether the operation already reached a final result.
    ///
    /// A terminal state accepts no further event. `requires_recovery` alone
    /// cannot express this: a completed or cancelled operation needs no
    /// recovery either, yet it must not be failed or cancelled a second time.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::FailedBeforeRegionChange | Self::Cancelled
        )
    }
}

/// Facts and commands accepted by [`InstallationOperationMachine`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallationOperationEvent {
    BeginResolvingProduct,
    ProductResolved,
    BeginPreparing,
    BeginSavingRecoveryState,
    /// The recovery record became durable, which is what admits the region writer.
    RecoveryStateSaved,
    RegionSwitchFinished(RegionSwitchOutcome),
    BeginRefreshingStoreContext,
    BeginInstall,
    InstallRequestAccepted,
    ObservationStarted,
    Observation(InstallObservation),
    /// The configured observer deadline elapsed; this never restores a region.
    ObservationTimedOut,
    /// Read-back confirmed the original region while the install kept running.
    EarlyRegionRestoreConfirmed,
    CompletionBecameUncertain,
    /// Apply a user-validated resolution after timeout uncertainty.
    TimeoutResolved(TimeoutResolution),
    BeginRestoringRegion,
    OriginalRegionRestored,
    Complete,
    FailBeforeRegionChange,
    FailWithTemporaryRegion,
    RestoreConflict,
    RestoreFailed,
    Cancel,
}

/// A rejected operation transition or durable persistence failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstallationOperationError<PersistenceError> {
    /// The event is not legal from the current state.
    InvalidTransition {
        from: InstallationOperationState,
        event: InstallationOperationEvent,
    },
    /// The required recovery milestone was not made durable.
    Persistence(PersistenceError),
}

/// Core-owned state machine for one guarded Store-installation operation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InstallationOperationMachine {
    state: InstallationOperationState,
}

impl InstallationOperationMachine {
    /// Construct an idle operation state machine.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: InstallationOperationState::Idle,
        }
    }

    /// Return the current operation state for presentation or orchestration.
    #[must_use]
    pub const fn state(&self) -> InstallationOperationState {
        self.state
    }

    /// Check whether an event is legal without mutating state or persistence.
    #[must_use]
    pub const fn can_apply(&self, event: InstallationOperationEvent) -> bool {
        self.next_state(event).is_some()
    }

    /// Apply one event, persisting its recovery milestone before changing state.
    ///
    /// # Errors
    ///
    /// Returns [`InstallationOperationError::InvalidTransition`] without
    /// calling persistence for an illegal event. Returns
    /// [`InstallationOperationError::Persistence`] and preserves the prior
    /// state when a required durable write fails.
    pub fn apply<P>(
        &mut self,
        event: InstallationOperationEvent,
        persistence: &mut P,
    ) -> Result<InstallationOperationState, InstallationOperationError<P::Error>>
    where
        P: OperationStatePersistence,
    {
        let next = self
            .next_state(event)
            .ok_or(InstallationOperationError::InvalidTransition {
                from: self.state,
                event,
            })?;
        if let Some(durable_state) = durable_state_for_event(event) {
            persistence
                .persist_state(durable_state)
                .map_err(InstallationOperationError::Persistence)?;
        }
        self.state = next;
        Ok(next)
    }

    const fn next_state(
        self,
        event: InstallationOperationEvent,
    ) -> Option<InstallationOperationState> {
        use InstallationOperationEvent as Event;
        use InstallationOperationState as State;

        match (self.state, event) {
            (State::Idle, Event::BeginResolvingProduct) => Some(State::ResolvingProduct),
            (State::ResolvingProduct, Event::ProductResolved) => Some(State::ProductResolved),
            (State::ProductResolved, Event::BeginPreparing) => Some(State::Preparing),
            (State::Preparing, Event::BeginSavingRecoveryState) => Some(State::SavingRecoveryState),
            (State::SavingRecoveryState, Event::RecoveryStateSaved) => Some(State::SwitchingRegion),
            (
                State::SwitchingRegion,
                Event::RegionSwitchFinished(RegionSwitchOutcome::RegionSwitched),
            ) => Some(State::RegionSwitched),
            (
                State::SwitchingRegion,
                Event::RegionSwitchFinished(RegionSwitchOutcome::OriginalRegionPreserved),
            ) => Some(State::FailedBeforeRegionChange),
            (
                State::SwitchingRegion,
                Event::RegionSwitchFinished(RegionSwitchOutcome::RegionConflict { .. }),
            ) => Some(State::RestoreConflict),
            (State::RegionSwitched, Event::BeginRefreshingStoreContext) => {
                Some(State::RefreshingStoreContext)
            }
            (State::RefreshingStoreContext, Event::BeginInstall) => Some(State::StartingInstall),
            (State::StartingInstall, Event::InstallRequestAccepted) => {
                Some(State::InstallRequested)
            }
            (State::InstallRequested, Event::ObservationStarted)
            | (
                State::CompletionUncertain,
                Event::TimeoutResolved(TimeoutResolution::ResumeObserver),
            ) => Some(State::InstallObserved),
            (State::InstallObserved, Event::Observation(InstallObservation::Installing { .. }))
            | (State::Installing, Event::Observation(InstallObservation::Installing { .. })) => {
                Some(State::Installing)
            }
            (State::Installing, Event::EarlyRegionRestoreConfirmed) => {
                Some(State::RegionRestoredEarly)
            }
            (
                State::RegionRestoredEarly,
                Event::Observation(InstallObservation::Installing { .. }),
            ) => Some(State::RegionRestoredEarly),
            // The region is already original and was read back, so completion
            // here lands directly on the restored milestone instead of asking
            // the writer to repeat work that is done.
            (State::RegionRestoredEarly, Event::Observation(InstallObservation::Completed)) => {
                Some(State::Restored)
            }
            (
                State::InstallObserved | State::Installing,
                Event::Observation(InstallObservation::Completed),
            ) => Some(State::Installed),
            (
                State::InstallRequested
                | State::InstallObserved
                | State::Installing
                | State::RegionRestoredEarly,
                Event::Observation(InstallObservation::CompletionUncertain)
                | Event::CompletionBecameUncertain
                | Event::ObservationTimedOut,
            ) => Some(State::CompletionUncertain),
            (
                State::CompletionUncertain,
                Event::TimeoutResolved(
                    TimeoutResolution::RestoreAfterUserAffirmedCompletion
                    | TimeoutResolution::ForceRestoreAfterWarningAcknowledged,
                ),
            ) => Some(State::RestoringRegion),
            (State::Installed | State::FailedWithTemporaryRegion, Event::BeginRestoringRegion) => {
                Some(State::RestoringRegion)
            }
            (State::RestoringRegion, Event::OriginalRegionRestored) => Some(State::Restored),
            (State::Restored, Event::Complete) => Some(State::Completed),
            (state, Event::FailBeforeRegionChange)
                if !state.requires_recovery() && !state.is_terminal() =>
            {
                Some(State::FailedBeforeRegionChange)
            }
            (state, Event::FailWithTemporaryRegion) if state.requires_recovery() => {
                Some(State::FailedWithTemporaryRegion)
            }
            (state, Event::RestoreConflict) if state.requires_recovery() => {
                Some(State::RestoreConflict)
            }
            (State::RestoringRegion, Event::RestoreFailed) => Some(State::RestoreFailed),
            (state, Event::Cancel) if !state.requires_recovery() && !state.is_terminal() => {
                Some(State::Cancelled)
            }
            _ => None,
        }
    }
}

const fn durable_state_for_event(
    event: InstallationOperationEvent,
) -> Option<DurableOperationState> {
    match event {
        InstallationOperationEvent::RecoveryStateSaved => Some(DurableOperationState::Prepared),
        InstallationOperationEvent::RegionSwitchFinished(RegionSwitchOutcome::RegionSwitched) => {
            Some(DurableOperationState::RegionSwitched)
        }
        InstallationOperationEvent::InstallRequestAccepted => {
            Some(DurableOperationState::InstallRequested)
        }
        InstallationOperationEvent::EarlyRegionRestoreConfirmed => {
            Some(DurableOperationState::RegionRestoredEarly)
        }
        InstallationOperationEvent::Observation(InstallObservation::CompletionUncertain)
        | InstallationOperationEvent::CompletionBecameUncertain
        | InstallationOperationEvent::ObservationTimedOut => {
            Some(DurableOperationState::CompletionUncertain)
        }
        InstallationOperationEvent::BeginRestoringRegion
        | InstallationOperationEvent::TimeoutResolved(
            TimeoutResolution::RestoreAfterUserAffirmedCompletion
            | TimeoutResolution::ForceRestoreAfterWarningAcknowledged,
        ) => Some(DurableOperationState::RestoringRegion),
        InstallationOperationEvent::OriginalRegionRestored => Some(DurableOperationState::Restored),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{InstallObservation, InstallPhase};
    use crate::recovery::RecoveryStore;
    use crate::recovery::record::DurableOperationState;
    use crate::recovery::store::PendingRestoreGuard;
    use crate::region::RegionSwitchOutcome;
    use crate::test_support::{
        MemoryRecoveryStore, RecordingPersistence, TestPersistenceError, pending_restore_for_test,
    };

    #[test]
    fn operation_machine_accepts_the_guarded_happy_path_and_persists_critical_states() {
        let mut machine = InstallationOperationMachine::new();
        let mut persistence = RecordingPersistence::default();
        let events = [
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
            InstallationOperationEvent::Observation(InstallObservation::Installing {
                phase: InstallPhase::Installing,
                progress: None,
            }),
            InstallationOperationEvent::Observation(InstallObservation::Completed),
            InstallationOperationEvent::BeginRestoringRegion,
            InstallationOperationEvent::OriginalRegionRestored,
            InstallationOperationEvent::Complete,
        ];

        for event in events {
            assert!(machine.can_apply(event));
            machine
                .apply(event, &mut persistence)
                .expect("legal operation transition");
        }

        assert_eq!(machine.state(), InstallationOperationState::Completed);
        assert!(!machine.state().requires_recovery());
        assert_eq!(
            persistence.states,
            vec![
                DurableOperationState::Prepared,
                DurableOperationState::RegionSwitched,
                DurableOperationState::InstallRequested,
                DurableOperationState::RestoringRegion,
                DurableOperationState::Restored,
            ]
        );
    }

    /// Drive the machine to the point where an install is being observed.
    fn machine_observing_an_install(
        persistence: &mut RecordingPersistence,
    ) -> InstallationOperationMachine {
        let mut machine = InstallationOperationMachine::new();
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
            InstallationOperationEvent::Observation(InstallObservation::Installing {
                phase: InstallPhase::Installing,
                progress: None,
            }),
        ] {
            machine
                .apply(event, persistence)
                .expect("legal operation transition");
        }
        assert_eq!(machine.state(), InstallationOperationState::Installing);
        machine
    }

    #[test]
    fn an_early_region_restore_keeps_the_operation_running_and_skips_a_second_restore() {
        let mut persistence = RecordingPersistence::default();
        let mut machine = machine_observing_an_install(&mut persistence);

        machine
            .apply(
                InstallationOperationEvent::EarlyRegionRestoreConfirmed,
                &mut persistence,
            )
            .expect("an early restore is legal while installing");
        assert_eq!(
            machine.state(),
            InstallationOperationState::RegionRestoredEarly
        );
        assert!(
            machine.state().requires_recovery(),
            "a safe region is not an outcome: the record must survive"
        );

        machine
            .apply(
                InstallationOperationEvent::Observation(InstallObservation::Installing {
                    phase: InstallPhase::PostInstall,
                    progress: None,
                }),
                &mut persistence,
            )
            .expect("observation continues after the region is back");
        machine
            .apply(
                InstallationOperationEvent::Observation(InstallObservation::Completed),
                &mut persistence,
            )
            .expect("completion is legal from an early-restored operation");
        assert_eq!(
            machine.state(),
            InstallationOperationState::Restored,
            "the region is already original and was read back, so nothing restores twice"
        );

        machine
            .apply(InstallationOperationEvent::Complete, &mut persistence)
            .expect("cleanup completes the operation");
        assert_eq!(machine.state(), InstallationOperationState::Completed);
        assert!(
            persistence
                .states
                .contains(&DurableOperationState::RegionRestoredEarly),
            "a restart must be able to see that the region already went back"
        );
        assert!(
            !persistence
                .states
                .contains(&DurableOperationState::RestoringRegion),
            "no second restoration was attempted"
        );
    }

    #[test]
    fn an_early_region_restore_is_illegal_before_the_observer_reports_installing() {
        let mut persistence = RecordingPersistence::default();
        let mut machine = InstallationOperationMachine::new();
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
        ] {
            machine
                .apply(event, &mut persistence)
                .expect("legal operation transition");
        }

        assert!(!machine.can_apply(InstallationOperationEvent::EarlyRegionRestoreConfirmed));
        let before = persistence.states.clone();
        assert!(
            machine
                .apply(
                    InstallationOperationEvent::EarlyRegionRestoreConfirmed,
                    &mut persistence
                )
                .is_err()
        );
        assert_eq!(
            persistence.states, before,
            "a rejected event must not persist anything"
        );
    }

    #[test]
    fn uncertainty_after_an_early_restore_still_demands_a_user_decision() {
        let mut persistence = RecordingPersistence::default();
        let mut machine = machine_observing_an_install(&mut persistence);
        machine
            .apply(
                InstallationOperationEvent::EarlyRegionRestoreConfirmed,
                &mut persistence,
            )
            .expect("an early restore is legal while installing");

        machine
            .apply(
                InstallationOperationEvent::ObservationTimedOut,
                &mut persistence,
            )
            .expect("a timeout is legal after an early restore");
        assert_eq!(
            machine.state(),
            InstallationOperationState::CompletionUncertain
        );
        assert!(machine.state().requires_recovery());
    }

    #[test]
    fn operation_machine_refuses_every_event_once_a_terminal_result_is_reached() {
        const HAPPY_PATH: [InstallationOperationEvent; 14] = [
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
            InstallationOperationEvent::Observation(InstallObservation::Completed),
            InstallationOperationEvent::BeginRestoringRegion,
            InstallationOperationEvent::OriginalRegionRestored,
            InstallationOperationEvent::Complete,
        ];

        let routes: [&[InstallationOperationEvent]; 3] = [
            &HAPPY_PATH,
            &[
                InstallationOperationEvent::BeginResolvingProduct,
                InstallationOperationEvent::FailBeforeRegionChange,
            ],
            &[
                InstallationOperationEvent::BeginResolvingProduct,
                InstallationOperationEvent::Cancel,
            ],
        ];

        for route in routes {
            let mut machine = InstallationOperationMachine::new();
            let mut persistence = RecordingPersistence::default();
            for event in route {
                machine
                    .apply(*event, &mut persistence)
                    .expect("legal route to a terminal state");
            }
            let terminal = machine.state();
            assert!(terminal.is_terminal());
            assert!(!terminal.requires_recovery());

            let persisted_before = persistence.states.len();
            for event in [
                InstallationOperationEvent::Cancel,
                InstallationOperationEvent::FailBeforeRegionChange,
                InstallationOperationEvent::BeginResolvingProduct,
                InstallationOperationEvent::BeginRestoringRegion,
                InstallationOperationEvent::Complete,
            ] {
                assert!(!machine.can_apply(event));
                assert_eq!(
                    machine.apply(event, &mut persistence),
                    Err(InstallationOperationError::InvalidTransition {
                        from: terminal,
                        event,
                    })
                );
                assert_eq!(machine.state(), terminal);
            }
            assert_eq!(persistence.states.len(), persisted_before);
        }
    }

    #[test]
    fn operation_machine_writes_critical_states_through_pending_restore_guard() {
        let mut machine = InstallationOperationMachine::new();
        let pending = pending_restore_for_test();
        let mut guard = PendingRestoreGuard::new(MemoryRecoveryStore::default(), pending);
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
        ] {
            machine
                .apply(event, &mut guard)
                .expect("legal durable path");
        }

        assert_eq!(
            machine.state(),
            InstallationOperationState::InstallRequested
        );
        assert_eq!(
            guard.pending().state(),
            DurableOperationState::InstallRequested
        );
        let store = guard.into_store();
        assert_eq!(
            store.load().expect("read persisted state"),
            Some(pending_restore_for_test().with_state(DurableOperationState::InstallRequested))
        );
    }

    #[test]
    fn operation_machine_cannot_start_install_without_confirmed_temporary_geoid() {
        let mut machine = InstallationOperationMachine::new();
        let mut persistence = RecordingPersistence::default();

        assert_eq!(
            machine.apply(InstallationOperationEvent::BeginInstall, &mut persistence),
            Err(InstallationOperationError::InvalidTransition {
                from: InstallationOperationState::Idle,
                event: InstallationOperationEvent::BeginInstall,
            })
        );
        assert!(persistence.states.is_empty());

        for event in [
            InstallationOperationEvent::BeginResolvingProduct,
            InstallationOperationEvent::ProductResolved,
            InstallationOperationEvent::BeginPreparing,
            InstallationOperationEvent::BeginSavingRecoveryState,
            InstallationOperationEvent::RecoveryStateSaved,
        ] {
            machine.apply(event, &mut persistence).expect("legal setup");
        }
        assert_eq!(machine.state(), InstallationOperationState::SwitchingRegion);
        machine
            .apply(
                InstallationOperationEvent::RegionSwitchFinished(
                    RegionSwitchOutcome::OriginalRegionPreserved,
                ),
                &mut persistence,
            )
            .expect("original region does not permit install");

        assert_eq!(
            machine.state(),
            InstallationOperationState::FailedBeforeRegionChange
        );
        assert!(!machine.state().allows_install_start());
        assert_eq!(
            machine.apply(
                InstallationOperationEvent::BeginRefreshingStoreContext,
                &mut persistence,
            ),
            Err(InstallationOperationError::InvalidTransition {
                from: InstallationOperationState::FailedBeforeRegionChange,
                event: InstallationOperationEvent::BeginRefreshingStoreContext,
            })
        );
    }

    #[test]
    fn a_failure_while_the_region_is_being_written_never_claims_nothing_changed() {
        let mut machine = InstallationOperationMachine::new();
        let mut persistence = RecordingPersistence::default();
        for event in [
            InstallationOperationEvent::BeginResolvingProduct,
            InstallationOperationEvent::ProductResolved,
            InstallationOperationEvent::BeginPreparing,
            InstallationOperationEvent::BeginSavingRecoveryState,
            InstallationOperationEvent::RecoveryStateSaved,
        ] {
            machine.apply(event, &mut persistence).expect("legal setup");
        }
        assert_eq!(machine.state(), InstallationOperationState::SwitchingRegion);
        assert!(machine.state().requires_recovery());

        // The record is already published and the writer may already have run,
        // so both events that mean "no recovery is owed" are refused here.
        for event in [
            InstallationOperationEvent::FailBeforeRegionChange,
            InstallationOperationEvent::Cancel,
        ] {
            assert_eq!(
                machine.apply(event, &mut persistence),
                Err(InstallationOperationError::InvalidTransition {
                    from: InstallationOperationState::SwitchingRegion,
                    event,
                })
            );
        }
        machine
            .apply(
                InstallationOperationEvent::FailWithTemporaryRegion,
                &mut persistence,
            )
            .expect("a failure mid-write keeps the recovery record");
        assert_eq!(
            machine.state(),
            InstallationOperationState::FailedWithTemporaryRegion
        );
    }

    #[test]
    fn operation_machine_retains_recovery_for_uncertain_completion_and_failed_persistence() {
        let mut machine = InstallationOperationMachine::new();
        let mut persistence = RecordingPersistence::default();
        for event in [
            InstallationOperationEvent::BeginResolvingProduct,
            InstallationOperationEvent::ProductResolved,
            InstallationOperationEvent::BeginPreparing,
            InstallationOperationEvent::BeginSavingRecoveryState,
            InstallationOperationEvent::RecoveryStateSaved,
        ] {
            machine.apply(event, &mut persistence).expect("legal setup");
        }
        persistence.fail_next = true;
        assert_eq!(
            machine.apply(
                InstallationOperationEvent::RegionSwitchFinished(
                    RegionSwitchOutcome::RegionSwitched
                ),
                &mut persistence,
            ),
            Err(InstallationOperationError::Persistence(
                TestPersistenceError::Write
            ))
        );
        assert_eq!(machine.state(), InstallationOperationState::SwitchingRegion);

        machine
            .apply(
                InstallationOperationEvent::RegionSwitchFinished(
                    RegionSwitchOutcome::RegionSwitched,
                ),
                &mut persistence,
            )
            .expect("state changes only after durable region-switched record");
        for event in [
            InstallationOperationEvent::BeginRefreshingStoreContext,
            InstallationOperationEvent::BeginInstall,
            InstallationOperationEvent::InstallRequestAccepted,
            InstallationOperationEvent::ObservationStarted,
            InstallationOperationEvent::Observation(InstallObservation::CompletionUncertain),
        ] {
            machine
                .apply(event, &mut persistence)
                .expect("legal uncertain path");
        }

        assert_eq!(
            machine.state(),
            InstallationOperationState::CompletionUncertain
        );
        assert!(machine.state().requires_recovery());
        assert_eq!(
            machine.apply(InstallationOperationEvent::Cancel, &mut persistence),
            Err(InstallationOperationError::InvalidTransition {
                from: InstallationOperationState::CompletionUncertain,
                event: InstallationOperationEvent::Cancel,
            })
        );
        assert_eq!(
            persistence.states,
            vec![
                DurableOperationState::Prepared,
                DurableOperationState::RegionSwitched,
                DurableOperationState::InstallRequested,
                DurableOperationState::CompletionUncertain,
            ]
        );
    }
}
