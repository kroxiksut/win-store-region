//! Installation through the `WinGet` COM API.
//!
//! Three measured facts shape this adapter. The install runs in
//! `WindowsPackageManagerServer.exe` and outlives the process that started it,
//! so cancelling and detaching are genuinely different and a restart can
//! re-attach. Progress is real but partial: an installation percentage arrives,
//! byte counts never do, because Microsoft Store performs the download. And the
//! catalogue never reports a Store product as installed, so completion evidence
//! comes from the package identity, which core owns, not from here.

use super::{CLSID_INSTALL_OPTIONS, WinGetSession, WinGetUnavailable};
use crate::platform::winget::bindings::{
    InstallOptions, InstallProgress as NativeProgress, InstallResult, InstallResultStatus,
    LocalPackageCatalog, PackageInstallMode, PackageInstallProgressState,
};
use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows_future::{AsyncOperationProgressHandler, AsyncStatus, IAsyncOperationWithProgress};
use winstoreregion_core::{
    DetachOrCancelOutcome, DiagnosticCode, InstallAttemptId, InstallBackend,
    InstallBackendCapability, InstallBackendError, InstallBackendKind, InstallHandle,
    InstallObservation, InstallObservationRoute, InstallPhase, InstallProgress, InstallRequest,
    install_supported_in_v0_1,
};

/// The source rejected the install because the temporary region was not applied.
///
/// `0x8A150010`, written signed because that is how the API reports it.
const APPINSTALLER_NO_APPLICABLE_INSTALLER: i32 = -1_978_335_216;

/// Another install of the same product is already running: `0x80070652`.
const ERROR_INSTALL_ALREADY_RUNNING: i32 = -2_147_023_278;

/// Microsoft Store will not deliver this product to this device: `0x803FB104`.
///
/// Reported when every package of the product targets another device family.
/// The Store log calls it `Product not fulfillable`.
const STORE_PRODUCT_NOT_FULFILLABLE: i32 = -2_143_309_564;

/// Name the failure a finished operation reported, if it reported one.
///
/// `None` means the backend did not state a failure, and the outcome stays the
/// question core answers: an unproven completion is uncertain, not failed.
/// A code with no name of its own is still a stated failure — it is reported as
/// one, with the number, rather than being softened into uncertainty.
#[must_use]
pub(crate) fn stated_failure(detail: BackendNativeDetail) -> Option<DiagnosticCode> {
    // Status carries the verdict; `Ok` means the backend claims no failure.
    let status = detail.status?;
    if status == InstallResultStatus::Ok.0 {
        return None;
    }
    Some(match detail.extended {
        Some(STORE_PRODUCT_NOT_FULFILLABLE) => DiagnosticCode::ProductNotOnThisDevice,
        _ => DiagnosticCode::BackendReportedUnknownFailure,
    })
}

/// Native evidence behind the most recent answer this backend gave.
///
/// The domain error type names what the user must be told, and deliberately
/// carries no platform number. The numbers still exist, and a failed install is
/// unreadable without them, so they are kept here for the diagnostic log to
/// pick up. Nothing decides anything from this.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct BackendNativeDetail {
    /// `HRESULT` of the COM call that failed, when one failed.
    pub(crate) hresult: Option<i32>,
    /// `InstallResultStatus` of an operation that ran to its end.
    pub(crate) status: Option<i32>,
    /// `ExtendedErrorCode` reported beside that status.
    pub(crate) extended: Option<i32>,
    /// Exit code of the installer itself, when the API exposes one.
    pub(crate) installer: Option<u32>,
}

/// Latest facts an in-flight operation has reported.
#[derive(Clone, Copy, Debug, Default)]
struct ProgressSnapshot {
    phase: Option<PackageInstallProgressState>,
    percent: Option<u8>,
}

/// One install this process is following.
struct RunningInstall {
    operation: IAsyncOperationWithProgress<InstallResult, NativeProgress>,
    snapshot: Arc<Mutex<ProgressSnapshot>>,
}

/// `InstallBackend` over the `WinGet` COM API and its `msstore` catalogue.
///
/// Bound to the thread that created it: the COM objects it holds are apartment
/// bound, and the operation it follows lives beside them.
pub(crate) struct WinGetComBackend {
    session: WinGetSession,
    running: RefCell<Option<RunningInstall>>,
    native: Cell<BackendNativeDetail>,
}

