//! Narrow Win32 and `WinRT` adapters used by the GUI.
//!
//! Every submodule owns one Windows subject and keeps its `unsafe` local. Nothing
//! here decides policy: adapters read, write, or launch exactly what core asks for
//! and return structured results.

pub(crate) mod diagnostic_log;
pub(crate) mod handoff;
pub(crate) mod installer_download;
pub(crate) mod market_probe;
pub(crate) mod packaged;
pub(crate) mod prerequisites;
pub(crate) mod region;
pub(crate) mod storage;
pub(crate) mod store;
pub(crate) mod stub;
pub(crate) mod uninstall;
pub(crate) mod winget;

use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::HRESULT;
use winstoreregion_core::ObservationTimestamp;

pub(crate) struct WindowsRuntimeGuard {
    /// Whether this guard is the one that entered the apartment.
    ///
    /// Only the entering guard may leave it. A guard taken on a thread that was
    /// already in an apartment must not undo an initialisation it did not make.
    entered_apartment: bool,
}

impl WindowsRuntimeGuard {
    /// Make sure the calling thread is in an apartment that can activate `WinRT`.
    ///
    /// Activation reports `CO_E_NOTINITIALIZED` instead of the real
    /// availability of a `WinRT` class when this is skipped, so every caller
    /// that activates one must hold this guard.
    ///
    /// A worker thread joins the multithreaded apartment. The UI thread is
    /// already single-threaded through `OleInitialize`, and Windows answers a
    /// differing request with `RPC_E_CHANGED_MODE`. That is not a failure to
    /// initialise: the thread is in an apartment and activation works there.
    /// Treating it as one turned every prerequisite into `Unknown` and blocked
    /// installation on a machine that satisfied all of them.
    #[allow(unsafe_code)]
    pub(super) fn initialize() -> windows::core::Result<Self> {
        /// The thread already belongs to an apartment of the other kind.
        const RPC_E_CHANGED_MODE: HRESULT = HRESULT(-2_147_417_850);
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            Ok(()) => Ok(Self {
                entered_apartment: true,
            }),
            Err(error) if error.code() == RPC_E_CHANGED_MODE => Ok(Self {
                entered_apartment: false,
            }),
            Err(error) => Err(error),
        }
    }
}

impl Drop for WindowsRuntimeGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if self.entered_apartment {
            unsafe { RoUninitialize() };
        }
    }
}

pub(crate) fn observation_timestamp_now() -> ObservationTimestamp {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    ObservationTimestamp::from_unix_millis(u64::try_from(milliseconds).unwrap_or(u64::MAX))
}
