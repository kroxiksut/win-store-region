#![cfg_attr(windows, windows_subsystem = "windows")]

//! Entrypoint for the single `WinStoreRegion` executable.
//!
//! This file classifies arguments and hands control to exactly one
//! presentation surface. It holds no rule, no state, and no Windows call
//! beyond reporting a startup failure.
//!
//! The executable has three layers, and new code belongs to the first one that
//! describes it:
//!
//! 1. A rule, a state transition, or a validated value belongs in
//!    `winstoreregion-core`, never here.
//! 2. A Win32 or `WinRT` call belongs in `platform`, behind a narrow adapter
//!    that returns a structured result and decides no policy.
//! 3. Drawing, text, and user actions belong in `gui`, which renders core state
//!    and never restates a core rule.
//!

#[cfg(windows)]
#[allow(unsafe_code, clippy::inline_always, clippy::ref_as_ptr)]
mod gui;
#[cfg(windows)]
mod platform;

use winstoreregion_core::{
    APPLICATION_NAME, ArgumentError, CLI_USAGE, CliCommand, GuiLaunch, LaunchDisposition,
    classify_arguments,
};

fn main() {
    let arguments = std::env::args().skip(1);

    match classify_arguments(arguments) {
        Ok(LaunchDisposition::Gui(launch)) => match run_gui(launch) {
            Ok(()) => {}
            Err(message) => {
                write_cli_message(&message, true);
                std::process::exit(1);
            }
        },
        Ok(LaunchDisposition::Cli(CliCommand::Help)) => {
            write_cli_message(CLI_USAGE, false);
        }
        Err(error) => {
            write_cli_message(&format_argument_error(&error), true);
            std::process::exit(2);
        }
    }
}

fn format_argument_error(error: &ArgumentError) -> String {
    let reason = match error {
        ArgumentError::UnknownOption(option) => format!("Unknown option: {option}"),
        ArgumentError::TooManyPositionals => "Only one application input is allowed.".to_owned(),
    };

    format!("{reason}\n\n{CLI_USAGE}")
}

#[cfg(windows)]
fn run_gui(launch: GuiLaunch) -> Result<(), String> {
    gui::run(launch).map_err(|error| format!("Unable to start the graphical interface: {error}"))
}

#[cfg(not(windows))]
fn run_gui(_launch: GuiLaunch) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn write_cli_message(message: &str, is_error: bool) {
    if attach_to_parent_console() {
        if is_error {
            eprintln!("{message}");
        } else {
            println!("{message}");
        }
        return;
    }

    show_cli_dialog(message, is_error);
}

#[cfg(not(windows))]
fn write_cli_message(message: &str, is_error: bool) {
    if is_error {
        eprintln!("{message}");
    } else {
        println!("{message}");
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn attach_to_parent_console() -> bool {
    // A GUI-subsystem executable needs an explicit attachment before stdio can
    // target the terminal that launched its CLI branch.
    unsafe {
        windows::Win32::System::Console::AttachConsole(
            windows::Win32::System::Console::ATTACH_PARENT_PROCESS,
        )
        .is_ok()
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn show_cli_dialog(message: &str, is_error: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW,
    };
    use windows::core::HSTRING;

    let text = HSTRING::from(message);
    let title = HSTRING::from(APPLICATION_NAME);
    let icon = if is_error {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };

    unsafe {
        let _ = MessageBoxW(None, &text, &title, MB_OK | icon);
    }
}
