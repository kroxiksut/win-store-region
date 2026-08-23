//! What a guarded operation should do next.
//!
//! The state machine answers whether a transition is legal. This module answers
//! a different question: given where the operation stands and what has been
//! observed, which single step comes now. It stays pure. It owns no backend, no
//! region writer and no clock; the caller supplies the facts, receives one
//! instruction, carries it out, and reports the result back as an event.

use crate::install::InstallObservation;
use crate::machine::InstallationOperationState;
use crate::observe::timeout::{ObservationElapsedMillis, OperationTimeoutPolicy};

/// What the caller knows at the moment it asks for the next step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationFacts {
    /// Most recent observation from the backend, if one has arrived.
    pub last_observation: Option<InstallObservation>,
    /// Whether the backend has reported an installing observation at least once.
    ///
    /// Distinct from `last_observation`: the region may go back only after the
    /// backend has begun to act, and a later observation does not undo that.
    pub progress_seen: bool,
    /// Time since the backend accepted the install request.
    pub since_request: ObservationElapsedMillis,
    /// Time since the last observation arrived.
    pub silence: ObservationElapsedMillis,
    /// Whether the original region is already back in place.
    pub region_restored_early: bool,
    /// Whether the product has been resolved under the temporary region.
    ///
    /// The catalogue is regional at search time, so a product missing from the
    /// home region is only knowable once that region is in place. Until this
    /// holds, there is nothing to install.
    pub resolved_under_temporary_region: bool,
}

impl OperationFacts {
    /// Facts for an operation whose install has just been accepted.
    #[must_use]
    pub const fn at_request() -> Self {
        Self {
            last_observation: None,
            progress_seen: false,
            since_request: ObservationElapsedMillis::new(0),
            silence: ObservationElapsedMillis::new(0),
            region_restored_early: false,
            resolved_under_temporary_region: false,
        }
    }
}

/// The single step the caller should perform next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationStep {
    /// Resolve the product now that the temporary region is in force.
    ResolveUnderTemporaryRegion,
    /// Ask the selected backend to start the install.
    StartInstall,
    /// Keep observing; nothing else is due.
    KeepObserving,
    /// Restore the original region while the install continues.
    RestoreRegionEarly,
    /// Restore the original region now that the operation has an outcome.
    RestoreRegion,
    /// Hand the decision to the user because the operation is uncertain.
    AskUserAfterTimeout,
    /// Clear the recovery record and finish.
    Finish,
    /// Nothing is due in this state.
    Wait,
}

/// Decide the one step that follows from the current state and facts.
///
/// Three rules hold regardless of state, because each protects the user from a
/// region left where they did not put it:
///
/// - a timeout never restores a region, it only asks the user;
/// - a region already restored early is never restored again;
/// - an install that has not reported progress is never abandoned early.
#[must_use]
pub fn next_step(
    state: InstallationOperationState,
    facts: OperationFacts,
    policy: OperationTimeoutPolicy,
) -> OperationStep {
    use InstallationOperationState as State;

    match state {
        State::RefreshingStoreContext => {
            if facts.resolved_under_temporary_region {
                OperationStep::StartInstall
            } else {
                OperationStep::ResolveUnderTemporaryRegion
            }
        }
        State::InstallRequested | State::InstallObserved | State::Installing => {
            observing_step(facts, policy)
        }
        // The region is already back, so only the install's fate is still open.
        State::RegionRestoredEarly => {
            if timed_out(facts, policy) {
                OperationStep::AskUserAfterTimeout
            } else {
                OperationStep::KeepObserving
            }
        }
        State::CompletionUncertain => OperationStep::AskUserAfterTimeout,
        State::Installed | State::FailedWithTemporaryRegion => {
            if facts.region_restored_early {
                // Nothing to restore: read-back already confirmed the original
                // region when it went back during the install.
                OperationStep::Finish
            } else {
                OperationStep::RestoreRegion
            }
        }
        State::Restored => OperationStep::Finish,
        _ => OperationStep::Wait,
    }
}

/// The step for an operation whose install is being observed.
fn observing_step(facts: OperationFacts, policy: OperationTimeoutPolicy) -> OperationStep {
    if matches!(facts.last_observation, Some(InstallObservation::Completed)) {
        return if facts.region_restored_early {
            OperationStep::Finish
        } else {
            OperationStep::RestoreRegion
        };
    }
    if timed_out(facts, policy) {
        return OperationStep::AskUserAfterTimeout;
    }
    if !facts.region_restored_early
        && policy
            .early_region_restore
            .admits_restore(facts.progress_seen, facts.since_request)
    {
        return OperationStep::RestoreRegionEarly;
    }
    OperationStep::KeepObserving
}