impl WinGetComBackend {
    /// Connect to the catalogue this backend installs from.
    ///
    /// # Errors
    ///
    /// Returns the structured reason the API cannot be used.
    pub(crate) fn connect() -> Result<Self, WinGetUnavailable> {
        Ok(Self {
            session: WinGetSession::connect()?,
            running: RefCell::new(None),
            native: Cell::new(BackendNativeDetail::default()),
        })
    }

    /// Native evidence behind the most recent answer, for the diagnostic log.
    pub(crate) fn native_detail(&self) -> BackendNativeDetail {
        self.native.get()
    }

    /// Keep the platform code behind a refusal and return the domain error.
    fn refused(&self, hresult: i32, error: InstallBackendError) -> InstallBackendError {
        self.native.set(BackendNativeDetail {
            hresult: Some(hresult),
            ..BackendNativeDetail::default()
        });
        error
    }

    /// Follow an operation, recording its progress for later observation.
    fn follow(
        &self,
        request: &InstallRequest,
        operation: IAsyncOperationWithProgress<InstallResult, NativeProgress>,
    ) -> Result<InstallHandle, InstallBackendError> {
        let snapshot = Arc::new(Mutex::new(ProgressSnapshot::default()));
        let sink = Arc::clone(&snapshot);
        let handler = AsyncOperationProgressHandler::new(
            move |_operation: windows_core::Ref<
                '_,
                IAsyncOperationWithProgress<InstallResult, NativeProgress>,
            >,
                  progress: windows_core::Ref<'_, NativeProgress>| {
                // The callback arrives on a backend thread; it only records, so
                // a slow observer cannot stall the installation.
                let progress = *progress.ok()?;
                if let Ok(mut sink) = sink.lock() {
                    sink.phase = Some(progress.State);
                    sink.percent = percent_from(&progress);
                }
                Ok(())
            },
        );
        operation
            .SetProgress(&handler)
            .map_err(|_| InstallBackendError::ObservationFailed)?;

        *self.running.borrow_mut() = Some(RunningInstall {
            operation,
            snapshot,
        });
        Ok(InstallHandle::new(
            self.kind(),
            InstallAttemptId::new(request.product_id().as_str())
                .ok_or(InstallBackendError::RequestRejected)?,
        ))
    }

    /// The operation this handle refers to, when this backend owns it.
    fn operation_for(&self, handle: &InstallHandle) -> Result<(), InstallBackendError> {
        if handle.backend() != self.kind() || self.running.borrow().is_none() {
            return Err(InstallBackendError::InvalidHandle);
        }
        Ok(())
    }
}

impl InstallBackend for WinGetComBackend {
    fn kind(&self) -> InstallBackendKind {
        InstallBackendKind::WingetComMsStore
    }

    fn capability(&self, request: &InstallRequest) -> InstallBackendCapability {
        if install_supported_in_v0_1(request.install_classification()) {
            InstallBackendCapability::CanInstall {
                observation_route: InstallObservationRoute::Packaged,
            }
        } else {
            // The product resolved and this backend works; only the way the
            // Store delivers it is out of scope for this version.
            InstallBackendCapability::UnsupportedInstallKind
        }
    }

    #[allow(unsafe_code)]
    fn start_install(
        &self,
        request: &InstallRequest,
    ) -> Result<InstallHandle, InstallBackendError> {
        self.native.set(BackendNativeDetail::default());
        if !install_supported_in_v0_1(request.install_classification()) {
            return Err(InstallBackendError::RequestRejected);
        }
        let package = self
            .session
            .find_exact(request.product_id().as_str())
            .map_err(|code| self.refused(code, InstallBackendError::RequestRejected))?
            // An empty answer is not evidence about the region: this call runs
            // under a region confirmed by read-back, and the same catalogue
            // named the product under it moments ago. Whatever happened to the
            // product since, saying "the temporary region did not apply" would
            // be a statement this backend cannot support.
            .ok_or(InstallBackendError::PackageNotOffered)?;

        let options: InstallOptions =
            unsafe { CoCreateInstance(&CLSID_INSTALL_OPTIONS, None, CLSCTX_ALL) }
                .map_err(|error| self.refused(error.code().0, InstallBackendError::Unavailable))?;
        options
            .SetAcceptPackageAgreements(true)
            .map_err(|error| self.refused(error.code().0, InstallBackendError::RequestRejected))?;
        options
            .SetPackageInstallMode(PackageInstallMode::Silent)
            .map_err(|error| self.refused(error.code().0, InstallBackendError::RequestRejected))?;

        let operation = self
            .session
            .manager()
            .InstallPackageAsync(&package, &options)
            .map_err(|error| {
                self.refused(error.code().0, classify_start_failure(error.code().0))
            })?;
        self.follow(request, operation)
    }

