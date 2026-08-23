//! Presentation of the operation journal.

use crate::gui::diagnostic::{diagnostic_details, diagnostic_message, journal_diagnostic};
use crate::gui::state::{AppState, JournalView};
use crate::gui::strings::{Language, Strings, fill};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    LVIF_TEXT, LVITEMW, LVM_DELETEALLITEMS, LVM_INSERTITEMW, LVM_SETITEMW,
};
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::core::{HSTRING, PWSTR};
use winstoreregion_core::{InstallKind, JournalRecord, JournalResult, JournalSubject};

pub(super) fn journal_details(state: &AppState, strings: &Strings) -> String {
    match &state.journal {
        JournalView::Loading => strings.journal_loading.to_owned(),
        JournalView::Ready(records) if records.is_empty() => strings.journal_empty.to_owned(),
        JournalView::Ready(_) => state.selected_journal_record().map_or_else(
            || strings.journal_empty.to_owned(),
            |record| journal_record_details(record, state.language),
        ),
        JournalView::Failed(error) => {
            let preserved = state.language.strings().journal_journal_details;
            format!(
                "{} {preserved}

{}",
                diagnostic_message(state.language, journal_diagnostic(*error).code()),
                diagnostic_details(state.language, &journal_diagnostic(*error)),
            )
        }
    }
}

/// One row of the journal table, left to right.
///
/// The outcome has a column of its own rather than a prefix, because "needs
/// attention" is the reason a person opens this tab and reading it should not
/// depend on the width of the name beside it.
pub(super) fn journal_row(record: &JournalRecord, language: Language) -> [String; 4] {
    // A handoff entry has no Product ID to name, so the file's digest is what
    // tells two of them apart.
    let identity = record.product_id().map_or_else(
        || {
            record
                .subject()
                .installer_file()
                .map_or_else(String::new, |file| format!("sha256:{}", file.sha256()))
        },
        |product_id| product_id.as_str().to_owned(),
    );
    [
        record.installed_at().as_str().to_owned(),
        record.display_name().to_owned(),
        identity,
        result_label(language, record.result()).to_owned(),
    ]
}

/// The localized name of one durable outcome.
///
/// A handoff has its own line and deliberately does not read as an
/// installation: nothing in that operation established one.
const fn result_label(language: Language, result: JournalResult) -> &'static str {
    match result {
        JournalResult::Installed => language.strings().journal_result_installed,
        JournalResult::HandedOffToInstaller => {
            language.strings().journal_result_handed_off_to_installer
        }
        JournalResult::CompletionUncertain => {
            language.strings().journal_result_completion_uncertain
        }
        JournalResult::RecoveryRequired => language.strings().journal_result_recovery_required,
        JournalResult::Failed => language.strings().journal_result_failed,
    }
}

/// Everything one entry can honestly say about itself.
///
/// The two subjects produce different blocks because they know different
/// things. A file entry has no product name, publisher, version or package
/// identity, and it says so once rather than filling six fields with "unknown".
fn journal_record_details(record: &JournalRecord, language: Language) -> String {
    let strings = language.strings();
    let subject = match record.subject() {
        JournalSubject::Product(product) => {
            let kind = match product.install_kind() {
                InstallKind::Packaged => "Packaged",
                InstallKind::Win32 => "Win32",
                InstallKind::Unknown => strings.journal_details_kind_unknown,
            };
            fill(
                strings.journal_details_product,
                &[
                    ("name", product.name()),
                    ("id", product.product_id().as_str()),
                    ("kind", kind),
                    (
                        "publisher",
                        product
                            .publisher()
                            .unwrap_or(strings.journal_details_publisher_not_provided),
                    ),
                    (
                        "version",
                        product
                            .installed_version()
                            .unwrap_or(strings.journal_details_version_not_confirmed),
                    ),
                    (
                        "family",
                        product
                            .package_family_name()
                            .map_or(strings.journal_details_family_not_confirmed, |value| {
                                value.as_str()
                            }),
                    ),
                ],
            )
        }
        JournalSubject::InstallerFile(file) => fill(
            strings.journal_details_file,
            &[("name", file.file_name()), ("sha256", file.sha256())],
        ),
    };
    let geo_id = record.region().value().to_string();
    let backend = record.backend().to_string();
    let common = fill(
        strings.journal_details_common,
        &[
            ("date", record.installed_at().as_str()),
            ("geo_id", &geo_id),
            ("operation", record.operation_id().as_str()),
            ("backend", &backend),
            ("result", result_label(language, record.result())),
        ],
    );
    format!("{subject}\n{common}")
}

#[allow(unsafe_code)]
pub(super) unsafe fn rebuild_journal_list(window: HWND, state: &AppState) {
    let _ = unsafe { SendMessageW(window, LVM_DELETEALLITEMS, None, None) };
    let JournalView::Ready(records) = &state.journal else {
        return;
    };
    for (index, record) in records.iter().enumerate() {
        unsafe { insert_row(window, index, &journal_row(record, state.language)) };
    }
}

/// Put one row into a report-mode list, cell by cell.
#[allow(unsafe_code)]
pub(super) unsafe fn insert_row(list: HWND, index: usize, row: &[String; 4]) {
    let first = HSTRING::from(row[0].as_str());
    let mut item = LVITEMW {
        mask: LVIF_TEXT,
        iItem: i32::try_from(index).unwrap_or(i32::MAX),
        pszText: PWSTR(first.as_ptr().cast_mut()),
        ..LVITEMW::default()
    };
    let _ = unsafe {
        SendMessageW(
            list,
            LVM_INSERTITEMW,
            Some(WPARAM(0)),
            Some(LPARAM(std::ptr::from_mut(&mut item) as isize)),
        )
    };
    for (column, text) in row.iter().enumerate().skip(1) {
        let text = HSTRING::from(text.as_str());
        let mut cell = LVITEMW {
            mask: LVIF_TEXT,
            iItem: i32::try_from(index).unwrap_or(i32::MAX),
            iSubItem: i32::try_from(column).unwrap_or(0),
            pszText: PWSTR(text.as_ptr().cast_mut()),
            ..LVITEMW::default()
        };
        let _ = unsafe {
            SendMessageW(
                list,
                LVM_SETITEMW,
                Some(WPARAM(0)),
                Some(LPARAM(std::ptr::from_mut(&mut cell) as isize)),
            )
        };
    }
}
