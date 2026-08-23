//! Classification of command-line arguments into a launch surface.

/// Command-line usage for the single executable.
pub const CLI_USAGE: &str = concat!(
    "Usage:\n",
    "  WinStoreRegion.exe [Product ID | Store URL/URI]\n",
    "  WinStoreRegion.exe --help\n\n",
    "A positional application input opens the graphical interface with that input prefilled.\n",
);

/// Chooses the presentation surface without interpreting application input.
///
/// Product-ID, URL, and URI validation belongs to the input parser. This
/// classifier only prevents command-line switches from being mistaken for GUI
/// input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchDisposition {
    /// Start the graphical interface, optionally with a draft application input.
    Gui(GuiLaunch),
    /// Execute a console-oriented command without creating a window.
    Cli(CliCommand),
}

/// Values supplied to the graphical interface at startup.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GuiLaunch {
    /// Product ID or Store URL/URI supplied as text.
    pub initial_input: Option<String>,
}

/// Commands whose behaviour is already part of the public executable contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CliCommand {
    /// Print command-line usage.
    Help,
}

/// A malformed command line that must finish with exit code 2.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArgumentError {
    /// A switch is not part of the executable contract.
    UnknownOption(String),
    /// More than one positional application input was supplied.
    TooManyPositionals,
}

/// Classify executable arguments after the executable name.
///
/// A single positional value stays on the GUI path. Future CLI commands must
/// be added here explicitly; they must not be inferred from application input.
///
/// # Errors
///
/// Returns [`ArgumentError`] for an unrecognised switch or more than one
/// positional application input.
pub fn classify_arguments<I, S>(arguments: I) -> Result<LaunchDisposition, ArgumentError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut arguments = arguments.into_iter();
    let Some(first) = arguments.next() else {
        return Ok(LaunchDisposition::Gui(GuiLaunch::default()));
    };
    let first = first.as_ref();

    if arguments.next().is_some() {
        return Err(ArgumentError::TooManyPositionals);
    }

    match first {
        "--help" | "-h" => Ok(LaunchDisposition::Cli(CliCommand::Help)),
        option if option.starts_with('-') => Err(ArgumentError::UnknownOption(option.to_owned())),
        input => Ok(LaunchDisposition::Gui(GuiLaunch {
            initial_input: Some(input.to_owned()),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_start_the_gui_without_a_prefill() {
        assert_eq!(
            classify_arguments::<[&str; 0], _>([]),
            Ok(LaunchDisposition::Gui(GuiLaunch {
                initial_input: None,
            }))
        );
    }

    #[test]
    fn a_product_id_starts_the_gui_with_a_prefill() {
        assert_eq!(
            classify_arguments(["9WZDNCRFJ3PZ"]),
            Ok(LaunchDisposition::Gui(GuiLaunch {
                initial_input: Some("9WZDNCRFJ3PZ".to_owned()),
            }))
        );
    }

    #[test]
    fn a_store_url_starts_the_gui_with_a_prefill() {
        assert_eq!(
            classify_arguments(["https://apps.microsoft.com/detail/9WZDNCRFJ3PZ"]),
            Ok(LaunchDisposition::Gui(GuiLaunch {
                initial_input: Some("https://apps.microsoft.com/detail/9WZDNCRFJ3PZ".to_owned()),
            }))
        );
    }

    #[test]
    fn a_store_uri_starts_the_gui_with_a_prefill() {
        assert_eq!(
            classify_arguments(["ms-windows-store://pdp/?productid=9WZDNCRFJ3PZ"]),
            Ok(LaunchDisposition::Gui(GuiLaunch {
                initial_input: Some("ms-windows-store://pdp/?productid=9WZDNCRFJ3PZ".to_owned()),
            }))
        );
    }

    #[test]
    fn an_unsupported_path_remains_untrusted_gui_text() {
        assert_eq!(
            classify_arguments([r"C:\Downloads\installer.exe"]),
            Ok(LaunchDisposition::Gui(GuiLaunch {
                initial_input: Some(r"C:\Downloads\installer.exe".to_owned()),
            }))
        );
    }

    #[test]
    fn help_is_a_cli_command() {
        assert_eq!(
            classify_arguments(["--help"]),
            Ok(LaunchDisposition::Cli(CliCommand::Help))
        );
        assert_eq!(
            classify_arguments(["-h"]),
            Ok(LaunchDisposition::Cli(CliCommand::Help))
        );
        assert!(CLI_USAGE.contains("--help"));
    }

    #[test]
    fn an_unknown_option_is_rejected() {
        assert_eq!(
            classify_arguments(["--install"]),
            Err(ArgumentError::UnknownOption("--install".to_owned()))
        );
    }

    #[test]
    fn multiple_positional_values_are_rejected() {
        assert_eq!(
            classify_arguments(["9WZDNCRFJ3PZ", "unexpected"]),
            Err(ArgumentError::TooManyPositionals)
        );
    }
}
