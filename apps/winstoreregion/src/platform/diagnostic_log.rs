//! Size-rotated plain-text diagnostic log below the per-user data directory.
//!
//! Writing here is best effort by design. A failure to log must never change
//! what an operation does, so every entry point returns a structured result
//! that callers are expected to discard rather than propagate.
//!
//! This module performs no network access; a test asserts that it never will.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR};
use winstoreregion_core::{
    LOCAL_DATA_DIRECTORY, LOG_DIRECTORY_NAME, LOG_FILE_NAME, LogEvent, UtcTimestamp,
};

/// Largest size the current log may reach before it is rotated.
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// Number of rotated files kept beside the current one.
const KEPT_ROTATIONS: u32 = 3;

/// A local failure while recording diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LogWriteError {
    /// The per-user data directory could not be determined.
    DirectoryUnavailable,
    /// The record could not be appended.
    Write { os_error: Option<i32> },
}

/// Append-only diagnostic log for the current user.
pub(crate) struct DiagnosticLog {
    directory: PathBuf,
}

impl DiagnosticLog {
    /// Open the log below `%LOCALAPPDATA%\WinStoreRegion\logs`.
    pub(crate) fn for_current_user() -> Result<Self, LogWriteError> {
        let directory = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|base| base.join(LOCAL_DATA_DIRECTORY).join(LOG_DIRECTORY_NAME))
            .ok_or(LogWriteError::DirectoryUnavailable)?;
        Ok(Self { directory })
    }

    #[cfg(test)]
    pub(crate) fn for_test(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn current_path(&self) -> PathBuf {
        self.directory.join(LOG_FILE_NAME)
    }

    fn rotated_path(&self, index: u32) -> PathBuf {
        self.directory.join(format!("winstoreregion.{index}.log"))
    }

    /// Append one already-rendered record.
    ///
    /// Callers discard the result: the log is diagnostic, and an operation that
    /// failed to be recorded is still an operation that happened.
    pub(crate) fn append(&self, event: &LogEvent) -> Result<(), LogWriteError> {
        let line = event.to_line(&now());
        fs::create_dir_all(&self.directory).map_err(|error| write_error(&error))?;
        let path = self.current_path();
        if Self::should_rotate(&path, line.len()) {
            // A failed rotation must not lose the record: keep appending.
            let _ = self.rotate();
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| write_error(&error))?;
        file.write_all(line.as_bytes())
            .map_err(|error| write_error(&error))?;
        file.write_all(b"\n").map_err(|error| write_error(&error))
    }

    fn should_rotate(path: &Path, incoming: usize) -> bool {
        let Ok(metadata) = fs::metadata(path) else {
            return false;
        };
        metadata.len() + incoming as u64 > MAX_FILE_BYTES
    }

    /// Shift `winstoreregion.N.log` up by one and drop the oldest.
    fn rotate(&self) -> Result<(), LogWriteError> {
        let oldest = self.rotated_path(KEPT_ROTATIONS);
        if oldest.exists() {
            fs::remove_file(&oldest).map_err(|error| write_error(&error))?;
        }
        for index in (1..KEPT_ROTATIONS).rev() {
            let from = self.rotated_path(index);
            if from.exists() {
                fs::rename(&from, self.rotated_path(index + 1))
                    .map_err(|error| write_error(&error))?;
            }
        }
        let current = self.current_path();
        if current.exists() {
            fs::rename(&current, self.rotated_path(1)).map_err(|error| write_error(&error))?;
        }
        Ok(())
    }
}

/// Open the diagnostic log directory in Explorer.
///
/// Returns whether Windows accepted the request. The directory is created first
/// so the menu item is honest even before the first record is written.
#[allow(unsafe_code)]
pub(crate) fn open_log_directory() -> bool {
    let Ok(log) = DiagnosticLog::for_current_user() else {
        return false;
    };
    if fs::create_dir_all(&log.directory).is_err() {
        return false;
    }
    let directory = HSTRING::from(log.directory.as_os_str());
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::null(),
            &directory,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    (result.0 as usize) > 32
}

