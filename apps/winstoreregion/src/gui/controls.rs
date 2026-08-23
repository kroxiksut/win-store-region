//! Creation of the child controls and their static content.

use crate::gui::ids::{
    ID_CHECK_REMAINING, ID_CLEAR_FILE, ID_DETAILS, ID_FIND_REGION, ID_INSTALL, ID_JOURNAL_CLEAR,
    ID_JOURNAL_COPY_ID, ID_JOURNAL_DELETE, ID_JOURNAL_LIST, ID_JOURNAL_OPEN_STORE,
    ID_JOURNAL_REPEAT, ID_LANGUAGE, ID_OPEN_STORE_PAGE, ID_RESTORE_REGION, ID_SHOW_ALL_REGIONS,
    ID_SOURCE_ACTION, ID_SOURCE_FILE, ID_SOURCE_LINK, ID_STORE_INPUT, ID_TABS, ID_TEMPORARY_GEO_ID,
    ID_UPDATES_LIST, ID_UPDATES_OPEN, ID_UPDATES_REFRESH, STATIC_ICON_STYLE,
};
use crate::gui::render::region_choice_label;
use crate::gui::state::{AppState, Controls, Tab, WindowChrome};
use crate::gui::strings::{LANGUAGE_NAMES, Language, Strings};
use std::ffi::c_void;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVM_INSERTCOLUMNW, LVM_SETCOLUMNW,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVS_EX_FULLROWSELECT, LVS_EX_GRIDLINES, LVS_REPORT,
    LVS_SHOWSELALWAYS, LVS_SINGLESEL, PBS_MARQUEE, PROGRESS_CLASS, TCIF_TEXT, TCITEMW,
    TCM_DELETEALLITEMS, TCM_INSERTITEMW, TCM_SETCURSEL, WC_LISTVIEWW, WC_TABCONTROLW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_SETCHECK, BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON, CB_ADDSTRING, CB_SETCURSEL,
    CBS_DROPDOWNLIST, CreateWindowExW, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY, HMENU,
    STM_SETICON, SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WS_BORDER, WS_CHILD, WS_DISABLED,
    WS_GROUP, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HSTRING, PCWSTR, PWSTR, Result, w};
use winstoreregion_core::{ApplicationSourceKind, Region};

