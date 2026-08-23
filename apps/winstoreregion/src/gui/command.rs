//! Routing of Windows notifications into presentation state.

use crate::gui::diagnostic::{clipboard_diagnostic, diagnostic_message};
use crate::gui::dialogs::{
    choose_installer_file, confirm_installer_handoff, copy_text_to_clipboard, read_window_text,
    show_stub_details,
};
use crate::gui::handoff::{HandoffRequest, HandoffStage, start_handoff, start_region_restore};
use crate::gui::ids::{
    ID_CHECK_REMAINING, ID_CLEAR_FILE, ID_DETAILS, ID_DIALOG_CANCEL, ID_DIALOG_OK, ID_FIND_REGION,
    ID_INSTALL, ID_JOURNAL_CLEAR, ID_JOURNAL_COPY_ID, ID_JOURNAL_DELETE, ID_JOURNAL_LIST,
    ID_JOURNAL_OPEN_STORE, ID_JOURNAL_REPEAT, ID_LANGUAGE, ID_OPEN_STORE_PAGE, ID_RESTORE_REGION,
    ID_SHOW_ALL_REGIONS, ID_SOURCE_ACTION, ID_SOURCE_FILE, ID_SOURCE_LINK, ID_STORE_INPUT, ID_TABS,
    ID_TEMPORARY_GEO_ID, ID_UPDATES_LIST, ID_UPDATES_OPEN, ID_UPDATES_REFRESH, menu_action,
};
use crate::gui::install::{InstallationRequest, start_installation};
use crate::gui::menu::{dispatch_menu_action, replace_window_menu};
use crate::gui::render::operation_status;
use crate::gui::render::{
    file_error_status, handoff_declined_status, inspecting_file_status, region_choice_label,
    remaining_markets, render, source_status, store_source_status, temporary_region_status,
};
use crate::gui::state::{
    AppState, Controls, FileSelectionError, HandoffProgress, JournalView, MarketSurvey,
    MarketSurveyUpdate, ModalScope, RunningOperation, Tab, UiAction, UiEvent, UiUpdate,
    UpdatesView, WindowChrome,
};
use crate::gui::strings::{Language, fill};
use crate::gui::work::{
    start_device_compatibility_probe, start_installer_download, start_journal_clear,
    start_journal_delete, start_market_survey, start_product_resolution, start_stub_inspection,
    start_updates_scan,
};
use crate::platform::store::WinStorePageOpener;
use std::path::PathBuf;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    LVM_GETNEXTITEM, LVN_ITEMCHANGED, LVNI_SELECTED, NMHDR, TCM_GETCURSEL, TCM_SETCURSEL,
    TCN_SELCHANGE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{IsWindowEnabled, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    CB_GETCURSEL, CBN_SELCHANGE, EN_CHANGE, IDYES, IsWindowVisible, MB_ICONWARNING, MB_YESNO,
    MessageBoxW, PostMessageW, SendMessageW, SetWindowTextW, WM_CLOSE, WM_PASTE,
};
use windows::core::HSTRING;
use winstoreregion_core::{
    ApplicationSourceKind, InstallerStubPath, InstallerStubPathError, JournalRecord,
    MarketAvailability, MarketCode, StoreInputCandidate, StorePageRequest, StoreProductId,
    SurveyScope, curated_markets, open_store_page,
};

/// Start one guarded installation from the confirmed product and region.
///
/// The gate is re-checked here rather than trusted from the button's enabled
/// state: a window can be clicked between a state change and its repaint.
#[allow(unsafe_code)]
unsafe fn begin_installation(window: HWND, chrome: &WindowChrome, state: &mut AppState) {
    if !state.install_is_available() {
        return;
    }
    let (Some(product_id), Some(region)) = (
        state.accepted_product_id().cloned(),
        state.selected_temporary_region.clone(),
    ) else {
        return;
    };
    let Ok(market) = MarketCode::from_region(&region) else {
        return;
    };

    // The resolve belongs to the operation: under the home region this product
    // may not exist at all, and the card can only arrive once the temporary
    // region is in force.
    // A finished handoff is still on screen; this operation takes the window.
    state.handoff = None;
    let resolve = state.resolution.begin(product_id, market);
    state.running_operation = Some(RunningOperation {
        resolve: Some(resolve.clone()),
        ..RunningOperation::default()
    });
    state.operation_state = None;
    state.operation_status = operation_status(state);
    unsafe { render(window, chrome, state) };
    start_installation(
        window,
        InstallationRequest {
            resolve,
            temporary_region: region.geo_id,
        },
    );
}

/// Start whichever operation the selected source actually has.
///
/// The two paths are not interchangeable. A link or Product ID names a product
/// the backend can install and observe; a file names nothing at all and can
/// only be handed to Microsoft Store under a switched region.
#[allow(unsafe_code)]
unsafe fn begin_primary_command(window: HWND, chrome: &WindowChrome, state: &mut AppState) {
    match state.application_source.selected() {
        ApplicationSourceKind::StoreText => unsafe { begin_installation(window, chrome, state) },
        ApplicationSourceKind::InstallerFile => unsafe { begin_handoff(window, chrome, state) },
    }
}