/// Record one event, discarding any failure.
///
/// Diagnostics must never change what an operation does, so this returns
/// nothing: there is no result a caller could accidentally propagate into the
/// outcome of a region change or an installation.
pub(crate) fn record(event: &LogEvent) {
    let _ = DiagnosticLog::for_current_user().and_then(|log| log.append(event));
}

fn write_error(error: &std::io::Error) -> LogWriteError {
    LogWriteError::Write {
        os_error: error.raw_os_error(),
    }
}

/// Current UTC instant, or the epoch when the clock is unavailable.
///
/// A record with a doubtful timestamp is still better than a lost record.
fn now() -> UtcTimestamp {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    UtcTimestamp::from_unix_seconds(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use winstoreregion_core::{LogEventCode, LogLevel};

    static NEXT_TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    fn test_directory() -> PathBuf {
        let unique = NEXT_TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        PathBuf::from(r"C:\temp\WinStoreRegion\log-test")
            .join(format!("{}-{unique}", std::process::id()))
    }

    fn remove_test_directory(directory: &Path) {
        let _ = fs::remove_dir_all(directory);
    }

    fn event(token: &'static str) -> LogEvent {
        LogEvent::new(LogLevel::Info, LogEventCode::ApplicationStarted).with_token("marker", token)
    }

    #[test]
    fn records_are_appended_one_line_each() {
        let directory = test_directory();
        let log = DiagnosticLog::for_test(directory.clone());
        log.append(&event("first")).expect("first record");
        log.append(&event("second")).expect("second record");

        let contents = fs::read_to_string(directory.join(LOG_FILE_NAME)).expect("log file");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("marker=first"));
        assert!(lines[1].contains("marker=second"));
        remove_test_directory(&directory);
    }

    #[test]
    fn rotation_shifts_names_and_keeps_the_configured_number_of_files() {
        let directory = test_directory();
        fs::create_dir_all(&directory).expect("test directory");
        // Start just under the threshold so the next record triggers rotation.
        fs::write(
            directory.join(LOG_FILE_NAME),
            vec![b'x'; usize::try_from(MAX_FILE_BYTES).expect("threshold fits")],
        )
        .expect("oversized current log");
        for index in 1..=KEPT_ROTATIONS {
            fs::write(
                directory.join(format!("winstoreregion.{index}.log")),
                b"old",
            )
            .expect("rotated log");
        }

        let log = DiagnosticLog::for_test(directory.clone());
        log.append(&event("after-rotation")).expect("record");

        let current = fs::read_to_string(directory.join(LOG_FILE_NAME)).expect("current log");
        assert!(current.contains("marker=after-rotation"));
        assert!(
            current.len() < 1024,
            "the oversized file must have been rotated away"
        );
        assert_eq!(
            fs::metadata(directory.join("winstoreregion.1.log"))
                .expect("first rotation")
                .len(),
            MAX_FILE_BYTES
        );
        assert!(
            !directory
                .join(format!("winstoreregion.{}.log", KEPT_ROTATIONS + 1))
                .exists(),
            "rotation must not grow past the configured count"
        );
        remove_test_directory(&directory);
    }

    #[test]
    fn an_unwritable_directory_reports_an_error_instead_of_panicking() {
        // A file where the directory should be makes creation fail.
        let directory = test_directory();
        fs::create_dir_all(directory.parent().expect("parent")).expect("parent directory");
        fs::write(&directory, b"not a directory").expect("blocking file");

        let log = DiagnosticLog::for_test(directory.clone());
        assert!(matches!(
            log.append(&event("blocked")),
            Err(LogWriteError::Write { .. })
        ));
        let _ = fs::remove_file(&directory);
    }

    #[test]
    fn the_log_module_performs_no_network_access() {
        let source = include_str!("diagnostic_log.rs");
        for forbidden in [
            ["Tcp", "Stream"].concat(),
            ["Udp", "Socket"].concat(),
            ["To", "SocketAddrs"].concat(),
            ["req", "west"].concat(),
            ["ur", "eq"].concat(),
        ] {
            assert!(
                !source.contains(&forbidden),
                "the diagnostic log must never reach the network: {forbidden}"
            );
        }
    }
}
