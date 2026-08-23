//! Window creation, the message loop, and the close policy.

use crate::gui::command::{
    apply_market_answers, dispatch_command, dispatch_file_candidate, dispatch_notification,
    dispatch_ui_event, probe_selected_region,
};
use crate::gui::controls::{create_controls, framed_controls};
use crate::gui::diagnostic::{diagnostic_message, journal_diagnostic, prerequisite_status};
use crate::gui::dragdrop::register_drop_target;
use crate::gui::handoff::{HandoffUpdate, record_restored_handoff};
use crate::gui::ids::{
    APPLICATION_ICON_RESOURCE_ID, BASE_DPI_I32, BRAND_BACKGROUND, FRAME_BORDER,
    SINGLE_INSTANCE_MUTEX, STATUS_BACKGROUND, WHITE, WINDOW_CLASS, WM_APP_DEVICE_COMPATIBILITY,
    WM_APP_DROP_ENTER, WM_APP_DROP_FILE, WM_APP_DROP_LEAVE, WM_APP_HANDOFF_PROGRESS,
    WM_APP_INSTALL_PROGRESS, WM_APP_INSTALLER_DOWNLOADED, WM_APP_JOURNAL_DELETED,
    WM_APP_JOURNAL_LOADED, WM_APP_MARKET_ANSWERS, WM_APP_PRODUCT_RESOLVED, WM_APP_STUB_INSPECTED,
    WM_APP_UPDATES_SCANNED, window_long_from_pointer,
};
use crate::gui::install::{InstallationUpdate, resume_installation};
use crate::gui::layout::{DESIGN_HEIGHT, DESIGN_WIDTH, layout_controls};
use crate::gui::menu::replace_window_menu;
use crate::gui::recovery::{
    ask_pending_recovery, execute_gui_recovery, inspect_gui_startup_recovery,
    recovery_execution_status, recovery_startup_notice, show_recovery_notice,
};
use crate::gui::render::{
    installer_download_failed, installer_download_ready, operation_status, render,
    resolution_status, source_status,
};
use crate::gui::state::{
    AppState, DeviceCompatibilityUpdate, FileSelectionError, InstallerDownloadUpdate,
    JournalDeleteUpdate, MarketSurveyUpdate, ModalScope, OleGuard, ProductResolutionUpdate,
    RunningOperation, SingleInstanceGuard, StubInspectionUpdate, UiEvent, UiUpdate,
    UpdatesScanUpdate, UpdatesView, WindowBootstrap, WindowChrome, WindowContextOwner,
    hold_until_modal_ends, modal_call_is_active,
};
use crate::gui::strings::{Language, fill};
use crate::gui::work::start_journal_load;
use crate::platform::diagnostic_log::record;
use crate::platform::installer_download::sweep_downloaded_installers;
use crate::platform::prerequisites::check_prerequisites;
use crate::platform::region::Win32RegionReader;
use crate::platform::storage::{JournalStoreError, load_updates_scan};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::size_of;
use std::path::PathBuf;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_WINDOW, CreateSolidBrush, EndPaint, FrameRect, GetMonitorInfoW,
    GetStockObject, GetSysColorBrush, HDC, HOLLOW_BRUSH, InvalidateRect, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromWindow, PAINTSTRUCT, ScreenToClient, SetBkColor, SetTextColor,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{OleInitialize, RevokeDragDrop};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::{
    ICC_LISTVIEW_CLASSES, ICC_PROGRESS_CLASS, ICC_TAB_CLASSES, INITCOMMONCONTROLSEX,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW,
    FindWindowW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, GetWindowRect, HICON, ICON_BIG,
    ICON_SMALL, IDC_ARROW, IDNO, IDYES, IsDialogMessageW, IsWindowVisible, LoadCursorW, LoadIconW,
    MB_DEFBUTTON3, MB_ICONWARNING, MB_YESNO, MB_YESNOCANCEL, MINMAXINFO, MSG, MessageBoxW,
    PostQuitMessage, RegisterClassW, SW_HIDE, SW_RESTORE, SW_SHOW, SWP_NOZORDER, SendMessageW,
    SetForegroundWindow, SetMenu, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_LONG_PTR_INDEX, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_CTLCOLORSTATIC,
    WM_DESTROY, WM_DPICHANGED, WM_GETMINMAXINFO, WM_NCCREATE, WM_NCDESTROY, WM_NOTIFY, WM_PAINT,
    WM_SETICON, WM_SIZE, WNDCLASSW, WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW,
};
use windows::core::{HSTRING, PCWSTR, Result};
use winstoreregion_core::{
    APPLICATION_NAME, Diagnostic, GuiLaunch, InstallationOperationState, JournalRecord, LogEvent,
    LogEventCode, LogLevel, MarketCode, PendingRecoveryChoice, PendingRecoveryExecution,
    PendingRecoveryExecutionError, RegionReader, StartupRecoveryError, StartupRecoveryOutcome,
    install_supported_in_v0_1, startup_recovery_admits_new_operation,
};