/// Hand one verified installer to Microsoft Store under a temporary region.
///
/// The gate is re-checked here for the same reason the installation one is, and
/// the confirmation is asked before anything is touched: a user who says no
/// must not find their region already changed.
#[allow(unsafe_code)]
unsafe fn begin_handoff(window: HWND, chrome: &WindowChrome, state: &mut AppState) {
    if !state.handoff_is_available() {
        return;
    }
    let (Some(file), Some(region), Some(path)) = (
        state.admitted_installer_file(),
        state.selected_temporary_region.clone(),
        state
            .application_source
            .installer_file()
            .map(|selected| selected.as_path().to_path_buf()),
    ) else {
        return;
    };
    // Gathered before the modal call so nothing borrows presentation state
    // while the dialog is running.
    let publisher = state.stub_inspection.as_ref().map_or_else(
        || state.language.strings().command_begin_handoff.to_owned(),
        |inspection| {
            inspection
                .signer
                .as_ref()
                .map_or_else(String::new, |signer| signer.subject.clone())
        },
    );
    let region_label = region_choice_label(&region, &state.language.strings());
    let confirmed = unsafe {
        confirm_installer_handoff(
            window,
            state.language,
            file.file_name(),
            &publisher,
            file.sha256(),
            &region_label,
        )
    };
    if !confirmed {
        state.operation_status = handoff_declined_status(state.language);
        unsafe { render(window, chrome, state) };
        return;
    }
    state.operation_failure = None;
    state.handoff = Some(HandoffProgress {
        stage: HandoffStage::SwitchingRegion,
        file: Some(file.clone()),
        original_region: None,
        temporary_region: Some(region.clone()),
    });
    state.operation_status = operation_status(state);
    unsafe { render(window, chrome, state) };
    start_handoff(
        window,
        HandoffRequest {
            path,
            confirmed_sha256: file.sha256().to_owned(),
            temporary_region: region.geo_id,
        },
    );
}

/// Put the original region back after the user says Microsoft Store is done.
#[allow(unsafe_code)]
unsafe fn begin_region_restore(window: HWND, chrome: &WindowChrome, state: &mut AppState) {
    // The same control asks Microsoft for the Store installer when there is no
    // region to give back. Restoring wins whenever both are possible: a region
    // left switched is the more urgent of the two.
    if !state.region_restore_is_available() {
        if state.store_installer_download_is_available() {
            unsafe { begin_installer_download(window, chrome, state) };
        }
        return;
    }
    if let Some(handoff) = state.handoff.as_mut() {
        handoff.stage = HandoffStage::RestoringRegion;
    }
    state.operation_failure = None;
    state.operation_status = operation_status(state);
    unsafe { render(window, chrome, state) };
    start_region_restore(window);
}

/// Drop a handoff that has nothing left to do, keeping one that has.
///
/// A handoff still holding the region owns the window until the user puts it
/// back; one that already finished is only a message, and a new draft replaces
/// the message it was showing.
fn forget_finished_handoff(state: &mut AppState) {
    if !state.handoff_blocks_new_operation() && !state.region_restore_is_available() {
        state.handoff = None;
    }
}

/// Start one pass of market probes, keeping whatever this survey already knows.
///
/// A pass that is already running is stopped first: its answers belong to a
/// question the window has moved on from, and its generation makes them
/// recognisable when they arrive late.
fn start_survey_pass(
    window: HWND,
    state: &mut AppState,
    product_id: &StoreProductId,
    markets: Vec<MarketCode>,
    scope: SurveyScope,
) {
    if markets.is_empty() {
        return;
    }
    state.survey_passes += 1;
    let generation = state.survey_passes;
    let total = markets.len();
    let survey = match state.availability.take() {
        Some(existing) if existing.survey.answers_about(product_id) => {
            existing.cancel();
            existing.restart(generation, total)
        }
        Some(existing) => {
            existing.cancel();
            MarketSurvey::new(product_id.clone(), generation, total)
        }
        None => MarketSurvey::new(product_id.clone(), generation, total),
    };
    let cancellation = survey.cancellation();
    state.availability = Some(survey);
    start_market_survey(
        window,
        product_id.clone(),
        markets,
        generation,
        scope,
        cancellation,
    );
}

/// Ask the selected region's market for a card, before any region changes.
///
/// One request, and only when it would add something: a running pass will reach
/// this market on its own, and a market that already answered is not asked
/// twice.
pub(super) fn probe_selected_region(window: HWND, state: &mut AppState) {
    // The same identifier every region question uses. Asking the card probe
    // about one product and the search about another would let each reset the
    // other's survey, and the window would ping-pong between two passes.
    let (Some(product_id), Some(region)) = (
        state.searchable_product_id(),
        state.selected_temporary_region.clone(),
    ) else {
        return;
    };
    let Ok(market) = MarketCode::from_region(&region) else {
        return;
    };
    ask_device_compatibility(window, state, &product_id, &market);
    if let Some(availability) = state.availability.as_ref()
        && availability.survey.answers_about(&product_id)
    {
        if availability.running {
            return;
        }
        if availability
            .survey
            .answer_for(&market)
            .is_some_and(|answer| answer.availability != MarketAvailability::Unknown)
        {
            return;
        }
    }
    start_survey_pass(
        window,
        state,
        &product_id,
        vec![market],
        SurveyScope::Curated,
    );
}