/// Whether either deadline has passed.
const fn timed_out(facts: OperationFacts, policy: OperationTimeoutPolicy) -> bool {
    policy.operation_timed_out(facts.since_request) || policy.backend_went_quiet(facts.silence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::InstallPhase;
    use crate::observe::timeout::{
        DEFAULT_EARLY_REGION_RESTORE_HOLD_MILLIS, DEFAULT_QUIET_THRESHOLD_MILLIS,
    };

    /// Facts for an install that has been running long enough to restore early.
    fn installing_for(millis: u64) -> OperationFacts {
        OperationFacts {
            last_observation: Some(InstallObservation::Installing {
                phase: InstallPhase::Installing,
                progress: None,
            }),
            progress_seen: true,
            since_request: ObservationElapsedMillis::new(millis),
            silence: ObservationElapsedMillis::new(0),
            ..OperationFacts::at_request()
        }
    }

    #[test]
    fn a_confirmed_region_resolves_the_product_before_installing_it() {
        let policy = OperationTimeoutPolicy::default();
        assert_eq!(
            next_step(
                InstallationOperationState::RefreshingStoreContext,
                OperationFacts::at_request(),
                policy,
            ),
            OperationStep::ResolveUnderTemporaryRegion,
            "the catalogue is regional, so the product is only knowable here"
        );

        let resolved = OperationFacts {
            resolved_under_temporary_region: true,
            ..OperationFacts::at_request()
        };
        assert_eq!(
            next_step(
                InstallationOperationState::RefreshingStoreContext,
                resolved,
                policy,
            ),
            OperationStep::StartInstall
        );
    }

    #[test]
    fn the_region_goes_back_only_after_progress_and_the_hold() {
        let policy = OperationTimeoutPolicy::default();
        let too_early = OperationFacts {
            since_request: ObservationElapsedMillis::new(500),
            ..installing_for(500)
        };
        assert_eq!(
            next_step(InstallationOperationState::Installing, too_early, policy),
            OperationStep::KeepObserving
        );

        let no_progress_yet = OperationFacts {
            last_observation: None,
            progress_seen: false,
            ..installing_for(DEFAULT_EARLY_REGION_RESTORE_HOLD_MILLIS * 10)
        };
        assert_eq!(
            next_step(
                InstallationOperationState::InstallRequested,
                no_progress_yet,
                policy
            ),
            OperationStep::KeepObserving,
            "an install that has not begun must keep its temporary region"
        );

        assert_eq!(
            next_step(
                InstallationOperationState::Installing,
                installing_for(DEFAULT_EARLY_REGION_RESTORE_HOLD_MILLIS),
                policy
            ),
            OperationStep::RestoreRegionEarly
        );
    }

    #[test]
    fn a_region_restored_early_is_never_restored_a_second_time() {
        let policy = OperationTimeoutPolicy::default();
        let after_early_restore = OperationFacts {
            region_restored_early: true,
            ..installing_for(DEFAULT_EARLY_REGION_RESTORE_HOLD_MILLIS * 5)
        };
        assert_eq!(
            next_step(
                InstallationOperationState::RegionRestoredEarly,
                after_early_restore,
                policy
            ),
            OperationStep::KeepObserving
        );

        let completed = OperationFacts {
            last_observation: Some(InstallObservation::Completed),
            ..after_early_restore
        };
        assert_eq!(
            next_step(InstallationOperationState::Installing, completed, policy),
            OperationStep::Finish
        );
        assert_eq!(
            next_step(InstallationOperationState::Installed, completed, policy),
            OperationStep::Finish
        );
    }

    #[test]
    fn a_completed_install_restores_the_region_when_it_is_still_temporary() {
        let completed = OperationFacts {
            last_observation: Some(InstallObservation::Completed),
            ..installing_for(1_000)
        };
        assert_eq!(
            next_step(
                InstallationOperationState::Installing,
                completed,
                OperationTimeoutPolicy::default()
            ),
            OperationStep::RestoreRegion
        );
    }

    #[test]
    fn a_timeout_only_ever_asks_the_user() {
        let policy = OperationTimeoutPolicy::default();
        let silent = OperationFacts {
            silence: ObservationElapsedMillis::new(DEFAULT_QUIET_THRESHOLD_MILLIS),
            ..installing_for(DEFAULT_QUIET_THRESHOLD_MILLIS)
        };

        for state in [
            InstallationOperationState::Installing,
            InstallationOperationState::RegionRestoredEarly,
            InstallationOperationState::CompletionUncertain,
        ] {
            assert_eq!(
                next_step(state, silent, policy),
                OperationStep::AskUserAfterTimeout,
                "{state:?} must not restore a region on a deadline"
            );
        }
    }

    #[test]
    fn states_without_a_due_step_ask_for_nothing() {
        let policy = OperationTimeoutPolicy::default();
        for state in [
            InstallationOperationState::Idle,
            InstallationOperationState::ResolvingProduct,
            InstallationOperationState::SwitchingRegion,
            InstallationOperationState::RestoringRegion,
            InstallationOperationState::Completed,
            InstallationOperationState::RestoreConflict,
        ] {
            assert_eq!(
                next_step(state, OperationFacts::at_request(), policy),
                OperationStep::Wait,
                "{state:?} has no step of its own"
            );
        }
    }
}