#[allow(clippy::too_many_lines, unsafe_code)]
pub(super) unsafe fn create_controls(
    window: HWND,
    chrome: &WindowChrome,
    state: &AppState,
) -> Result<Controls> {
    let strings = state.language.strings();
    let brand_badge = unsafe { create_icon_static(window, chrome.instance, chrome.app_icon)? };
    let title = unsafe { create_static(window, strings.title, chrome.instance)? };
    let subtitle = unsafe { create_static(window, strings.subtitle, chrome.instance)? };
    let language_label = unsafe { create_static(window, strings.language, chrome.instance)? };
    let language = unsafe {
        create_child(
            window,
            w!("COMBOBOX"),
            PCWSTR::null(),
            WS_BORDER | WS_CHILD | WS_TABSTOP | WS_VISIBLE | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
            chrome.instance,
            Some(ID_LANGUAGE),
        )?
    };
    unsafe { add_language_choices(language, state.language) };
    let tabs = unsafe {
        create_child(
            window,
            WC_TABCONTROLW,
            PCWSTR::null(),
            WS_CHILD | WS_TABSTOP | WS_VISIBLE,
            chrome.instance,
            Some(ID_TABS),
        )?
    };
    let tab_content = unsafe { create_framed_static(window, chrome.instance)? };
    let journal_list = unsafe { create_table(window, chrome.instance, ID_JOURNAL_LIST)? };
    unsafe { add_columns(journal_list, strings.journal_columns) };
    let journal_details = unsafe { create_framed_readonly_text(window, chrome.instance)? };
    let journal_open_store = unsafe {
        create_button(
            window,
            strings.journal_open_store,
            WS_DISABLED,
            chrome.instance,
            Some(ID_JOURNAL_OPEN_STORE),
        )?
    };
    let journal_repeat = unsafe {
        create_button(
            window,
            strings.journal_repeat,
            WS_DISABLED,
            chrome.instance,
            Some(ID_JOURNAL_REPEAT),
        )?
    };
    let journal_copy_id = unsafe {
        create_button(
            window,
            strings.journal_copy_id,
            WS_DISABLED,
            chrome.instance,
            Some(ID_JOURNAL_COPY_ID),
        )?
    };
    let journal_delete = unsafe {
        create_button(
            window,
            strings.journal_delete,
            WS_DISABLED,
            chrome.instance,
            Some(ID_JOURNAL_DELETE),
        )?
    };
    let journal_clear = unsafe {
        create_button(
            window,
            strings.journal_clear,
            WS_DISABLED,
            chrome.instance,
            Some(ID_JOURNAL_CLEAR),
        )?
    };
    // Updates tab: same shape as the journal — one chooser, one details box,
    // and its own buttons on the shared command row.
    let updates_list = unsafe { create_table(window, chrome.instance, ID_UPDATES_LIST)? };
    unsafe { add_columns(updates_list, strings.updates_columns) };
    let updates_details = unsafe { create_framed_readonly_text(window, chrome.instance)? };
    let updates_refresh = unsafe {
        create_button(
            window,
            strings.updates_refresh,
            WINDOW_STYLE::default(),
            chrome.instance,
            Some(ID_UPDATES_REFRESH),
        )?
    };
    let updates_open = unsafe {
        create_button(
            window,
            strings.updates_open,
            WS_DISABLED,
            chrome.instance,
            Some(ID_UPDATES_OPEN),
        )?
    };
    let source_panel = unsafe { create_framed_static(window, chrome.instance)? };
    let source_title = unsafe { create_static(window, strings.source_title, chrome.instance)? };
    let source_hint = unsafe { create_static(window, strings.source_hint, chrome.instance)? };
    let source_link = unsafe {
        create_button(
            window,
            strings.source_link,
            // `WS_GROUP` opens the source group: arrows move inside it and Tab
            // leaves it, which is what a radio pair must do.
            WINDOW_STYLE(BS_AUTORADIOBUTTON as u32) | WS_GROUP,
            chrome.instance,
            Some(ID_SOURCE_LINK),
        )?
    };
    let source_file = unsafe {
        create_button(
            window,
            strings.source_file,
            WINDOW_STYLE(BS_AUTORADIOBUTTON as u32),
            chrome.instance,
            Some(ID_SOURCE_FILE),
        )?
    };
    let link_checked =
        usize::from(state.application_source.selected() == ApplicationSourceKind::StoreText);
    let file_checked =
        usize::from(state.application_source.selected() == ApplicationSourceKind::InstallerFile);
    let _ = unsafe { SendMessageW(source_link, BM_SETCHECK, Some(WPARAM(link_checked)), None) };
    let _ = unsafe { SendMessageW(source_file, BM_SETCHECK, Some(WPARAM(file_checked)), None) };
    let input_label = unsafe { create_static(window, strings.input, chrome.instance)? };
    let initial_input = HSTRING::from(state.application_source.store_text());
    let input = unsafe {
        create_child(
            window,
            w!("EDIT"),
            PCWSTR(initial_input.as_ptr()),
            WS_BORDER | WS_CHILD | WS_TABSTOP | WS_VISIBLE,
            chrome.instance,
            Some(ID_STORE_INPUT),
        )?
    };
    let file_path = unsafe { create_framed_static(window, chrome.instance)? };
    let clear_file = unsafe {
        create_button(
            window,
            strings.clear_file,
            WINDOW_STYLE::default(),
            chrome.instance,
            Some(ID_CLEAR_FILE),
        )?
    };
    let source_action = unsafe {
        create_button(
            window,
            strings.paste,
            WINDOW_STYLE::default(),
            chrome.instance,
            Some(ID_SOURCE_ACTION),
        )?
    };
    let app_card = unsafe { create_framed_readonly_text(window, chrome.instance)? };
    let region_panel = unsafe { create_framed_static(window, chrome.instance)? };
    let current_region_label =
        unsafe { create_static(window, strings.current_region, chrome.instance)? };
    let current_region = unsafe { create_framed_static(window, chrome.instance)? };
    let temporary_region_label =
        unsafe { create_static(window, strings.temporary_region, chrome.instance)? };
    let temporary_region = unsafe {
        create_child(
            window,
            w!("COMBOBOX"),
            PCWSTR::null(),
            WS_BORDER | WS_CHILD | WS_TABSTOP | WS_VISIBLE | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
            chrome.instance,
            Some(ID_TEMPORARY_GEO_ID),
        )?
    };
    unsafe {
        add_temporary_region_choices(
            temporary_region,
            &state.temporary_regions,
            state.selected_temporary_region.as_ref(),
            &strings,
        );
    }
    let find_region = unsafe {
        create_button(
            window,
            strings.find_region,
            WINDOW_STYLE::default() | WS_DISABLED,
            chrome.instance,
            Some(ID_FIND_REGION),
        )?
    };
    let check_remaining = unsafe {
        create_button(
            window,
            strings.check_remaining,
            WINDOW_STYLE::default() | WS_DISABLED,
            chrome.instance,
            Some(ID_CHECK_REMAINING),
        )?
    };
    let show_all_regions = unsafe {
        create_button(
            window,
            strings.show_all_regions,
            WINDOW_STYLE(BS_AUTOCHECKBOX as u32) | WS_DISABLED,
            chrome.instance,
            Some(ID_SHOW_ALL_REGIONS),
        )?
    };
    let availability_status =
        unsafe { create_static(window, strings.availability_idle, chrome.instance)? };
    let status = unsafe { create_framed_readonly_text(window, chrome.instance)? };
    // Created hidden and with the marquee style already on: an installation
    // reports no percentage for its first fifteen seconds,
    // and a bar sitting at zero for that long reads as a
    // stuck installation rather than a running one.
    let progress = unsafe {
        create_child(
            window,
            PROGRESS_CLASS,
            PCWSTR::null(),
            WS_CHILD | WINDOW_STYLE(PBS_MARQUEE),
            chrome.instance,
            None,
        )?
    };
    let install = unsafe {
        let disabled = if state.install_is_available() {
            WINDOW_STYLE::default()
        } else {
            WS_DISABLED
        };
        create_button(
            window,
            strings.install,
            WINDOW_STYLE(BS_DEFPUSHBUTTON as u32) | disabled,
            chrome.instance,
            Some(ID_INSTALL),
        )?
    };
    let restore = unsafe {
        create_button(
            window,
            strings.restore,
            WINDOW_STYLE::default() | WS_DISABLED,
            chrome.instance,
            Some(ID_RESTORE_REGION),
        )?
    };
    let open_store_page = unsafe {
        let disabled = if state.resolved_product().is_some() {
            WINDOW_STYLE::default()
        } else {
            WS_DISABLED
        };
        create_button(
            window,
            strings.open_store_page,
            disabled,
            chrome.instance,
            Some(ID_OPEN_STORE_PAGE),
        )?
    };
    let details = unsafe {
        create_button(
            window,
            strings.details,
            WINDOW_STYLE::default() | WS_DISABLED,
            chrome.instance,
            Some(ID_DETAILS),
        )?
    };
    let drop_overlay =
        unsafe { create_hidden_framed_static(window, strings.drop_overlay, chrome.instance)? };
    Ok(Controls {
        brand_badge,
        title,
        subtitle,
        language_label,
        language,
        tabs,
        tab_content,
        journal_list,
        journal_details,
        journal_open_store,
        journal_repeat,
        journal_delete,
        journal_copy_id,
        journal_clear,
        updates_list,
        updates_details,
        updates_refresh,
        updates_open,
        source_panel,
        source_title,
        source_hint,
        source_link,
        source_file,
        input_label,
        input,
        file_path,
        source_action,
        clear_file,
        drop_overlay,
        app_card,
        region_panel,
        current_region_label,
        current_region,
        temporary_region_label,
        temporary_region,
        find_region,
        check_remaining,
        show_all_regions,
        availability_status,
        status,
        progress,
        install,
        restore,
        open_store_page,
        details,
    })
}

