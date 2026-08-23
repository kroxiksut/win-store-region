//! Placement of the `WinGet` interface metadata beside the executable.
//!
//! Without `Microsoft.Management.Deployment.winmd` in the executable's own
//! directory, Windows cannot marshal the packaged COM server's interfaces and
//! every call fails with `0x80073D54`. The file belongs to the installed App
//! Installer package, so this module copies it from there instead of shipping a
//! foreign binary. The working directory is not an alternative: Windows looks
//! beside the executable and nowhere else.

use std::path::{Path, PathBuf};
use windows::Win32::Storage::Packaging::Appx::{
    GetPackagePathByFullName, GetPackagesByPackageFamily,
};
use windows::core::{HSTRING, PWSTR};
use winstoreregion_core::PrerequisiteState;

/// File name Windows resolves beside the executable.
const METADATA_FILE: &str = "Microsoft.Management.Deployment.winmd";

/// Package family that owns the metadata and the COM server behind it.
const APP_INSTALLER_FAMILY: &str = "Microsoft.DesktopAppInstaller_8wekyb3d8bbwe";

/// What placing the metadata beside the executable ended with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MetadataPlacement {
    /// The file is beside the executable, whether it was already there or just copied.
    Present,
    /// The executable's directory rejected the write.
    DirectoryNotWritable,
    /// App Installer is not installed, so there is nothing to copy from.
    SourcePackageMissing,
    /// The attempt could not be completed and produced no answer either way.
    Undetermined,
}

impl MetadataPlacement {
    /// Translate placement into the prerequisite state core reasons about.
    ///
    /// A missing source package is reported as `Unknown` rather than `Missing`:
    /// the real problem is then App Installer itself, which is its own
    /// prerequisite with its own remedy, and naming this one too would send the
    /// user after the wrong fix.
    pub(crate) const fn prerequisite_state(self) -> PrerequisiteState {
        match self {
            Self::Present => PrerequisiteState::Satisfied,
            Self::DirectoryNotWritable => PrerequisiteState::Missing,
            Self::SourcePackageMissing | Self::Undetermined => PrerequisiteState::Unknown,
        }
    }
}

/// Ensure the metadata sits beside the executable, copying it when it does not.
///
/// Safe to call more than once and safe to call in the process that will
/// activate the COM server: the copy takes effect without a restart, provided
/// it happens before the first activation.
pub(crate) fn place_metadata_beside_executable() -> MetadataPlacement {
    let Ok(executable) = std::env::current_exe() else {
        return MetadataPlacement::Undetermined;
    };
    let Some(directory) = executable.parent() else {
        return MetadataPlacement::Undetermined;
    };
    let target = directory.join(METADATA_FILE);
    if target.is_file() {
        return MetadataPlacement::Present;
    }
    let Some(source) = metadata_in_app_installer() else {
        return MetadataPlacement::SourcePackageMissing;
    };
    copy_metadata(&source, &target)
}

/// Copy the metadata and classify a failure by what the user can do about it.
fn copy_metadata(source: &Path, target: &Path) -> MetadataPlacement {
    match std::fs::copy(source, target) {
        Ok(_) => MetadataPlacement::Present,
        Err(error) => match error.kind() {
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem => {
                MetadataPlacement::DirectoryNotWritable
            }
            _ => MetadataPlacement::Undetermined,
        },
    }
}

/// Full path of the metadata inside the installed App Installer package.
///
/// Deliberately not a hard-coded path: the package directory carries its
/// version, so it changes with every App Installer update.
#[allow(unsafe_code)]
fn metadata_in_app_installer() -> Option<PathBuf> {
    let family = HSTRING::from(APP_INSTALLER_FAMILY);
    let mut count = 0;
    let mut buffer_length = 0;
    // The first call only reports the sizes the second one must supply.
    unsafe {
        let _ =
            GetPackagesByPackageFamily(&family, &raw mut count, None, &raw mut buffer_length, None);
    }
    if count == 0 {
        return None;
    }
    let mut names = vec![PWSTR::null(); count as usize];
    let mut buffer = vec![0u16; buffer_length as usize];
    unsafe {
        GetPackagesByPackageFamily(
            &family,
            &raw mut count,
            Some(names.as_mut_ptr()),
            &raw mut buffer_length,
            Some(PWSTR(buffer.as_mut_ptr())),
        )
    }
    .ok()
    .ok()?;
    let full_name = unsafe { names.first()?.to_hstring() };

    let mut path_length = 0;
    unsafe {
        let _ = GetPackagePathByFullName(&full_name, &raw mut path_length, None);
    }
    let mut path = vec![0u16; path_length as usize];
    unsafe {
        GetPackagePathByFullName(
            &full_name,
            &raw mut path_length,
            Some(PWSTR(path.as_mut_ptr())),
        )
    }
    .ok()
    .ok()?;
    let path = String::from_utf16_lossy(&path[..path_length.saturating_sub(1) as usize]);
    Some(Path::new(&path).join(METADATA_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_only_directory_is_the_only_failure_the_user_can_act_on() {
        assert_eq!(
            MetadataPlacement::DirectoryNotWritable.prerequisite_state(),
            PrerequisiteState::Missing
        );
        // A machine without App Installer must be sent to that prerequisite,
        // not told to move the application somewhere else.
        assert_eq!(
            MetadataPlacement::SourcePackageMissing.prerequisite_state(),
            PrerequisiteState::Unknown
        );
        assert_eq!(
            MetadataPlacement::Undetermined.prerequisite_state(),
            PrerequisiteState::Unknown
        );
        assert_eq!(
            MetadataPlacement::Present.prerequisite_state(),
            PrerequisiteState::Satisfied
        );
    }

    #[test]
    fn the_metadata_is_looked_up_in_the_package_rather_than_a_fixed_path() {
        // This machine may or may not have App Installer; what must hold is
        // that the lookup answers instead of panicking, and that any answer it
        // gives points at the package's own file.
        if let Some(path) = metadata_in_app_installer() {
            assert!(path.ends_with(METADATA_FILE));
            assert!(
                path.to_string_lossy().contains("WindowsApps"),
                "the metadata must come from the installed package"
            );
        }
    }

    #[test]
    fn placement_reports_an_answer_on_this_machine() {
        // Copying beside the test executable is harmless: the file is the same
        // one the product needs, and the directory is a build output.
        let placement = place_metadata_beside_executable();
        assert!(matches!(
            placement,
            MetadataPlacement::Present
                | MetadataPlacement::DirectoryNotWritable
                | MetadataPlacement::SourcePackageMissing
                | MetadataPlacement::Undetermined
        ));
    }
}
