//! Presentation of a classified failure: one sentence plus separate details.
//!
//! The primary message is always a localized sentence chosen by
//! [`DiagnosticCode`]. Technical evidence never reaches it; it lives in the
//! details block, which the user can copy for a report.

use crate::gui::dialogs::ClipboardError;
use crate::gui::state::FileSelectionError;
use crate::gui::strings::Language;
use crate::platform::storage::JournalStoreError;
use crate::platform::stub::StubInspectionError;
use winstoreregion_core::{
    BackendPrerequisite, Diagnostic, DiagnosticCode, PrerequisiteReport, PrerequisiteState,
    TechnicalDetail,
};

/// Classify a journal failure without exposing its internal shape.
pub(super) fn journal_diagnostic(error: JournalStoreError) -> Diagnostic {
    let (variant, native_code) = match error {
        JournalStoreError::Read { os_error } => ("JournalStoreError::Read", os_error),
        JournalStoreError::Write { os_error } => ("JournalStoreError::Write", os_error),
        JournalStoreError::Publish { os_error } => ("JournalStoreError::Publish", os_error),
        JournalStoreError::Verification => ("JournalStoreError::Verification", None),
        JournalStoreError::Format(_) => ("JournalStoreError::Format", None),
        JournalStoreError::MissingRecord => ("JournalStoreError::MissingRecord", None),
    };
    Diagnostic::with_detail(
        DiagnosticCode::JournalUnavailable,
        TechnicalDetail {
            variant,
            native_code,
            operation_id: None,
        },
    )
}

/// Classify a clipboard failure.
pub(super) fn clipboard_diagnostic(error: ClipboardError) -> Diagnostic {
    let variant = match error {
        ClipboardError::Open => "ClipboardError::Open",
        ClipboardError::Allocate => "ClipboardError::Allocate",
        ClipboardError::Lock => "ClipboardError::Lock",
        ClipboardError::Empty => "ClipboardError::Empty",
        ClipboardError::Set => "ClipboardError::Set",
    };
    Diagnostic::with_detail(
        DiagnosticCode::ClipboardUnavailable,
        TechnicalDetail {
            variant,
            native_code: None,
            operation_id: None,
        },
    )
}

/// Classify a read-only stub inspection failure.
pub(super) const fn stub_diagnostic(error: StubInspectionError) -> Diagnostic {
    let variant = match error {
        StubInspectionError::Io => "StubInspectionError::Io",
        StubInspectionError::ChangedDuringInspection => {
            "StubInspectionError::ChangedDuringInspection"
        }
    };
    Diagnostic::with_detail(
        DiagnosticCode::InstallerFileUnreadable,
        TechnicalDetail {
            variant,
            native_code: None,
            operation_id: None,
        },
    )
}

/// Classify a rejected file choice.
pub(super) fn file_selection_diagnostic(error: FileSelectionError) -> Diagnostic {
    let variant = match error {
        FileSelectionError::EmptyPath => "FileSelectionError::EmptyPath",
        FileSelectionError::UnsupportedExtension => "FileSelectionError::UnsupportedExtension",
        FileSelectionError::NotAFile => "FileSelectionError::NotAFile",
        FileSelectionError::LinkOrDevicePath => "FileSelectionError::LinkOrDevicePath",
        FileSelectionError::MultipleFiles => "FileSelectionError::MultipleFiles",
    };
    Diagnostic::with_detail(
        DiagnosticCode::InstallerFileNotSupported,
        TechnicalDetail {
            variant,
            native_code: None,
            operation_id: None,
        },
    )
}

