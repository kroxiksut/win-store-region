//! Rendering presentation state into the existing controls.

use crate::gui::controls::{
    installation_controls, journal_controls, rebuild_tabs, refresh_column_headings,
    updates_controls,
};
use crate::gui::diagnostic::{
    diagnostic_details, diagnostic_message, file_selection_diagnostic, prerequisite_status,
    stub_diagnostic,
};
use crate::gui::direction::apply_interface_direction;
use crate::gui::handoff::HandoffStage;
use crate::gui::ids::WindowLong;
use crate::gui::journal::{insert_row, journal_details, rebuild_journal_list};
use crate::gui::layout::layout_controls;
use crate::gui::state::{
    AppState, Controls, FileSelectionError, MarketSurvey, RunningOperation, Tab, UpdatesView,
    WindowChrome,
};
use crate::gui::strings::{Language, Strings, fill};
use crate::platform::installer_download::InstallerDownloadError;
use crate::platform::stub::StubInspectionError;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::{HWND, WPARAM};
use windows::Win32::UI::Controls::{
    LVM_DELETEALLITEMS, PBM_SETMARQUEE, PBM_SETPOS, PBM_SETRANGE32, PBS_MARQUEE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_SETCHECK, CB_ADDSTRING, CB_RESETCONTENT, CB_SETCURSEL, GWL_STYLE, GetWindowLongPtrW,
    SW_HIDE, SW_SHOW, SendMessageW, SetWindowLongPtrW, SetWindowTextW, ShowWindow, WINDOW_STYLE,
    WS_DISABLED,
};
use windows::core::HSTRING;
use winstoreregion_core::{
    ApplicationSourceKind, DeviceCompatibility, Diagnostic, HandoffRefusal, InstallPhase,
    InstallationOperationState, MarketAvailability, MarketCode, OfferedProduct,
    ProductResolutionState, Region, ResolveError, StoreInputCandidate, StoreInputError,
    SurveyScope, UpdateCandidate, admit_installer_handoff, curated_markets,
};

/// Write text into a control only when it differs from what is already there.
///
/// `SetWindowTextW` repaints unconditionally, even for identical text. The
/// window renders once per posted result, and a forty-market search posts forty
/// of them, so rewriting two dozen unchanged captions each time is what the
/// user sees as flicker.
#[allow(unsafe_code)]
unsafe fn set_text(chrome: &WindowChrome, control: HWND, text: &str) {
    let key = control.0 as usize;
    if chrome
        .last_text
        .borrow()
        .get(&key)
        .is_some_and(|written| written == text)
    {
        return;
    }
    let value = HSTRING::from(text);
    let _ = unsafe { SetWindowTextW(control, &value) };
    chrome.last_text.borrow_mut().insert(key, text.to_owned());
}