/// The controls, or an empty borrow-free fallback the caller cannot use.
fn controls_of(chrome: &WindowChrome) -> &Controls {
    chrome
        .controls
        .as_ref()
        .expect("controls exist for the whole life of the window")
}

/// Ask Windows and the catalogue what is installed and where it is served.
///
/// Costs one request per installed Store application, so it is started by the
/// button and never by merely opening the tab.
#[allow(unsafe_code)]
unsafe fn begin_updates_scan(window: HWND, chrome: &WindowChrome, state: &mut AppState) {
    if matches!(state.updates, UpdatesView::Scanning { .. }) {
        return;
    }
    // The question is about the region the machine is in right now, so a region
    // Windows will not name leaves nothing to ask.
    let Some(Ok(region)) = state.current_region.as_ref() else {
        return;
    };
    let Ok(market) = MarketCode::from_region(region) else {
        return;
    };
    state.updates = UpdatesView::Scanning { done: 0, total: 0 };
    state.selected_update_index = None;
    // The remembered answer is being replaced, so its age no longer applies.
    state.updates_taken = None;
    unsafe { render(window, chrome, state) };
    start_updates_scan(window, market);
}

/// Carry the chosen application to the tab that can act on it.
///
/// Nothing is started here. The Installation tab owns every gate this product
/// has, and an update is not a new kind of operation — it is the same one,
/// aimed at a product the user already has.
#[allow(unsafe_code)]
unsafe fn open_update_in_installation(window: HWND, chrome: &WindowChrome, state: &mut AppState) {
    let Some(product_id) = state
        .selected_update()
        .and_then(|candidate| candidate.product_id.clone())
    else {
        return;
    };
    let controls = controls_of(chrome);
    // Writing the control rather than the draft: its change notification is the
    // one path that also drops the stale resolution and survey.
    let text = HSTRING::from(product_id.as_str());
    let _ = unsafe { SetWindowTextW(controls.input, &text) };
    state.selected_tab = Tab::Installation;
    let _ = unsafe {
        SendMessageW(
            controls.tabs,
            TCM_SETCURSEL,
            Some(WPARAM(Tab::Installation.index())),
            None,
        )
    };
    // The product is installed and unavailable here, so the one thing the user
    // needs next is a region that offers it.
    state.show_all_regions = false;
    start_survey_pass(
        window,
        state,
        &product_id,
        curated_markets(),
        SurveyScope::Curated,
    );
    state.operation_status = source_status(state);
    unsafe { render(window, chrome, state) };
}

/// Ask Microsoft for this product's own Store installer.
///
/// The second path, offered after the backend installed nothing. The file is
/// not run here and not admitted here: it arrives, it is inspected exactly as a
/// file the user chose by hand, and only then can the ordinary handoff start.
#[allow(unsafe_code)]
unsafe fn begin_installer_download(window: HWND, chrome: &WindowChrome, state: &mut AppState) {
    let Some(product_id) = state.accepted_product_id().cloned() else {
        return;
    };
    state.installer_download = Some(product_id.clone());
    state.operation_failure = None;
    state.operation_status = fill(
        state
            .language
            .strings()
            .command_installer_download_requested,
        &[("id", product_id.as_str())],
    );
    unsafe { render(window, chrome, state) };
    start_installer_download(window, product_id);
}

/// Ask the catalogue about delivery, once per product and market.
///
/// The answer decides whether an installation may start at all, so it is asked
/// before the region changes rather than discovered after it. Asking again for
/// a subject already answered would add nothing: the catalogue's verdict does
/// not depend on anything this window can do.
fn ask_device_compatibility(
    window: HWND,
    state: &AppState,
    product_id: &StoreProductId,
    market: &MarketCode,
) {
    if let Some((asked_product, asked_market, _)) = state.device_compatibility.as_ref()
        && asked_product == product_id
        && asked_market == market
    {
        return;
    }
    start_device_compatibility_probe(window, product_id.clone(), market.clone());
}

/// Start a full search by itself when the offered region turns out to refuse.
///
/// One market is asked for a card whenever the draft or the region changes. If
/// that market answers that the product is not offered there, the user is left
/// holding a region that cannot work and no hint about which one can — and the
/// region offered by default is a guess, not an answer. So the search they
/// would otherwise have to start by hand starts itself.
///
/// Only a refusal does this. A market that never answered says nothing about
/// the product, and widening on silence would turn every offline moment into
/// forty more requests.
fn widen_search_after_refusal(window: HWND, state: &mut AppState) {
    if !selected_region_refused(state) {
        return;
    }
    let Some(product_id) = state.searchable_product_id() else {
        return;
    };
    start_survey_pass(
        window,
        state,
        &product_id,
        curated_markets(),
        SurveyScope::Curated,
    );
}

