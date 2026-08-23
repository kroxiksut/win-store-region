//! Observation deadlines and their user-directed resolution.

/// Elapsed observer time supplied by the caller's monotonic clock.
///
/// Core deliberately does not read wall-clock time: adapters own the clock,
/// while this value keeps timeout decisions deterministic in tests.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ObservationElapsedMillis(u64);

impl ObservationElapsedMillis {
    /// Construct elapsed observer time in milliseconds.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the elapsed time in milliseconds.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// A positive timeout duration used by an observation policy.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ObservationTimeoutMillis(u64);

impl ObservationTimeoutMillis {
    /// Construct a non-zero observation timeout.
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    /// Return the configured timeout in milliseconds.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Configurable timeout policy for one installation observer.
///
/// The default is intentionally disabled. A concrete duration must be chosen
/// from the packaged and Win32 VM experiments, not invented in production code.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ObservationTimeoutPolicy {
    /// Do not impose a deadline until experiments justify one.
    #[default]
    Disabled,
    /// Enter the explicit timeout flow once this duration has elapsed.
    After(ObservationTimeoutMillis),
}

impl ObservationTimeoutPolicy {
    /// Return whether the supplied elapsed time reached this policy's deadline.
    #[must_use]
    pub const fn has_elapsed(self, elapsed: ObservationElapsedMillis) -> bool {
        match self {
            Self::Disabled => false,
            Self::After(timeout) => elapsed.value() >= timeout.value(),
        }
    }
}

/// How long the temporary region is held before an install is left to finish alone.
///
/// Measured on the test VM: restoring 0.16 s or 0.5 s after the backend handed
/// back its operation breaks the install, while 1 s, 3 s, 6 s and 8 s do not.
/// The margin below is therefore generous on purpose, because the boundary is a
/// race inside the package manager rather than a documented contract.
pub const DEFAULT_EARLY_REGION_RESTORE_HOLD_MILLIS: u64 = 10_000;

/// When the original region may return while an installation is still running.
///
/// Holding the temporary region for the whole install protects nothing and
/// leaves the user in a foreign region for minutes, including across a crash.
/// Restoring it early shortens that window to seconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EarlyRegionRestorePolicy {
    /// Keep the temporary region until the operation reaches an outcome.
    HoldUntilOutcome,
    /// Restore once the observer saw progress and this much time has elapsed.
    ///
    /// Both conditions are required. Elapsed time alone would restore during a
    /// request the backend has not begun to act on; a progress event alone
    /// arrives too early, within the racy window measured above.
    AfterProgress(ObservationTimeoutMillis),
}

impl Default for EarlyRegionRestorePolicy {
    fn default() -> Self {
        Self::AfterProgress(
            ObservationTimeoutMillis::new(DEFAULT_EARLY_REGION_RESTORE_HOLD_MILLIS)
                .expect("the default hold is non-zero"),
        )
    }
}

impl EarlyRegionRestorePolicy {
    /// Whether the original region may be restored now.
    ///
    /// `progress_seen` means the observer reported at least one installing
    /// observation for this attempt, with or without a percentage.
    #[must_use]
    pub const fn admits_restore(
        self,
        progress_seen: bool,
        elapsed: ObservationElapsedMillis,
    ) -> bool {
        match self {
            Self::HoldUntilOutcome => false,
            Self::AfterProgress(hold) => progress_seen && elapsed.value() >= hold.value(),
        }
    }
}

/// Shortest silence that may be taken as a stalled installation.
///
/// Measured on the test VM: a healthy install went 50 seconds without emitting
/// a single progress event, and the whole install took up to 130 seconds. A
/// threshold anywhere near that silence would declare a working install stuck.
pub const DEFAULT_QUIET_THRESHOLD_MILLIS: u64 = 300_000;

/// The three independent deadlines of one guarded operation.
///
/// They are deliberately separate values rather than one derived from another.
/// They answer different questions: how long the whole operation may last, how
/// long the backend may stay silent, and how long the temporary region must
/// remain before the original may come back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationTimeoutPolicy {
    /// Deadline for the operation as a whole.
    pub overall: ObservationTimeoutPolicy,
    /// Deadline for silence, measured since the last observation.
    pub quiet: ObservationTimeoutPolicy,
    /// When the original region may return while the install continues.
    pub early_region_restore: EarlyRegionRestorePolicy,
}