#[allow(clippy::too_many_lines, unsafe_code)]
pub(super) unsafe fn render(window: HWND, chrome: &WindowChrome, state: &AppState) {
    let strings = state.language.strings();
    let Some(controls) = chrome.controls.as_ref() else {
        return;
    };
    // Every value is computed first, then written only where it
    // differs from what the control already shows. Rewriting identical
    // text still repaints the control, and a forty-market search renders
    // the window once per answer: that is the flicker.
    let file_path = state.application_source.installer_file().map_or_else(
        || strings.no_file.to_owned(),
        |path| path.as_path().display().to_string(),
    );
    // A filled field has nothing left to paste into it, and the owner read the
    // unchanged caption as a button that had stopped working. The file source
    // already had its own clear button; the link field now has the same way out.
    let source_action = match state.application_source.selected() {
        ApplicationSourceKind::StoreText => {
            if state.application_source.store_text().is_empty() {
                strings.paste
            } else {
                strings.clear_input
            }
        }
        ApplicationSourceKind::InstallerFile => strings.choose_file,
    };
    // One control, two meanings, chosen by what the window can actually do:
    // a handoff that still holds the region has to give it back before
    // anything else, so the second meaning only appears when it does not.
    let restore_action = if state.installer_download.is_some() {
        strings.downloading_installer
    } else if state.store_installer_download_is_available() {
        strings.download_installer
    } else {
        strings.restore
    };
    let app_card = copyable(&application_card(state, &strings));
    let current_region = region_summary(state, &strings);
    let availability_status = availability_status(state, &strings);
    unsafe {
        set_text(chrome, controls.title, strings.title);
    }
    unsafe {
        set_text(chrome, controls.subtitle, strings.subtitle);
    }
    unsafe {
        set_text(chrome, controls.language_label, strings.language);
    }
    unsafe {
        set_text(chrome, controls.input_label, strings.input);
    }
    unsafe {
        set_text(chrome, controls.source_title, strings.source_title);
    }
    unsafe {
        set_text(chrome, controls.source_hint, strings.source_hint);
    }
    unsafe {
        set_text(chrome, controls.source_link, strings.source_link);
    }
    unsafe {
        set_text(chrome, controls.source_file, strings.source_file);
    }
    unsafe {
        set_text(chrome, controls.file_path, &file_path);
    }
    unsafe {
        set_text(chrome, controls.source_action, source_action);
    }
    unsafe {
        set_text(chrome, controls.clear_file, strings.clear_file);
    }
    unsafe {
        set_text(chrome, controls.drop_overlay, strings.drop_overlay);
    }
    unsafe {
        set_text(chrome, controls.app_card, &app_card);
    }
    unsafe {
        set_text(
            chrome,
            controls.current_region_label,
            strings.current_region,
        );
    }
    unsafe {
        set_text(chrome, controls.current_region, &current_region);
    }
    unsafe {
        set_text(
            chrome,
            controls.temporary_region_label,
            strings.temporary_region,
        );
    }
    unsafe {
        set_text(chrome, controls.find_region, strings.find_region);
    }
    unsafe {
        set_text(chrome, controls.check_remaining, strings.check_remaining);
    }
    unsafe {
        set_text(chrome, controls.show_all_regions, strings.show_all_regions);
    }
    unsafe {
        set_text(chrome, controls.availability_status, &availability_status);
    }
    unsafe { refresh_region_choices(chrome, controls, state) };
    let survey_running = state
        .availability
        .as_ref()
        .is_some_and(|availability| availability.running);
    // A file names no product, so the search answers from the preserved link
    // draft or not at all. It installs nothing either way.
    let _ = unsafe {
        EnableWindow(
            controls.find_region,
            state.searchable_product_id().is_some() && !survey_running,
        )
    };
    let _ = unsafe { EnableWindow(controls.check_remaining, remaining_markets_exist(state)) };
    let _ = unsafe {
        EnableWindow(
            controls.show_all_regions,
            state.show_all_regions || state.regions_are_narrowed(),
        )
    };
    let _ = unsafe {
        SendMessageW(
            controls.show_all_regions,
            BM_SETCHECK,
            Some(WPARAM(usize::from(state.show_all_regions))),
            None,
        )
    };
    let status = copyable(&state.operation_status);
    let journal_details = copyable(&journal_details(state, &strings));
    let updates_details = copyable(&updates_details(state, &strings));
    unsafe {
        set_text(chrome, controls.status, &status);
    }
    unsafe {
        set_text(chrome, controls.install, strings.install);
    }
    unsafe {
        set_text(chrome, controls.restore, restore_action);
    }
    unsafe {
        set_text(chrome, controls.open_store_page, strings.open_store_page);
    }
    unsafe {
        set_text(chrome, controls.details, strings.details);
    }
    unsafe {
        set_text(chrome, controls.tab_content, tab_content(state, &strings));
    }
    unsafe {
        set_text(chrome, controls.journal_details, &journal_details);
    }
    unsafe {
        set_text(chrome, controls.updates_details, &updates_details);
    }
    unsafe {
        set_text(chrome, controls.updates_refresh, strings.updates_refresh);
    }
    unsafe {
        set_text(chrome, controls.updates_open, strings.updates_open);
    }
    unsafe {
        set_text(
            chrome,
            controls.journal_open_store,
            strings.journal_open_store,
        );
    }
    unsafe {
        set_text(chrome, controls.journal_repeat, strings.journal_repeat);
    }
    unsafe {
        set_text(chrome, controls.journal_delete, strings.journal_delete);
    }
    unsafe {
        set_text(chrome, controls.journal_copy_id, strings.journal_copy_id);
    }
    unsafe {
        set_text(chrome, controls.journal_clear, strings.journal_clear);
    }
    unsafe { rebuild_journal_list(controls.journal_list, state) };
    let journal_selection_available = state.selected_journal_record().is_some();
    // Opening a page, repeating an operation and copying an identifier all need
    // a Product ID. A handoff entry has none, so those three stay unavailable
    // for it rather than acting on something the entry does not contain.
    let journal_product_available = state
        .selected_journal_record()
        .and_then(winstoreregion_core::JournalRecord::product_id)
        .is_some();
    for control in [
        controls.journal_open_store,
        controls.journal_repeat,
        controls.journal_copy_id,
    ] {
        let _ = unsafe { EnableWindow(control, journal_product_available) };
    }
    let _ = unsafe { EnableWindow(controls.journal_delete, journal_selection_available) };
    // Clearing is about the history as a whole, so it follows whether there is
    // any history rather than whether one entry happens to be selected.
    let journal_has_entries = matches!(&state.journal, crate::gui::state::JournalView::Ready(records) if !records.is_empty());
    let _ = unsafe { EnableWindow(controls.journal_clear, journal_has_entries) };
    let details_style = if state.stub_inspection.is_some() {
        WINDOW_STYLE::default()
    } else {
        WS_DISABLED
    };
    let _ = unsafe { EnableWindow(controls.details, details_style != WS_DISABLED) };
    let _ = unsafe { EnableWindow(controls.open_store_page, state.resolved_product().is_some()) };
    // The button is the gate made visible: everything it depends on lives in
    // the state's own gates, and nothing here may widen them. Which operation
    // it starts follows the selected source, not this button.
    let _ = unsafe { EnableWindow(controls.install, state.primary_command_is_available()) };
    // The region goes back only where a handoff actually left one to put back.
    let _ = unsafe {
        EnableWindow(
            controls.restore,
            state.region_restore_is_available() || state.store_installer_download_is_available(),
        )
    };
    let link_selected = state.application_source.selected() == ApplicationSourceKind::StoreText;
    let file_selected = !link_selected;
    let _ = unsafe {
        SendMessageW(
            controls.source_link,
            BM_SETCHECK,
            Some(WPARAM(usize::from(link_selected))),
            None,
        )
    };
    let _ = unsafe {
        SendMessageW(
            controls.source_file,
            BM_SETCHECK,
            Some(WPARAM(usize::from(file_selected))),
            None,
        )
    };
    let installation_visible = state.selected_tab == Tab::Installation;
    let journal_visible = state.selected_tab == Tab::Journal;
    for control in installation_controls(controls) {
        let _ = unsafe {
            ShowWindow(
                control,
                if installation_visible {
                    SW_SHOW
                } else {
                    SW_HIDE
                },
            )
        };
    }
    for control in journal_controls(controls) {
        let _ = unsafe { ShowWindow(control, if journal_visible { SW_SHOW } else { SW_HIDE }) };
    }
    let updates_visible = state.selected_tab == Tab::Updates;
    for control in updates_controls(controls) {
        let _ = unsafe { ShowWindow(control, if updates_visible { SW_SHOW } else { SW_HIDE }) };
    }
    // Every tab now has controls of its own, so the shared caption has nothing
    // left to say; it stays for a tab that has not grown any yet.
    let _ = unsafe {
        ShowWindow(
            controls.tab_content,
            if installation_visible || journal_visible || updates_visible {
                SW_HIDE
            } else {
                SW_SHOW
            },
        )
    };
    if updates_visible {
        unsafe { refresh_update_table(chrome, controls, state) };
        let _ = unsafe {
            EnableWindow(
                controls.updates_refresh,
                !matches!(state.updates, UpdatesView::Scanning { .. }),
            )
        };
        let _ = unsafe { EnableWindow(controls.updates_open, state.selected_update().is_some()) };
    }
    if installation_visible {
        let _ = unsafe {
            ShowWindow(
                controls.input_label,
                if link_selected { SW_SHOW } else { SW_HIDE },
            )
        };
        let _ = unsafe {
            ShowWindow(
                controls.input,
                if link_selected { SW_SHOW } else { SW_HIDE },
            )
        };
        let _ = unsafe {
            ShowWindow(
                controls.file_path,
                if file_selected { SW_SHOW } else { SW_HIDE },
            )
        };
        let _ = unsafe {
            ShowWindow(
                controls.clear_file,
                if file_selected { SW_SHOW } else { SW_HIDE },
            )
        };
        let _ = unsafe { ShowWindow(controls.drop_overlay, SW_HIDE) };
    }
    unsafe { rebuild_tabs(controls.tabs, state.selected_tab, &strings) };
    unsafe { refresh_column_headings(chrome, controls, state) };
    unsafe { show_progress(controls, state) };
    unsafe { apply_interface_direction(window, chrome, state.language) };
    unsafe { layout_controls(window, chrome) };
}

/// Refill the updates table only when its rows have changed.
///
/// Same rule as the region box, for the same reason: rebuilding drops the
/// selection and scrolls back to the top under the user's hand.
#[allow(unsafe_code)]
unsafe fn refresh_update_table(chrome: &WindowChrome, controls: &Controls, state: &AppState) {
    let rows: Vec<[String; 4]> = match &state.updates {
        UpdatesView::Ready(candidates) => candidates.iter().map(update_row).collect(),
        UpdatesView::Idle | UpdatesView::Scanning { .. } | UpdatesView::Unavailable => Vec::new(),
    };
    // Rebuilding drops the selection and scrolls back to the top, so it happens
    // only when the table would actually differ.
    let unchanged = *chrome.listed_updates.borrow() == rows;
    if unchanged {
        return;
    }
    let _ = unsafe { SendMessageW(controls.updates_list, LVM_DELETEALLITEMS, None, None) };
    for (index, row) in rows.iter().enumerate() {
        unsafe { insert_row(controls.updates_list, index, row) };
    }
    *chrome.listed_updates.borrow_mut() = rows;
}

/// One row of the updates table, left to right.
///
/// The two versions sit in columns of their own so they can be read across
/// without either being dressed up as following from the other.
fn update_row(candidate: &UpdateCandidate) -> [String; 4] {
    [
        candidate.application.display_name.clone(),
        candidate
            .product_id
            .as_ref()
            .map_or_else(|| "—".to_owned(), |id| id.as_str().to_owned()),
        candidate.application.installed_version.clone(),
        candidate
            .offered_version
            .clone()
            .unwrap_or_else(|| "—".to_owned()),
    ]
}