/// The one sentence shown as the primary message.
///
/// The text itself lives in the language files; this only says which key
/// belongs to which cause, and the compiler checks that every cause has one.
pub(super) const fn diagnostic_message(language: Language, code: DiagnosticCode) -> &'static str {
    let strings = language.strings();
    match code {
        DiagnosticCode::BackendFailed => strings.diagnostic_backend_failed,
        DiagnosticCode::BackendReportedUnknownFailure => {
            strings.diagnostic_backend_reported_unknown_failure
        }
        DiagnosticCode::BackendUnavailable => strings.diagnostic_backend_unavailable,
        DiagnosticCode::ClipboardUnavailable => strings.diagnostic_clipboard_unavailable,
        DiagnosticCode::CompletionUncertain => strings.diagnostic_completion_uncertain,
        DiagnosticCode::InstallKindNotSupported => strings.diagnostic_install_kind_not_supported,
        DiagnosticCode::InstallerFileNotAStoreStub => {
            strings.diagnostic_installer_file_not_a_store_stub
        }
        DiagnosticCode::InstallerFileNotAdmitted => strings.diagnostic_installer_file_not_admitted,
        DiagnosticCode::InstallerFileNotSupported => {
            strings.diagnostic_installer_file_not_supported
        }
        DiagnosticCode::InstallerFileUnreadable => strings.diagnostic_installer_file_unreadable,
        DiagnosticCode::InstallerLaunchRejected => strings.diagnostic_installer_launch_rejected,
        DiagnosticCode::JournalUnavailable => strings.diagnostic_journal_unavailable,
        DiagnosticCode::MicrosoftAccountRequired => strings.diagnostic_microsoft_account_required,
        DiagnosticCode::MicrosoftStoreUnavailable => strings.diagnostic_microsoft_store_unavailable,
        DiagnosticCode::MsStoreSourceTermsNotAccepted => {
            strings.diagnostic_ms_store_source_terms_not_accepted
        }
        DiagnosticCode::MsStoreSourceUnavailable => strings.diagnostic_ms_store_source_unavailable,
        DiagnosticCode::NetworkUnavailable => strings.diagnostic_network_unavailable,
        DiagnosticCode::OperationSequenceViolated => strings.diagnostic_operation_sequence_violated,
        DiagnosticCode::ProductAlreadyInstalled => strings.diagnostic_product_already_installed,
        DiagnosticCode::ProductAlreadyInstalling => strings.diagnostic_product_already_installing,
        DiagnosticCode::ProductNotFound => strings.diagnostic_product_not_found,
        DiagnosticCode::ProductNotOnThisDevice => strings.diagnostic_product_not_on_this_device,
        DiagnosticCode::ProductUnavailableInRegion => {
            strings.diagnostic_product_unavailable_in_region
        }
        DiagnosticCode::RecoveryRecordUnreadable => strings.diagnostic_recovery_record_unreadable,
        DiagnosticCode::RegionChangeRejected => strings.diagnostic_region_change_rejected,
        DiagnosticCode::RegionConflict => strings.diagnostic_region_conflict,
        DiagnosticCode::RegionUnavailable => strings.diagnostic_region_unavailable,
        DiagnosticCode::RegionUnchanged => strings.diagnostic_region_unchanged,
        DiagnosticCode::ResolverAnswerMismatch => strings.diagnostic_resolver_answer_mismatch,
        DiagnosticCode::ResolverUnavailable => strings.diagnostic_resolver_unavailable,
        DiagnosticCode::RestoreFailed => strings.diagnostic_restore_failed,
        DiagnosticCode::StoreInputNotUnderstood => strings.diagnostic_store_input_not_understood,
        DiagnosticCode::StorePageLaunchRejected => strings.diagnostic_store_page_launch_rejected,
        DiagnosticCode::StoreRefusedWithoutReason => {
            strings.diagnostic_store_refused_without_reason
        }
        DiagnosticCode::TemporaryRegionNotApplied => {
            strings.diagnostic_temporary_region_not_applied
        }
        DiagnosticCode::UnsupportedMarket => strings.diagnostic_unsupported_market,
        DiagnosticCode::WinGetMissing => strings.diagnostic_win_get_missing,
    }
}

