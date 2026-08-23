//! Diagnostic trace of one guarded installation.
//!
//! Every step an installation performs is recorded here as one closed log
//! event, so a failed operation can be read afterwards instead of guessed at.
//! This module decides nothing: it holds no state an operation depends on,
//! returns nothing a caller could branch on, and a trace that cannot be written
//! never changes what the installation does.

use crate::platform::diagnostic_log::record;
use crate::platform::winget::backend::BackendNativeDetail;
use winstoreregion_core::{
    Diagnostic, GeoId, InstallKind, InstallObservation, InstallPhase, JournalResult, LogEvent,
    LogEventCode, LogLevel, MarketCode, OperationId, RegionSwitchError, RegionSwitchOutcome,
    RegionWriteError, ResolvedProduct, StoreProductId,
};

/// Trace of one guarded operation.
///
/// Every record carries the operation identity that the recovery record and the
/// journal entry also carry, so one attempt can be followed across all three.
pub(super) struct OperationTrace {
    operation_id: OperationId,
}

impl OperationTrace {
    pub(super) const fn new(operation_id: OperationId) -> Self {
        Self { operation_id }
    }

    /// Record that a guarded operation began.
    pub(super) fn started(
        &self,
        product_id: &StoreProductId,
        market: &MarketCode,
        temporary_region: GeoId,
        resumed: bool,
    ) {
        record(&started_event(
            &self.operation_id,
            product_id,
            market,
            temporary_region,
            resumed,
        ));
    }

    /// Record the durable recovery record written before Windows is touched.
    pub(super) fn recovery_record_written(&self, original: GeoId, temporary: GeoId) {
        record(&recovery_record_event(
            &self.operation_id,
            original,
            temporary,
        ));
    }

    /// Record a region switch that reached its mandatory read-back.
    pub(super) fn region_switched(
        &self,
        original: GeoId,
        target: GeoId,
        writer_result: Result<(), RegionWriteError>,
        outcome: RegionSwitchOutcome,
    ) {
        record(&region_switch_event(
            &self.operation_id,
            original,
            target,
            writer_result,
            outcome,
        ));
    }

    /// Record a region switch that never got far enough to be classified.
    pub(super) fn region_switch_failed<E>(&self, target: GeoId, error: &RegionSwitchError<E>) {
        record(&region_switch_failure_event(
            &self.operation_id,
            target,
            error,
        ));
    }

    /// Record the product the catalogue returned under the temporary region.
    pub(super) fn product_resolved(&self, product: &ResolvedProduct) {
        record(&product_resolved_event(&self.operation_id, product));
    }

    /// Record that the catalogue returned no usable product.
    pub(super) fn product_not_resolved(&self, product_id: &StoreProductId, market: &MarketCode) {
        record(&product_not_resolved_event(
            &self.operation_id,
            product_id,
            market,
        ));
    }

    /// Record which backend is about to be asked for which kind of install.
    pub(super) fn backend_selected(&self, backend: &'static str, kind: InstallKind) {
        record(&backend_selected_event(&self.operation_id, backend, kind));
    }

    /// Record that the backend accepted the install request.
    pub(super) fn backend_accepted(&self, native: BackendNativeDetail) {
        record(&backend_exited_event(
            &self.operation_id,
            BackendOutcome::Accepted,
            native,
        ));
    }

    /// Record that the backend refused to start, with the codes behind it.
    pub(super) fn backend_refused(&self, native: BackendNativeDetail) {
        record(&backend_exited_event(
            &self.operation_id,
            BackendOutcome::Refused,
            native,
        ));
    }

    /// Record the result codes of an install the backend ran to its end.
    pub(super) fn backend_finished(&self, native: BackendNativeDetail) {
        record(&backend_exited_event(
            &self.operation_id,
            BackendOutcome::Finished,
            native,
        ));
    }

    /// Record one observation, which the caller emits only when it changed.
    pub(super) fn observation(&self, observation: InstallObservation) {
        record(&observation_event(&self.operation_id, observation));
    }

    /// Record an attempt to put the original region back.
    pub(super) fn region_restore(&self, original: GeoId, early: bool, restored: bool) {
        record(&region_restore_event(
            &self.operation_id,
            original,
            early,
            restored,
        ));
    }

