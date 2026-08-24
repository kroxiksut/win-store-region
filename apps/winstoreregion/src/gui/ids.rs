//! Window class, control identifiers, private messages, and colours.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WINDOW_STYLE, WM_APP};
use windows::core::{PCWSTR, w};

pub(super) const WINDOW_CLASS: PCWSTR = w!("WinStoreRegion.MainWindow");

pub(super) const BASE_DPI_I32: i32 = 96;

/// The integer a window word holds, as wide as a pointer.
///
/// `GetWindowLongPtrW` and `SetWindowLongPtrW` are typed `LONG_PTR`, which is
/// pointer-sized: `isize` on x64 and ARM64, `i32` on 32-bit x86. The `windows`
/// crate follows the API exactly, so code that writes `isize` compiles on two
/// of the three targets this project builds for and fails on the third. The
/// alias makes the width follow the target instead of the developer's machine.
#[cfg(target_pointer_width = "64")]
pub(super) type WindowLong = isize;
#[cfg(target_pointer_width = "32")]
pub(super) type WindowLong = i32;

/// Put a pointer into a window word.
///
/// The window word exists to hold a pointer — that is what `LONG_PTR` is for —
/// so this is the storage the API asks for rather than a narrowing conversion.
#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
pub(super) fn window_long_from_pointer<T>(pointer: *mut T) -> WindowLong {
    pointer as usize as WindowLong
}

/// The window posted results belong to, for as long as it exists.
///
/// Windows reuses window handles. A worker holds its window as a plain value
/// for as long as its request takes — a market sweep runs for half a minute —
/// and a handle reused in the meantime would be handed the raw pointer of a
/// payload meant for a window that is gone. This is published when the window
/// is created and withdrawn when it is destroyed, so a post can be refused
/// before it is made rather than interpreted by whatever now answers to that
/// value.
static LIVE_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// Record the window that may receive posted results.
pub(super) fn publish_live_window(window: HWND) {
    LIVE_WINDOW.store(window.0 as usize, Ordering::Release);
}

/// Withdraw a window that is being destroyed, if it is still the live one.
pub(super) fn withdraw_live_window(window: HWND) {
    let _ = LIVE_WINDOW.compare_exchange(window.0 as usize, 0, Ordering::AcqRel, Ordering::Relaxed);
}

/// Hand one owned value to a window as the payload of one message.
///
/// The value crosses the thread boundary as a raw pointer and the handler takes
/// ownership back with `Box::from_raw`. A window that has gone refuses the
/// post, and then the value is still owned here and is dropped rather than
/// leaked — the one thing every hand-written copy of this had to remember, in
/// ten places that each did it identically.
#[allow(unsafe_code)]
pub(super) fn post_boxed<T>(window_handle: usize, message: u32, value: T) {
    // A result for a window that is gone is dropped here. The value is still
    // owned at this point, and posting it to a handle that has been reused
    // would hand a raw pointer to a window that knows nothing about it.
    if window_handle == 0 || LIVE_WINDOW.load(Ordering::Acquire) != window_handle {
        return;
    }
    let raw = Box::into_raw(Box::new(value));
    let posted = unsafe {
        PostMessageW(
            Some(HWND(window_handle as *mut c_void)),
            message,
            WPARAM(raw as usize),
            LPARAM(0),
        )
    };
    if posted.is_err() {
        unsafe { drop(Box::from_raw(raw)) };
    }
}

pub(super) const ID_LANGUAGE: usize = 100;

pub(super) const ID_TABS: usize = 101;

pub(super) const ID_STORE_INPUT: usize = 102;

pub(super) const ID_SOURCE_LINK: usize = 103;

pub(super) const ID_SOURCE_FILE: usize = 104;

pub(super) const ID_SOURCE_ACTION: usize = 105;

pub(super) const ID_CLEAR_FILE: usize = 106;

pub(super) const ID_DETAILS: usize = 107;

pub(super) const ID_TEMPORARY_GEO_ID: usize = 108;

pub(super) const ID_OPEN_STORE_PAGE: usize = 109;

pub(super) const ID_JOURNAL_LIST: usize = 110;

pub(super) const ID_JOURNAL_OPEN_STORE: usize = 111;

pub(super) const ID_JOURNAL_REPEAT: usize = 112;

pub(super) const ID_JOURNAL_DELETE: usize = 113;

pub(super) const ID_JOURNAL_COPY_ID: usize = 114;

/// Starts one guarded installation.
pub(super) const ID_INSTALL: usize = 115;

/// Asks the curated markets where this product is offered.
pub(super) const ID_FIND_REGION: usize = 116;