pub(super) const fn prerequisite_message(
    language: Language,
    prerequisite: BackendPrerequisite,
    state: PrerequisiteState,
) -> &'static str {
    let strings = language.strings();
    match (prerequisite, state) {
        (BackendPrerequisite::AppInstaller, PrerequisiteState::Missing) => {
            strings.prerequisite_app_installer_missing
        }
        (BackendPrerequisite::AppInstaller, _) => strings.prerequisite_app_installer_unknown,
        (BackendPrerequisite::MicrosoftStore, PrerequisiteState::Missing) => {
            strings.prerequisite_microsoft_store_missing
        }
        (BackendPrerequisite::MicrosoftStore, _) => strings.prerequisite_microsoft_store_unknown,
        (BackendPrerequisite::ComMetadata, PrerequisiteState::Missing) => {
            strings.prerequisite_com_metadata_missing
        }
        (BackendPrerequisite::ComMetadata, _) => strings.prerequisite_com_metadata_unknown,
    }
}

/// Full status text for every unmet prerequisite, or `None` when all are met.
pub(super) fn prerequisite_status(
    language: Language,
    report: PrerequisiteReport,
) -> Option<String> {
    let unsatisfied = report.unsatisfied();
    if unsatisfied.is_empty() {
        return None;
    }
    let heading = language.strings().prerequisite_heading;
    let lines: Vec<&str> = unsatisfied
        .iter()
        .map(|prerequisite| {
            prerequisite_message(language, *prerequisite, report.state(*prerequisite))
        })
        .collect();
    Some(format!(
        "{heading}

{}",
        lines.join(
            "
"
        )
    ))
}

