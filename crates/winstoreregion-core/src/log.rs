//! Local diagnostic records describing how an operation went.
//!
//! A log record is not recovery state and not the operation journal. Nothing in
//! this crate reads it back, and no decision depends on it. Core builds and
//! renders records; a platform adapter owns the file.

use crate::region::{GeoId, OperationId};
use crate::time::UtcTimestamp;

/// Directory name for diagnostic logs below the per-user data directory.
pub const LOG_DIRECTORY_NAME: &str = "logs";

/// File name of the current diagnostic log.
pub const LOG_FILE_NAME: &str = "winstoreregion.log";

/// Severity of one diagnostic record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogLevel {
    /// An expected step of normal operation.
    Info,
    /// Something the user may need to know about, but the operation continues.
    Warning,
    /// A failure the user was told about.
    Error,
}

impl LogLevel {
    /// Fixed-width tag so lines stay aligned when read in a plain editor.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Info => "INFO ",
            Self::Warning => "WARN ",
            Self::Error => "ERROR",
        }
    }
}

/// Closed set of events worth recording.
///
/// The set is closed on purpose: a log that can say anything says nothing, and
/// a new event should be a deliberate decision rather than a formatted string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogEventCode {
    ApplicationStarted,
    ApplicationStopped,
    StartupRecoveryInspected,
    PrerequisitesChecked,
    RecoveryActionExecuted,
    /// A guarded operation began; every later record shares its operation id.
    OperationStarted,
    /// The recovery record was published before Windows was touched.
    RecoveryRecordWritten,
    /// Windows was asked for the list of regions it knows.
    ///
    /// Recorded only when the answer is unusable: an empty list is what the
    /// user sees, and nothing on screen explains where it came from.
    RegionListUnavailable,
    /// The package manager was asked whether a recorded installation is running.
    ///
    /// The answer decides whether a restart takes the operation over or settles
    /// its record, and the three ways it can be "no" are not the same thing.
    ResumeProbed,
    RegionSwitchAttempted,
    RegionRestoreAttempted,
    /// The catalogue was asked for the product under the temporary region.
    ProductResolveAttempted,
    BackendSelected,
    BackendExited,
    /// The verified Microsoft Store installer was handed the operation.
    ///
    /// Separate from [`Self::BackendExited`] on purpose: nothing here reports a
    /// result, because a launched installer proves nothing about an install.
    HandoffLaunchAttempted,
    InstallObservation,
    /// An entry was appended to the operation history, or was not.
    JournalEntryWritten,
    /// The guarded operation reached a state it cannot leave.
    OperationFinished,
    DiagnosticReported,
}

impl LogEventCode {
    /// Stable machine-facing name; never localized.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApplicationStarted => "application_started",
            Self::ApplicationStopped => "application_stopped",
            Self::StartupRecoveryInspected => "startup_recovery_inspected",
            Self::PrerequisitesChecked => "prerequisites_checked",
            Self::RecoveryActionExecuted => "recovery_action_executed",
            Self::OperationStarted => "operation_started",
            Self::RecoveryRecordWritten => "recovery_record_written",
            Self::RegionListUnavailable => "region_list_unavailable",
            Self::ResumeProbed => "resume_probed",
            Self::RegionSwitchAttempted => "region_switch_attempted",
            Self::RegionRestoreAttempted => "region_restore_attempted",
            Self::ProductResolveAttempted => "product_resolve_attempted",
            Self::BackendSelected => "backend_selected",
            Self::BackendExited => "backend_exited",
            Self::HandoffLaunchAttempted => "handoff_launch_attempted",
            Self::InstallObservation => "install_observation",
            Self::JournalEntryWritten => "journal_entry_written",
            Self::OperationFinished => "operation_finished",
            Self::DiagnosticReported => "diagnostic_reported",
        }
    }
}

/// One diagnostic record.
///
/// Values are attached through the typed helpers below. There is deliberately
/// no way to attach a filesystem path: a log must not carry one, so the API
/// does not offer it rather than relying on every caller to remember.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogEvent {
    level: LogLevel,
    code: LogEventCode,
    operation_id: Option<OperationId>,
    fields: Vec<(&'static str, String)>,
}

impl LogEvent {
    /// Start a record for one event.
    #[must_use]
    pub const fn new(level: LogLevel, code: LogEventCode) -> Self {
        Self {
            level,
            code,
            operation_id: None,
            fields: Vec::new(),
        }
    }

    /// Attach the guarded operation this record belongs to.
    #[must_use]
    pub fn for_operation(mut self, operation_id: OperationId) -> Self {
        self.operation_id = Some(operation_id);
        self
    }

    /// Attach a Windows Home Location value.
    #[must_use]
    pub fn with_geo_id(mut self, name: &'static str, geo_id: GeoId) -> Self {
        self.fields.push((name, geo_id.value().to_string()));
        self
    }

    /// Attach a native or process result code.
    #[must_use]
    pub fn with_code(mut self, name: &'static str, code: i32) -> Self {
        self.fields.push((name, code.to_string()));
        self
    }