/// One report-mode table: whole-row selection, grid lines, one selected row.
///
/// A drop-down list cannot do this: it shows one line at a time, so a list of
/// results reads as an empty one until it is opened.
#[allow(unsafe_code)]
unsafe fn create_table(window: HWND, instance: HINSTANCE, id: usize) -> Result<HWND> {
    let table = unsafe {
        create_child(
            window,
            WC_LISTVIEWW,
            PCWSTR::null(),
            WS_BORDER
                | WS_CHILD
                | WS_TABSTOP
                | WS_VISIBLE
                | WINDOW_STYLE(LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS),
            instance,
            Some(id),
        )?
    };
    let _ = unsafe {
        SendMessageW(
            table,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(0)),
            Some(LPARAM(
                isize::try_from(LVS_EX_FULLROWSELECT | LVS_EX_GRIDLINES).unwrap_or(0),
            )),
        )
    };
    Ok(table)
}

/// Put the current language's headings on both tables.
///
/// A list-view column keeps the text it was created with, and `create_controls`
/// runs once. Every other caption is rewritten by a render, so the headings were
/// the one place a language change could not reach: switching the interface left
/// two tables labelled in whatever language the window had started in.
#[allow(unsafe_code)]
pub(super) unsafe fn refresh_column_headings(
    chrome: &WindowChrome,
    controls: &Controls,
    state: &AppState,
) {
    if chrome.heading_language.get() == Some(state.language) {
        return;
    }
    let strings = state.language.strings();
    unsafe {
        set_columns(controls.journal_list, strings.journal_columns);
        set_columns(controls.updates_list, strings.updates_columns);
    }
    chrome.heading_language.set(Some(state.language));
}