/// What the Updates tab says under its chooser.
pub(super) fn updates_details(state: &AppState, strings: &Strings) -> String {
    match &state.updates {
        UpdatesView::Idle => strings.updates_idle.to_owned(),
        UpdatesView::Scanning { done, total } => {
            format!("{} {done}/{total}", strings.updates_scanning)
        }
        UpdatesView::Unavailable => strings.updates_unavailable.to_owned(),
        UpdatesView::Ready(candidates) if candidates.is_empty() => {
            format!("{}{}", scan_age(state), strings.updates_none)
        }
        UpdatesView::Ready(_) => state.selected_update().map_or_else(
            || strings.updates_choose_entry.to_owned(),
            |candidate| update_entry_details(state, candidate),
        ),
    }
}

/// A line naming when the shown scan was taken, when it came from a past run.
///
/// A remembered answer is useful and must not be mistaken for a fresh one, so
/// it states its own age instead of being silently current.
fn scan_age(state: &AppState) -> String {
    let strings = state.language.strings();
    state
        .updates_taken
        .as_ref()
        .map_or_else(String::new, |taken| {
            fill(
                strings.updates_scan_age,
                &[("taken", taken), ("button", strings.updates_refresh)],
            )
        })
}

/// Everything known about one entry, and what can be done about it.
///
/// It never says that an update exists: `winget` cannot update a Store product
/// at all, and two version numbers are not always comparable. The only claim
/// made here is the provable one — that Microsoft Store does not serve this
/// product in this region, and so will not update it here either.
fn update_entry_details(state: &AppState, candidate: &UpdateCandidate) -> String {
    let application = &candidate.application;
    let strings = state.language.strings();
    let identifier = candidate.product_id.as_ref().map_or("—", |id| id.as_str());
    fill(
        strings.updates_entry_details,
        &[
            ("name", &application.display_name),
            ("id", identifier),
            ("family", application.family_name.as_str()),
            ("installed", &application.installed_version),
            (
                "offered",
                candidate
                    .offered_version
                    .as_deref()
                    .unwrap_or(strings.updates_version_not_named),
            ),
            ("button", strings.updates_open),
        ],
    )
}

/// Show how far the installation has got, or that it is running at all.
///
/// The bar belongs to the installation path only. A handoff has no progress to
/// report and must not appear to have any: Microsoft Store owns that work and
/// tells this window nothing about it.
#[allow(unsafe_code)]
unsafe fn show_progress(controls: &Controls, state: &AppState) {
    let running = state
        .running_operation
        .as_ref()
        .filter(|_| state.handoff.is_none());
    let Some(running) = running else {
        let _ = unsafe { ShowWindow(controls.progress, SW_HIDE) };
        return;
    };
    let style = unsafe { GetWindowLongPtrW(controls.progress, GWL_STYLE) };
    let marquee = WindowLong::try_from(PBS_MARQUEE).unwrap_or(0);
    if let Some(progress) = running.progress {
        // Leaving marquee first: a bar still animating ignores a position.
        if style & marquee != 0 {
            let _ =
                unsafe { SendMessageW(controls.progress, PBM_SETMARQUEE, Some(WPARAM(0)), None) };
            unsafe {
                SetWindowLongPtrW(controls.progress, GWL_STYLE, style & !marquee);
            };
            let _ = unsafe {
                SendMessageW(
                    controls.progress,
                    PBM_SETRANGE32,
                    Some(WPARAM(0)),
                    Some(LPARAM(100)),
                )
            };
        }
        let _ = unsafe {
            SendMessageW(
                controls.progress,
                PBM_SETPOS,
                Some(WPARAM(progress.percent() as usize)),
                None,
            )
        };
    } else {
        if style & marquee == 0 {
            unsafe {
                SetWindowLongPtrW(controls.progress, GWL_STYLE, style | marquee);
            };
        }
        let _ = unsafe {
            SendMessageW(
                controls.progress,
                PBM_SETMARQUEE,
                Some(WPARAM(1)),
                Some(LPARAM(30)),
            )
        };
    }
    let _ = unsafe { ShowWindow(controls.progress, SW_SHOW) };
}

/// The card one market's answer supports, before any region has changed.
///
/// The last line is not decoration: this is what a market offers, and an
/// installation still resolves the product again under the region it holds.
fn offered_card(state: &AppState, offered: &OfferedProduct) -> String {
    let strings = state.language.strings();
    let publisher = offered
        .publisher
        .as_deref()
        .unwrap_or(strings.card_publisher_not_provided);
    let mut fields = vec![
        offered.display_name.clone(),
        publisher.to_owned(),
        fill(
            strings.card_product_id,
            &[("id", offered.product_id.as_str())],
        ),
        fill(
            strings.card_store_catalogue,
            &[("market", offered.market.as_str())],
        ),
    ];
    if let Some(version) = offered.version.as_deref() {
        fields.push(fill(strings.card_version, &[("version", version)]));
    }
    fields.push(if offered.is_installable_kind() {
        strings.card_offered_installable.to_owned()
    } else {
        strings.card_offered_not_installable.to_owned()
    });
    fields.join(
        "
",
    )
}

