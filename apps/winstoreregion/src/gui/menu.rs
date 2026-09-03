//! The application menu and the actions it may dispatch.

use crate::gui::diagnostic::prerequisite_status;
use crate::gui::dialogs::copy_text_to_clipboard;
use crate::gui::direction::message_box_direction;
use crate::gui::ids::{
    ID_MENU_ABOUT, ID_MENU_COPY_DETAILS, ID_MENU_EXIT, ID_MENU_GET_APP_INSTALLER, ID_MENU_GITHUB,
    ID_MENU_OPEN_DIAGNOSTIC_LOG, ID_MENU_PASTE, ID_MENU_RECHECK_PREREQUISITES,
    ID_MENU_REGION_DOCUMENTATION, MenuAction,
};
use crate::gui::render::render;
use crate::gui::state::{AppState, ModalScope, Tab, WindowChrome};
use crate::gui::strings::{Language, Strings, TRANSLATION_CREDITS, fill};
use crate::platform::diagnostic_log::open_log_directory as open_diagnostic_log_directory;
use crate::platform::prerequisites::check_prerequisites;
use crate::platform::store::{
    open_prerequisite_page, open_project_repository, open_region_documentation,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateMenu, CreatePopupMenu, DestroyMenu, DrawMenuBar, HMENU, MB_ICONINFORMATION,
    MB_OK, MF_POPUP, MF_SEPARATOR, MF_STRING, MessageBoxW, SendMessageW, SetMenu, WM_CLOSE,
    WM_PASTE,
};
use windows::core::{HSTRING, PCWSTR, Result};
use winstoreregion_core::{
    APPLICATION_NAME, BackendPrerequisite, MICROSOFT_REGION_DOCUMENTATION,
    MICROSOFT_REGION_DOCUMENTATION_TITLE, PROJECT_AUTHOR, PROJECT_LICENSE, PROJECT_REPOSITORY,
};