#[allow(unsafe_code)]
pub(crate) fn run(launch: GuiLaunch) -> Result<()> {
    unsafe {
        let Some(single_instance) = acquire_single_instance()? else {
            activate_existing_window();
            return Ok(());
        };
        OleInitialize(None)?;
        let _ole_guard = OleGuard;
        let common_controls = INITCOMMONCONTROLSEX {
            dwSize: u32::try_from(size_of::<INITCOMMONCONTROLSEX>()).unwrap_or(u32::MAX),
            dwICC: ICC_TAB_CLASSES | ICC_PROGRESS_CLASS | ICC_LISTVIEW_CLASSES,
        };
        let _ = windows::Win32::UI::Controls::InitCommonControlsEx(&raw const common_controls);
        let module = GetModuleHandleW(None)?;
        let instance = HINSTANCE(module.0);
        let app_icon = LoadIconW(
            Some(instance),
            PCWSTR(APPLICATION_ICON_RESOURCE_ID as *const u16),
        )?;
        register_window_class(instance, app_icon)?;
        let context_owner = WindowContextOwner::new(
            WindowChrome {
                instance,
                app_icon,
                _single_instance: single_instance,
                drop_target: None,
                // COLORREF stores components as 0x00BBGGRR.
                brand_brush: CreateSolidBrush(BRAND_BACKGROUND),
                status_brush: CreateSolidBrush(STATUS_BACKGROUND),
                frame_brush: CreateSolidBrush(FRAME_BORDER),
                menu: Cell::new(None),
                last_text: RefCell::new(std::collections::HashMap::new()),
                last_layout: Cell::new(None),
                listed_regions: RefCell::new(Vec::new()),
                listed_region_language: Cell::new(None),
                listed_updates: RefCell::new(Vec::new()),
                heading_language: Cell::new(None),
                controls: None,
            },
            AppState::new(launch),
        );
        let title = HSTRING::from(APPLICATION_NAME);
        let window = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            WINDOW_CLASS,
            &title,
            // `WS_CLIPCHILDREN` is what stops the flicker. Without it the
            // window erases its own background under every child before the
            // child repaints, so each caption that changes costs a visible
            // flash of the area behind it — and a forty-market search changes
            // text dozens of times. Caching decides how many repaints happen;
            // this decides whether a repaint flashes at all.
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            80,
            40,
            DESIGN_WIDTH,
            DESIGN_HEIGHT,
            None,
            None,
            Some(instance),
            Some(context_owner.as_create_parameter()),
        )?;
        let dpi = i32::try_from(GetDpiForWindow(window)).unwrap_or(BASE_DPI_I32);
        let menu_result = match (chrome_ref(window), state_ref(window)) {
            (Some(chrome), Some(state)) => replace_window_menu(window, chrome, state.language),
            _ => Err(windows::core::Error::from_thread()),
        };
        if let Err(error) = menu_result {
            let _ = DestroyWindow(window);
            return Err(error);
        }
        apply_window_icons_and_size(window, app_icon, dpi);
        let _ = ShowWindow(window, SW_SHOW);
        record(&LogEvent::new(
            LogLevel::Info,
            LogEventCode::ApplicationStarted,
        ));
        let mut message = MSG::default();
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            // Dialog-style navigation for a plain window: Tab, arrows, Enter
            // and Escape reach the controls only through this call. Without it
            // `WS_TABSTOP` is inert and the window cannot be used without a
            // mouse.
            if IsDialogMessageW(window, &raw const message).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
        record(&LogEvent::new(
            LogLevel::Info,
            LogEventCode::ApplicationStopped,
        ));
    }
    Ok(())
}

/// Register the window class, including the extra word that holds `AppState`.
#[allow(unsafe_code)]
unsafe fn register_window_class(instance: HINSTANCE, app_icon: HICON) -> Result<()> {
    let window_class = WNDCLASSW {
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        hInstance: instance,
        hIcon: app_icon,
        lpszClassName: WINDOW_CLASS,
        lpfnWndProc: Some(window_procedure),
        // One extra word holds the `AppState` pointer; chrome uses
        // `GWLP_USERDATA`. Two words keep the two borrows independent.
        cbWndExtra: i32::try_from(size_of::<*mut c_void>()).unwrap_or(0),
        ..Default::default()
    };
    let _ = unsafe { RegisterClassW(&raw const window_class) };
    Ok(())
}

/// Outline every panel and boxed field the design frames.
///
/// The children carry no border of their own, so the outline is drawn by the
/// window just outside each child's rectangle, where the child's own clipping
/// cannot cover it. Hidden children are skipped: the two tabs share the same
/// area and would otherwise frame each other's boxes.
#[allow(unsafe_code)]
unsafe fn paint_panel_outlines(window: HWND, chrome: &WindowChrome) {
    let mut paint = PAINTSTRUCT::default();
    let device_context = unsafe { BeginPaint(window, &raw mut paint) };
    if let Some(controls) = chrome.controls.as_ref() {
        for control in framed_controls(controls) {
            if !unsafe { IsWindowVisible(control) }.as_bool() {
                continue;
            }
            let Some(mut bounds) = (unsafe { control_bounds_in_client(window, control) }) else {
                continue;
            };
            bounds.left -= 1;
            bounds.top -= 1;
            bounds.right += 1;
            bounds.bottom += 1;
            let _ = unsafe { FrameRect(device_context, &raw const bounds, chrome.frame_brush) };
        }
    }
    let _ = unsafe { EndPaint(window, &raw const paint) };
}

/// One child's rectangle expressed in the window's own client coordinates.
#[allow(unsafe_code)]
unsafe fn control_bounds_in_client(window: HWND, control: HWND) -> Option<RECT> {
    let mut bounds = RECT::default();
    unsafe { GetWindowRect(control, &raw mut bounds) }.ok()?;
    let mut top_left = windows::Win32::Foundation::POINT {
        x: bounds.left,
        y: bounds.top,
    };
    let mut bottom_right = windows::Win32::Foundation::POINT {
        x: bounds.right,
        y: bounds.bottom,
    };
    if !unsafe { ScreenToClient(window, &raw mut top_left) }.as_bool()
        || !unsafe { ScreenToClient(window, &raw mut bottom_right) }.as_bool()
    {
        return None;
    }
    Some(RECT {
        left: top_left.x,
        top: top_left.y,
        right: bottom_right.x,
        bottom: bottom_right.y,
    })
}

/// Apply both icon sizes and the DPI-scaled initial size.
#[allow(unsafe_code)]
unsafe fn apply_window_icons_and_size(window: HWND, app_icon: HICON, dpi: i32) {
    for size in [ICON_BIG, ICON_SMALL] {
        let _ = unsafe {
            SendMessageW(
                window,
                WM_SETICON,
                Some(WPARAM(usize::try_from(size).unwrap_or_default())),
                Some(LPARAM(app_icon.0 as isize)),
            )
        };
    }
    // The design size in physical pixels, but never larger than the desktop can
    // actually show: a window taller than the work area hides its own command
    // row behind the taskbar, which is exactly what a small screen must not do.
    let work_area = unsafe { work_area_for(window) };
    let width = (DESIGN_WIDTH * dpi / BASE_DPI_I32).min(work_area.right - work_area.left);
    let height = (DESIGN_HEIGHT * dpi / BASE_DPI_I32).min(work_area.bottom - work_area.top);
    let left = work_area.left + ((work_area.right - work_area.left - width) / 2).max(0);
    let top = work_area.top + ((work_area.bottom - work_area.top - height) / 3).max(0);
    let _ = unsafe { SetWindowPos(window, None, left, top, width, height, SWP_NOZORDER) };
}