    fn resume(
        &self,
        request: &InstallRequest,
    ) -> Result<Option<InstallHandle>, InstallBackendError> {
        let manager = self.session.manager();
        let installing = manager
            .GetLocalPackageCatalog(LocalPackageCatalog::InstallingPackages)
            .map_err(|_| InstallBackendError::Unavailable)?;
        let connect = installing
            .Connect()
            .map_err(|_| InstallBackendError::Unavailable)?;
        let Ok(catalog) = connect.PackageCatalog() else {
            return Ok(None);
        };
        let Ok(Some(package)) = super::find_exact_in(&catalog, request.product_id().as_str())
        else {
            // Nothing in flight. That is not evidence of failure: the install
            // may equally have finished while this process was gone.
            return Ok(None);
        };

        // Measured: the operation comes back only when the package is taken
        // from the local installing catalogue and the info from the remote one.
        // The same-catalogue pairing returns nothing.
        let info = self
            .session
            .catalog_reference()
            .Info()
            .map_err(|_| InstallBackendError::Unavailable)?;
        let Ok(operation) = manager.GetInstallProgress(&package, &info) else {
            return Ok(None);
        };
        self.follow(request, operation).map(Some)
    }

    fn observe(&self, handle: &InstallHandle) -> Result<InstallObservation, InstallBackendError> {
        self.operation_for(handle)?;
        let running = self.running.borrow();
        let running = running.as_ref().ok_or(InstallBackendError::InvalidHandle)?;
        let status = running
            .operation
            .Status()
            .map_err(|_| InstallBackendError::ObservationFailed)?;
        if status == AsyncStatus::Started {
            let snapshot = running
                .snapshot
                .lock()
                .map_err(|_| InstallBackendError::ObservationFailed)?;
            return Ok(InstallObservation::Installing {
                phase: phase_of(snapshot.phase),
                progress: snapshot.percent.and_then(InstallProgress::new),
            });
        }
        let Ok(result) = running.operation.GetResults() else {
            // A cancelled or faulted operation has no result to read. Nothing
            // here may claim the installation finished.
            return Ok(InstallObservation::CompletionUncertain);
        };
        self.native.set(native_detail_of(&result));
        Ok(observation_from(&result))
    }

    fn detach_or_cancel(
        &self,
        handle: &InstallHandle,
    ) -> Result<DetachOrCancelOutcome, InstallBackendError> {
        self.operation_for(handle)?;
        let running = self.running.borrow_mut().take();
        let running = running.ok_or(InstallBackendError::InvalidHandle)?;
        match running.operation.Cancel() {
            Ok(()) => Ok(DetachOrCancelOutcome::Cancelled),
            // Cancellation failed, so the install keeps running without an
            // observer. Saying so is the honest answer.
            Err(_) => Ok(DetachOrCancelOutcome::Detached),
        }
    }
}

/// Turn a completed operation into the observation core reasons about.
///
/// A successful status is not completion on its own: the package identity has
/// to confirm it, and that check belongs to core's observer.
fn observation_from(result: &InstallResult) -> InstallObservation {
    match result.Status() {
        Ok(InstallResultStatus::Ok) => InstallObservation::Completed,
        _ => InstallObservation::CompletionUncertain,
    }
}

/// Keep every number a finished operation carries.
///
/// A refused install is indistinguishable from any other refusal without them:
/// the domain error says only that the request was rejected.
fn native_detail_of(result: &InstallResult) -> BackendNativeDetail {
    BackendNativeDetail {
        hresult: None,
        status: result.Status().ok().map(|status| status.0),
        extended: result.ExtendedErrorCode().ok().map(|code| code.0),
        installer: result.InstallerErrorCode().ok(),
    }
}

/// Classify a refusal to start by what the user must be told.
const fn classify_start_failure(code: i32) -> InstallBackendError {
    match code {
        APPINSTALLER_NO_APPLICABLE_INSTALLER => InstallBackendError::TemporaryRegionNotApplied,
        ERROR_INSTALL_ALREADY_RUNNING => InstallBackendError::AlreadyInstalling,
        _ => InstallBackendError::RequestRejected,
    }
}