#[allow(unsafe_code)]
unsafe fn create_application_menu(strings: &Strings) -> Result<HMENU> {
    let menu = unsafe { CreateMenu() }?;
    let file_menu = unsafe { CreatePopupMenu() }?;
    let help_menu = unsafe { CreatePopupMenu() }?;
    let result = (|| {
        let paste = HSTRING::from(strings.menu_paste);
        unsafe { AppendMenuW(file_menu, MF_STRING, ID_MENU_PASTE, &paste) }?;
        unsafe { AppendMenuW(file_menu, MF_SEPARATOR, 0, PCWSTR::null()) }?;
        let exit = HSTRING::from(strings.menu_exit);
        unsafe { AppendMenuW(file_menu, MF_STRING, ID_MENU_EXIT, &exit) }?;

        let about = HSTRING::from(strings.menu_about);
        unsafe { AppendMenuW(help_menu, MF_STRING, ID_MENU_ABOUT, &about) }?;
        let diagnostic_log = HSTRING::from(strings.menu_open_diagnostic_log);
        unsafe {
            AppendMenuW(
                help_menu,
                MF_STRING,
                ID_MENU_OPEN_DIAGNOSTIC_LOG,
                &diagnostic_log,
            )
        }?;
        let recheck = HSTRING::from(strings.menu_recheck);
        unsafe {
            AppendMenuW(
                file_menu,
                MF_STRING,
                ID_MENU_RECHECK_PREREQUISITES,
                &recheck,
            )
        }?;
        let get_app_installer = HSTRING::from(strings.menu_get_app_installer);
        unsafe {
            AppendMenuW(
                file_menu,
                MF_STRING,
                ID_MENU_GET_APP_INSTALLER,
                &get_app_installer,
            )
        }?;
        let copy_details = HSTRING::from(strings.menu_copy_details);
        unsafe { AppendMenuW(help_menu, MF_STRING, ID_MENU_COPY_DETAILS, &copy_details) }?;
        let region_documentation = HSTRING::from(strings.menu_region_documentation);
        unsafe {
            AppendMenuW(
                help_menu,
                MF_STRING,
                ID_MENU_REGION_DOCUMENTATION,
                &region_documentation,
            )
        }?;
        let github = HSTRING::from(strings.menu_github);
        unsafe { AppendMenuW(help_menu, MF_STRING, ID_MENU_GITHUB, &github) }?;

        let file = HSTRING::from(strings.menu_file);
        unsafe { AppendMenuW(menu, MF_POPUP, file_menu.0 as usize, &file) }?;
        let help = HSTRING::from(strings.menu_help);
        unsafe { AppendMenuW(menu, MF_POPUP, help_menu.0 as usize, &help) }?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = unsafe { DestroyMenu(menu) };
        return Err(error);
    }
    Ok(menu)
}

#[allow(unsafe_code)]
pub(super) unsafe fn replace_window_menu(
    window: HWND,
    chrome: &WindowChrome,
    language: Language,
) -> Result<()> {
    let menu = unsafe { create_application_menu(&language.strings()) };
    let menu = menu?;
    if let Err(error) = unsafe { SetMenu(window, Some(menu)) } {
        let _ = unsafe { DestroyMenu(menu) };
        return Err(error);
    }
    if let Some(previous) = chrome.menu.replace(Some(menu)) {
        let _ = unsafe { DestroyMenu(previous) };
    }
    let _ = unsafe { DrawMenuBar(window) };
    Ok(())
}

#[allow(unsafe_code)]
unsafe fn show_information(language: Language, message: &str) {
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

/// How the vendor documentation is introduced, in English in every language.
///
/// The claim is exactly what the article supports: Microsoft documents the
/// setting and how to change it. What an application bought in another region
/// does afterwards is the user's own risk, and this product does not pretend
/// otherwise anywhere else either.
const VENDOR_DOCUMENTATION_NOTE: &str =
    "Changing the Windows country or region is a procedure documented by Microsoft:";

/// What to say after asking Windows to open the vendor documentation.
const fn documentation_status(opened: bool, language: Language) -> &'static str {
    let strings = language.strings();
    if opened {
        strings.menu_documentation_opened
    } else {
        strings.menu_documentation_not_opened
    }
}

#[allow(unsafe_code)]
/// What to say after trying to open the project page.
///
/// The address is named when the launch failed, so a user whose browser did not
/// come up still knows where to go. It is filled in from `PROJECT_REPOSITORY`
/// rather than written into the text, so moving the project cannot leave a
/// language file pointing at the old address.
fn repository_status(opened: bool, language: Language) -> String {
    let strings = language.strings();
    if opened {
        strings.menu_project_page_opened.to_owned()
    } else {
        fill(
            strings.menu_project_page_not_opened,
            &[("address", PROJECT_REPOSITORY)],
        )
    }
}

/// Who translated the languages this build carries, for the About dialog.
///
/// Generated from the language files, so a language and the people who provided
/// it cannot drift apart. Empty when nobody is credited, and then the dialog
/// says nothing rather than showing a heading over nothing.
fn translation_credits(language: Language) -> String {
    let credits = TRANSLATION_CREDITS
        .iter()
        .filter(|(_, authors)| !authors.is_empty())
        .map(|(name, authors)| format!("{name} — {authors}"))
        .collect::<Vec<_>>()
        .join("\n");
    if credits.is_empty() {
        return String::new();
    }
    format!("\n\n{}\n{credits}", language.strings().about_translations)
}

pub(super) unsafe fn dispatch_menu_action(
    window: HWND,
    chrome: &WindowChrome,
    state: &mut AppState,
    action: MenuAction,
) {
    match action {
        MenuAction::PasteStoreInput => {
            state.selected_tab = Tab::Installation;
            if let Some(controls) = chrome.controls.as_ref() {
                let _ = unsafe { SendMessageW(controls.input, WM_PASTE, None, None) };
            }
        }
        MenuAction::Exit => {
            let _ = unsafe { SendMessageW(window, WM_CLOSE, None, None) };
            return;
        }
        MenuAction::About => {
            let strings = state.language.strings();
            // The heading is assembled here rather than translated, so no
            // language file carries a version number that could fall behind
            // the build it ships in.
            let about = format!(
                "{APPLICATION_NAME} {}\n\n{}",
                env!("CARGO_PKG_VERSION"),
                strings.about_description
            );
            // Who wrote it, under what licence, and where it lives come before
            // anything else this dialog says. A user holding only the
            // executable learns all three from here and from nowhere else.
            let attribution = format!(
                "{}: {PROJECT_AUTHOR}\n{}: {PROJECT_LICENSE}\n{PROJECT_REPOSITORY}",
                strings.about_author, strings.about_license
            );
            // Microsoft documents this setting and how to change it, which is
            // what this line says. It stays in English in every language: that
            // is the article that stays where it is.
            let message = format!(
                "{about}

{attribution}

{VENDOR_DOCUMENTATION_NOTE}
{MICROSOFT_REGION_DOCUMENTATION_TITLE}
{MICROSOFT_REGION_DOCUMENTATION}{}",
                translation_credits(state.language)
            );
            unsafe { show_information(state.language, &message) };
        }
        MenuAction::OpenDiagnosticLog => {
            let strings = state.language.strings();
            let message = if open_diagnostic_log_directory() {
                strings.menu_log_directory_opened
            } else {
                strings.menu_log_directory_not_opened
            };
            unsafe { show_information(state.language, message) };
        }
        MenuAction::RecheckPrerequisites => {
            state.prerequisites = check_prerequisites();
            state.operation_status = prerequisite_status(state.language, state.prerequisites)
                .unwrap_or_else(|| {
                    state
                        .language
                        .strings()
                        .menu_prerequisites_all_met
                        .to_owned()
                });
        }
        MenuAction::OpenAppInstallerPage => {
            let opened = open_prerequisite_page(BackendPrerequisite::AppInstaller.remedy());
            let strings = state.language.strings();
            let message = if opened {
                strings.menu_app_installer_page_opened
            } else {
                strings.menu_app_installer_page_not_opened
            };
            unsafe { show_information(state.language, message) };
        }
        MenuAction::CopyDiagnosticDetails => {
            // The status already carries the technical block, so copying it is
            // exactly what a bug report needs.
            let copied = unsafe { copy_text_to_clipboard(window, &state.operation_status) };
            let strings = state.language.strings();
            let message = if copied.is_ok() {
                strings.menu_details_copied
            } else {
                strings.menu_details_not_copied
            };
            unsafe { show_information(state.language, message) };
        }
        MenuAction::OpenRegionDocumentation => {
            let opened = open_region_documentation();
            unsafe {
                show_information(state.language, documentation_status(opened, state.language));
            };
        }
        MenuAction::OpenProjectGithub => {
            let opened = open_project_repository();
            unsafe { show_information(state.language, &repository_status(opened, state.language)) };
        }
    }
    unsafe { render(window, chrome, state) };
}