/// The desktop area this window may occupy, minus the taskbar.
///
/// Falls back to the whole virtual screen when the monitor cannot be queried:
/// an approximate limit still beats placing a window off the bottom edge.
#[allow(unsafe_code)]
unsafe fn work_area_for(window: HWND) -> RECT {
    let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or_default(),
        ..MONITORINFO::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &raw mut info) }.as_bool() {
        return info.rcWork;
    }
    RECT {
        left: 0,
        top: 0,
        right: DESIGN_WIDTH,
        bottom: DESIGN_HEIGHT,
    }
}

#[allow(unsafe_code)]
unsafe fn acquire_single_instance() -> Result<Option<SingleInstanceGuard>> {
    let handle = unsafe { CreateMutexW(None, false, SINGLE_INSTANCE_MUTEX) }?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let _ = unsafe { CloseHandle(handle) };
        return Ok(None);
    }
    Ok(Some(SingleInstanceGuard(handle)))
}

#[allow(unsafe_code)]
unsafe fn activate_existing_window() {
    if let Ok(existing) = unsafe { FindWindowW(WINDOW_CLASS, None) } {
        let _ = unsafe { ShowWindow(existing, SW_RESTORE) };
        let _ = unsafe { SetForegroundWindow(existing) };
    }
}

/// Whether this message carries a result some thread posted to the window.
///
/// These are the only messages that arrive without the user doing anything, so
/// they are the only ones a modal dialog can deliver behind an outer handler's
/// back. Everything else reaches the window through input, which a modal call
/// disables, or through painting, which reads chrome and never `AppState`.
const fn is_posted_application_message(message: u32) -> bool {
    matches!(
        message,
        WM_APP_DROP_ENTER
            | WM_APP_DROP_LEAVE
            | WM_APP_DROP_FILE
            | WM_APP_STUB_INSPECTED
            | WM_APP_PRODUCT_RESOLVED
            | WM_APP_JOURNAL_LOADED
            | WM_APP_JOURNAL_DELETED
            | WM_APP_INSTALL_PROGRESS
            | WM_APP_MARKET_ANSWERS
            | WM_APP_HANDOFF_PROGRESS
            | WM_APP_DEVICE_COMPATIBILITY
            | WM_APP_INSTALLER_DOWNLOADED
            | WM_APP_UPDATES_SCANNED
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowCloseRequirement {
    ImmediateExit,
    ConfirmActiveOperation(InstallationOperationState),
}

const fn window_close_requirement(
    operation_state: Option<InstallationOperationState>,
) -> WindowCloseRequirement {
    match operation_state {
        Some(state) if state.requires_recovery() => {
            WindowCloseRequirement::ConfirmActiveOperation(state)
        }
        _ => WindowCloseRequirement::ImmediateExit,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveOperationCloseChoice {
    ContinueWorking,
    RestoreThenExit,
    ExitWithPendingRecovery,
}

#[allow(unsafe_code)]
unsafe fn show_active_operation_close_dialog(
    window: HWND,
    language: Language,
    state: InstallationOperationState,
) -> ActiveOperationCloseChoice {
    // The application never cancels an installation: Windows owns it, and the
    // dialog says where the user can.
    let message = HSTRING::from(fill(
        language.strings().window_close_with_running_operation,
        &[("state", operation_state_label(language, state))],
    ));
    let title = HSTRING::from(APPLICATION_NAME);
    let _modal = ModalScope::enter();
    match unsafe {
        MessageBoxW(
            Some(window),
            &message,
            &title,
            MB_YESNOCANCEL | MB_DEFBUTTON3 | MB_ICONWARNING,
        )
    } {
        IDYES => ActiveOperationCloseChoice::RestoreThenExit,
        IDNO => ActiveOperationCloseChoice::ExitWithPendingRecovery,
        _ => ActiveOperationCloseChoice::ContinueWorking,
    }
}

fn operation_state_label(language: Language, state: InstallationOperationState) -> &'static str {
    let strings = language.strings();
    match state {
        InstallationOperationState::RegionSwitched => strings.window_state_region_switched,
        InstallationOperationState::RefreshingStoreContext => {
            strings.window_state_refreshing_store_context
        }
        InstallationOperationState::StartingInstall => strings.window_state_starting_install,
        InstallationOperationState::InstallRequested => strings.window_state_install_requested,
        InstallationOperationState::InstallObserved => strings.window_state_install_observed,
        InstallationOperationState::Installing => strings.window_state_installing,
        InstallationOperationState::CompletionUncertain => {
            strings.window_state_completion_uncertain
        }
        InstallationOperationState::Installed => strings.window_state_installed,
        InstallationOperationState::RestoringRegion => strings.window_state_restoring_region,
        InstallationOperationState::Restored => strings.window_state_restored,
        InstallationOperationState::FailedWithTemporaryRegion => {
            strings.window_state_failed_with_temporary_region
        }
        InstallationOperationState::RestoreConflict => strings.window_state_restore_conflict,
        InstallationOperationState::RestoreFailed => strings.window_state_restore_failed,
        _ => strings.window_state_requires_recovery,
    }
}

/// Ask before closing while a handoff still holds the temporary region.
///
/// There is nothing to cancel and nothing to wait for: Microsoft Store owns the
/// installation, and this window owns only the region. The honest choice is
/// whether to hand that region to the next start or put it back first.
#[allow(unsafe_code)]
unsafe fn confirm_close_with_pending_handoff(window: HWND, language: Language) -> bool {
    let strings = language.strings();
    let message = HSTRING::from(fill(
        strings.window_close_with_pending_handoff,
        &[("button", strings.restore)],
    ));
    let title = HSTRING::from(APPLICATION_NAME);
    let _modal = ModalScope::enter();
    let answer = unsafe { MessageBoxW(Some(window), &message, &title, MB_YESNO | MB_ICONWARNING) };
    answer == IDYES
}

#[allow(unsafe_code)]
unsafe fn request_window_close(window: HWND, chrome: &WindowChrome, app: &mut AppState) -> bool {
    // A handoff has its own answer: nothing here can be cancelled or waited on.
    if app
        .handoff
        .as_ref()
        .is_some_and(|handoff| handoff.stage.requires_recovery())
    {
        return unsafe { confirm_close_with_pending_handoff(window, app.language) };
    }
    let WindowCloseRequirement::ConfirmActiveOperation(state) =
        window_close_requirement(app.operation_state)
    else {
        return true;
    };
    // The published record is the truth about whether anything still needs
    // recovering; the operation state is only this window's memory of how the
    // last attempt ended. An operation that failed and then put the region back
    // leaves that memory reading "failed with the temporary region" forever,
    // and asking the user about a record that no longer exists trapped the
    // window: every answer in the dialog refused to close it.
    if matches!(
        inspect_gui_startup_recovery(),
        Ok(StartupRecoveryOutcome::NoPendingRecord
            | StartupRecoveryOutcome::VerifiedRecordCleaned { .. })
    ) {
        return true;
    }
    match unsafe { show_active_operation_close_dialog(window, app.language, state) } {
        ActiveOperationCloseChoice::ContinueWorking => false,
        ActiveOperationCloseChoice::ExitWithPendingRecovery => {
            match inspect_gui_startup_recovery() {
                // Nothing is pending, or a record needs the next start to deal
                // with it. Both are safe to close on.
                Ok(
                    StartupRecoveryOutcome::UserDecisionRequired { .. }
                    | StartupRecoveryOutcome::NoPendingRecord
                    | StartupRecoveryOutcome::VerifiedRecordCleaned { .. },
                ) => true,
                Err(error) => {
                    let diagnostic = Diagnostic::from(error);
                    let cancelled = app.language.strings().window_request_window_close;
                    app.operation_status = format!(
                        "{cancelled} {}",
                        diagnostic_message(app.language, diagnostic.code())
                    );
                    unsafe { render(window, chrome, app) };
                    false
                }
            }
        }
        ActiveOperationCloseChoice::RestoreThenExit => {
            // This offered to restore and then close, and then refused to do
            // either: the text spoke of a writer that has been confirmed for
            // weeks. It now does what it says, through the same protocol the
            // next start would use.
            let Ok(StartupRecoveryOutcome::UserDecisionRequired {
                pending,
                disposition,
                ..
            }) = inspect_gui_startup_recovery()
            else {
                // Nothing is published, so there is nothing to restore and no
                // reason to keep the window open.
                return true;
            };
            let outcome = execute_gui_recovery(
                &pending,
                disposition,
                PendingRecoveryChoice::RestoreOriginalRegion,
            );
            record(
                &LogEvent::new(LogLevel::Info, LogEventCode::RecoveryActionExecuted)
                    .with_token("choice", "restore")
                    .with_token("outcome", recovery_outcome_token(&outcome)),
            );
            if matches!(
                outcome,
                Ok(PendingRecoveryExecution::Restored { .. }
                    | PendingRecoveryExecution::VerifiedRecordCleared)
            ) {
                return true;
            }
            // The region did not come back, so the window stays and says so
            // rather than closing over an unresolved region.
            app.operation_status = recovery_execution_status(app.language, &outcome);
            let region = Win32RegionReader.current_region();
            unsafe {
                dispatch_ui_event(
                    window,
                    chrome,
                    app,
                    UiEvent::StateUpdate(UiUpdate::CurrentRegion(region)),
                );
            };
            false
        }
    }
}

#[allow(clippy::too_many_lines, unsafe_code)]
unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // A posted result must never be handled while a modal dialog is up: the
    // dialog runs its own message loop, and the handler below would take a
    // second `&mut AppState` on top of the one the outer handler still holds.
    if is_posted_application_message(message) && modal_call_is_active() {
        hold_until_modal_ends(window, message, wparam);
        return LRESULT(0);
    }
    match message {
        WM_NCCREATE => {
            let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            let bootstrap = create.lpCreateParams.cast::<WindowBootstrap>();
            if bootstrap.is_null() {
                return LRESULT(0);
            }
            let bootstrap = unsafe { &*bootstrap };
            unsafe {
                SetWindowLongPtrW(
                    window,
                    GWLP_USERDATA,
                    window_long_from_pointer(bootstrap.chrome),
                );
                SetWindowLongPtrW(
                    window,
                    STATE_WINDOW_WORD,
                    window_long_from_pointer(bootstrap.state),
                );
            }
        }
        WM_CREATE => {
            let (Some(chrome), Some(state)) =
                (unsafe { chrome_mut(window) }, unsafe { state_mut(window) })
            else {
                return LRESULT(-1);
            };
            let Ok(controls) = (unsafe { create_controls(window, chrome, state) }) else {
                return LRESULT(-1);
            };
            // Publish chrome before anything modal can re-enter the window.
            chrome.controls = Some(controls);
            // A window that is not registered never hears about a drag, so the
            // target is put in place here and revoked in `WM_DESTROY`. Failing
            // to register costs only drag and drop: the file picker still works.
            chrome.drop_target = register_drop_target(window).ok();
            let chrome = unsafe { &*std::ptr::from_ref(chrome) };
            let status = state.operation_status.clone();
            unsafe {
                dispatch_ui_event(
                    window,
                    chrome,
                    state,
                    UiEvent::StateUpdate(UiUpdate::OperationStatus(status)),
                );
                let current_region = Win32RegionReader.current_region();
                dispatch_ui_event(
                    window,
                    chrome,
                    state,
                    UiEvent::StateUpdate(UiUpdate::CurrentRegion(current_region)),
                );
            };
            start_journal_load(window);
            // Installers this application downloaded are never kept. The user
            // did not ask for a folder of Store stubs, cannot be expected to
            // find it, and the file can always be fetched again by Product ID.
            // Startup is where this works: nothing of ours is running the file.
            sweep_downloaded_installers();
            // A scan remembered from a previous run costs nothing to show and
            // saves the user a wait. It is labelled with when it was taken
            // rather than presented as current.
            if let Ok(region) = Win32RegionReader.current_region()
                && let Ok(market) = MarketCode::from_region(&region)
                && let Some((taken, candidates)) = load_updates_scan(&market)
            {
                state.updates = UpdatesView::Ready(candidates);
                state.updates_taken = Some(taken);
            }
            state.prerequisites = check_prerequisites();
            record(
                &LogEvent::new(LogLevel::Info, LogEventCode::PrerequisitesChecked).with_token(
                    "install",
                    if state.prerequisites.admits_installation() {
                        "admitted"
                    } else {
                        "blocked"
                    },
                ),
            );
            if let Some(message) = prerequisite_status(state.language, state.prerequisites) {
                unsafe {
                    dispatch_ui_event(
                        window,
                        chrome,
                        state,
                        UiEvent::StateUpdate(UiUpdate::OperationStatus(message)),
                    );
                };
            }
            let recovery = inspect_gui_startup_recovery();
            // Core owns the rule; the window only records its verdict.
            state.recovery_admits_new_operation = startup_recovery_admits_new_operation(&recovery);
            record(
                &LogEvent::new(LogLevel::Info, LogEventCode::StartupRecoveryInspected)
                    .with_token("outcome", startup_recovery_token(&recovery))
                    .with_token(
                        "new_operation",
                        if state.recovery_admits_new_operation {
                            "admitted"
                        } else {
                            "blocked"
                        },
                    ),
            );
            if let Some(message) = recovery_startup_notice(state.language, &recovery) {
                unsafe {
                    dispatch_ui_event(
                        window,
                        chrome,
                        state,
                        UiEvent::StateUpdate(UiUpdate::OperationStatus(message.clone())),
                    );
                };
                // A record that needs a decision gets one; everything else is
                // only reported, because there is nothing for the user to do.
                if let Ok(StartupRecoveryOutcome::UserDecisionRequired {
                    pending,
                    disposition,
                    ..
                }) = &recovery
                {
                    // An install started by a previous run may still be going:
                    // the package manager outlives the process that asked for
                    // it. Taking that over is better than asking the user to
                    // decide about a region the operation still needs.
                    if resume_installation(window, pending) {
                        state.running_operation = Some(RunningOperation::default());
                        let resumed = state.language.strings().window_installation_resumed;
                        resumed.clone_into(&mut state.operation_status);
                        unsafe { render(window, chrome, state) };
                        return LRESULT(0);
                    }
                    let choice = unsafe { ask_pending_recovery(window, state.language, &message) };
                    let outcome = execute_gui_recovery(pending, *disposition, choice);
                    record(
                        &LogEvent::new(LogLevel::Info, LogEventCode::RecoveryActionExecuted)
                            .with_token(
                                "choice",
                                match choice {
                                    PendingRecoveryChoice::RestoreOriginalRegion => "restore",
                                    PendingRecoveryChoice::KeepCurrentRegion => "keep_current",
                                },
                            )
                            .with_token("outcome", recovery_outcome_token(&outcome)),
                    );
                    // A handoff that the previous run left behind gets its
                    // history entry here, because this is where its region
                    // actually came back.
                    if matches!(
                        outcome,
                        Ok(PendingRecoveryExecution::Restored { .. }
                            | PendingRecoveryExecution::VerifiedRecordCleared)
                    ) {
                        record_restored_handoff(pending);
                    }
                    state.operation_status = recovery_execution_status(state.language, &outcome);
                    // Re-inspect: a confirmed restore lifts the gate, anything
                    // else must leave it exactly where it was.
                    let after = inspect_gui_startup_recovery();
                    state.recovery_admits_new_operation =
                        startup_recovery_admits_new_operation(&after);
                    let region = Win32RegionReader.current_region();
                    unsafe {
                        dispatch_ui_event(
                            window,
                            chrome,
                            state,
                            UiEvent::StateUpdate(UiUpdate::CurrentRegion(region)),
                        );
                    };
                } else {
                    unsafe { show_recovery_notice(&message) };
                }
            }
            // The status was written before recovery and the prerequisites were
            // checked, so it can still name a condition those checks have since
            // satisfied. It is recomputed only when the gate is now open: a real
            // blocking notice keeps the gate closed and must survive.
            if state.install_is_available() {
                state.operation_status = source_status(state);
            }
            // A window launched with an identifier already has its question and
            // its default region, so the card for that region is asked for now
            // rather than waiting for the user to retype what they passed in.
            probe_selected_region(window, state);
            unsafe { render(window, chrome, state) };
        }
        WM_GETMINMAXINFO => {
            // This layout places content, it does not reflow it, so a window
            // smaller than the design size would clip its own controls.
            if lparam.0 != 0 {
                let dpi = i32::try_from(unsafe { GetDpiForWindow(window) })
                    .unwrap_or(BASE_DPI_I32)
                    .max(BASE_DPI_I32);
                let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                info.ptMinTrackSize.x = DESIGN_WIDTH * dpi / BASE_DPI_I32;
                info.ptMinTrackSize.y = DESIGN_HEIGHT * dpi / BASE_DPI_I32;
                return LRESULT(0);
            }
        }
        WM_SIZE => {
            // Re-entrant during a modal loop: chrome only, never `AppState`.
            if let Some(chrome) = unsafe { chrome_ref(window) } {
                unsafe { layout_controls(window, chrome) };
            }
        }
        WM_PAINT => {
            // Re-entrant during a modal loop: chrome only, never `AppState`.
            if let Some(chrome) = unsafe { chrome_ref(window) } {
                unsafe { paint_panel_outlines(window, chrome) };
                return LRESULT(0);
            }
        }
        WM_CTLCOLORSTATIC => {
            // Re-entrant during a modal loop: chrome only, never `AppState`.
            if let Some(chrome) = unsafe { chrome_ref(window) }
                && let Some(controls) = chrome.controls.as_ref()
            {
                let control = HWND(lparam.0 as *mut c_void);
                let device_context = HDC(wparam.0 as *mut c_void);
                if control == controls.brand_badge {
                    let _ = unsafe { SetBkColor(device_context, BRAND_BACKGROUND) };
                    let _ = unsafe { SetTextColor(device_context, WHITE) };
                    return LRESULT(chrome.brand_brush.0 as isize);
                }
                if control == controls.status {
                    let _ = unsafe { SetBkColor(device_context, STATUS_BACKGROUND) };
                    return LRESULT(chrome.status_brush.0 as isize);
                }
                // The grouping panels carry no text and must not fill anything:
                // they sit over the boxes inside them, and an opaque panel
                // erased the outlines this window had just drawn around those
                // boxes. A hollow brush leaves the window's own painting alone.
                if control == controls.source_panel || control == controls.region_panel {
                    return LRESULT(unsafe { GetStockObject(HOLLOW_BRUSH) }.0 as isize);
                }
            }
            return LRESULT(unsafe { GetSysColorBrush(COLOR_WINDOW) }.0 as isize);
        }
        WM_DPICHANGED => {
            let suggested = unsafe { &*(lparam.0 as *const RECT) };
            let _ = unsafe {
                SetWindowPos(
                    window,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER,
                )
            };
            return LRESULT(0);
        }
        WM_COMMAND => {
            if let (Some(chrome), Some(state)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                unsafe { dispatch_command(window, chrome, state, wparam) };
                return LRESULT(0);
            }
        }
        WM_APP_DROP_ENTER => {
            if let Some(chrome) = unsafe { chrome_ref(window) } {
                if let Some(controls) = chrome.controls.as_ref() {
                    let _ = unsafe {
                        ShowWindow(
                            controls.drop_overlay,
                            if wparam.0 == 0 { SW_HIDE } else { SW_SHOW },
                        )
                    };
                    // The overlay's outline is painted by the window, so it
                    // appears only once the client area repaints.
                    let _ = unsafe { InvalidateRect(Some(window), None, true) };
                }
                return LRESULT(0);
            }
        }
        WM_APP_DROP_LEAVE => {
            if let Some(chrome) = unsafe { chrome_ref(window) }
                && let Some(controls) = chrome.controls.as_ref()
            {
                let _ = unsafe { ShowWindow(controls.drop_overlay, SW_HIDE) };
                return LRESULT(0);
            }
        }
        WM_APP_DROP_FILE => {
            // Reclaim the posted allocation before anything can return early.
            let candidate = (wparam.0 != 0).then(|| unsafe {
                *Box::from_raw(wparam.0 as *mut std::result::Result<PathBuf, FileSelectionError>)
            });
            if let (Some(chrome), Some(state)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                if let Some(controls) = chrome.controls.as_ref() {
                    let _ = unsafe { ShowWindow(controls.drop_overlay, SW_HIDE) };
                }
                if let Some(candidate) = candidate {
                    unsafe { dispatch_file_candidate(window, chrome, state, candidate) };
                }
                return LRESULT(0);
            }
        }
        WM_APP_STUB_INSPECTED => {
            if wparam.0 == 0 {
                return LRESULT(0);
            }
            // Reclaim the posted allocation before anything can return early.
            let update = unsafe { *Box::from_raw(wparam.0 as *mut StubInspectionUpdate) };
            if let (Some(chrome), Some(app)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                let still_selected = app
                    .application_source
                    .installer_file()
                    .is_some_and(|file| file.as_path() == update.path);
                if still_selected {
                    app.resolution.invalidate();
                    match update.result {
                        Ok(inspection) => {
                            // The status is recomputed from the whole state:
                            // whether this file may run at all is a gate, not a
                            // property of the inspection on its own.
                            app.stub_inspection = Some(inspection);
                            app.stub_inspection_error = None;
                            app.operation_status = source_status(app);
                        }
                        Err(error) => {
                            app.stub_inspection = None;
                            app.stub_inspection_error = Some(error);
                            app.operation_status = source_status(app);
                        }
                    }
                    unsafe { render(window, chrome, app) };
                }
                return LRESULT(0);
            }
        }
        WM_APP_PRODUCT_RESOLVED => {
            if wparam.0 == 0 {
                return LRESULT(0);
            }
            // Reclaim the posted allocation before anything can return early.
            let update = unsafe { *Box::from_raw(wparam.0 as *mut ProductResolutionUpdate) };
            if let (Some(chrome), Some(app)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                if app.resolution.complete(&update.request, update.result) {
                    // The backend is confirmed per product, not per machine:
                    // only an `MSStore` installer type can be proven complete.
                    app.installation_backend_confirmed =
                        app.resolved_product().is_some_and(|product| {
                            install_supported_in_v0_1(product.install_classification)
                        });
                    app.operation_status = resolution_status(app);
                    unsafe { render(window, chrome, app) };
                }
                return LRESULT(0);
            }
        }
        WM_APP_INSTALL_PROGRESS => {
            if wparam.0 == 0 {
                return LRESULT(0);
            }
            // Reclaim the posted allocation before anything can return early.
            let update = unsafe { *Box::from_raw(wparam.0 as *mut InstallationUpdate) };
            if let (Some(chrome), Some(app)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                let was_running = app.running_operation.is_some();
                apply_installation_update(app, update);
                // The operation writes its own history entry and then ends.
                // Reading the journal only at startup would leave a product
                // installed just now missing from the tab until the next launch.
                if was_running && app.running_operation.is_none() {
                    start_journal_load(window);
                }
                app.operation_status = operation_status(app);
                unsafe { render(window, chrome, app) };
                return LRESULT(0);
            }
        }
        WM_APP_HANDOFF_PROGRESS => {
            if wparam.0 == 0 {
                return LRESULT(0);
            }
            // Reclaim the posted allocation before anything can return early.
            let update = unsafe { *Box::from_raw(wparam.0 as *mut HandoffUpdate) };
            if let (Some(chrome), Some(app)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                let held_region = app.region_restore_is_available();
                apply_handoff_update(app, update);
                app.operation_status = operation_status(app);
                // A handoff writes its history entry when the region comes
                // back, so that is when the tab has something new to show.
                if held_region && !app.region_restore_is_available() {
                    start_journal_load(window);
                    // The handoff is over, so its installer has no further use.
                    // A stub still running holds the file open and survives
                    // this; the sweep at the next startup catches it.
                    sweep_downloaded_installers();
                }
                // A finished handoff releases the machine, and the current
                // region is worth re-reading: it just changed.
                let region = Win32RegionReader.current_region();
                unsafe {
                    dispatch_ui_event(
                        window,
                        chrome,
                        app,
                        UiEvent::StateUpdate(UiUpdate::CurrentRegion(region)),
                    );
                };
                return LRESULT(0);
            }
        }
        WM_APP_MARKET_ANSWERS => {
            if wparam.0 == 0 {
                return LRESULT(0);
            }
            // Reclaim the posted allocation before anything can return early.
            let update = unsafe { *Box::from_raw(wparam.0 as *mut MarketSurveyUpdate) };
            if let (Some(chrome), Some(app)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                apply_market_answers(window, app, update);
                unsafe { render(window, chrome, app) };
                return LRESULT(0);
            }
        }
        WM_APP_UPDATES_SCANNED => {
            if wparam.0 == 0 {
                return LRESULT(0);
            }
            // Reclaim the posted allocation before anything can return early.
            let update = unsafe { *Box::from_raw(wparam.0 as *mut UpdatesScanUpdate) };
            if let (Some(chrome), Some(app)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                app.updates = if update.unavailable {
                    UpdatesView::Unavailable
                } else if update.finished {
                    UpdatesView::Ready(update.candidates)
                } else {
                    UpdatesView::Scanning {
                        done: update.done,
                        total: update.total,
                    }
                };
                // A selection made during a scan belongs to a list that has
                // since grown; only a finished list can be pointed into.
                if !matches!(app.updates, UpdatesView::Ready(_)) {
                    app.selected_update_index = None;
                }
                unsafe { render(window, chrome, app) };
                return LRESULT(0);
            }
        }
        WM_APP_INSTALLER_DOWNLOADED => {
            if wparam.0 == 0 {
                return LRESULT(0);
            }
            // Reclaim the posted allocation before anything can return early.
            let update = unsafe { *Box::from_raw(wparam.0 as *mut InstallerDownloadUpdate) };
            if let (Some(chrome), Some(app)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                app.installer_download = None;
                match update.result {
                    Ok(downloaded) => {
                        // The file takes the ordinary route from here: the same
                        // selection, the same inspection, the same gate. This
                        // window learns nothing about it that a hand-picked
                        // file would not have told it.
                        unsafe {
                            dispatch_file_candidate(window, chrome, app, Ok(downloaded.path));
                        };
                        app.operation_status = installer_download_ready(app.language);
                    }
                    Err(error) => {
                        app.operation_status = installer_download_failed(app.language, error);
                    }
                }
                unsafe { render(window, chrome, app) };
                return LRESULT(0);
            }
        }
        WM_APP_DEVICE_COMPATIBILITY => {
            if wparam.0 == 0 {
                return LRESULT(0);
            }
            // Reclaim the posted allocation before anything can return early.
            let update = unsafe { *Box::from_raw(wparam.0 as *mut DeviceCompatibilityUpdate) };
            if let (Some(chrome), Some(app)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                app.device_compatibility =
                    Some((update.product_id, update.market, update.compatibility));
                app.operation_status = source_status(app);
                unsafe { render(window, chrome, app) };
                return LRESULT(0);
            }
        }
        WM_APP_JOURNAL_LOADED => {
            if wparam.0 == 0 {
                return LRESULT(0);
            }
            let result = unsafe {
                *Box::from_raw(
                    wparam.0 as *mut std::result::Result<Vec<JournalRecord>, JournalStoreError>,
                )
            };
            if let (Some(chrome), Some(app)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                unsafe {
                    dispatch_ui_event(
                        window,
                        chrome,
                        app,
                        UiEvent::StateUpdate(UiUpdate::JournalLoaded(result)),
                    );
                }
                return LRESULT(0);
            }
        }
        WM_APP_JOURNAL_DELETED => {
            if wparam.0 == 0 {
                return LRESULT(0);
            }
            let update = unsafe { *Box::from_raw(wparam.0 as *mut JournalDeleteUpdate) };
            if let (Some(chrome), Some(app)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                app.operation_status = match update.result {
                    Ok(()) => {
                        start_journal_load(window);
                        app.language
                            .strings()
                            .window_journal_entry_deleted
                            .to_owned()
                    }
                    Err(error) => fill(
                        app.language.strings().window_journal_entry_not_deleted,
                        &[(
                            "cause",
                            diagnostic_message(app.language, journal_diagnostic(error).code()),
                        )],
                    ),
                };
                unsafe { render(window, chrome, app) };
                return LRESULT(0);
            }
        }
        WM_NOTIFY => {
            if let (Some(chrome), Some(app)) =
                (unsafe { chrome_ref(window) }, unsafe { state_mut(window) })
            {
                unsafe { dispatch_notification(window, chrome, app, lparam) };
                return LRESULT(0);
            }
        }
        WM_CLOSE => {
            let may_close = match (unsafe { chrome_ref(window) }, unsafe { state_mut(window) }) {
                (Some(chrome), Some(state)) => unsafe {
                    request_window_close(window, chrome, state)
                },
                _ => true,
            };
            if may_close {
                let _ = unsafe { DestroyWindow(window) };
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            // Release only what needs a live window here. The allocation
            // itself belongs to `WindowContextOwner` in `run`.
            if let Some(chrome) = unsafe { chrome_mut(window) } {
                if chrome.drop_target.take().is_some() {
                    let _ = unsafe { RevokeDragDrop(window) };
                }
                if let Some(menu) = chrome.menu.take() {
                    let _ = unsafe { SetMenu(window, None) };
                    let _ = unsafe { DestroyMenu(menu) };
                }
            }
            unsafe {
                SetWindowLongPtrW(window, GWLP_USERDATA, 0);
                SetWindowLongPtrW(window, STATE_WINDOW_WORD, 0);
            }
        }
        _ => {}
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Stable token naming how a recovery action ended, for the diagnostic log.
const fn recovery_outcome_token(
    outcome: &std::result::Result<PendingRecoveryExecution, PendingRecoveryExecutionError>,
) -> &'static str {
    match outcome {
        Ok(PendingRecoveryExecution::Restored { .. }) => "restored",
        Ok(PendingRecoveryExecution::VerifiedRecordCleared) => "record_cleared",
        Ok(PendingRecoveryExecution::PendingRecordRetained) => "record_retained",
        Ok(PendingRecoveryExecution::RestoreConflict { .. }) => "restore_conflict",
        Err(_) => "failed",
    }
}

/// Stable token naming what startup recovery found, for the diagnostic log.
const fn startup_recovery_token(
    recovery: &std::result::Result<StartupRecoveryOutcome, StartupRecoveryError>,
) -> &'static str {
    match recovery {
        Ok(StartupRecoveryOutcome::NoPendingRecord) => "no_pending_record",
        Ok(StartupRecoveryOutcome::VerifiedRecordCleaned { .. }) => "verified_record_cleaned",
        Ok(StartupRecoveryOutcome::UserDecisionRequired { .. }) => "user_decision_required",
        Err(StartupRecoveryError::Store(_)) => "store_unreadable",
        Err(StartupRecoveryError::Region(_)) => "region_unreadable",
    }
}

/// Window word holding the `AppState` pointer, next to chrome in `GWLP_USERDATA`.
const STATE_WINDOW_WORD: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(0);

/// Borrow the long-lived window resources.
///
/// This is the only accessor a re-entrant message may use. It hands out a
/// shared reference into an allocation that no handler ever borrows mutably
/// while a modal call is possible, so it cannot alias `state_mut`.
/// Fold one handoff report into the window's presentation state.
///
/// A finished handoff is kept rather than forgotten: its last sentence is the
/// only place the user is told the region came back. It stops blocking a new
/// operation through its stage, and the next operation clears it.
fn apply_handoff_update(state: &mut AppState, update: HandoffUpdate) {
    state.operation_failure = update.failure;
    let Some(handoff) = state.handoff.as_mut() else {
        return;
    };
    handoff.stage = update.stage;
    if update.original_region.is_some() {
        handoff.original_region = update.original_region;
    }
    if update.file.is_some() {
        handoff.file = update.file;
    }
}

/// Fold one installation report into the window's presentation state.
///
/// The operation is only forgotten once it can report nothing further, so a
/// finished, uncertain, or failed operation still shows what it ended with.
fn apply_installation_update(state: &mut AppState, update: InstallationUpdate) {
    state.operation_state = Some(update.state);
    state.operation_failure = update.failure;
    let running = state.running_operation.get_or_insert_default();
    if update.phase.is_some() {
        running.phase = update.phase;
    }
    if update.progress.is_some() {
        running.progress = update.progress;
    }
    if let Some(product) = update.resolved {
        running.product_name = Some(product.display_name.clone());
        if let Some(resolve) = running.resolve.clone() {
            // The card found under the temporary region is the operation's own
            // answer, so it lands in the same place as any other resolve.
            state.resolution.complete(&resolve, Ok(product));
        }
    }
    running.region_restored_early = update.region_restored_early;
    if update.state.is_terminal() || state.operation_failure.is_some() {
        state.running_operation = None;
    }
}

#[allow(unsafe_code)]
unsafe fn chrome_ref(window: HWND) -> Option<&'static WindowChrome> {
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) };
    (pointer != 0).then(|| unsafe { &*(pointer as *const WindowChrome) })
}