    /// Attach a short machine-facing token, such as an enum variant name.
    ///
    /// Free user text must never reach this: the caller passes a fixed token,
    /// not something typed into the window.
    #[must_use]
    pub fn with_token(mut self, name: &'static str, token: &'static str) -> Self {
        self.fields.push((name, token.to_owned()));
        self
    }

    /// Attach an already-validated identifier, such as a Store product id.
    #[must_use]
    pub fn with_identifier(mut self, name: &'static str, value: &str) -> Self {
        self.fields.push((name, sanitize(value)));
        self
    }

    /// Attach a backend-reported percentage.
    ///
    /// Separate from [`Self::with_code`] so a progress value can never be read
    /// back as a result code.
    #[must_use]
    pub fn with_percent(mut self, name: &'static str, percent: u8) -> Self {
        self.fields.push((name, percent.to_string()));
        self
    }

    /// Attach a plain yes-or-no fact about a step that was attempted.
    #[must_use]
    pub fn with_flag(mut self, name: &'static str, value: bool) -> Self {
        self.fields
            .push((name, if value { "yes" } else { "no" }.to_owned()));
        self
    }

    /// Attach a file identity without disclosing where the file lives.
    ///
    /// The full path is deliberately unrepresentable; only the file name and
    /// its content hash are recorded.
    #[must_use]
    pub fn with_file_identity(mut self, file_name: &str, sha256: &str) -> Self {
        self.fields.push(("file", sanitize(file_name)));
        self.fields.push(("sha256", sanitize(sha256)));
        self
    }

    /// Return the severity of this record.
    #[must_use]
    pub const fn level(&self) -> LogLevel {
        self.level
    }

    /// Render the record as one plain-text line, without a trailing newline.
    ///
    /// The format is fixed-field text rather than JSON because the reader is a
    /// person attaching the file to a report.
    #[must_use]
    pub fn to_line(&self, at: &UtcTimestamp) -> String {
        let mut line = format!(
            "{} {} {} op={}",
            at.as_str(),
            self.level.tag(),
            self.code.as_str(),
            self.operation_id
                .as_ref()
                .map_or("-", |operation_id| operation_id.as_str()),
        );
        for (name, value) in &self.fields {
            line.push(' ');
            line.push_str(name);
            line.push('=');
            line.push_str(value);
        }
        line
    }
}

/// Replace whitespace and control characters so one record stays one line.
fn sanitize(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if character.is_control() || character.is_whitespace() {
                '_'
            } else {
                character
            }
        })
        .collect();
    if cleaned.is_empty() {
        "-".to_owned()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region::{GeoId, OperationId};
    use crate::time::UtcTimestamp;

    fn timestamp() -> UtcTimestamp {
        UtcTimestamp::parse("2026-08-17T12:34:56Z").expect("valid UTC timestamp")
    }

    #[test]
    fn a_record_renders_as_one_aligned_plain_text_line() {
        let event = LogEvent::new(LogLevel::Info, LogEventCode::RegionSwitchAttempted)
            .for_operation(OperationId::new("op-1").expect("non-empty operation identifier"))
            .with_geo_id("from", GeoId::new(203).expect("positive GeoId"))
            .with_geo_id("to", GeoId::new(244).expect("positive GeoId"))
            .with_token("outcome", "region_switched");

        assert_eq!(
            event.to_line(&timestamp()),
            "2026-08-17T12:34:56Z INFO  region_switch_attempted op=op-1 from=203 to=244 outcome=region_switched"
        );
    }

    #[test]
    fn a_record_without_an_operation_still_names_the_field() {
        let event = LogEvent::new(LogLevel::Error, LogEventCode::ApplicationStarted);
        assert_eq!(
            event.to_line(&timestamp()),
            "2026-08-17T12:34:56Z ERROR application_started op=-"
        );
    }

    #[test]
    fn a_file_is_recorded_by_name_and_hash_but_never_by_path() {
        let event = LogEvent::new(LogLevel::Info, LogEventCode::DiagnosticReported)
            .with_file_identity("StoreInstaller.exe", "abc123");
        let line = event.to_line(&timestamp());

        assert!(line.contains("file=StoreInstaller.exe"));
        assert!(line.contains("sha256=abc123"));
        assert!(!line.contains(":\\"), "a log line must not carry a path");
    }

    #[test]
    fn a_percentage_and_a_yes_or_no_fact_are_rendered_as_their_own_fields() {
        let event = LogEvent::new(LogLevel::Info, LogEventCode::InstallObservation)
            .with_percent("percent", 42)
            .with_flag("early", true)
            .with_flag("resumed", false);

        assert_eq!(
            event.to_line(&timestamp()),
            "2026-08-17T12:34:56Z INFO  install_observation op=- percent=42 early=yes resumed=no"
        );
    }

    #[test]
    fn embedded_whitespace_can_never_split_one_record_across_lines() {
        let event = LogEvent::new(LogLevel::Warning, LogEventCode::DiagnosticReported)
            .with_identifier("product", "9WZ\nDNC\tRF JP")
            .with_file_identity("", "");
        let line = event.to_line(&timestamp());

        assert_eq!(line.lines().count(), 1);
        assert!(line.contains("product=9WZ_DNC_RF_JP"));
        assert!(line.contains("file=-"));
    }
}
