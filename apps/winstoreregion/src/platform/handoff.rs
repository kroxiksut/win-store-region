//! Starting one verified Microsoft Store installer.
//!
//! This is the only place in the product that executes a file the user chose.
//! It decides nothing: the admission rule lives in core, and this adapter runs
//! exactly the path it is handed, with no arguments, and does not wait for the
//! process. The installer's exit code is deliberately never collected — a
//! Microsoft Store installer that exits proves nothing about an installation.

use std::path::Path;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR};

/// Why Windows did not start the verified installer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HandoffLaunchError {
    /// The shell refused the launch and returned its legacy error value.
    LaunchRejected(i32),
}

impl HandoffLaunchError {
    /// The native value the shell returned, kept for the details block only.
    pub(crate) const fn native_code(self) -> i32 {
        match self {
            Self::LaunchRejected(code) => code,
        }
    }
}

/// Hand the operation to a file core has already admitted.
///
/// The working directory is the file's own, which is where an installer expects
/// to be started from. No verb is requested, so Windows applies the default
/// action rather than an elevated one.
///
/// # Errors
///
/// Returns [`HandoffLaunchError`] when the shell refuses. Success means only
/// that Windows started the process; it says nothing about what it will do.
#[allow(unsafe_code)]
pub(crate) fn launch_admitted_installer(path: &Path) -> Result<(), HandoffLaunchError> {
    let file = HSTRING::from(path.as_os_str());
    let directory = path
        .parent()
        .map(|parent| HSTRING::from(parent.as_os_str()));
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::null(),
            &file,
            PCWSTR::null(),
            directory
                .as_ref()
                .map_or(PCWSTR::null(), |directory| PCWSTR(directory.as_ptr())),
            SW_SHOWNORMAL,
        )
    };
    // `ShellExecuteW` reports success as a pseudo-handle greater than 32 and
    // every failure as a small legacy code, which is not an `HRESULT`.
    let code = result.0 as usize;
    if code > 32 {
        return Ok(());
    }
    Err(HandoffLaunchError::LaunchRejected(
        i32::try_from(code).unwrap_or(i32::MAX),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_file_that_does_not_exist_is_refused_with_the_shell_code() {
        let missing = PathBuf::from(r"C:\Windows\System32\winstoreregion-no-such-installer.exe");
        let error = launch_admitted_installer(&missing).expect_err("nothing to start");

        // The number is diagnostic only; what matters is that a refusal is a
        // refusal and never quietly reads as a started installer.
        assert!(error.native_code() <= 32);
    }

    #[test]
    fn the_launch_boundary_never_waits_for_the_process_it_started() {
        // A handoff that waited would turn an exit code into an outcome, and a
        // Microsoft Store installer's exit code means nothing about an install.
        let source = include_str!("handoff.rs");
        // Split so the barrier does not trip over its own words.
        let forbidden = [
            ["WaitFor", "SingleObject"].concat(),
            ["GetExitCode", "Process"].concat(),
            [".wa", "it()"].concat(),
        ];
        for forbidden in forbidden {
            assert!(
                !source.contains(&forbidden),
                "the handoff boundary must not observe the process: {forbidden}"
            );
        }
    }
}