/// Text for a control the user can select and copy from.
///
/// A multiline edit breaks a line on a carriage return and line feed pair and
/// runs everything together on a bare line feed, which is what a static
/// accepted. The conversion lives here rather than in every message, so no
/// future text has to remember it.
fn copyable(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Refill the region box only when the set of offered regions has changed.
///
/// Rebuilding on every render would close the dropdown while the user is
/// reading it and drop the selection, so the listed regions are remembered and
/// compared first.
#[allow(unsafe_code)]
unsafe fn refresh_region_choices(chrome: &WindowChrome, controls: &Controls, state: &AppState) {
    let regions = state.offered_regions();
    let listed: Vec<i32> = regions.iter().map(|region| region.geo_id.value()).collect();
    // The same regions in another language are not the same entries: each label
    // carries translated text, so a language change has to refill the list even
    // though the set of regions has not moved.
    let unchanged = *chrome.listed_regions.borrow() == listed
        && chrome.listed_region_language.get() == Some(state.language);
    if unchanged {
        return;
    }
    chrome.listed_region_language.set(Some(state.language));
    let _ = unsafe { SendMessageW(controls.temporary_region, CB_RESETCONTENT, None, None) };
    for region in &regions {
        let text = HSTRING::from(region_choice_label(region, &state.language.strings()));
        let _ = unsafe {
            SendMessageW(
                controls.temporary_region,
                CB_ADDSTRING,
                None,
                Some(LPARAM(text.as_ptr() as isize)),
            )
        };
    }
    if let Some(selected) = state.selected_temporary_region.as_ref()
        && let Some(index) = regions
            .iter()
            .position(|region| region.geo_id == selected.geo_id)
    {
        let _ = unsafe {
            SendMessageW(
                controls.temporary_region,
                CB_SETCURSEL,
                Some(WPARAM(index)),
                None,
            )
        };
    }
    *chrome.listed_regions.borrow_mut() = listed;
}

/// What the one market asked for a card actually answered.
///
/// Deliberately not a claim about anywhere else. The point of the sentence is
/// to separate "your region was checked and refused" from "nothing has been
/// checked", because the user cannot act on those the same way.
fn selected_region_status(state: &AppState, availability: &MarketSurvey) -> String {
    let Some(region) = state.selected_temporary_region.as_ref() else {
        return state.language.strings().availability_idle.to_owned();
    };
    let strings = state.language.strings();
    if availability.running {
        return strings.region_checking_selected.to_owned();
    }
    let answer = MarketCode::from_region(region)
        .ok()
        .and_then(|market| availability.survey.answer_for(&market))
        .map(|answer| answer.availability);
    match answer {
        Some(MarketAvailability::Offered) => strings.region_offered_here.to_owned(),
        Some(MarketAvailability::NotOffered) => strings.region_not_offered_here.to_owned(),
        _ => strings.region_no_answer_here.to_owned(),
    }
}

/// Whether the window has no identifier the region search could ask about.
///
/// True only for a file with nothing in the preserved link draft. That is the
/// one case where the search is not merely unstarted but impossible, and the
/// window has to say which of the two it is.
fn nothing_to_ask_about(state: &AppState) -> bool {
    state.application_source.selected() == ApplicationSourceKind::InstallerFile
        && state.searchable_product_id().is_none()
}

/// Whether a second pass over the markets still has anything to ask.
pub(super) fn remaining_markets_exist(state: &AppState) -> bool {
    let Some(availability) = state.availability.as_ref() else {
        return false;
    };
    // The second step widens a search; it is not a way to skip one. Without the
    // curated pass its result would be a scattered set of markets nobody asked
    // for in any particular order.
    availability.searched
        && !availability.running
        && availability.survey.scope() == SurveyScope::Curated
        && !remaining_markets(state).is_empty()
}

/// Markets a second pass would ask about, in the order they would be asked.
///
/// Everything Windows knows, minus what the curated pass already answered. A
/// market that could not be reached is asked again: nothing was learned about
/// it the first time.
pub(super) fn remaining_markets(state: &AppState) -> Vec<MarketCode> {
    let Some(availability) = state.availability.as_ref() else {
        return Vec::new();
    };
    let curated = curated_markets();
    let all: Vec<MarketCode> = state
        .temporary_regions
        .iter()
        .filter_map(|region| MarketCode::from_region(region).ok())
        .filter(|market| !curated.contains(market))
        .collect();
    availability.survey.unasked(&all)
}

/// What the window says about regional availability right now.
fn availability_status(state: &AppState, strings: &Strings) -> String {
    // "Not checked" reads as "not yet", and beside a button that will never
    // enable itself that is what makes the window look broken. A file names no
    // product, so this check has not been postponed: it has nothing to ask.
    if nothing_to_ask_about(state) {
        return strings.region_nothing_to_check.to_owned();
    }
    let Some(availability) = state.availability.as_ref() else {
        return strings.availability_idle.to_owned();
    };
    if availability.running && availability.total > 1 {
        return fill(
            strings.region_checked_progress,
            &[
                ("done", &availability.answered.to_string()),
                ("total", &availability.total.to_string()),
            ],
        );
    }
    if !availability.searched {
        // One market was asked, for a card. Reporting that as "not checked" is
        // what made the window look as if it had done nothing at all: a region
        // was checked, just not the rest of the world. Say which is which,
        // without claiming anything about anywhere else.
        return selected_region_status(state, availability);
    }
    let offering = availability.survey.offering_markets().len();
    if offering == 0 {
        // Nothing was found, and why matters: a source that never answered is
        // not a source that refused.
        return if availability.survey.answers().is_empty()
            || availability.survey.unknown_count() == availability.survey.answers().len()
        {
            strings.availability_offline.to_owned()
        } else {
            strings.availability_none.to_owned()
        };
    }
    let found = fill(strings.region_found, &[("count", &offering.to_string())]);
    if availability.survey.scope() == SurveyScope::Curated {
        return format!("{found} {}", strings.availability_incomplete);
    }
    found
}

pub(super) fn tab_content<'a>(state: &AppState, strings: &'a Strings) -> &'a str {
    match state.selected_tab {
        Tab::Journal => strings.journal_placeholder,
        // Both of these speak through controls of their own, so the shared
        // caption has nothing to add and is hidden for them.
        Tab::Installation | Tab::Updates => "",
    }
}

fn region_summary(state: &AppState, strings: &Strings) -> String {
    let Some(region) = state.current_region.as_ref() else {
        return strings.region_unavailable.to_owned();
    };
    let Ok(region) = region else {
        return strings.region_unavailable.to_owned();
    };
    region_choice_label(region, strings)
}

/// One region as it appears in the list and in the current-region box.
///
/// The name comes first because a list box searches by the text it starts
/// with: with the country code in front, typing the name found nothing, which
/// is what made a list of two hundred regions unusable without a mouse.
pub(super) fn region_choice_label(region: &Region, strings: &Strings) -> String {
    let geo_id = region.geo_id.value().to_string();
    match region_marker(region) {
        Some(code) => fill(
            strings.region_choice,
            &[
                ("name", &region.display_name),
                ("code", code),
                ("geo_id", &geo_id),
            ],
        ),
        None => fill(
            strings.region_choice_without_code,
            &[("name", &region.display_name), ("geo_id", &geo_id)],
        ),
    }
}

/// The short country marker shown before a region name.
///
/// Deliberately the ISO code and not a flag. Windows ships no flag glyphs: the
/// regional-indicator pair this used to build renders as two empty boxes in
/// every UI font on the system, which is worse than no marker at all. A real
/// flag needs artwork of its own, which this product does not carry yet.
fn region_marker(region: &Region) -> Option<&str> {
    let code = region.iso_alpha2.as_ref()?.as_str();
    (code.len() == 2 && code.bytes().all(|byte| byte.is_ascii_uppercase())).then_some(code)
}

fn application_card(state: &AppState, strings: &Strings) -> String {
    // A resolve under the region actually in force outranks a reference: it is
    // the answer the operation itself will use. The reference fills the gap
    // where the home region has nothing to say about a product offered
    // elsewhere, which is the case this application exists for.
    // Only for the link source. With a file chosen, the search can only have
    // answered about whatever the user typed into the preserved draft, and a
    // card beside a file would read as a description of that file — which is
    // the one thing nothing here can establish.
    if state.application_source.selected() == ApplicationSourceKind::StoreText
        && !matches!(
            state.resolution.state(),
            ProductResolutionState::Resolved { .. }
        )
        && let Some(offered) = state.offered_product()
    {
        return offered_card(state, offered);
    }
    match state.resolution.state() {
        ProductResolutionState::Idle => strings.app_card.to_owned(),
        ProductResolutionState::SyntaxAccepted { product_id } => {
            fill(strings.card_syntax_accepted, &[("id", product_id.as_str())])
        }
        ProductResolutionState::Resolving { .. } => strings.card_retrieving.to_owned(),
        ProductResolutionState::Resolved { product } => {
            let publisher = product
                .publisher
                .as_deref()
                .unwrap_or(strings.card_publisher_not_provided);
            let mut fields = vec![
                product.display_name.clone(),
                publisher.to_owned(),
                fill(
                    strings.card_product_id,
                    &[("id", product.product_id.as_str())],
                ),
                fill(
                    strings.card_store_catalogue,
                    &[("market", product.market.as_str())],
                ),
            ];
            if let Some(version) = product.version.as_deref() {
                fields.push(fill(strings.card_version, &[("version", version)]));
            }
            if let Some(size_bytes) = product.size_bytes {
                fields.push(format_size_bytes(size_bytes, strings));
            }
            if let Some(region) = state.selected_temporary_region.as_ref() {
                fields.push(region_choice_label(region, strings));
            }
            fields.push(backend_status_line(
                state.language,
                state.installation_backend_confirmed,
            ));
            fields.join("\n")
        }
        ProductResolutionState::Failed { failure, .. } => {
            let diagnostic = Diagnostic::from(failure);
            let message = diagnostic_message(state.language, diagnostic.code());
            // The catalogue answers under the region in force right now, not
            // under the temporary one, so an unavailable product here says
            // nothing about the region the user picked. Naming the region that
            // was asked keeps the card from reading as a verdict on that choice.
            let asked = state
                .current_region
                .as_ref()
                .and_then(|region| region.as_ref().ok())
                .map(|region| region_choice_label(region, strings));
            // The cause is deliberately not repeated. `ProductUnavailableInRegion`
            // reads as "not offered in the selected region", while the region it
            // was actually checked in is the current one — which is how this card
            // ended up contradicting the region search sitting right above it.
            let _ = message;
            asked.map_or_else(
                || strings.card_unavailable.to_owned(),
                |region| fill(strings.card_unavailable_in_region, &[("region", &region)]),
            )
        }
    }
}