/// Borrow the window resources exclusively.
///
/// Only creation and destruction may use this, and neither may make a modal
/// call while the borrow is alive.
#[allow(unsafe_code)]
unsafe fn chrome_mut(window: HWND) -> Option<&'static mut WindowChrome> {
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) };
    (pointer != 0).then(|| unsafe { &mut *(pointer as *mut WindowChrome) })
}

/// Borrow the presentation state exclusively.
///
/// It lives in its own allocation, so an outer handler can hold this across a
/// modal call while re-entrant messages take `chrome_ref`.
#[allow(unsafe_code)]
unsafe fn state_mut(window: HWND) -> Option<&'static mut AppState> {
    let pointer = unsafe { GetWindowLongPtrW(window, STATE_WINDOW_WORD) };
    (pointer != 0).then(|| unsafe { &mut *(pointer as *mut AppState) })
}

#[allow(unsafe_code)]
unsafe fn state_ref(window: HWND) -> Option<&'static AppState> {
    let pointer = unsafe { GetWindowLongPtrW(window, STATE_WINDOW_WORD) };
    (pointer != 0).then(|| unsafe { &*(pointer as *const AppState) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::state::UiUpdate;
    use winstoreregion_core::InstallationOperationState;

    #[test]
    fn every_message_a_worker_can_post_is_held_back_during_a_modal_call() {
        // A handler for one of these takes `&mut AppState`, and a modal dialog
        // dispatches whatever is in the queue while an outer handler already
        // holds that borrow. Missing one here is how that becomes a crash.
        for offset in 1..=10 {
            let message = windows::Win32::UI::WindowsAndMessaging::WM_APP + offset;
            assert!(
                is_posted_application_message(message),
                "WM_APP + {offset} is posted to the window but would be handled                  during a modal call; add it to `is_posted_application_message`"
            );
        }
        assert!(
            !is_posted_application_message(windows::Win32::UI::WindowsAndMessaging::WM_APP + 14),
            "a new posted message was added: extend `is_posted_application_message`              and the range this test walks"
        );
        // Everything the user drives reaches a window a modal call has disabled,
        // and painting reads chrome rather than state, so none of these belong.
        for message in [
            WM_COMMAND, WM_NOTIFY, WM_PAINT, WM_CLOSE, WM_CREATE, WM_SIZE,
        ] {
            assert!(!is_posted_application_message(message));
        }
    }

    #[test]
    fn close_policy_requires_an_explicit_choice_only_while_recovery_is_needed() {
        assert_eq!(
            window_close_requirement(None),
            WindowCloseRequirement::ImmediateExit
        );
        assert_eq!(
            window_close_requirement(Some(InstallationOperationState::Completed)),
            WindowCloseRequirement::ImmediateExit
        );
        assert_eq!(
            window_close_requirement(Some(InstallationOperationState::Installing)),
            WindowCloseRequirement::ConfirmActiveOperation(InstallationOperationState::Installing)
        );
        assert_eq!(
            window_close_requirement(Some(InstallationOperationState::RestoreFailed)),
            WindowCloseRequirement::ConfirmActiveOperation(
                InstallationOperationState::RestoreFailed
            )
        );

        let update = UiUpdate::OperationState(InstallationOperationState::Installing);
        assert!(matches!(
            update,
            UiUpdate::OperationState(InstallationOperationState::Installing)
        ));
    }
}