#[allow(unsafe_code)]
unsafe fn set_columns(table: HWND, headings: [&'static str; 4]) {
    for (index, heading) in headings.into_iter().enumerate() {
        let heading = HSTRING::from(heading);
        let mut column = LVCOLUMNW {
            mask: LVCF_TEXT,
            pszText: PWSTR(heading.as_ptr().cast_mut()),
            ..LVCOLUMNW::default()
        };
        let _ = unsafe {
            SendMessageW(
                table,
                LVM_SETCOLUMNW,
                Some(WPARAM(index)),
                Some(LPARAM(std::ptr::from_mut(&mut column) as isize)),
            )
        };
    }
}

/// Give a table its headings. Widths follow the window and are set by the layout.
#[allow(unsafe_code)]
unsafe fn add_columns(table: HWND, headings: [&'static str; 4]) {
    for (index, heading) in headings.into_iter().enumerate() {
        let heading = HSTRING::from(heading);
        let mut column = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH,
            cx: 160,
            pszText: PWSTR(heading.as_ptr().cast_mut()),
            ..LVCOLUMNW::default()
        };
        let _ = unsafe {
            SendMessageW(
                table,
                LVM_INSERTCOLUMNW,
                Some(WPARAM(index)),
                Some(LPARAM(std::ptr::from_mut(&mut column) as isize)),
            )
        };
    }
}

unsafe fn create_static(window: HWND, text: &str, instance: HINSTANCE) -> Result<HWND> {
    let text = HSTRING::from(text);
    unsafe {
        create_child(
            window,
            w!("STATIC"),
            PCWSTR(text.as_ptr()),
            WS_CHILD | WS_VISIBLE,
            instance,
            None,
        )
    }
}

#[allow(unsafe_code)]
unsafe fn create_icon_static(
    window: HWND,
    instance: HINSTANCE,
    icon: windows::Win32::UI::WindowsAndMessaging::HICON,
) -> Result<HWND> {
    let control = unsafe {
        create_child(
            window,
            w!("STATIC"),
            PCWSTR::null(),
            STATIC_ICON_STYLE | WS_CHILD | WS_VISIBLE,
            instance,
            None,
        )?
    };
    let _ = unsafe { SendMessageW(control, STM_SETICON, Some(WPARAM(icon.0 as usize)), None) };
    Ok(control)
}

/// A boxed area whose outline the window paints for itself.
///
/// The system border is deliberately absent: `WS_BORDER` is black and one
/// pixel of it reads as a heavy frame, while the agreed design outlines these
/// areas in a light colour. `framed_controls` lists what gets an outline.
/// A framed box whose text the user can select and copy.
///
/// A read-only multiline edit rather than a static: the card, the status and
/// the journal details are exactly the text a person needs to paste into a
/// message or a bug report, and a static offers no way to take it. Read-only
/// edits answer `WM_CTLCOLORSTATIC`, so the window's own colouring still
/// applies, and the frame is painted around them as before. They are left out
/// of the tab order on purpose: reaching them costs a click, and putting three
/// text boxes between the fields and the Install button would cost every
/// keyboard user three keystrokes each time.
#[allow(unsafe_code)]
/// A framed, selectable, read-only block of text that can outgrow its box.
///
/// `WS_VSCROLL` is not decoration. These blocks hold a status sentence, a card,
/// or a journal entry, and any of them can be longer than the space the layout
/// has to give. Without a scrollbar the text is simply cut off with nothing to
/// say so, and the only way to read the rest is to drag the window bigger —
/// which is what the user ended up doing.
unsafe fn create_framed_readonly_text(window: HWND, instance: HINSTANCE) -> Result<HWND> {
    unsafe {
        create_child(
            window,
            w!("EDIT"),
            PCWSTR::null(),
            WS_CHILD
                | WS_VISIBLE
                | WS_VSCROLL
                | WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32),
            instance,
            None,
        )
    }
}

