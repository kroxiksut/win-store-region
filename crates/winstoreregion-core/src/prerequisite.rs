//! What must already exist on the machine before an installation is possible.
//!
//! This module checks nothing itself: a platform adapter reports what it found,
//! and the rules here decide whether a new guarded operation may start. Nothing
//! here downloads, installs, or changes the Windows region.

/// Page for installing App Installer, which provides `winget`.
///
/// A web page rather than an `ms-windows-store://` URI on purpose: the URI is
/// handled by Microsoft Store, and Microsoft Store is itself one of the things
/// that may be missing. The address is a constant so no caller can send the
/// user to an arbitrary destination.
pub const APP_INSTALLER_PAGE: &str = "https://apps.microsoft.com/detail/9NBLGGH4NNS1";

/// Something that must be present before an installation can be attempted.
///
/// Accepting the `msstore` source agreements is deliberately absent. It cannot
/// be established read-only, the backend passes the acceptance flag on every
/// call, and a refusal surfaces during the operation as its own diagnostic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendPrerequisite {
    /// The App Installer package, which provides `winget`.
    AppInstaller,
    /// Microsoft Store, through which `msstore` packages are acquired.
    MicrosoftStore,
    /// Interface metadata beside the executable, without which no COM call works.
    ///
    /// The application satisfies this itself by copying the file out of the
    /// installed App Installer package. It becomes a prerequisite the user has
    /// to act on only when that copy cannot be made.
    ComMetadata,
}

impl BackendPrerequisite {
    /// Stable machine-facing name for logs and details.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AppInstaller => "app_installer",
            Self::MicrosoftStore => "microsoft_store",
            Self::ComMetadata => "com_metadata",
        }
    }

    /// What the user can do about this prerequisite.
    #[must_use]
    pub const fn remedy(self) -> PrerequisiteRemedy {
        match self {
            Self::AppInstaller => PrerequisiteRemedy::OpenAppInstallerPage,
            Self::MicrosoftStore => PrerequisiteRemedy::RepairMicrosoftStore,
            Self::ComMetadata => PrerequisiteRemedy::MoveToWritableDirectory,
        }
    }
}

/// The only action offered for an unmet prerequisite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrerequisiteRemedy {
    /// Open the official page from which App Installer can be installed.
    OpenAppInstallerPage,
    /// Microsoft Store has no standalone download; Windows must be repaired.
    ///
    /// This deliberately carries no page: offering a link that cannot install
    /// Microsoft Store would promise something the product cannot deliver.
    RepairMicrosoftStore,
    /// The executable's own directory rejects writes, so metadata cannot land beside it.
    ///
    /// The remedy belongs to the user: move the application somewhere writable.
    /// The product does not copy itself elsewhere to work around this.
    MoveToWritableDirectory,
}

impl PrerequisiteRemedy {
    /// The fixed page for this remedy, when an honest one exists.
    #[must_use]
    pub const fn page(self) -> Option<&'static str> {
        match self {
            Self::OpenAppInstallerPage => Some(APP_INSTALLER_PAGE),
            Self::RepairMicrosoftStore | Self::MoveToWritableDirectory => None,
        }
    }
}

/// What the platform adapter established about one prerequisite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrerequisiteState {
    /// The prerequisite is present.
    Satisfied,
    /// The prerequisite is definitely absent.
    Missing,
    /// The check could not be completed.
    ///
    /// This is not the same as absent, and it must never be treated as
    /// present: an unverified machine is not a machine that is known to work.
    Unknown,
}

/// The state of every prerequisite at one moment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrerequisiteReport {
    app_installer: PrerequisiteState,
    microsoft_store: PrerequisiteState,
    com_metadata: PrerequisiteState,
}

impl PrerequisiteReport {
    /// Build a report from what the adapter found.
    #[must_use]
    pub const fn new(
        app_installer: PrerequisiteState,
        microsoft_store: PrerequisiteState,
        com_metadata: PrerequisiteState,
    ) -> Self {
        Self {
            app_installer,
            microsoft_store,
            com_metadata,
        }
    }

    /// A report for a machine that has not been checked yet.
    ///
    /// Everything is `Unknown`, so an operation cannot start before the first
    /// real check has run.
    #[must_use]
    pub const fn unchecked() -> Self {
        Self::new(
            PrerequisiteState::Unknown,
            PrerequisiteState::Unknown,
            PrerequisiteState::Unknown,
        )
    }