fn format_size_bytes(size_bytes: u64, strings: &Strings) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * KIB;
    if size_bytes >= MIB {
        format_size_unit(size_bytes, MIB, strings.card_size_mebibytes, strings)
    } else if size_bytes >= KIB {
        format_size_unit(size_bytes, KIB, strings.card_size_kibibytes, strings)
    } else {
        fill(
            strings.card_size_bytes,
            &[("size", &size_bytes.to_string())],
        )
    }
}

fn format_size_unit(size_bytes: u64, unit: u64, suffix: &str, strings: &Strings) -> String {
    let whole = size_bytes / unit;
    let tenths = (size_bytes % unit).saturating_mul(10) / unit;
    fill(
        strings.card_size,
        &[
            ("whole", &whole.to_string()),
            ("tenths", &tenths.to_string()),
            ("unit", suffix),
        ],
    )
}

fn backend_status_line(language: Language, confirmed: bool) -> String {
    let strings = language.strings();
    if confirmed {
        strings.card_install_ready.to_owned()
    } else {
        strings.card_install_not_ready.to_owned()
    }
}

/// One sentence describing where the guarded operation stands.
///
/// The percentage appears only when the backend reports one. For Store products
/// it does, but only for the installation itself: the download is performed by
/// Microsoft Store, which reports nothing.
pub(super) fn operation_status(state: &AppState) -> String {
    // A handoff owns the window while it lasts, and its facts are not the
    // installation machine's: it has no phase, no percentage and no completion.
    if state.handoff.is_some() {
        return handoff_status(state);
    }
    if let Some(diagnostic) = &state.operation_failure {
        return diagnostic_message(state.language, diagnostic.code()).to_owned();
    }
    let Some(operation_state) = state.operation_state else {
        return started_or_draft_status(state);
    };
    let running = state.running_operation.clone().unwrap_or_default();
    let strings = state.language.strings();
    let region_note = if running.region_restored_early {
        strings.operation_region_already_back
    } else {
        ""
    };

    let body = match operation_state {
        InstallationOperationState::SwitchingRegion => {
            strings.operation_switching_region.to_owned()
        }
        InstallationOperationState::RegionSwitched
        | InstallationOperationState::RefreshingStoreContext
        | InstallationOperationState::StartingInstall => running.product_name.as_ref().map_or_else(
            || strings.operation_looking_up.to_owned(),
            |name| fill(strings.operation_found, &[("name", name)]),
        ),
        InstallationOperationState::InstallRequested
        | InstallationOperationState::InstallObserved
        | InstallationOperationState::Installing
        | InstallationOperationState::RegionRestoredEarly => installing_text(state, &running),
        InstallationOperationState::Installed | InstallationOperationState::RestoringRegion => {
            strings.operation_restoring_region.to_owned()
        }
        InstallationOperationState::Restored | InstallationOperationState::Completed => {
            strings.operation_done.to_owned()
        }
        InstallationOperationState::CompletionUncertain => {
            strings.operation_completion_uncertain.to_owned()
        }
        _ => started_or_draft_status(state),
    };
    format!("{body}{region_note}")
}

/// What to say when the machine itself has nothing specific to report.
///
/// A started operation says so. Falling through to the draft status there would
/// ask the source what the operation is doing, and the source asks back: the two
/// would call each other until the stack ran out.
fn started_or_draft_status(state: &AppState) -> String {
    if state.running_operation.is_none() {
        return resolution_status(state);
    }
    state.language.strings().operation_started.to_owned()
}

/// One sentence describing where a Store-installer handoff stands.
///
/// Nothing here reports progress or completion, because the operation has
/// neither. What it does report is exactly what the user has to act on: which
/// region is in force and who is finishing the installation.
fn handoff_status(state: &AppState) -> String {
    let Some(handoff) = state.handoff.as_ref() else {
        return resolution_status(state);
    };
    let strings = state.language.strings();
    let region = handoff
        .temporary_region
        .as_ref()
        .map_or_else(String::new, |region| region_choice_label(region, &strings));
    match handoff.stage {
        HandoffStage::SwitchingRegion => strings.handoff_switching_region.to_owned(),
        HandoffStage::HandedOff => fill(
            strings.handoff_handed_off,
            &[("region", &region), ("button", strings.restore)],
        ),
        HandoffStage::RestoringRegion => strings.handoff_restoring_region.to_owned(),
        HandoffStage::Restored => strings.handoff_restored.to_owned(),
        stopped => handoff_failure_status(state, state.language, stopped),
    }
}

/// What a handoff that stopped leaves the user with.
///
/// The cause comes first, then the one fact that decides what to do next:
/// whether Windows still holds the temporary region.
fn handoff_failure_status(state: &AppState, language: Language, stopped: HandoffStage) -> String {
    let strings = language.strings();
    let cause = state.operation_failure.as_ref().map_or_else(
        || strings.handoff_did_not_happen.to_owned(),
        |diagnostic| diagnostic_message(language, diagnostic.code()).to_owned(),
    );
    let region = if stopped.offers_region_restore() {
        fill(
            strings.handoff_region_may_be_temporary,
            &[("button", strings.restore)],
        )
    } else {
        strings.handoff_region_original.to_owned()
    };
    format!("{cause} {region}")
}

/// What the window says when the user declines the handoff confirmation.
pub(super) fn handoff_declined_status(language: Language) -> String {
    language.strings().handoff_declined.to_owned()
}

/// Text for an installation that is under way.
fn installing_text(state: &AppState, running: &RunningOperation) -> String {
    let strings = state.language.strings();
    let phase = if matches!(running.phase, Some(InstallPhase::PostInstall)) {
        strings.installing_phase_finishing
    } else {
        strings.installing_phase_running
    };
    running.progress.map_or_else(
        || fill(strings.installing_without_progress, &[("phase", phase)]),
        |progress| {
            fill(
                strings.installing_with_progress,
                &[
                    ("phase", phase),
                    ("percent", &progress.percent().to_string()),
                ],
            )
        },
    )
}

pub(super) fn resolution_status(state: &AppState) -> String {
    let strings = state.language.strings();
    match state.resolution.state() {
        ProductResolutionState::Resolved { .. } => strings.resolution_card_confirmed.to_owned(),
        ProductResolutionState::Failed { failure, .. } => {
            if failure.error == ResolveError::ResolverUnavailable {
                strings.resolution_catalogue_not_searchable.to_owned()
            } else {
                let language = state.language;
                // The catalogue answers under the region Windows holds right
                // now, not under the temporary one, so a product missing here
                // says nothing about the region the user picked — and it does
                // not block anything: the install gate needs a parsed Product
                // ID, not a card. Saying "unavailable in the selected region,
                // installation unavailable" was wrong on both counts, and it
                // contradicted the region search sitting right above it.
                let diagnostic = Diagnostic::from(failure);
                let asked = state
                    .current_region
                    .as_ref()
                    .and_then(|region| region.as_ref().ok())
                    .map(|region| region_choice_label(region, &strings));
                let where_checked = asked.map_or_else(
                    || strings.resolution_checked_in_current_region.to_owned(),
                    |region| fill(strings.resolution_checked_in_region, &[("region", &region)]),
                );
                let still_possible = strings.resolution_still_possible;
                format!(
                    "{} {where_checked} {still_possible}",
                    diagnostic_message(language, diagnostic.code())
                )
            }
        }
        _ => source_status(state),
    }
}