#[allow(unsafe_code)]
unsafe fn create_framed_static(window: HWND, instance: HINSTANCE) -> Result<HWND> {
    unsafe {
        create_child(
            window,
            w!("STATIC"),
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE,
            instance,
            None,
        )
    }
}

#[allow(unsafe_code)]
unsafe fn create_hidden_framed_static(
    window: HWND,
    text: &str,
    instance: HINSTANCE,
) -> Result<HWND> {
    let text = HSTRING::from(text);
    unsafe {
        create_child(
            window,
            w!("STATIC"),
            PCWSTR(text.as_ptr()),
            WS_CHILD,
            instance,
            None,
        )
    }
}

#[allow(unsafe_code)]
unsafe fn create_button(
    window: HWND,
    text: &str,
    style: WINDOW_STYLE,
    instance: HINSTANCE,
    id: Option<usize>,
) -> Result<HWND> {
    let text = HSTRING::from(text);
    unsafe {
        create_child(
            window,
            w!("BUTTON"),
            PCWSTR(text.as_ptr()),
            style | WS_CHILD | WS_TABSTOP | WS_VISIBLE,
            instance,
            id,
        )
    }
}

#[allow(unsafe_code)]
unsafe fn create_child(
    window: HWND,
    class: PCWSTR,
    text: PCWSTR,
    style: WINDOW_STYLE,
    instance: HINSTANCE,
    id: Option<usize>,
) -> Result<HWND> {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            text,
            style,
            0,
            0,
            0,
            0,
            Some(window),
            id.map(|identifier| HMENU(identifier as *mut c_void)),
            Some(instance),
            None,
        )
    }
}

#[allow(unsafe_code)]
unsafe fn add_language_choices(window: HWND, selected: Language) {
    for name in LANGUAGE_NAMES {
        let name = HSTRING::from(name);
        let _ = unsafe {
            SendMessageW(
                window,
                CB_ADDSTRING,
                None,
                Some(LPARAM(name.as_ptr() as isize)),
            )
        };
    }
    let _ = unsafe { SendMessageW(window, CB_SETCURSEL, Some(WPARAM(selected.index())), None) };
}

#[allow(unsafe_code)]
unsafe fn add_temporary_region_choices(
    window: HWND,
    regions: &[Region],
    selected: Option<&Region>,
    strings: &Strings,
) {
    for region in regions {
        let text = HSTRING::from(region_choice_label(region, strings));
        let _ = unsafe {
            SendMessageW(
                window,
                CB_ADDSTRING,
                None,
                Some(LPARAM(text.as_ptr() as isize)),
            )
        };
    }
    if let Some(selected) = selected
        && let Some(index) = regions
            .iter()
            .position(|region| region.geo_id == selected.geo_id)
    {
        let _ = unsafe { SendMessageW(window, CB_SETCURSEL, Some(WPARAM(index)), None) };
    }
}

pub(super) fn installation_controls(controls: &Controls) -> [HWND; 26] {
    [
        controls.source_panel,
        controls.source_title,
        controls.source_hint,
        controls.source_link,
        controls.source_file,
        controls.input_label,
        controls.input,
        controls.file_path,
        controls.source_action,
        controls.clear_file,
        controls.drop_overlay,
        controls.app_card,
        controls.region_panel,
        controls.current_region_label,
        controls.current_region,
        controls.temporary_region_label,
        controls.temporary_region,
        controls.find_region,
        controls.check_remaining,
        controls.show_all_regions,
        controls.availability_status,
        controls.status,
        controls.install,
        controls.restore,
        controls.open_store_page,
        controls.details,
    ]
}

/// Every area the window outlines for itself.
///
/// These are the children created without a system border, so this list and
/// `create_framed_static` must stay in step: a box missing here simply has no
/// frame on screen.
pub(super) fn framed_controls(controls: &Controls) -> [HWND; 9] {
    [
        controls.tab_content,
        controls.journal_details,
        controls.source_panel,
        controls.file_path,
        controls.app_card,
        controls.region_panel,
        controls.current_region,
        controls.status,
        controls.drop_overlay,
    ]
}