    /// Record the failure the user was shown.
    pub(super) fn diagnostic(&self, diagnostic: &Diagnostic) {
        record(&diagnostic_event(&self.operation_id, diagnostic));
    }

    /// Record whether the operation left an entry in the history.
    pub(super) fn journal_entry(&self, result: JournalResult, written: bool) {
        record(&journal_entry_event(&self.operation_id, result, written));
    }

    /// Record the outcome the operation ended on.
    pub(super) fn finished(&self, result: JournalResult) {
        record(&finished_event(&self.operation_id, result));
    }
}

/// Record a failure that happened before any operation identity existed.
pub(super) fn operation_never_started(diagnostic: &Diagnostic) {
    record(&unattributed_diagnostic_event(diagnostic));
}

fn started_event(
    operation_id: &OperationId,
    product_id: &StoreProductId,
    market: &MarketCode,
    temporary_region: GeoId,
    resumed: bool,
) -> LogEvent {
    LogEvent::new(LogLevel::Info, LogEventCode::OperationStarted)
        .for_operation(operation_id.clone())
        .with_identifier("product", product_id.as_str())
        .with_identifier("market", market.as_str())
        .with_geo_id("to", temporary_region)
        .with_flag("resumed", resumed)
}

fn recovery_record_event(
    operation_id: &OperationId,
    original: GeoId,
    temporary: GeoId,
) -> LogEvent {
    LogEvent::new(LogLevel::Info, LogEventCode::RecoveryRecordWritten)
        .for_operation(operation_id.clone())
        .with_geo_id("from", original)
        .with_geo_id("to", temporary)
}

fn region_switch_event(
    operation_id: &OperationId,
    original: GeoId,
    target: GeoId,
    writer_result: Result<(), RegionWriteError>,
    outcome: RegionSwitchOutcome,
) -> LogEvent {
    let event = LogEvent::new(level_for(outcome), LogEventCode::RegionSwitchAttempted)
        .for_operation(operation_id.clone())
        .with_geo_id("from", original)
        .with_geo_id("to", target)
        .with_token("outcome", outcome_token(outcome));
    let event = match writer_result {
        Ok(()) => event.with_token("writer", "accepted"),
        Err(RegionWriteError::Rejected) => event.with_token("writer", "rejected"),
        Err(RegionWriteError::PlatformFailure(code)) => event
            .with_token("writer", "platform_failure")
            .with_code("native", code),
    };
    match outcome {
        // The read-back is the only fact that matters here, so the region it
        // actually found is named rather than left to be inferred.
        RegionSwitchOutcome::RegionConflict { current_region } => {
            event.with_geo_id("read_back", current_region)
        }
        RegionSwitchOutcome::RegionSwitched => event.with_geo_id("read_back", target),
        RegionSwitchOutcome::OriginalRegionPreserved => event.with_geo_id("read_back", original),
    }
}

fn region_switch_failure_event<E>(
    operation_id: &OperationId,
    target: GeoId,
    error: &RegionSwitchError<E>,
) -> LogEvent {
    LogEvent::new(LogLevel::Error, LogEventCode::RegionSwitchAttempted)
        .for_operation(operation_id.clone())
        .with_geo_id("to", target)
        .with_token("outcome", "not_classified")
        .with_token("stage", switch_failure_token(error))
}

fn product_resolved_event(operation_id: &OperationId, product: &ResolvedProduct) -> LogEvent {
    let event = LogEvent::new(LogLevel::Info, LogEventCode::ProductResolveAttempted)
        .for_operation(operation_id.clone())
        .with_identifier("product", product.product_id.as_str())
        .with_identifier("market", product.market.as_str())
        .with_token("outcome", "resolved")
        .with_token("kind", kind_token(product.install_classification.kind()));
    // The display name is catalogue text, not something the user typed, but it
    // is still text: only the machine-facing identity is recorded.
    match product.package_family_name.as_ref() {
        Some(family) => event.with_identifier("family", family.as_str()),
        None => event,
    }
}

fn product_not_resolved_event(
    operation_id: &OperationId,
    product_id: &StoreProductId,
    market: &MarketCode,
) -> LogEvent {
    LogEvent::new(LogLevel::Warning, LogEventCode::ProductResolveAttempted)
        .for_operation(operation_id.clone())
        .with_identifier("product", product_id.as_str())
        .with_identifier("market", market.as_str())
        .with_token("outcome", "not_resolved")
}