/// Whether the region the window offers has itself refused the product.
///
/// A refusal is an answer; silence is not. `Unknown` means the market was never
/// reached, and treating that as a refusal would widen the search every time
/// the network hiccups.
fn selected_region_refused(state: &AppState) -> bool {
    let Some(availability) = state.availability.as_ref() else {
        return false;
    };
    // A pass that already searched has the answer; one still running will
    // arrive at it on its own.
    if availability.searched || availability.running {
        return false;
    }
    let (Some(product_id), Some(region)) = (
        state.searchable_product_id(),
        state.selected_temporary_region.as_ref(),
    ) else {
        return false;
    };
    if !availability.survey.answers_about(&product_id) {
        return false;
    }
    let Ok(market) = MarketCode::from_region(region) else {
        return false;
    };
    availability
        .survey
        .answer_for(&market)
        .is_some_and(|answer| answer.availability == MarketAvailability::NotOffered)
}

/// Fold one report from a survey pass into the window's state.
pub(super) fn apply_market_answers(window: HWND, state: &mut AppState, update: MarketSurveyUpdate) {
    let Some(availability) = state.availability.as_mut() else {
        return;
    };
    // A superseded pass still answers; its answers are about a question the
    // window no longer asks.
    if availability.generation != update.generation {
        return;
    }
    availability.answered += update.answers.len();
    for answer in update.answers {
        availability.survey.record(answer);
    }
    let finished = update.finished;
    if finished {
        availability.running = false;
        // The pass is over, so its answers become the list the user sees. Doing
        // this once per pass rather than per answer is what stops the flicker.
        availability.settle();
        if update.scope == SurveyScope::Complete {
            availability.survey.widen_to_complete();
        }
    }
    if finished {
        widen_search_after_refusal(window, state);
    }
    keep_selection_reachable(state);
}

/// Move the selection into the list the window actually shows.
///
/// A search can narrow the list past the region that was selected, and a
/// selection nobody can see is one an installation would still use.
fn keep_selection_reachable(state: &mut AppState) {
    let offered = state.offered_regions();
    if offered.is_empty() {
        return;
    }
    let reachable = state
        .selected_temporary_region
        .as_ref()
        .is_some_and(|selected| {
            offered
                .iter()
                .any(|region| region.geo_id == selected.geo_id)
        });
    if reachable {
        return;
    }
    let first = offered[0].clone();
    state.temporary_region_input = first.geo_id.value().to_string();
    state.selected_temporary_region = Some(first);
}

