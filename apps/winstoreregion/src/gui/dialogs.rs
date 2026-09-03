//! Modal dialogs, the file picker, and clipboard access.

use crate::gui::command::validate_installer_file;
use crate::gui::direction::message_box_direction;
use crate::gui::state::{FileSelectionError, ModalScope};
use crate::gui::strings::{Language, fill};
use std::mem::size_of;
use std::path::PathBuf;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowTextLengthW, GetWindowTextW, IDYES, MB_DEFBUTTON2, MB_ICONINFORMATION, MB_ICONWARNING,
    MB_OK, MB_YESNO, MessageBoxW,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};
use winstoreregion_core::{
    APPLICATION_NAME, StoreProductId, StubInspection, StubInspectionWarning,
};

/// Opens the native picker only for the inspection-only `.exe` source.
/// Cancelling the dialog deliberately leaves both source drafts untouched.
#[allow(unsafe_code)]
pub(super) unsafe fn choose_installer_file(
    window: HWND,
    language: Language,
) -> Option<std::result::Result<PathBuf, FileSelectionError>> {
    // The dialog wants label and pattern pairs separated by NUL and closed by
    // two of them. That is a structure, not a sentence: the labels are
    // translated, the separators belong to the API and stay here.
    let strings = language.strings();
    let filter_text = format!(
        "{}\0*.exe\0{}\0*.*\0\0",
        strings.dialog_filter_executables, strings.dialog_filter_all_files
    );
    let filter = filter_text.encode_utf16().collect::<Vec<_>>();
    let mut file = vec![0_u16; 32_768];
    let mut dialog = OPENFILENAMEW {
        lStructSize: u32::try_from(size_of::<OPENFILENAMEW>()).unwrap_or_default(),
        hwndOwner: window,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: u32::try_from(file.len()).unwrap_or_default(),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    let chosen = {
        let _modal = ModalScope::enter();
        unsafe { GetOpenFileNameW(&raw mut dialog) }.as_bool()
    };
    if !chosen {
        return None;
    }
    let end = file
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(file.len());
    Some(validate_installer_file(PathBuf::from(
        String::from_utf16_lossy(&file[..end]),
    )))
}

/// Ask before the one thing this product does that runs foreign code.
///
/// Everything the answer depends on is on screen at once: which file, who
/// signed it, its digest, which region will be held, and what the application
/// will and will not know afterwards. The default answer is no.
///
/// Returns whether the user agreed. Nothing has been changed when it returns.
#[allow(unsafe_code)]
pub(super) unsafe fn confirm_installer_handoff(
    window: HWND,
    language: Language,
    file_name: &str,
    publisher: &str,
    sha256: &str,
    region: &str,
) -> bool {
    let strings = language.strings();
    // The button that ends the handoff is named by its own caption rather than
    // quoted in the text, so renaming it cannot leave this dialog telling the
    // user to press something that is no longer there.
    let message = HSTRING::from(fill(
        strings.dialog_confirm_handoff,
        &[
            ("file", file_name),
            ("publisher", publisher),
            ("sha256", sha256),
            ("region", region),
            ("button", strings.restore),
        ],
    ));
    let title = HSTRING::from(APPLICATION_NAME);
    let _modal = ModalScope::enter();
    let answer = unsafe {
        MessageBoxW(
            Some(window),
            &message,
            &title,
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | message_box_direction(language),
        )
    };
    answer == IDYES
}

#[allow(unsafe_code)]
pub(super) unsafe fn show_stub_details(language: Language, inspection: &StubInspection) {
    let signature = format!("{:?}", inspection.signature_status);
    let status = format!("{:?}", inspection.store_stub_status);
    let strings = language.strings();
    // A warning nobody has written a sentence for is still shown, as its own
    // name. An unnamed warning would be a warning silently dropped.
    let warnings = inspection
        .warnings
        .iter()
        .map(|warning| match warning {
            StubInspectionWarning::NativeVerificationCode(_) => strings
                .dialog_stub_warning_native_verification_code
                .to_owned(),
            StubInspectionWarning::ProductIdNotAvailable => strings
                .dialog_stub_warning_product_id_not_available
                .to_owned(),
            StubInspectionWarning::SignerDataUnavailable => strings
                .dialog_stub_warning_signer_data_unavailable
                .to_owned(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let signer = inspection.signer.as_ref().map_or_else(
        || strings.dialog_show_stub_details.to_owned(),
        |signer| {
            fill(
                strings.dialog_stub_signer,
                &[
                    ("subject", &signer.subject),
                    ("issuer", &signer.issuer),
                    ("serial", &signer.serial_number),
                    ("thumbprint", &signer.sha256_thumbprint),
                    ("valid_from", &signer.valid_from),
                    ("valid_to", &signer.valid_to),
                    (
                        "eku",
                        if signer.has_code_signing_eku {
                            strings.dialog_stub_eku_yes
                        } else {
                            strings.dialog_stub_eku_no
                        },
                    ),
                ],
            )
        },
    );
    let byte_len = inspection.file_identity.byte_len.to_string();
    let message = fill(
        strings.dialog_stub_details,
        &[
            ("sha256", &inspection.file_identity.sha256),
            ("size", &byte_len),
            ("signature", &signature),
            ("status", &status),
            (
                "product_id",
                inspection.product_id.as_ref().map_or(
                    strings.dialog_stub_product_id_not_extracted,
                    StoreProductId::as_str,
                ),
            ),
            ("signer", &signer),
            ("warnings", &warnings),
        ],
    );
    let title = HSTRING::from(APPLICATION_NAME);
    let message = HSTRING::from(message);
    let _modal = ModalScope::enter();
    let _ = unsafe {
        MessageBoxW(
            None,
            &message,
            &title,
            MB_OK | MB_ICONINFORMATION | message_box_direction(language),
        )
    };
}

#[allow(unsafe_code)]
pub(super) unsafe fn read_window_text(window: HWND) -> String {
    let length = usize::try_from(unsafe { GetWindowTextLengthW(window) }).unwrap_or_default();
    let mut buffer = vec![0_u16; length.saturating_add(1)];
    let copied =
        usize::try_from(unsafe { GetWindowTextW(window, &mut buffer) }).unwrap_or_default();
    String::from_utf16_lossy(&buffer[..copied])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ClipboardError {
    Open,
    Allocate,
    Lock,
    Empty,
    Set,
}

/// Copy text using ownership rules required by `CF_UNICODETEXT`.
///
/// On success Windows owns the allocated memory. On every earlier failure,
/// this function releases it before returning.
#[allow(unsafe_code)]
pub(super) unsafe fn copy_text_to_clipboard(
    owner: HWND,
    text: &str,
) -> std::result::Result<(), ClipboardError> {
    let mut utf16 = text.encode_utf16().collect::<Vec<_>>();
    utf16.push(0);
    let bytes = utf16.len().saturating_mul(size_of::<u16>());
    let memory =
        unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }.map_err(|_| ClipboardError::Allocate)?;
    let destination = unsafe { GlobalLock(memory) }.cast::<u16>();
    if destination.is_null() {
        let _ = unsafe { GlobalFree(Some(memory)) };
        return Err(ClipboardError::Lock);
    }
    unsafe { std::ptr::copy_nonoverlapping(utf16.as_ptr(), destination, utf16.len()) };
    let _ = unsafe { GlobalUnlock(memory) };
    if unsafe { OpenClipboard(Some(owner)) }.is_err() {
        let _ = unsafe { GlobalFree(Some(memory)) };
        return Err(ClipboardError::Open);
    }
    let result = (|| {
        unsafe { EmptyClipboard() }.map_err(|_| ClipboardError::Empty)?;
        unsafe { SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(memory.0))) }
            .map_err(|_| ClipboardError::Set)?;
        Ok(())
    })();
    let _ = unsafe { CloseClipboard() };
    if result.is_err() {
        let _ = unsafe { GlobalFree(Some(memory)) };
    }
    result
}