/// Asks every remaining market after the curated set has answered.
pub(super) const ID_CHECK_REMAINING: usize = 117;

/// Shows every Windows region again after a search narrowed the list.
pub(super) const ID_SHOW_ALL_REGIONS: usize = 118;

/// Puts the original region back after a Store-installer handoff.
pub(super) const ID_RESTORE_REGION: usize = 119;

/// Removes every entry from the local operation history.
pub(super) const ID_JOURNAL_CLEAR: usize = 120;

/// Chooses one application on the Updates tab.
pub(super) const ID_UPDATES_LIST: usize = 121;

/// Asks Windows and the catalogue again what is installed and where it is served.
pub(super) const ID_UPDATES_REFRESH: usize = 122;

/// Takes the chosen application to the Installation tab, ready to act on.
pub(super) const ID_UPDATES_OPEN: usize = 123;

/// `IDOK` and `IDCANCEL` as `IsDialogMessageW` reports Enter and Escape.
pub(super) const ID_DIALOG_OK: usize = 1;
pub(super) const ID_DIALOG_CANCEL: usize = 2;
pub(super) const ID_MENU_PASTE: usize = 200;

pub(super) const ID_MENU_EXIT: usize = 201;

pub(super) const ID_MENU_ABOUT: usize = 202;

pub(super) const ID_MENU_OPEN_DIAGNOSTIC_LOG: usize = 203;

pub(super) const ID_MENU_GITHUB: usize = 204;
pub(super) const ID_MENU_COPY_DETAILS: usize = 205;
pub(super) const ID_MENU_RECHECK_PREREQUISITES: usize = 206;
pub(super) const ID_MENU_GET_APP_INSTALLER: usize = 207;

/// Opens Microsoft's own page about changing the country or region.
pub(super) const ID_MENU_REGION_DOCUMENTATION: usize = 208;

pub(super) const WM_APP_DROP_ENTER: u32 = WM_APP + 1;

pub(super) const WM_APP_DROP_LEAVE: u32 = WM_APP + 2;

pub(super) const WM_APP_DROP_FILE: u32 = WM_APP + 3;

pub(super) const WM_APP_STUB_INSPECTED: u32 = WM_APP + 4;

pub(super) const WM_APP_PRODUCT_RESOLVED: u32 = WM_APP + 5;

pub(super) const WM_APP_JOURNAL_LOADED: u32 = WM_APP + 6;

pub(super) const WM_APP_JOURNAL_DELETED: u32 = WM_APP + 7;

/// One installation state change reported by the operation thread.
pub(super) const WM_APP_INSTALL_PROGRESS: u32 = WM_APP + 8;

/// Answers from the market survey running off the UI thread.
pub(super) const WM_APP_MARKET_ANSWERS: u32 = WM_APP + 9;

/// One handoff state change reported by the handoff thread.
pub(super) const WM_APP_HANDOFF_PROGRESS: u32 = WM_APP + 10;

/// What the catalogue says this device can be delivered, asked off the UI thread.
pub(super) const WM_APP_DEVICE_COMPATIBILITY: u32 = WM_APP + 11;

/// The Store installer this window asked Microsoft for, once it has arrived.
pub(super) const WM_APP_INSTALLER_DOWNLOADED: u32 = WM_APP + 12;

/// One report from the scan behind the Updates tab.
pub(super) const WM_APP_UPDATES_SCANNED: u32 = WM_APP + 13;

/// The startup checks that ask Windows have finished.
pub(super) const WM_APP_STARTUP_CHECKED: u32 = WM_APP + 14;

/// Whether an installation from a previous run is still going.
pub(super) const WM_APP_RESUME_PROBED: u32 = WM_APP + 15;

pub(super) const SINGLE_INSTANCE_MUTEX: PCWSTR = w!("Local\\WinStoreRegion-v0");

pub(super) const BRAND_BACKGROUND: COLORREF = COLORREF(0x00d7_7800);

pub(super) const STATUS_BACKGROUND: COLORREF = COLORREF(0x00ff_f7eb);

pub(super) const WHITE: COLORREF = COLORREF(0x00ff_ffff);

/// Outline of every panel and boxed field.
///
/// `WS_BORDER` draws the system's black frame, which reads as a heavy box
/// rather than the light outline of the agreed design, so the window paints
/// these outlines itself in this colour.
pub(super) const FRAME_BORDER: COLORREF = COLORREF(0x00e2_dcd6);

pub(super) const APPLICATION_ICON_RESOURCE_ID: usize = 1;

pub(super) const STATIC_ICON_STYLE: WINDOW_STYLE = WINDOW_STYLE(3);