#[allow(unsafe_code)]
#[allow(clippy::too_many_lines)]
pub(super) unsafe fn dispatch_command(
    window: HWND,
    chrome: &WindowChrome,
    state: &mut AppState,
    wparam: WPARAM,
) {
    let id = wparam.0 & 0xffff;
    let notification = (wparam.0 >> 16) & 0xffff;
    if notification == 0 && id == ID_DIALOG_CANCEL {
        // Escape must take the same route as the close button and the Exit
        // menu item, never a direct `DestroyWindow`.
        let _ = unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        return;
    }
    if id == ID_UPDATES_REFRESH && notification == 0 {
        unsafe { begin_updates_scan(window, chrome, state) };
        return;
    }
    if id == ID_UPDATES_OPEN && notification == 0 {
        unsafe { open_update_in_installation(window, chrome, state) };
        return;
    }
    if notification == 0 && id == ID_INSTALL {
        unsafe { begin_primary_command(window, chrome, state) };
        return;
    }
    if notification == 0 && id == ID_RESTORE_REGION {
        unsafe { begin_region_restore(window, chrome, state) };
        return;
    }
    if notification == 0 && id == ID_FIND_REGION {
        // Deliberately the searchable identifier and not the accepted one: with
        // a file as the source there is no accepted product, and the link draft
        // is the only thing that can be asked about.
        if let Some(product_id) = state.searchable_product_id() {
            state.show_all_regions = false;
            start_survey_pass(
                window,
                state,
                &product_id,
                curated_markets(),
                SurveyScope::Curated,
            );
            unsafe { render(window, chrome, state) };
        }
        return;
    }
    if notification == 0 && id == ID_CHECK_REMAINING {
        let markets = remaining_markets(state);
        if let Some(product_id) = state.searchable_product_id()
            && !markets.is_empty()
        {
            start_survey_pass(window, state, &product_id, markets, SurveyScope::Complete);
            unsafe { render(window, chrome, state) };
        }
        return;
    }
    if notification == 0 && id == ID_SHOW_ALL_REGIONS {
        state.show_all_regions = !state.show_all_regions;
        unsafe { render(window, chrome, state) };
        return;
    }
    if notification == 0 && id == ID_DIALOG_OK {
        // Enter must not invent an action: the primary command has no handler
        // until a confirmed backend exists, and pressing it changes nothing.
        return;
    }
    if notification == 0
        && let Some(action) = menu_action(id)
    {
        unsafe { dispatch_menu_action(window, chrome, state, action) };
        return;
    }
    if id == ID_JOURNAL_OPEN_STORE && notification == 0 {
        // A handoff entry names a file, not a product, so there is no page to
        // open: the button is disabled for it and this refuses it as well.
        let Some(product_id) = state
            .selected_journal_record()
            .and_then(JournalRecord::product_id)
            .cloned()
        else {
            return;
        };
        let request = StorePageRequest::from_product_id(product_id);
        state.operation_status = match open_store_page(&WinStorePageOpener, &request) {
            Ok(_) => state.language.strings().command_dispatch_command.to_owned(),
            Err(_) => state
                .language
                .strings()
                .command_dispatch_command_2
                .to_owned(),
        };
        unsafe { render(window, chrome, state) };
        return;
    }
    if id == ID_JOURNAL_REPEAT && notification == 0 {
        let Some(product_id) = state
            .selected_journal_record()
            .and_then(JournalRecord::product_id)
            .cloned()
        else {
            return;
        };
        state.selected_tab = Tab::Installation;
        unsafe {
            dispatch_ui_event(
                window,
                chrome,
                state,
                UiEvent::User(UiAction::SetStoreText(product_id.as_str().to_owned())),
            );
        };
        state
            .language
            .strings()
            .command_dispatch_command_3
            .clone_into(&mut state.operation_status);
        unsafe { render(window, chrome, state) };
        return;
    }
    if id == ID_JOURNAL_COPY_ID && notification == 0 {
        let Some(product_id) = state
            .selected_journal_record()
            .and_then(JournalRecord::product_id)
            .cloned()
        else {
            return;
        };
        state.operation_status =
            match unsafe { copy_text_to_clipboard(window, product_id.as_str()) } {
                Ok(()) => state
                    .language
                    .strings()
                    .command_dispatch_command_4
                    .to_owned(),
                Err(error) => fill(
                    state.language.strings().command_clipboard_copy_failed,
                    &[(
                        "cause",
                        diagnostic_message(state.language, clipboard_diagnostic(error).code()),
                    )],
                ),
            };
        unsafe { render(window, chrome, state) };
        return;
    }
    if id == ID_JOURNAL_CLEAR && notification == 0 {
        if !matches!(&state.journal, JournalView::Ready(records) if !records.is_empty()) {
            return;
        }
        let message = state.language.strings().command_dispatch_command_5;
        let title = HSTRING::from(state.language.strings().title);
        let message = HSTRING::from(message);
        let confirmed = {
            let _modal = ModalScope::enter();
            let answer =
                unsafe { MessageBoxW(Some(window), &message, &title, MB_YESNO | MB_ICONWARNING) };
            answer == IDYES
        };
        if confirmed {
            state.journal = JournalView::Loading;
            state.selected_journal_index = None;
            start_journal_clear(window);
            unsafe { render(window, chrome, state) };
        }
        return;
    }
    if id == ID_JOURNAL_DELETE && notification == 0 {
        let Some(record) = state.selected_journal_record().cloned() else {
            return;
        };
        let message = state.language.strings().command_dispatch_command_6;
        let title = HSTRING::from(state.language.strings().title);
        let message = HSTRING::from(message);
        let confirmed = {
            let _modal = ModalScope::enter();
            let answer =
                unsafe { MessageBoxW(Some(window), &message, &title, MB_YESNO | MB_ICONWARNING) };
            answer == IDYES
        };
        if confirmed {
            state.journal = JournalView::Loading;
            state.selected_journal_index = None;
            start_journal_delete(window, record);
            unsafe { render(window, chrome, state) };
        }
        return;
    }
    let event = if id == ID_LANGUAGE && notification == CBN_SELCHANGE as usize {
        let Some(controls) = chrome.controls.as_ref() else {
            return;
        };
        let selection = unsafe { SendMessageW(controls.language, CB_GETCURSEL, None, None) }.0;
        UiEvent::User(UiAction::SelectLanguage(Language::from_index(selection)))
    } else if id == ID_STORE_INPUT && notification == EN_CHANGE as usize {
        let Some(controls) = chrome.controls.as_ref() else {
            return;
        };
        UiEvent::User(UiAction::SetStoreText(unsafe {
            read_window_text(controls.input)
        }))
    } else if id == ID_TEMPORARY_GEO_ID && notification == CBN_SELCHANGE as usize {
        let Some(controls) = chrome.controls.as_ref() else {
            return;
        };
        let selection =
            unsafe { SendMessageW(controls.temporary_region, CB_GETCURSEL, None, None) }.0;
        // The box lists whatever a survey left offered, so a selection is an
        // index into that list rather than into every region Windows knows.
        let offered = state.offered_regions();
        let region = usize::try_from(selection)
            .ok()
            .and_then(|index| offered.get(index).cloned());
        UiEvent::User(UiAction::SelectTemporaryRegion(region))
    } else if id == ID_SOURCE_LINK && notification == 0 {
        UiEvent::User(UiAction::SelectSource(ApplicationSourceKind::StoreText))
    } else if id == ID_SOURCE_FILE && notification == 0 {
        UiEvent::User(UiAction::SelectSource(ApplicationSourceKind::InstallerFile))
    } else if id == ID_CLEAR_FILE && notification == 0 {
        if state.application_source.selected() == ApplicationSourceKind::InstallerFile {
            state.application_source.clear_installer_file();
            state.stub_inspection = None;
            state.stub_inspection_error = None;
            state.resolution.invalidate();
            state.operation_status = source_status(state);
            unsafe { render(window, chrome, state) };
        }
        return;
    } else if id == ID_DETAILS && notification == 0 {
        if let Some(inspection) = state.stub_inspection.as_ref() {
            unsafe { show_stub_details(state.language, inspection) };
        }
        return;
    } else if id == ID_OPEN_STORE_PAGE && notification == 0 {
        let Some(product) = state.resolved_product() else {
            return;
        };
        let request = winstoreregion_core::StorePageRequest::from_resolved_product(product);
        state.operation_status = match open_store_page(&WinStorePageOpener, &request) {
            Ok(_) => state
                .language
                .strings()
                .command_dispatch_command_7
                .to_owned(),
            Err(_) => state
                .language
                .strings()
                .command_dispatch_command_8
                .to_owned(),
        };
        unsafe { render(window, chrome, state) };
        return;
    } else if id == ID_SOURCE_ACTION && notification == 0 {
        match state.application_source.selected() {
            ApplicationSourceKind::StoreText => {
                let Some(controls) = chrome.controls.as_ref() else {
                    return;
                };
                if state.application_source.store_text().is_empty() {
                    let _ = unsafe { SendMessageW(controls.input, WM_PASTE, None, None) };
                } else {
                    // Emptying the control rather than the draft: the change
                    // notification is what carries a new text through the one
                    // path that also drops the resolution and the survey.
                    let empty = HSTRING::new();
                    let _ = unsafe { SetWindowTextW(controls.input, &empty) };
                }
            }
            ApplicationSourceKind::InstallerFile => {
                if let Some(candidate) = unsafe { choose_installer_file(window, state.language) } {
                    unsafe { dispatch_file_candidate(window, chrome, state, candidate) };
                }
            }
        }
        return;
    } else {
        return;
    };
    unsafe { dispatch_ui_event(window, chrome, state, event) }
}