impl Default for OperationTimeoutPolicy {
    fn default() -> Self {
        Self {
            // No overall deadline is imposed: no measurement justifies one, and
            // an invented number would end healthy installs.
            overall: ObservationTimeoutPolicy::Disabled,
            quiet: ObservationTimeoutMillis::new(DEFAULT_QUIET_THRESHOLD_MILLIS)
                .map_or(ObservationTimeoutPolicy::Disabled, |threshold| {
                    ObservationTimeoutPolicy::After(threshold)
                }),
            early_region_restore: EarlyRegionRestorePolicy::default(),
        }
    }
}

impl OperationTimeoutPolicy {
    /// Whether the operation as a whole has run out of time.
    #[must_use]
    pub const fn operation_timed_out(self, elapsed: ObservationElapsedMillis) -> bool {
        self.overall.has_elapsed(elapsed)
    }

    /// Whether the backend has been silent long enough to count as stalled.
    ///
    /// `silence` is time since the last observation, not since the start.
    #[must_use]
    pub const fn backend_went_quiet(self, silence: ObservationElapsedMillis) -> bool {
        self.quiet.has_elapsed(silence)
    }
}

/// Choice explicitly made by a user after an observation timeout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeoutResolutionChoice {
    /// Resume observation only; Windows region remains unchanged.
    ContinueWaiting,
    /// The user states installation is complete and asks to restore the region.
    InstallationCompletedRestoreRegion,
    /// Restore immediately despite a possibly unfinished installation.
    RestoreRegionNow {
        /// The UI displayed its warning and the user explicitly acknowledged it.
        warning_acknowledged: bool,
    },
}

/// Validated command derived from one explicit timeout choice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeoutResolution {
    /// Resume only the observer after the timeout.
    ResumeObserver,
    /// Restore after the user explicitly affirmed completion.
    RestoreAfterUserAffirmedCompletion,
    /// Restore after the user acknowledged the incomplete-installation warning.
    ForceRestoreAfterWarningAcknowledged,
}

/// Why a timeout choice cannot produce an executable resolution command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeoutResolutionError {
    /// Forced restoration requires an explicit warning acknowledgement.
    ForcedRestoreWarningNotAcknowledged,
}

/// Validate an explicit timeout choice before it reaches the state machine.
///
/// This function never restores a region. It makes the forced-restore warning
/// acknowledgement an input requirement instead of a presentation convention.
///
/// # Errors
///
/// Returns [`TimeoutResolutionError::ForcedRestoreWarningNotAcknowledged`] when
/// the user did not explicitly acknowledge the possible incomplete installation.
pub fn resolve_timeout_choice(
    choice: TimeoutResolutionChoice,
) -> Result<TimeoutResolution, TimeoutResolutionError> {
    match choice {
        TimeoutResolutionChoice::ContinueWaiting => Ok(TimeoutResolution::ResumeObserver),
        TimeoutResolutionChoice::InstallationCompletedRestoreRegion => {
            Ok(TimeoutResolution::RestoreAfterUserAffirmedCompletion)
        }
        TimeoutResolutionChoice::RestoreRegionNow {
            warning_acknowledged: true,
        } => Ok(TimeoutResolution::ForceRestoreAfterWarningAcknowledged),
        TimeoutResolutionChoice::RestoreRegionNow {
            warning_acknowledged: false,
        } => Err(TimeoutResolutionError::ForcedRestoreWarningNotAcknowledged),
    }
}

#[cfg(test)]
mod early_restore_tests {
    use super::*;

    const HOLD: u64 = DEFAULT_EARLY_REGION_RESTORE_HOLD_MILLIS;

    #[test]
    fn early_restore_needs_progress_and_elapsed_time_together() {
        let policy = EarlyRegionRestorePolicy::default();
        assert!(!policy.admits_restore(false, ObservationElapsedMillis::new(HOLD * 10)));
        assert!(!policy.admits_restore(true, ObservationElapsedMillis::new(HOLD - 1)));
        assert!(policy.admits_restore(true, ObservationElapsedMillis::new(HOLD)));
    }

    #[test]
    fn holding_until_the_outcome_never_admits_an_early_restore() {
        let policy = EarlyRegionRestorePolicy::HoldUntilOutcome;
        assert!(!policy.admits_restore(true, ObservationElapsedMillis::new(u64::MAX)));
    }