/// What to say once Microsoft's installer has arrived.
///
/// It says what the file is and what pressing the button will now do, because
/// the button's meaning has changed underneath the user: the same «Install»
/// now hands a file to Microsoft Store instead of asking the backend.
pub(super) fn installer_download_ready(language: Language) -> String {
    let strings = language.strings();
    fill(
        strings.installer_download_ready,
        &[("button", strings.install), ("restore", strings.restore)],
    )
}

/// Why Microsoft's installer did not arrive.
pub(super) fn installer_download_failed(
    language: Language,
    error: InstallerDownloadError,
) -> String {
    let strings = language.strings();
    match error {
        InstallerDownloadError::TransportUnavailable => {
            strings.installer_download_network.to_owned()
        }
        InstallerDownloadError::Refused { status } => status.map_or_else(
            || strings.installer_download_refused.to_owned(),
            |status| {
                fill(
                    strings.installer_download_refused_with_status,
                    &[("status", &status.to_string())],
                )
            },
        ),
        InstallerDownloadError::TooLarge => strings.installer_download_too_large.to_owned(),
        InstallerDownloadError::NotWritable => strings.installer_download_not_writable.to_owned(),
    }
}

pub(super) fn source_status(state: &AppState) -> String {
    match state.application_source.selected() {
        ApplicationSourceKind::StoreText => store_text_status(state),
        ApplicationSourceKind::InstallerFile => file_source_status(state),
    }
}

/// What the typed application source means for the command that follows it.
///
/// A status that says "unavailable" while the button is enabled is worse than
/// no status at all, so this names the one condition still missing rather than
/// repeating a general sentence. The order matches the gate: a machine that
/// cannot start an operation is reported before anything about the draft.
fn store_text_status(state: &AppState) -> String {
    let text = state.application_source.store_text();
    let Ok(candidate) = StoreInputCandidate::parse(text) else {
        return store_source_status(state.language, text);
    };
    let product_id = candidate.product_id().as_str().to_owned();
    let strings = state.language.strings();
    // What the user can act on comes first; the machine's own conditions after.
    if state.selected_temporary_region.is_none() {
        return fill(
            strings.source_choose_region,
            &[("id", &product_id), ("button", strings.find_region)],
        );
    }
    if state.running_operation.is_some() {
        return operation_status(state);
    }
    if !state.recovery_admits_new_operation {
        // The flag also starts out false, before recovery has been inspected at
        // all, so this must not assert that an unfinished operation exists.
        return fill(strings.source_recovery_pending, &[("id", &product_id)]);
    }
    if !state.prerequisites.admits_installation() {
        return prerequisite_status(state.language, state.prerequisites)
            .unwrap_or_else(|| strings.source_prerequisites_unmet.to_owned());
    }
    // the catalogue answers this before anything is
    // touched, so a product that cannot be delivered here is refused while the
    // region is still the user's own.
    match state.device_verdict() {
        DeviceCompatibility::OtherDeviceOnly => {
            return fill(strings.source_other_device_only, &[("id", &product_id)]);
        }
        DeviceCompatibility::NewerWindowsRequired { required } => {
            let (major, minor, build, revision) = required.parts();
            let required = format!("{major}.{minor}.{build}.{revision}");
            return fill(
                strings.source_newer_windows_required,
                &[("id", &product_id), ("version", &required)],
            );
        }
        DeviceCompatibility::Deliverable | DeviceCompatibility::Unknown => {}
    }
    let region = state
        .selected_temporary_region
        .as_ref()
        .map_or_else(String::new, |region| region_choice_label(region, &strings));
    fill(
        strings.source_ready,
        &[("id", &product_id), ("region", &region)],
    )
}

pub(super) fn store_source_status(language: Language, value: &str) -> String {
    let strings = language.strings();
    match StoreInputCandidate::parse(value) {
        Ok(candidate) => fill(
            strings.source_syntax_accepted,
            &[("id", candidate.product_id().as_str())],
        ),
        Err(StoreInputError::Empty) => strings.status.to_owned(),
        Err(error) => diagnostic_message(language, Diagnostic::from(error).code()).to_owned(),
    }
}

/// What the selected file means for the command that follows it.
///
/// The order matches the gate, the same way the Store-text status does: the one
/// condition still missing is named, rather than a general sentence that would
/// leave the user guessing which of them applies.
fn file_source_status(state: &AppState) -> String {
    let strings = state.language.strings();
    if state.application_source.installer_file().is_none() {
        return strings.file_choose.to_owned();
    }
    if let Some(error) = state.stub_inspection_error {
        return stub_inspection_error_status(state.language, error);
    }
    let Some(inspection) = state.stub_inspection.as_ref() else {
        return inspecting_file_status(state.language);
    };
    let file = match admit_installer_handoff(inspection) {
        Ok(file) => file,
        Err(refusal) => return handoff_refusal_status(state.language, refusal),
    };
    // A running operation owns the sentence; the draft has nothing to add.
    if state.handoff.is_some() || state.running_operation.is_some() {
        return operation_status(state);
    }
    if !state.recovery_admits_new_operation {
        return strings.file_recovery_pending.to_owned();
    }
    if !state.prerequisites.admits_handoff() {
        return strings.file_store_not_confirmed.to_owned();
    }
    let Some(region) = state.selected_temporary_region.as_ref() else {
        return strings.file_choose_region.to_owned();
    };
    let region = region_choice_label(region, &strings);
    // The missing region check is named here rather than left as a dead button
    // for the user to puzzle over.
    let unchecked = fill(
        strings.file_region_is_your_choice,
        &[("button", strings.find_region)],
    );
    fill(
        strings.file_ready,
        &[
            ("name", file.file_name()),
            ("region", &region),
            ("unchecked", &unchecked),
        ],
    )
}

/// Why a selected file will not be executed, in the user's own terms.
fn handoff_refusal_status(language: Language, refusal: HandoffRefusal) -> String {
    let strings = language.strings();
    let reason = match refusal {
        HandoffRefusal::SignatureNotTrusted => strings.file_refusal_signature_not_trusted,
        HandoffRefusal::SignerUnavailable => strings.file_refusal_signer_unavailable,
        HandoffRefusal::SignerOutsideStorePolicy => strings.file_refusal_not_store_signed,
        HandoffRefusal::FileChangedDuringInspection | HandoffRefusal::FileDiffersFromConfirmed => {
            strings.file_refusal_changed_while_checked
        }
        HandoffRefusal::FileIdentityIncomplete => strings.file_refusal_identity_incomplete,
    };
    fill(strings.file_refused, &[("reason", reason)])
}

pub(super) fn inspecting_file_status(language: Language) -> String {
    language.strings().file_inspecting.to_owned()
}

fn stub_inspection_error_status(language: Language, error: StubInspectionError) -> String {
    let details = diagnostic_details(language, &stub_diagnostic(error));
    let strings = language.strings();
    let primary = match error {
        StubInspectionError::ChangedDuringInspection => strings.file_changed_during_inspection,
        StubInspectionError::Io => strings.file_unreadable,
    };
    // The sentence stays free of internal names; evidence goes below it.
    format!("{primary}\n\n{details}")
}