#[allow(unsafe_code)]
pub(super) unsafe fn dispatch_notification(
    window: HWND,
    chrome: &WindowChrome,
    state: &mut AppState,
    lparam: LPARAM,
) {
    if lparam.0 == 0 {
        return;
    }
    let notification = unsafe { &*(lparam.0 as *const NMHDR) };
    let Some(controls) = chrome.controls.as_ref() else {
        return;
    };
    // Both tables report their selection the same way the tab strip does.
    if notification.idFrom == ID_JOURNAL_LIST && notification.code == LVN_ITEMCHANGED {
        let selection = unsafe {
            SendMessageW(
                controls.journal_list,
                LVM_GETNEXTITEM,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM(isize::try_from(LVNI_SELECTED).unwrap_or(0))),
            )
        }
        .0;
        state.selected_journal_index = usize::try_from(selection).ok().filter(
            |index| matches!(&state.journal, JournalView::Ready(records) if *index < records.len()),
        );
        unsafe { render(window, chrome, state) };
        return;
    }
    if notification.idFrom == ID_UPDATES_LIST && notification.code == LVN_ITEMCHANGED {
        let selected = unsafe {
            SendMessageW(
                controls.updates_list,
                LVM_GETNEXTITEM,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM(isize::try_from(LVNI_SELECTED).unwrap_or(0))),
            )
        }
        .0;
        state.selected_update_index = usize::try_from(selected).ok();
        unsafe { render(window, chrome, state) };
        return;
    }
    if notification.idFrom != ID_TABS || notification.code != TCN_SELCHANGE {
        return;
    }
    let selection = unsafe { SendMessageW(controls.tabs, TCM_GETCURSEL, None, None) }.0;
    let Some(tab) = Tab::from_index(selection) else {
        let _ = unsafe {
            SendMessageW(
                controls.tabs,
                TCM_SETCURSEL,
                Some(WPARAM(state.selected_tab.index())),
                None,
            )
        };
        return;
    };
    unsafe {
        dispatch_ui_event(
            window,
            chrome,
            state,
            UiEvent::User(UiAction::SelectTab(tab)),
        );
        // The previously focused control may now be hidden, which would leave
        // the window without a focus a keyboard user can move from.
        focus_first_control_of_tab(controls, tab);
    }
}