    #[test]
    fn the_three_deadlines_stay_independent_of_one_another() {
        let policy = OperationTimeoutPolicy::default();
        // A silence shorter than the longest measured healthy pause must not
        // be read as a stall, and it must not affect the region hold either.
        assert!(!policy.backend_went_quiet(ObservationElapsedMillis::new(60_000)));
        assert!(policy.backend_went_quiet(ObservationElapsedMillis::new(
            DEFAULT_QUIET_THRESHOLD_MILLIS
        )));
        assert!(
            policy
                .early_region_restore
                .admits_restore(true, ObservationElapsedMillis::new(HOLD))
        );
        assert!(
            !policy.operation_timed_out(ObservationElapsedMillis::new(u64::MAX)),
            "no measurement justifies an overall deadline yet"
        );
    }

    #[test]
    fn the_quiet_threshold_is_far_above_the_longest_measured_healthy_pause() {
        // A healthy install stayed silent for 50 seconds.
        let policy = OperationTimeoutPolicy::default();
        assert!(!policy.backend_went_quiet(ObservationElapsedMillis::new(120_000)));
    }

    #[test]
    fn the_default_hold_covers_the_measured_failure_boundary() {
        // 0.5 s broke the install on the test VM and 1 s did not, so neither
        // may be admitted by the default policy.
        let policy = EarlyRegionRestorePolicy::default();
        for too_early in [500, 1_000] {
            assert!(
                !policy.admits_restore(true, ObservationElapsedMillis::new(too_early)),
                "{too_early} ms is inside the racy window"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{InstallObservation, InstallPhase};
    use crate::machine::{
        InstallationOperationError, InstallationOperationEvent, InstallationOperationMachine,
        InstallationOperationState,
    };
    use crate::recovery::record::DurableOperationState;
    use crate::region::RegionSwitchOutcome;
    use crate::test_support::RecordingPersistence;

    #[test]
    fn timeout_policy_and_user_resolution_never_restore_automatically() {
        assert!(
            !ObservationTimeoutPolicy::default()
                .has_elapsed(ObservationElapsedMillis::new(u64::MAX))
        );
        let policy = ObservationTimeoutPolicy::After(
            ObservationTimeoutMillis::new(500).expect("positive timeout"),
        );
        assert!(!policy.has_elapsed(ObservationElapsedMillis::new(499)));
        assert!(policy.has_elapsed(ObservationElapsedMillis::new(500)));
        assert_eq!(ObservationTimeoutMillis::new(0), None);
        assert_eq!(
            resolve_timeout_choice(TimeoutResolutionChoice::RestoreRegionNow {
                warning_acknowledged: false,
            }),
            Err(TimeoutResolutionError::ForcedRestoreWarningNotAcknowledged)
        );

        let mut machine = InstallationOperationMachine::new();
        let mut persistence = RecordingPersistence::default();
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
            InstallationOperationEvent::ObservationTimedOut,
        ] {
            machine
                .apply(event, &mut persistence)
                .expect("timeout flow is legal");
        }
        assert_eq!(
            machine.state(),
            InstallationOperationState::CompletionUncertain
        );
        assert_eq!(
            persistence.states.last(),
            Some(&DurableOperationState::CompletionUncertain),
            "timeout persists uncertainty before the UI can offer choices"
        );
        assert_eq!(
            machine.apply(
                InstallationOperationEvent::BeginRestoringRegion,
                &mut persistence
            ),
            Err(InstallationOperationError::InvalidTransition {
                from: InstallationOperationState::CompletionUncertain,
                event: InstallationOperationEvent::BeginRestoringRegion,
            })
        );

        let continue_waiting = resolve_timeout_choice(TimeoutResolutionChoice::ContinueWaiting)
            .expect("continuing never needs a warning");
        assert_eq!(continue_waiting, TimeoutResolution::ResumeObserver);
        machine
            .apply(
                InstallationOperationEvent::TimeoutResolved(continue_waiting),
                &mut persistence,
            )
            .expect("continue only resumes observer");
        assert_eq!(machine.state(), InstallationOperationState::InstallObserved);

        machine
            .apply(
                InstallationOperationEvent::ObservationTimedOut,
                &mut persistence,
            )
            .expect("a resumed observer may time out again");
        let restore =
            resolve_timeout_choice(TimeoutResolutionChoice::InstallationCompletedRestoreRegion)
                .expect("user affirmation is explicit");
        machine
            .apply(
                InstallationOperationEvent::TimeoutResolved(restore),
                &mut persistence,
            )
            .expect("explicit user request may start recovery");
        assert_eq!(machine.state(), InstallationOperationState::RestoringRegion);
        assert_eq!(
            persistence.states.last(),
            Some(&DurableOperationState::RestoringRegion)
        );
    }
}