/// Win32 menu command mapped to a presentation-only action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MenuAction {
    PasteStoreInput,
    Exit,
    About,
    OpenDiagnosticLog,
    OpenProjectGithub,
    /// Copy the current status, including its technical block, for a report.
    CopyDiagnosticDetails,
    /// Run the read-only prerequisite check again.
    RecheckPrerequisites,
    /// Open the page from which App Installer can be installed.
    OpenAppInstallerPage,
    /// Open Microsoft's own instructions for changing the country or region.
    OpenRegionDocumentation,
}

pub(super) const fn menu_action(command_id: usize) -> Option<MenuAction> {
    match command_id {
        ID_MENU_PASTE => Some(MenuAction::PasteStoreInput),
        ID_MENU_EXIT => Some(MenuAction::Exit),
        ID_MENU_ABOUT => Some(MenuAction::About),
        ID_MENU_OPEN_DIAGNOSTIC_LOG => Some(MenuAction::OpenDiagnosticLog),
        ID_MENU_GITHUB => Some(MenuAction::OpenProjectGithub),
        ID_MENU_COPY_DETAILS => Some(MenuAction::CopyDiagnosticDetails),
        ID_MENU_RECHECK_PREREQUISITES => Some(MenuAction::RecheckPrerequisites),
        ID_MENU_GET_APP_INSTALLER => Some(MenuAction::OpenAppInstallerPage),
        ID_MENU_REGION_DOCUMENTATION => Some(MenuAction::OpenRegionDocumentation),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::render::tab_content;
    use crate::gui::state::{AppState, Tab};
    use crate::gui::strings::Language;
    use winstoreregion_core::GuiLaunch;

    #[test]
    fn menus_and_tabs_route_only_presentation_actions() {
        assert_eq!(
            menu_action(super::ID_MENU_PASTE),
            Some(MenuAction::PasteStoreInput)
        );
        assert_eq!(menu_action(super::ID_MENU_EXIT), Some(MenuAction::Exit));
        assert_eq!(menu_action(super::ID_MENU_ABOUT), Some(MenuAction::About));
        assert_eq!(
            menu_action(super::ID_MENU_OPEN_DIAGNOSTIC_LOG),
            Some(MenuAction::OpenDiagnosticLog)
        );
        assert_eq!(
            menu_action(super::ID_MENU_GITHUB),
            Some(MenuAction::OpenProjectGithub)
        );
        assert_eq!(
            menu_action(super::ID_MENU_REGION_DOCUMENTATION),
            Some(MenuAction::OpenRegionDocumentation)
        );
        assert_eq!(menu_action(usize::MAX), None);

        let mut state = AppState::new(GuiLaunch::default());
        state.selected_tab = Tab::Journal;
        assert!(tab_content(&state, &Language::Russian.strings()).contains("Журнал"));
        assert!(!state.install_is_available());
    }

    #[test]
    fn dialog_navigation_ids_never_collide_with_a_control_or_a_menu_action() {
        // `IsDialogMessageW` reports Enter and Escape as these command ids, so
        // a control or menu item sharing one would be triggered by a key press.
        let controls = [
            ID_LANGUAGE,
            ID_TABS,
            ID_STORE_INPUT,
            ID_SOURCE_LINK,
            ID_SOURCE_FILE,
            ID_SOURCE_ACTION,
            ID_CLEAR_FILE,
            ID_DETAILS,
            ID_TEMPORARY_GEO_ID,
            ID_OPEN_STORE_PAGE,
            ID_JOURNAL_LIST,
            ID_JOURNAL_OPEN_STORE,
            ID_JOURNAL_REPEAT,
            ID_JOURNAL_DELETE,
            ID_JOURNAL_COPY_ID,
            ID_INSTALL,
            ID_FIND_REGION,
            ID_CHECK_REMAINING,
            ID_SHOW_ALL_REGIONS,
            ID_RESTORE_REGION,
            ID_JOURNAL_CLEAR,
            ID_MENU_PASTE,
            ID_MENU_EXIT,
            ID_MENU_ABOUT,
            ID_MENU_OPEN_DIAGNOSTIC_LOG,
            ID_MENU_GITHUB,
            ID_MENU_REGION_DOCUMENTATION,
        ];
        for id in controls {
            assert_ne!(id, ID_DIALOG_OK);
            assert_ne!(id, ID_DIALOG_CANCEL);
        }
        assert_eq!(menu_action(ID_DIALOG_OK), None);
        assert_eq!(menu_action(ID_DIALOG_CANCEL), None);
    }
}
