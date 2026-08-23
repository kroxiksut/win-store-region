//! Read-only check for the packages an installation depends on.
//!
//! The check asks Windows which packages are registered for the current user.
//! It never launches `winget.exe`, never deploys or repairs anything, and never
//! touches the region: running a packaged application merely to learn whether it
//! exists would be a side effect this module exists to avoid.

use crate::platform::WindowsRuntimeGuard;
use crate::platform::winget::metadata::place_metadata_beside_executable;
use windows::Management::Deployment::PackageManager;
use windows::core::HSTRING;
use winstoreregion_core::{PrerequisiteReport, PrerequisiteState};

/// Package family of App Installer, which provides `winget`.
const APP_INSTALLER_FAMILY: &str = "Microsoft.DesktopAppInstaller_8wekyb3d8bbwe";

/// Package family of Microsoft Store.
const MICROSOFT_STORE_FAMILY: &str = "Microsoft.WindowsStore_8wekyb3d8bbwe";

/// Report which prerequisites the current user's machine satisfies.
///
/// A failure to reach the package API yields `Unknown` for everything rather
/// than `Missing`: an unanswered question is not a negative answer.
pub(crate) fn check_prerequisites() -> PrerequisiteReport {
    let Ok(_runtime) = WindowsRuntimeGuard::initialize() else {
        return PrerequisiteReport::unchecked();
    };
    let Ok(manager) = PackageManager::new() else {
        return PrerequisiteReport::unchecked();
    };
    // Putting the metadata in place is not an installation: it copies one file
    // out of a package that is already on the machine, into this application's
    // own directory.
    PrerequisiteReport::new(
        family_state(&manager, APP_INSTALLER_FAMILY),
        family_state(&manager, MICROSOFT_STORE_FAMILY),
        place_metadata_beside_executable().prerequisite_state(),
    )
}

/// Whether one package family is registered for the current user.
fn family_state(manager: &PackageManager, family: &str) -> PrerequisiteState {
    let family = HSTRING::from(family);
    let Ok(packages) =
        manager.FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &family)
    else {
        return PrerequisiteState::Unknown;
    };
    let Ok(iterator) = packages.First() else {
        return PrerequisiteState::Unknown;
    };
    match iterator.HasCurrent() {
        Ok(true) => PrerequisiteState::Satisfied,
        Ok(false) => PrerequisiteState::Missing,
        Err(_) => PrerequisiteState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winstoreregion_core::BackendPrerequisite;

    #[test]
    fn the_check_answers_for_every_prerequisite_without_side_effects() {
        // This runs on a real Windows machine, so the values depend on it. What
        // must hold everywhere is that the check completes and commits to an
        // answer for each prerequisite rather than panicking or hanging.
        let report = check_prerequisites();
        for prerequisite in [
            BackendPrerequisite::AppInstaller,
            BackendPrerequisite::MicrosoftStore,
            BackendPrerequisite::ComMetadata,
        ] {
            let state = report.state(prerequisite);
            assert!(
                matches!(
                    state,
                    PrerequisiteState::Satisfied
                        | PrerequisiteState::Missing
                        | PrerequisiteState::Unknown
                ),
                "{prerequisite:?} produced no state"
            );
        }
    }

    #[test]
    fn the_prerequisite_check_never_launches_an_application() {
        // Only code counts: this module's own prose names the very APIs it
        // promises not to call.
        let source: String = include_str!("prerequisites.rs")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        for forbidden in [
            ["Shell", "ExecuteW"].concat(),
            ["Create", "ProcessW"].concat(),
            ["Command", "::new"].concat(),
            ["winget", ".exe"].concat(),
        ] {
            assert!(
                !source.contains(&forbidden),
                "the read-only prerequisite check must not run anything: {forbidden}"
            );
        }
    }
}