/// The copyable technical block, shown only in a details area.
pub(super) fn diagnostic_details(language: Language, diagnostic: &Diagnostic) -> String {
    let heading = language.strings().diagnostic_details_heading;
    format!(
        "{heading}\n\n{}\nversion={}",
        diagnostic.technical_block(),
        env!("CARGO_PKG_VERSION"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use winstoreregion_core::{DiagnosticAction, RegionReadError};

    const ALL_CODES: [DiagnosticCode; 30] = [
        DiagnosticCode::StoreInputNotUnderstood,
        DiagnosticCode::ProductNotFound,
        DiagnosticCode::ProductUnavailableInRegion,
        DiagnosticCode::ResolverUnavailable,
        DiagnosticCode::UnsupportedMarket,
        DiagnosticCode::ResolverAnswerMismatch,
        DiagnosticCode::RegionUnavailable,
        DiagnosticCode::RegionUnchanged,
        DiagnosticCode::RegionChangeRejected,
        DiagnosticCode::RegionConflict,
        DiagnosticCode::RecoveryRecordUnreadable,
        DiagnosticCode::RestoreFailed,
        DiagnosticCode::JournalUnavailable,
        DiagnosticCode::InstallerFileUnreadable,
        DiagnosticCode::InstallerFileNotAStoreStub,
        DiagnosticCode::InstallerFileNotSupported,
        DiagnosticCode::InstallerFileNotAdmitted,
        DiagnosticCode::InstallerLaunchRejected,
        DiagnosticCode::StorePageLaunchRejected,
        DiagnosticCode::ClipboardUnavailable,
        DiagnosticCode::BackendUnavailable,
        DiagnosticCode::BackendFailed,
        DiagnosticCode::CompletionUncertain,
        DiagnosticCode::WinGetMissing,
        DiagnosticCode::MsStoreSourceUnavailable,
        DiagnosticCode::MsStoreSourceTermsNotAccepted,
        DiagnosticCode::MicrosoftStoreUnavailable,
        DiagnosticCode::NetworkUnavailable,
        DiagnosticCode::MicrosoftAccountRequired,
        DiagnosticCode::StoreRefusedWithoutReason,
    ];

    #[test]
    fn every_cause_has_distinct_text_in_both_languages() {
        for language in [Language::Russian, Language::English] {
            let mut seen = Vec::new();
            for code in ALL_CODES {
                let text = diagnostic_message(language, code);
                assert!(!text.is_empty(), "{code:?} has no text");
                assert!(
                    !seen.contains(&text),
                    "{code:?} reuses the text of another cause"
                );
                seen.push(text);
            }
        }
    }

    #[test]
    fn a_primary_message_never_carries_technical_evidence() {
        let diagnostic = Diagnostic::from(RegionReadError::InvalidGeoId(-5));
        let message = diagnostic_message(Language::Russian, diagnostic.code());

        assert!(!message.contains("RegionReadError"));
        assert!(!message.contains("InvalidGeoId"));
        assert!(!message.contains("-5"));
        // The evidence still exists, just not in the sentence the user reads.
        assert!(diagnostic_details(Language::Russian, &diagnostic).contains("RegionReadError"));
    }

    #[test]
    fn local_failures_are_classified_without_leaking_their_internal_shape() {
        for (diagnostic, expected) in [
            (
                journal_diagnostic(JournalStoreError::Verification),
                DiagnosticCode::JournalUnavailable,
            ),
            (
                clipboard_diagnostic(ClipboardError::Open),
                DiagnosticCode::ClipboardUnavailable,
            ),
            (
                stub_diagnostic(StubInspectionError::ChangedDuringInspection),
                DiagnosticCode::InstallerFileUnreadable,
            ),
            (
                file_selection_diagnostic(FileSelectionError::MultipleFiles),
                DiagnosticCode::InstallerFileNotSupported,
            ),
        ] {
            assert_eq!(diagnostic.code(), expected);
            assert!(diagnostic.technical().is_some());
            assert!(
                !diagnostic_message(Language::English, diagnostic.code()).contains("::"),
                "a primary sentence must not name an internal variant"
            );
        }
    }

    #[test]
    fn details_are_copyable_and_name_the_application_version() {
        let diagnostic = journal_diagnostic(JournalStoreError::Read { os_error: Some(5) });
        let details = diagnostic_details(Language::English, &diagnostic);

        assert!(details.contains("code=journal_unavailable"));
        assert!(details.contains("variant=JournalStoreError::Read"));
        assert!(details.contains("native=0x00000005 (5)"));
        assert!(details.contains(concat!("version=", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn no_presentation_module_puts_debug_output_into_user_text() {
        // `{error:?}` in a user-facing string is the exact defect this block
        // exists to remove; a barrier keeps it from coming back.
        let sources = [
            ("command.rs", include_str!("command.rs")),
            ("journal.rs", include_str!("journal.rs")),
            ("menu.rs", include_str!("menu.rs")),
            ("recovery.rs", include_str!("recovery.rs")),
            ("render.rs", include_str!("render.rs")),
            ("window.rs", include_str!("window.rs")),
            ("dialogs.rs", include_str!("dialogs.rs")),
        ];
        let forbidden = ["{error:", "?}"].concat();
        for (name, source) in sources {
            assert!(
                !source.contains(&forbidden),
                "{name} formats an error with Debug into user-visible text"
            );
        }
    }

    #[test]
    fn a_missing_microsoft_store_is_never_offered_as_a_download() {
        // There is no official standalone installer for Microsoft Store, so the
        // text must not imply one exists.
        for language in [Language::Russian, Language::English] {
            let text = prerequisite_message(
                language,
                BackendPrerequisite::MicrosoftStore,
                PrerequisiteState::Missing,
            );
            assert!(!text.contains("http"), "the text must not offer a link");
            assert!(!text.contains("apps.microsoft.com"));
        }
        assert_eq!(BackendPrerequisite::MicrosoftStore.remedy().page(), None);
    }

    #[test]
    fn unmet_prerequisites_are_all_listed_and_a_satisfied_machine_says_nothing() {
        let blocked = PrerequisiteReport::new(
            PrerequisiteState::Missing,
            PrerequisiteState::Unknown,
            PrerequisiteState::Satisfied,
        );
        let text = prerequisite_status(Language::Russian, blocked)
            .expect("an unsatisfied machine must explain itself");
        assert!(text.contains("App Installer"));
        assert!(text.contains("Microsoft Store"));

        assert_eq!(
            prerequisite_status(
                Language::Russian,
                PrerequisiteReport::new(
                    PrerequisiteState::Satisfied,
                    PrerequisiteState::Satisfied,
                    PrerequisiteState::Satisfied
                )
            ),
            None
        );
    }

    #[test]
    fn a_notice_never_offers_a_recovery_action_it_cannot_perform() {
        assert!(
            !DiagnosticCode::ClipboardUnavailable
                .permitted_actions()
                .contains(&DiagnosticAction::ResolveRecovery)
        );
    }
}