/// Every control that belongs to the Updates tab and nowhere else.
pub(super) fn updates_controls(controls: &Controls) -> [HWND; 4] {
    [
        controls.updates_list,
        controls.updates_details,
        controls.updates_refresh,
        controls.updates_open,
    ]
}

pub(super) fn journal_controls(controls: &Controls) -> [HWND; 7] {
    [
        controls.journal_list,
        controls.journal_details,
        controls.journal_open_store,
        controls.journal_repeat,
        controls.journal_delete,
        controls.journal_clear,
        controls.journal_copy_id,
    ]
}

#[allow(unsafe_code)]
pub(super) unsafe fn rebuild_tabs(window: HWND, selected: Tab, strings: &Strings) {
    let _ = unsafe { SendMessageW(window, TCM_DELETEALLITEMS, None, None) };
    for (index, text) in strings.tabs.into_iter().enumerate() {
        let text = HSTRING::from(text);
        let mut item = TCITEMW {
            mask: TCIF_TEXT,
            pszText: PWSTR(text.as_ptr().cast_mut()),
            ..Default::default()
        };
        let _ = unsafe {
            SendMessageW(
                window,
                TCM_INSERTITEMW,
                Some(WPARAM(index)),
                Some(LPARAM((&raw mut item) as isize)),
            )
        };
    }
    let _ = unsafe { SendMessageW(window, TCM_SETCURSEL, Some(WPARAM(selected.index())), None) };
}

#[cfg(test)]
mod tests {
    /// Order in which `create_controls` creates children.
    ///
    /// Win32 derives z-order from creation order, and `IsDialogMessageW` walks
    /// z-order, so this list **is** the keyboard tab order. It must follow the
    /// visual order of the agreed baseline, and every labelled control must be
    /// preceded by its own `STATIC`, because that is where a screen reader
    /// takes the accessible name from.
    const EXPECTED_ORDER: [&str; 45] = [
        "brand_badge",
        "title",
        "subtitle",
        "language_label",
        "language",
        "tabs",
        "tab_content",
        "journal_list",
        "journal_details",
        "journal_open_store",
        "journal_repeat",
        "journal_copy_id",
        "journal_delete",
        "journal_clear",
        "updates_list",
        "updates_details",
        "updates_refresh",
        "updates_open",
        "source_panel",
        "source_title",
        "source_hint",
        "source_link",
        "source_file",
        "input_label",
        "input",
        "file_path",
        "clear_file",
        "source_action",
        "app_card",
        "region_panel",
        "current_region_label",
        "current_region",
        "temporary_region_label",
        "temporary_region",
        "find_region",
        "check_remaining",
        "show_all_regions",
        "availability_status",
        "status",
        "progress",
        "install",
        "restore",
        "open_store_page",
        "details",
        "drop_overlay",
    ];

    /// Controls with no text of their own and the label that must precede them.
    const LABELLED_CONTROLS: [(&str, &str); 4] = [
        ("language", "language_label"),
        ("input", "input_label"),
        ("current_region", "current_region_label"),
        ("temporary_region", "temporary_region_label"),
    ];

    fn creation_order() -> Vec<String> {
        let source = include_str!("controls.rs");
        let start = source
            .find("pub(super) unsafe fn create_controls")
            .expect("the control factory remains present");
        let end = source[start..]
            .find("Ok(Controls {")
            .expect("the factory still returns a control set")
            + start;
        let skipped = ["strings", "initial_input", "link_checked", "file_checked"];
        source[start..end]
            .lines()
            .filter_map(|line| line.strip_prefix("    let "))
            .filter_map(|rest| rest.split(['=', ':', ' ']).next())
            .filter(|name| !name.is_empty() && *name != "_" && !skipped.contains(name))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn tab_order_follows_the_visual_order_of_the_agreed_baseline() {
        assert_eq!(creation_order(), EXPECTED_ORDER);
    }

    #[test]
    fn every_labelled_control_is_preceded_by_its_own_label() {
        let order = creation_order();
        for (control, label) in LABELLED_CONTROLS {
            let control_index = order
                .iter()
                .position(|name| name == control)
                .unwrap_or_else(|| panic!("{control} is no longer created"));
            let label_index = order
                .iter()
                .position(|name| name == label)
                .unwrap_or_else(|| panic!("{label} is no longer created"));
            assert_eq!(
                label_index + 1,
                control_index,
                "{label} must be created immediately before {control}"
            );
        }
    }
}