/// Move focus to the first control of `tab` that can actually take it.
///
/// The tab control itself is the fallback, so focus never lands on a hidden or
/// disabled child and never disappears from the window.
#[allow(unsafe_code)]
unsafe fn focus_first_control_of_tab(controls: &Controls, tab: Tab) {
    let candidates: &[HWND] = match tab {
        Tab::Installation => &[controls.source_link, controls.input, controls.source_action],
        Tab::Journal => &[controls.journal_list, controls.journal_open_store],
        Tab::Updates => &[controls.updates_list, controls.updates_refresh],
    };
    for control in candidates {
        if unsafe { IsWindowVisible(*control) }.as_bool()
            && unsafe { IsWindowEnabled(*control) }.as_bool()
        {
            let _ = unsafe { SetFocus(Some(*control)) };
            return;
        }
    }
    let _ = unsafe { SetFocus(Some(controls.tabs)) };
}

#[allow(unsafe_code)]
pub(super) unsafe fn dispatch_ui_event(
    window: HWND,
    chrome: &WindowChrome,
    state: &mut AppState,
    event: UiEvent,
) {
    match event {
        UiEvent::User(UiAction::SelectLanguage(language)) => {
            state.language = language;
            state.operation_status = source_status(state);
            if unsafe { replace_window_menu(window, chrome, state.language) }.is_err() {
                language
                    .strings()
                    .command_dispatch_ui_event
                    .clone_into(&mut state.operation_status);
            }
        }
        UiEvent::User(UiAction::SelectTab(tab)) => state.selected_tab = tab,
        UiEvent::User(UiAction::SetStoreText(text)) => {
            // The inspection belongs to the file draft, and typing in the link
            // field does not touch that file. Discarding it here threw away a
            // verdict that was still true, and left the file source claiming an
            // inspection was running that nothing would ever finish. It is
            // cleared where the file itself changes, and nowhere else.
            forget_finished_handoff(state);
            state.application_source.set_store_text(text);
            state.application_source.select_store_text();
            state.resolution.invalidate();
            if let Ok(candidate) = StoreInputCandidate::parse(state.application_source.store_text())
            {
                state
                    .resolution
                    .accept_syntax(candidate.product_id().clone());
            }
            // A different product is a different question: nothing the previous
            // survey learned applies to it.
            if let Some(availability) = state.availability.as_ref() {
                availability.cancel();
            }
            state.availability = None;
            state.show_all_regions = false;
            probe_selected_region(window, state);
            unsafe { begin_resolution_if_ready(window, chrome, state) };
        }
        UiEvent::User(UiAction::SelectTemporaryRegion(region)) => {
            state.temporary_region_input = region
                .as_ref()
                .map_or_else(String::new, |region| region.geo_id.value().to_string());
            state.selected_temporary_region = region;
            probe_selected_region(window, state);
            unsafe { begin_resolution_if_ready(window, chrome, state) };
        }
        UiEvent::User(UiAction::SelectSource(source)) => {
            match source {
                ApplicationSourceKind::StoreText => {
                    state.application_source.select_store_text();
                }
                ApplicationSourceKind::InstallerFile => {
                    state.application_source.select_installer_file();
                }
            }
            state.operation_status = source_status(state);
        }
        UiEvent::StateUpdate(UiUpdate::OperationStatus(status)) => {
            state.operation_status = status;
        }
        UiEvent::StateUpdate(UiUpdate::OperationState(operation_state)) => {
            state.operation_state = Some(operation_state);
        }
        UiEvent::StateUpdate(UiUpdate::CurrentRegion(region)) => {
            state.current_region = Some(region);
        }
        UiEvent::StateUpdate(UiUpdate::JournalLoaded(result)) => {
            state.selected_journal_index = None;
            state.journal = match result {
                Ok(records) => {
                    state.selected_journal_index = (!records.is_empty()).then_some(0);
                    JournalView::Ready(records)
                }
                Err(error) => JournalView::Failed(error),
            };
        }
    }
    unsafe { render(window, chrome, state) }
}

#[allow(unsafe_code)]
unsafe fn begin_resolution_if_ready(window: HWND, chrome: &WindowChrome, state: &mut AppState) {
    state.resolution.invalidate();
    let Ok(candidate) = StoreInputCandidate::parse(state.application_source.store_text()) else {
        state.operation_status =
            store_source_status(state.language, state.application_source.store_text());
        unsafe { render(window, chrome, state) };
        return;
    };
    state
        .resolution
        .accept_syntax(candidate.product_id().clone());
    let Some(region) = state.selected_temporary_region.as_ref() else {
        state.operation_status =
            temporary_region_status(state.language, &state.temporary_region_input);
        unsafe { render(window, chrome, state) };
        return;
    };
    let Ok(market) = MarketCode::from_region(region) else {
        state
            .language
            .strings()
            .command_begin_resolution_if_ready
            .clone_into(&mut state.operation_status);
        unsafe { render(window, chrome, state) };
        return;
    };
    let request = state
        .resolution
        .begin(candidate.product_id().clone(), market);
    state
        .language
        .strings()
        .command_begin_resolution_if_ready_2
        .clone_into(&mut state.operation_status);
    start_product_resolution(window, request);
    unsafe { render(window, chrome, state) };
}