pub(super) fn file_error_status(language: Language, error: FileSelectionError) -> String {
    let strings = language.strings();
    let detail = match error {
        FileSelectionError::EmptyPath => strings.file_error_empty_path,
        FileSelectionError::UnsupportedExtension => strings.file_error_unsupported_extension,
        FileSelectionError::NotAFile => strings.file_error_not_a_file,
        FileSelectionError::LinkOrDevicePath => strings.file_error_link_or_device_path,
        FileSelectionError::MultipleFiles => strings.file_error_multiple_files,
    };
    let primary = fill(strings.file_not_selected, &[("detail", detail)]);
    format!(
        "{primary}

{}",
        diagnostic_details(language, &file_selection_diagnostic(error))
    )
}

pub(super) fn temporary_region_status(language: Language, value: &str) -> String {
    let strings = language.strings();
    if value.trim().is_empty() {
        return strings.region_choose_from_list.to_owned();
    }
    strings.region_not_confirmed.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::state::AppState;
    use crate::gui::state::MarketSurvey;
    use crate::gui::strings::Language;
    use winstoreregion_core::{
        GeoId, GuiLaunch, InstallClassification, IsoAlpha2, MarketAnswer, MarketCode,
        OfferedProduct, Region, ResolveRequestId, ResolvedProduct, ResolverSource, StoreProductId,
    };

    fn region(geo_id: i32, name: &str, alpha_2: &str) -> Region {
        Region::new(
            GeoId::new(geo_id).expect("positive GeoId"),
            name.to_owned(),
            Some(IsoAlpha2::parse(alpha_2).expect("valid ISO alpha-2")),
        )
        .expect("complete region")
    }

    fn surveyed_state() -> AppState {
        let mut state = AppState::new(GuiLaunch::default());
        let product_id = StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID");
        state.temporary_regions = vec![
            region(244, "United States", "US"),
            region(203, "Russia", "RU"),
            region(94, "Germany", "DE"),
            // Outside the curated set, so a second pass has something to ask.
            region(110, "Iceland", "IS"),
        ];
        state.selected_temporary_region = Some(state.temporary_regions[0].clone());
        let mut survey = MarketSurvey::new(product_id.clone(), 1, 3);
        survey.running = false;
        survey.searched = true;
        survey.survey.record(MarketAnswer::offered(OfferedProduct {
            product_id,
            market: MarketCode::parse("US").expect("valid market"),
            display_name: "Hulu".to_owned(),
            publisher: Some("Hulu, LLC".to_owned()),
            description: None,
            version: None,
            package_family_name: None,
            install_classification: InstallClassification::packaged_from_msstore_installer_type(),
        }));
        survey.survey.record(MarketAnswer::not_offered(
            MarketCode::parse("RU").expect("valid market"),
        ));
        survey.survey.record(MarketAnswer::unknown(
            MarketCode::parse("DE").expect("valid market"),
        ));
        // A pass that has ended has taken its answers into the shown list; the
        // window does this in `apply_market_answers`, and a fixture that skips
        // it would be a survey no run can produce.
        survey.settle();
        state.availability = Some(survey);
        state
    }

    #[test]
    fn a_market_reference_never_opens_the_install_gate_by_itself() {
        let state = surveyed_state();

        assert!(state.offered_product().is_some());
        // The gate needs recovery, prerequisites and a parsed identifier. A
        // market saying the product exists there is none of those.
        assert!(!state.install_is_available());
        // The card shows what the catalogue offers and says so in as many
        // words. Comparing against the language file rather than a quoted
        // phrase keeps this test about the claim, not about its wording.
        let strings = state.language.strings();
        assert!(
            application_card(&state, &strings).contains(strings.card_offered_installable),
            "the card must say that the application is looked up again under the temporary region"
        );
    }

    #[test]
    fn a_file_with_no_link_draft_says_the_check_is_impossible_not_merely_unstarted() {
        let mut state = AppState::new(GuiLaunch::default());
        let strings = state.language.strings();
        // Nothing typed and nothing chosen: the check simply has not run.
        assert_eq!(
            availability_status(&state, &strings),
            strings.availability_idle
        );

        let executable = std::env::current_exe().expect("test executable path");
        state
            .application_source
            .select_installer_stub(executable)
            .expect("an executable path is a supported draft");
        assert!(nothing_to_ask_about(&state));
        let label = availability_status(&state, &strings);
        assert!(
            label.contains("проверить нечем"),
            "beside a dead button the label must say why, not `not checked`: {label}"
        );
        assert_ne!(label, strings.availability_idle);

        // A link draft beside the file gives the search something to ask about
        // again, so the label goes back to the ordinary one.
        state.application_source.set_store_text("9WZDNCRFJ3L1");
        state.application_source.select_installer_file();
        assert!(!nothing_to_ask_about(&state));
        assert_eq!(
            availability_status(&state, &strings),
            strings.availability_idle
        );
    }

    #[test]
    fn a_card_from_the_link_draft_never_appears_beside_a_chosen_file() {
        let mut state = surveyed_state();
        // With the link source active the reference card is exactly right.
        assert!(state.offered_product().is_some());
        assert!(application_card(&state, &state.language.strings()).contains("Hulu"));

        let executable = std::env::current_exe().expect("test executable path");
        state
            .application_source
            .select_installer_stub(executable)
            .expect("an executable path is a supported draft");

        // The survey still knows what it found, and the region list still uses
        // it — but a card beside a file would read as a description of that
        // file, and nothing here can establish that.
        assert!(state.offered_product().is_some());
        let card = application_card(&state, &state.language.strings());
        assert!(
            !card.contains("Hulu"),
            "a file must never be described by a product the user merely typed"
        );
    }

    #[test]
    fn a_search_leaves_the_regions_it_found_and_says_the_set_is_incomplete() {
        let state = surveyed_state();
        let listed: Vec<i32> = state
            .offered_regions()
            .iter()
            .map(|region| region.geo_id.value())
            .collect();

        // Only the market that offered the product stays. Russia refused and
        // Germany never answered; neither belongs in an answer to "where can
        // this be installed", and the incomplete-set note keeps that honest.
        assert_eq!(listed, vec![244]);
        assert!(state.regions_are_narrowed());
        assert!(availability_status(&state, &state.language.strings()).contains("Набор неполный"));
    }

    #[test]
    fn answers_arriving_during_a_pass_do_not_change_the_list_the_window_shows() {
        // The shown list may change once, when the pass ends. An answer
        // arriving before that must not move it: rebuilding the region box on
        // every find is what a user sees as flicker.
        let mut state = surveyed_state();
        let before: Vec<i32> = state
            .offered_regions()
            .iter()
            .map(|region| region.geo_id.value())
            .collect();

        if let Some(availability) = state.availability.as_mut() {
            availability.running = true;
            availability
                .survey
                .record(MarketAnswer::offered(OfferedProduct {
                    product_id: StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID"),
                    market: MarketCode::parse("DE").expect("valid market"),
                    display_name: "Hulu".to_owned(),
                    publisher: None,
                    description: None,
                    version: None,
                    package_family_name: None,
                    install_classification:
                        InstallClassification::packaged_from_msstore_installer_type(),
                }));
        }
        let during: Vec<i32> = state
            .offered_regions()
            .iter()
            .map(|region| region.geo_id.value())
            .collect();
        assert_eq!(before, during, "a mid-pass answer moved the shown list");

        if let Some(availability) = state.availability.as_mut() {
            availability.running = false;
            availability.settle();
        }
        let after: Vec<i32> = state
            .offered_regions()
            .iter()
            .map(|region| region.geo_id.value())
            .collect();
        assert_eq!(
            after,
            vec![244, 94],
            "the finished pass did not take effect"
        );
    }

    #[test]
    fn a_search_that_found_nothing_leaves_every_region_reachable() {
        let mut state = surveyed_state();
        if let Some(availability) = state.availability.as_mut() {
            availability.survey = winstoreregion_core::AvailabilitySurvey::new(
                StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID"),
            );
            availability.survey.record(MarketAnswer::not_offered(
                MarketCode::parse("US").expect("valid market"),
            ));
            // This pass ended too, and it ended having found nothing.
            availability.settle();
        }

        // An empty list would leave the user nowhere to go.
        assert_eq!(state.offered_regions().len(), state.temporary_regions.len());
        assert!(!state.regions_are_narrowed());
    }

    #[test]
    fn the_second_pass_is_offered_only_after_a_search_has_actually_run() {
        let mut state = surveyed_state();
        if let Some(availability) = state.availability.as_mut() {
            availability.searched = false;
        }
        assert!(!remaining_markets_exist(&state));

        if let Some(availability) = state.availability.as_mut() {
            availability.searched = true;
        }
        assert!(remaining_markets_exist(&state));
    }

    #[test]
    fn showing_every_region_restores_the_full_windows_list() {
        let mut state = surveyed_state();
        state.show_all_regions = true;

        assert_eq!(state.offered_regions().len(), state.temporary_regions.len());
        assert!(!state.regions_are_narrowed());
    }

    #[test]
    fn one_market_asked_reports_its_answer_and_is_still_never_reported_as_a_search() {
        let mut state = surveyed_state();
        let strings = state.language.strings();
        if let Some(availability) = state.availability.as_mut() {
            availability.searched = false;
        }

        // The selected region is the United States, and the survey recorded an
        // offer there. Calling that "not checked" is what made the window look
        // as though nothing had happened at all.
        let label = availability_status(&state, &strings);
        assert!(
            label.contains("предлагается"),
            "the answer for the region that was asked must be reported: {label}"
        );
        assert_ne!(label, strings.availability_idle);
        // What it must still not do is imply anything about anywhere else.
        assert!(
            label.contains("Остальные регионы не проверялись"),
            "one market is not a search and the label must keep saying so: {label}"
        );
        assert!(!label.contains("Найдено регионов"));

        // A refusal reads as a refusal, not as an absence of information.
        let refused = MarketCode::parse("US").expect("valid market");
        if let Some(availability) = state.availability.as_mut() {
            availability.survey = winstoreregion_core::AvailabilitySurvey::new(
                StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID"),
            );
            availability
                .survey
                .record(winstoreregion_core::MarketAnswer::not_offered(refused));
        }
        assert!(availability_status(&state, &strings).contains("не предлагается"));
    }

    #[test]
    fn an_incomplete_survey_says_so_and_a_complete_one_stops_saying_it() {
        let mut state = surveyed_state();
        let strings = state.language.strings();

        assert!(availability_status(&state, &strings).contains("Набор неполный"));

        if let Some(availability) = state.availability.as_mut() {
            availability.survey.widen_to_complete();
        }
        assert!(!availability_status(&state, &strings).contains("Набор неполный"));
    }

    #[test]
    fn region_label_marks_a_region_only_when_windows_supplies_iso_alpha2() {
        let united_states = Region::new(
            GeoId::new(244).expect("positive GeoId"),
            "United States".to_owned(),
            Some(IsoAlpha2::parse("US").expect("valid ISO alpha-2")),
        )
        .expect("complete region");
        let neutral = Region::new(
            GeoId::new(1).expect("positive GeoId"),
            "Neutral territory".to_owned(),
            None,
        )
        .expect("complete region");

        // A flag glyph is deliberately not used: Windows has none, and the
        // regional-indicator pair renders as empty boxes. The name leads so
        // that the list can be searched by typing it.
        assert_eq!(
            region_choice_label(&united_states, &Language::English.strings()),
            "United States \u{00b7} US (region code 244)"
        );
        assert_eq!(
            region_choice_label(&neutral, &Language::English.strings()),
            "Neutral territory (region code 1)"
        );
    }

    #[test]
    fn a_started_operation_reports_itself_instead_of_asking_the_draft_about_itself() {
        // Regression: the status of a started operation used to fall through to
        // the source draft, which asked the operation what it was doing, which
        // asked the draft again. The window overflowed its stack on the first
        // click of the install button.
        let mut state = AppState::new(GuiLaunch::default());
        state.selected_temporary_region = Some(region(244, "United States", "US"));
        let request = state.resolution.begin(
            StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID"),
            MarketCode::parse("US").expect("valid market"),
        );
        state.application_source.set_store_text("9WZDNCRFJ3L1");
        state.running_operation = Some(crate::gui::state::RunningOperation {
            resolve: Some(request),
            ..crate::gui::state::RunningOperation::default()
        });
        state.operation_state = None;

        let starting = operation_status(&state);
        assert!(starting.contains("Операция запущена"));

        // The same holds once the machine reports a state the sentence table
        // does not name on its own.
        state.operation_state = Some(InstallationOperationState::Preparing);
        assert!(operation_status(&state).contains("Операция запущена"));

        // With nothing running, the draft is exactly what should be reported.
        state.running_operation = None;
        state.operation_state = None;
        assert!(operation_status(&state).contains("9WZDNCRFJ3L1"));
    }

    #[test]
    fn resolved_card_keeps_optional_fields_honest_in_both_locales() {
        let mut state = AppState::new(GuiLaunch::default());
        let product_id = StoreProductId::parse("9WZDNCRFJ3PZ").expect("valid product ID");
        let market = MarketCode::parse("us").expect("valid market");
        let request = state.resolution.begin(product_id.clone(), market.clone());
        let product = ResolvedProduct {
            request_id: request.request_id,
            product_id,
            market,
            display_name: "A very long application title that remains available to the card"
                .to_owned(),
            package_family_name: None,
            publisher: None,
            icon: None,
            install_classification: InstallClassification::unknown(),
            version: None,
            size_bytes: None,
            resolver_source: ResolverSource::WinGetMsStore,
        };
        assert!(state.resolution.complete(&request, Ok(product)));
        state.selected_temporary_region = Some(
            Region::new(
                GeoId::new(244).expect("positive GeoId"),
                "United States",
                None,
            )
            .expect("complete selected region"),
        );
        let russian = application_card(&state, &Language::Russian.strings());
        assert!(russian.contains("издатель не указан"));
        // The label is built from the language file, so this compares against
        // the key rather than quoting a sentence that wording may change.
        assert!(
            russian.contains(&region_choice_label(
                state
                    .selected_temporary_region
                    .as_ref()
                    .expect("a temporary region was selected above"),
                &Language::Russian.strings()
            ))
        );
        assert!(!russian.contains("Version:"));
        state.language = Language::English;
        let english = application_card(&state, &Language::English.strings());
        assert!(english.contains("publisher not provided"));
        assert!(english.contains("9WZDNCRFJ3PZ"));
        // The separator belongs to the language, which is the reason this line
        // moved out of the code: English writes 1.5, Russian writes 1,5.
        assert_eq!(
            format_size_bytes(1_572_864, &Language::English.strings()),
            "Size: 1.5 MiB"
        );
        assert_eq!(
            format_size_bytes(1_572_864, &Language::Russian.strings()),
            "Размер: 1,5 МиБ"
        );
        assert_eq!(
            format_size_bytes(512, &Language::English.strings()),
            "Size: 512 bytes"
        );
        assert_eq!(ResolveRequestId::new(1).value(), 1);
    }
}