/// Map the backend's phase onto the domain phase.
const fn phase_of(state: Option<PackageInstallProgressState>) -> InstallPhase {
    match state {
        Some(PackageInstallProgressState::PostInstall) => InstallPhase::PostInstall,
        Some(
            PackageInstallProgressState::Installing | PackageInstallProgressState::Downloading,
        ) => InstallPhase::Installing,
        _ => InstallPhase::Queued,
    }
}

/// The only percentage worth showing for a Store product.
///
/// The download fraction is reported as complete from the first event and the
/// byte counts stay at zero, because Microsoft Store performs the transfer.
fn percent_from(progress: &NativeProgress) -> Option<u8> {
    let fraction = progress.InstallationProgress;
    if !fraction.is_finite() || fraction <= 0.0 {
        return None;
    }
    // Clamped first, so the conversion below can only see 0 through 100.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let percent = (fraction * 100.0).round().clamp(0.0, 100.0) as u8;
    Some(percent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_start_names_the_cause_the_user_can_act_on() {
        assert_eq!(
            classify_start_failure(APPINSTALLER_NO_APPLICABLE_INSTALLER),
            InstallBackendError::TemporaryRegionNotApplied
        );
        assert_eq!(
            classify_start_failure(ERROR_INSTALL_ALREADY_RUNNING),
            InstallBackendError::AlreadyInstalling
        );
        assert_eq!(
            classify_start_failure(-1),
            InstallBackendError::RequestRejected
        );
    }

    #[test]
    fn a_backend_that_states_a_failure_is_not_reported_as_an_unknown_outcome() {
        // The measured case: a product built for another device family.
        assert_eq!(
            stated_failure(BackendNativeDetail {
                status: Some(6),
                extended: Some(STORE_PRODUCT_NOT_FULFILLABLE),
                ..BackendNativeDetail::default()
            }),
            Some(DiagnosticCode::ProductNotOnThisDevice)
        );
        // A stated failure stays a failure even when nothing here can name it.
        assert_eq!(
            stated_failure(BackendNativeDetail {
                status: Some(6),
                extended: Some(-1),
                ..BackendNativeDetail::default()
            }),
            Some(DiagnosticCode::BackendReportedUnknownFailure)
        );
        assert_eq!(
            stated_failure(BackendNativeDetail {
                status: Some(6),
                extended: None,
                ..BackendNativeDetail::default()
            }),
            Some(DiagnosticCode::BackendReportedUnknownFailure)
        );
    }

    #[test]
    fn silence_about_the_status_is_never_turned_into_a_failure() {
        // No status at all: a cancelled or faulted operation, which core still
        // has to treat as uncertain rather than failed.
        assert_eq!(stated_failure(BackendNativeDetail::default()), None);
        assert_eq!(
            stated_failure(BackendNativeDetail {
                status: Some(InstallResultStatus::Ok.0),
                extended: Some(0),
                ..BackendNativeDetail::default()
            }),
            None
        );
    }

    #[test]
    fn a_missing_phase_is_queued_rather_than_installing() {
        assert_eq!(phase_of(None), InstallPhase::Queued);
        assert_eq!(
            phase_of(Some(PackageInstallProgressState::Queued)),
            InstallPhase::Queued
        );
        assert_eq!(
            phase_of(Some(PackageInstallProgressState::Installing)),
            InstallPhase::Installing
        );
        assert_eq!(
            phase_of(Some(PackageInstallProgressState::PostInstall)),
            InstallPhase::PostInstall
        );
    }

    #[test]
    fn a_percentage_appears_only_once_the_backend_reports_real_work() {
        let mut progress = NativeProgress {
            State: PackageInstallProgressState::Installing,
            BytesDownloaded: 0,
            BytesRequired: 0,
            DownloadProgress: 1.0,
            InstallationProgress: 0.0,
        };
        assert_eq!(
            percent_from(&progress),
            None,
            "zero is the backend's initial value, not measured progress"
        );

        progress.InstallationProgress = 0.425;
        assert_eq!(percent_from(&progress), Some(43));

        progress.InstallationProgress = 2.0;
        assert_eq!(
            percent_from(&progress),
            Some(100),
            "a percentage can never exceed one hundred"
        );
    }
}