fn backend_selected_event(
    operation_id: &OperationId,
    backend: &'static str,
    kind: InstallKind,
) -> LogEvent {
    LogEvent::new(LogLevel::Info, LogEventCode::BackendSelected)
        .for_operation(operation_id.clone())
        .with_token("backend", backend)
        .with_token("kind", kind_token(kind))
}

/// How far the backend got with the request it was given.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackendOutcome {
    /// The request was accepted and an install is under way.
    Accepted,
    /// The backend refused to start.
    Refused,
    /// An accepted install reached its end and reported its result codes.
    Finished,
}

impl BackendOutcome {
    const fn token(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Refused => "refused",
            Self::Finished => "finished",
        }
    }

    const fn level(self) -> LogLevel {
        match self {
            Self::Accepted | Self::Finished => LogLevel::Info,
            Self::Refused => LogLevel::Error,
        }
    }
}

fn backend_exited_event(
    operation_id: &OperationId,
    outcome: BackendOutcome,
    native: BackendNativeDetail,
) -> LogEvent {
    let mut event = LogEvent::new(outcome.level(), LogEventCode::BackendExited)
        .for_operation(operation_id.clone())
        .with_token("outcome", outcome.token());
    if let Some(hresult) = native.hresult {
        event = event.with_code("native", hresult);
    }
    if let Some(status) = native.status {
        event = event.with_code("status", status);
    }
    if let Some(extended) = native.extended {
        event = event.with_code("extended", extended);
    }
    if let Some(installer) = native.installer {
        // The installer's own code is unsigned; it is recorded as the API
        // reports it rather than reinterpreted.
        event = event.with_identifier("installer", &installer.to_string());
    }
    event
}

fn observation_event(operation_id: &OperationId, observation: InstallObservation) -> LogEvent {
    let event = LogEvent::new(LogLevel::Info, LogEventCode::InstallObservation)
        .for_operation(operation_id.clone());
    match observation {
        InstallObservation::Installing { phase, progress } => {
            let event = event
                .with_token("observation", "installing")
                .with_token("phase", phase_token(phase));
            match progress {
                Some(progress) => event.with_percent("percent", progress.percent()),
                None => event,
            }
        }
        InstallObservation::Completed => event.with_token("observation", "completed"),
        InstallObservation::CompletionUncertain => {
            event.with_token("observation", "completion_uncertain")
        }
    }
}

fn region_restore_event(
    operation_id: &OperationId,
    original: GeoId,
    early: bool,
    restored: bool,
) -> LogEvent {
    let level = if restored {
        LogLevel::Info
    } else {
        LogLevel::Error
    };
    LogEvent::new(level, LogEventCode::RegionRestoreAttempted)
        .for_operation(operation_id.clone())
        .with_geo_id("to", original)
        .with_flag("early", early)
        .with_token("outcome", if restored { "restored" } else { "failed" })
}

fn diagnostic_event(operation_id: &OperationId, diagnostic: &Diagnostic) -> LogEvent {
    attach_diagnostic(
        LogEvent::new(LogLevel::Error, LogEventCode::DiagnosticReported)
            .for_operation(operation_id.clone()),
        diagnostic,
    )
}

fn unattributed_diagnostic_event(diagnostic: &Diagnostic) -> LogEvent {
    attach_diagnostic(
        LogEvent::new(LogLevel::Error, LogEventCode::DiagnosticReported),
        diagnostic,
    )
}

fn attach_diagnostic(event: LogEvent, diagnostic: &Diagnostic) -> LogEvent {
    let event = event.with_token("code", diagnostic.code().as_str());
    let Some(technical) = diagnostic.technical() else {
        return event;
    };
    let event = event.with_token("variant", technical.variant);
    match technical.native_code {
        Some(code) => event.with_code("native", code),
        None => event,
    }
}

fn journal_entry_event(
    operation_id: &OperationId,
    result: JournalResult,
    written: bool,
) -> LogEvent {
    let level = if written {
        LogLevel::Info
    } else {
        LogLevel::Warning
    };
    LogEvent::new(level, LogEventCode::JournalEntryWritten)
        .for_operation(operation_id.clone())
        .with_token("result", result_token(result))
        .with_flag("written", written)
}