    /// State of one prerequisite.
    #[must_use]
    pub const fn state(&self, prerequisite: BackendPrerequisite) -> PrerequisiteState {
        match prerequisite {
            BackendPrerequisite::AppInstaller => self.app_installer,
            BackendPrerequisite::MicrosoftStore => self.microsoft_store,
            BackendPrerequisite::ComMetadata => self.com_metadata,
        }
    }

    /// Whether a new guarded operation may start.
    ///
    /// Only a fully satisfied report admits one. `Unknown` blocks exactly like
    /// `Missing`: the caller must not turn an unfinished check into permission.
    #[must_use]
    pub const fn admits_installation(&self) -> bool {
        matches!(self.app_installer, PrerequisiteState::Satisfied)
            && matches!(self.microsoft_store, PrerequisiteState::Satisfied)
            && matches!(self.com_metadata, PrerequisiteState::Satisfied)
    }

    /// Whether a Store-installer handoff may start.
    ///
    /// Deliberately narrower than [`Self::admits_installation`]. A handoff asks
    /// no backend and makes no COM call: it switches the region and hands the
    /// signed installer to Microsoft Store, so only Microsoft Store itself has
    /// to be there. Requiring App Installer here would block an operation that
    /// does not use it.
    #[must_use]
    pub const fn admits_handoff(&self) -> bool {
        matches!(self.microsoft_store, PrerequisiteState::Satisfied)
    }

    /// Prerequisites that are not satisfied, in a stable order.
    #[must_use]
    pub fn unsatisfied(&self) -> Vec<BackendPrerequisite> {
        [
            BackendPrerequisite::AppInstaller,
            BackendPrerequisite::MicrosoftStore,
            BackendPrerequisite::ComMetadata,
        ]
        .into_iter()
        .filter(|prerequisite| self.state(*prerequisite) != PrerequisiteState::Satisfied)
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [BackendPrerequisite; 3] = [
        BackendPrerequisite::AppInstaller,
        BackendPrerequisite::MicrosoftStore,
        BackendPrerequisite::ComMetadata,
    ];

    fn report_with(
        prerequisite: BackendPrerequisite,
        state: PrerequisiteState,
    ) -> PrerequisiteReport {
        let met = PrerequisiteState::Satisfied;
        match prerequisite {
            BackendPrerequisite::AppInstaller => PrerequisiteReport::new(state, met, met),
            BackendPrerequisite::MicrosoftStore => PrerequisiteReport::new(met, state, met),
            BackendPrerequisite::ComMetadata => PrerequisiteReport::new(met, met, state),
        }
    }

    #[test]
    fn an_unchecked_machine_never_admits_an_installation() {
        let report = PrerequisiteReport::unchecked();
        assert!(!report.admits_installation());
        assert_eq!(report.unsatisfied(), ALL.to_vec());
    }

    #[test]
    fn unknown_blocks_exactly_like_missing() {
        for blocking in [PrerequisiteState::Missing, PrerequisiteState::Unknown] {
            for prerequisite in ALL {
                assert!(
                    !report_with(prerequisite, blocking).admits_installation(),
                    "{blocking:?} in {prerequisite:?} must block"
                );
            }
        }
        assert!(
            PrerequisiteReport::new(
                PrerequisiteState::Satisfied,
                PrerequisiteState::Satisfied,
                PrerequisiteState::Satisfied,
            )
            .admits_installation()
        );
    }

    #[test]
    fn only_app_installer_offers_a_page_and_it_is_a_web_address() {
        let page = BackendPrerequisite::AppInstaller
            .remedy()
            .page()
            .expect("App Installer can be installed from a page");
        assert!(page.starts_with("https://"));
        // An `ms-windows-store://` URI would need the very component that may
        // be missing, so the remedy must not use one.
        assert!(!page.contains("ms-windows-store"));

        assert_eq!(
            BackendPrerequisite::MicrosoftStore.remedy(),
            PrerequisiteRemedy::RepairMicrosoftStore
        );
        assert_eq!(
            BackendPrerequisite::MicrosoftStore.remedy().page(),
            None,
            "there is no official standalone download for Microsoft Store"
        );
    }

    #[test]
    fn unsatisfied_lists_only_what_actually_blocks() {
        let report = report_with(
            BackendPrerequisite::MicrosoftStore,
            PrerequisiteState::Missing,
        );
        assert_eq!(
            report.unsatisfied(),
            vec![BackendPrerequisite::MicrosoftStore]
        );
    }

    #[test]
    fn missing_metadata_offers_no_page_because_only_the_user_can_move_the_application() {
        let remedy = BackendPrerequisite::ComMetadata.remedy();
        assert_eq!(remedy, PrerequisiteRemedy::MoveToWritableDirectory);
        assert_eq!(remedy.page(), None);
    }
}