#[allow(dead_code)]
pub(super) fn validate_installer_file(
    path: PathBuf,
) -> std::result::Result<PathBuf, FileSelectionError> {
    let display = path.as_os_str().to_string_lossy();
    if display.starts_with(r"\\.\") || display.starts_with(r"\\?\") {
        return Err(FileSelectionError::LinkOrDevicePath);
    }
    InstallerStubPath::new(&path).map_err(|error| match error {
        InstallerStubPathError::EmptyPath => FileSelectionError::EmptyPath,
        InstallerStubPathError::UnsupportedExtension => FileSelectionError::UnsupportedExtension,
    })?;
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| FileSelectionError::NotAFile)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(FileSelectionError::NotAFile);
    }
    Ok(path)
}

#[allow(unsafe_code)]
pub(super) unsafe fn dispatch_file_candidate(
    window: HWND,
    chrome: &WindowChrome,
    state: &mut AppState,
    candidate: std::result::Result<PathBuf, FileSelectionError>,
) {
    match candidate {
        Ok(path) => {
            let selected = validate_installer_file(path).and_then(|path| {
                state
                    .application_source
                    .select_installer_stub(path.clone())
                    .map_err(|error| match error {
                        InstallerStubPathError::EmptyPath => FileSelectionError::EmptyPath,
                        InstallerStubPathError::UnsupportedExtension => {
                            FileSelectionError::UnsupportedExtension
                        }
                    })?;
                Ok(path)
            });
            match selected {
                Ok(path) => {
                    state.resolution.invalidate();
                    state.stub_inspection = None;
                    state.stub_inspection_error = None;
                    forget_finished_handoff(state);
                    state.operation_status = inspecting_file_status(state.language);
                    start_stub_inspection(window, path);
                }
                Err(error) => {
                    state.operation_status = file_error_status(state.language, error);
                }
            }
        }
        Err(error) => {
            state.operation_status = file_error_status(state.language, error);
        }
    }
    unsafe { render(window, chrome, state) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::state::FileSelectionError;
    use std::path::PathBuf;
    use winstoreregion_core::{GeoId, GuiLaunch, IsoAlpha2, MarketAnswer, Region};

    /// A window that asked one market about one product and got an answer.
    fn state_after_one_market(availability: MarketAvailability) -> AppState {
        let mut state = AppState::new(GuiLaunch {
            initial_input: Some("9WZDNCRFJ3L1".to_owned()),
        });
        let region = Region::new(
            GeoId::new(244).expect("valid GeoId"),
            "United States",
            IsoAlpha2::parse("us"),
        )
        .expect("complete region");
        state.selected_temporary_region = Some(region);
        let product_id = StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID");
        let market = MarketCode::parse("US").expect("valid market");
        let mut survey = MarketSurvey::new(product_id, 1, 1);
        survey.running = false;
        survey.survey.record(match availability {
            MarketAvailability::NotOffered => MarketAnswer::not_offered(market),
            _ => MarketAnswer::unknown(market),
        });
        state.availability = Some(survey);
        state
    }

    #[test]
    fn a_refusal_from_the_offered_region_widens_the_search_but_silence_does_not() {
        // The region offered by default is a guess. When the market itself says
        // the product is not there, the user is holding a region that cannot
        // work, and the search has to start without being asked.
        assert!(selected_region_refused(&state_after_one_market(
            MarketAvailability::NotOffered
        )));

        // A market that never answered says nothing about the product.
        assert!(!selected_region_refused(&state_after_one_market(
            MarketAvailability::Unknown
        )));

        // A pass that already searched has its answer; widening again would
        // repeat forty requests for nothing.
        let mut searched = state_after_one_market(MarketAvailability::NotOffered);
        if let Some(availability) = searched.availability.as_mut() {
            availability.searched = true;
        }
        assert!(!selected_region_refused(&searched));

        let mut running = state_after_one_market(MarketAvailability::NotOffered);
        if let Some(availability) = running.availability.as_mut() {
            availability.running = true;
        }
        assert!(!selected_region_refused(&running));

        // Nothing asked yet, nothing to widen after.
        let mut untouched = state_after_one_market(MarketAvailability::NotOffered);
        untouched.availability = None;
        assert!(!selected_region_refused(&untouched));
    }

    #[test]
    fn primary_file_check_rejects_wrong_extension_and_missing_executable() {
        let executable = std::env::current_exe().expect("test executable path");
        assert_eq!(
            validate_installer_file(executable.with_extension("txt")),
            Err(FileSelectionError::UnsupportedExtension)
        );
        assert_eq!(
            validate_installer_file(executable.with_file_name("missing-installer.exe")),
            Err(FileSelectionError::NotAFile)
        );
        assert_eq!(
            validate_installer_file(PathBuf::from(r"\\.\untrusted.exe")),
            Err(FileSelectionError::LinkOrDevicePath)
        );
    }
}