fn finished_event(operation_id: &OperationId, result: JournalResult) -> LogEvent {
    LogEvent::new(level_for_result(result), LogEventCode::OperationFinished)
        .for_operation(operation_id.clone())
        .with_token("result", result_token(result))
}

const fn level_for(outcome: RegionSwitchOutcome) -> LogLevel {
    match outcome {
        RegionSwitchOutcome::RegionSwitched => LogLevel::Info,
        RegionSwitchOutcome::OriginalRegionPreserved => LogLevel::Warning,
        RegionSwitchOutcome::RegionConflict { .. } => LogLevel::Error,
    }
}

const fn level_for_result(result: JournalResult) -> LogLevel {
    match result {
        JournalResult::Installed | JournalResult::HandedOffToInstaller => LogLevel::Info,
        JournalResult::CompletionUncertain | JournalResult::RecoveryRequired => LogLevel::Warning,
        JournalResult::Failed => LogLevel::Error,
    }
}

const fn outcome_token(outcome: RegionSwitchOutcome) -> &'static str {
    match outcome {
        RegionSwitchOutcome::RegionSwitched => "region_switched",
        RegionSwitchOutcome::OriginalRegionPreserved => "original_region_preserved",
        RegionSwitchOutcome::RegionConflict { .. } => "region_conflict",
    }
}

const fn switch_failure_token<E>(error: &RegionSwitchError<E>) -> &'static str {
    match error {
        RegionSwitchError::ReadOriginal(_) => "read_original",
        RegionSwitchError::PendingSave(_) => "pending_save",
        RegionSwitchError::ReadBack(_) => "read_back",
        RegionSwitchError::PendingCleanup(_) => "pending_cleanup",
    }
}

const fn kind_token(kind: InstallKind) -> &'static str {
    match kind {
        InstallKind::Unknown => "unknown",
        InstallKind::Packaged => "packaged",
        InstallKind::Win32 => "win32",
    }
}

const fn phase_token(phase: InstallPhase) -> &'static str {
    match phase {
        InstallPhase::Queued => "queued",
        InstallPhase::Installing => "installing",
        InstallPhase::PostInstall => "post_install",
    }
}

const fn result_token(result: JournalResult) -> &'static str {
    match result {
        JournalResult::Installed => "installed",
        // An installation never reaches this outcome; the handoff path owns it.
        JournalResult::HandedOffToInstaller => "handed_off_to_installer",
        JournalResult::CompletionUncertain => "completion_uncertain",
        JournalResult::RecoveryRequired => "recovery_required",
        JournalResult::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winstoreregion_core::{
        DiagnosticCode, InstallClassification, InstallProgress, PackageFamilyName,
        ResolveRequestId, ResolverSource, TechnicalDetail, UtcTimestamp,
    };

    fn operation_id() -> OperationId {
        OperationId::new("op-7").expect("non-empty operation identifier")
    }

    fn timestamp() -> UtcTimestamp {
        UtcTimestamp::parse("2026-08-20T09:00:00Z").expect("valid UTC timestamp")
    }

    fn geo(value: i32) -> GeoId {
        GeoId::new(value).expect("positive GeoId")
    }

    fn product_id() -> StoreProductId {
        StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID")
    }

    fn market() -> MarketCode {
        MarketCode::parse("US").expect("valid market")
    }

    #[test]
    fn a_region_switch_records_both_regions_the_writer_and_the_read_back() {
        let event = region_switch_event(
            &operation_id(),
            geo(203),
            geo(244),
            Ok(()),
            RegionSwitchOutcome::RegionSwitched,
        );

        assert_eq!(
            event.to_line(&timestamp()),
            "2026-08-20T09:00:00Z INFO  region_switch_attempted op=op-7 from=203 to=244 \
             outcome=region_switched writer=accepted read_back=244"
        );
    }

    #[test]
    fn a_conflicting_read_back_names_the_region_that_was_actually_found() {
        let event = region_switch_event(
            &operation_id(),
            geo(203),
            geo(244),
            Err(RegionWriteError::PlatformFailure(-2_147_024_891)),
            RegionSwitchOutcome::RegionConflict {
                current_region: geo(84),
            },
        );
        let line = event.to_line(&timestamp());

        assert!(line.starts_with("2026-08-20T09:00:00Z ERROR region_switch_attempted op=op-7"));
        assert!(line.contains("writer=platform_failure native=-2147024891"));
        assert!(line.contains("read_back=84"));
    }

    #[test]
    fn a_backend_refusal_keeps_every_native_number_it_was_given() {
        let event = backend_exited_event(
            &operation_id(),
            BackendOutcome::Refused,
            BackendNativeDetail {
                hresult: Some(-1_978_335_216),
                status: Some(8),
                extended: Some(-2_147_024_809),
                installer: Some(1603),
            },
        );
        let line = event.to_line(&timestamp());

        assert!(line.contains("outcome=refused"));
        assert!(line.contains("native=-1978335216"));
        assert!(line.contains("status=8"));
        assert!(line.contains("extended=-2147024809"));
        assert!(line.contains("installer=1603"));
    }

    #[test]
    fn an_observation_records_the_phase_and_only_a_percentage_that_exists() {
        let with_percent = observation_event(
            &operation_id(),
            InstallObservation::Installing {
                phase: InstallPhase::Installing,
                progress: InstallProgress::new(42),
            },
        );
        let without_percent = observation_event(
            &operation_id(),
            InstallObservation::Installing {
                phase: InstallPhase::Queued,
                progress: None,
            },
        );

        assert!(
            with_percent
                .to_line(&timestamp())
                .contains("observation=installing phase=installing percent=42")
        );
        assert!(
            without_percent
                .to_line(&timestamp())
                .ends_with("observation=installing phase=queued")
        );
    }

    #[test]
    fn a_diagnostic_is_recorded_by_code_variant_and_native_code() {
        let event = diagnostic_event(
            &operation_id(),
            &Diagnostic::with_detail(
                DiagnosticCode::TemporaryRegionNotApplied,
                TechnicalDetail {
                    variant: "InstallBackendError::TemporaryRegionNotApplied",
                    native_code: Some(-1_978_335_216),
                    operation_id: None,
                },
            ),
        );
        let line = event.to_line(&timestamp());

        assert!(line.contains("code=temporary_region_not_applied"));
        assert!(line.contains("variant=InstallBackendError::TemporaryRegionNotApplied"));
        assert!(line.contains("native=-1978335216"));
    }

    #[test]
    fn a_resolved_product_is_recorded_by_identity_never_by_its_display_name() {
        let product = ResolvedProduct {
            request_id: ResolveRequestId::new(1),
            product_id: product_id(),
            market: market(),
            display_name: "Hulu: Stream shows".to_owned(),
            package_family_name: PackageFamilyName::parse("HuluLLC.HuluPlus_fphbd3rr8ppte"),
            publisher: Some("Hulu, LLC".to_owned()),
            icon: None,
            install_classification: InstallClassification::default(),
            version: None,
            size_bytes: None,
            resolver_source: ResolverSource::WinGetMsStore,
        };

        let line = product_resolved_event(&operation_id(), &product).to_line(&timestamp());

        assert!(line.contains("product=9WZDNCRFJ3L1 market=US outcome=resolved kind=unknown"));
        assert!(line.contains("family=HuluLLC.HuluPlus_fphbd3rr8ppte"));
        assert!(
            !line.contains("Hulu:"),
            "a log line must not carry free text"
        );
    }

    #[test]
    fn an_operation_that_never_reached_its_identity_still_records_its_cause() {
        let line =
            unattributed_diagnostic_event(&Diagnostic::new(DiagnosticCode::BackendUnavailable))
                .to_line(&timestamp());

        assert_eq!(
            line,
            "2026-08-20T09:00:00Z ERROR diagnostic_reported op=- code=backend_unavailable"
        );
    }

    #[test]
    fn the_start_and_the_end_of_an_operation_are_both_recorded() {
        let started = started_event(&operation_id(), &product_id(), &market(), geo(244), false)
            .to_line(&timestamp());
        let finished = finished_event(&operation_id(), JournalResult::Failed).to_line(&timestamp());

        assert_eq!(
            started,
            "2026-08-20T09:00:00Z INFO  operation_started op=op-7 product=9WZDNCRFJ3L1 \
             market=US to=244 resumed=no"
        );
        assert_eq!(
            finished,
            "2026-08-20T09:00:00Z ERROR operation_finished op=op-7 result=failed"
        );
    }
}
