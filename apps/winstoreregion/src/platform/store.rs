//! Adapters for opening a Microsoft Store page.
//!
//! Product resolution moved to `platform::winget`, which reaches the same
//! catalogue through the COM API instead of probing for a `WinRT` class that
//! never existed.

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR};
use winstoreregion_core::{
    MICROSOFT_REGION_DOCUMENTATION, PROJECT_REPOSITORY, PrerequisiteRemedy, StorePageOpenError,
    StorePageOpener, StorePageRequest,
};

/// Open the fixed page that lets a user satisfy one prerequisite.
///
/// The address is not a parameter: it comes from the remedy itself, so no
/// caller can send the user somewhere else. Returns whether Windows accepted
/// the launch. Opening a page is never an installation.
#[allow(unsafe_code)]
pub(crate) fn open_prerequisite_page(remedy: PrerequisiteRemedy) -> bool {
    let Some(page) = remedy.page() else {
        return false;
    };
    let page = HSTRING::from(page);
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::null(),
            &page,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    (result.0 as usize) > 32
}

/// Open this project's repository page.
///
/// Same rule as every other address here: it comes from a constant in core, so
/// nothing a user typed can decide where the browser goes. Returns whether
/// Windows accepted the launch.
#[allow(unsafe_code)]
pub(crate) fn open_project_repository() -> bool {
    let page = HSTRING::from(PROJECT_REPOSITORY);
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::null(),
            &page,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    (result.0 as usize) > 32
}

/// Open Microsoft's own page about changing the country or region.
///
/// Like a prerequisite page, the address is not a parameter: it comes from the
/// constant in core, so no caller can send the user somewhere else. Returns
/// whether Windows accepted the launch.
#[allow(unsafe_code)]
pub(crate) fn open_region_documentation() -> bool {
    let page = HSTRING::from(MICROSOFT_REGION_DOCUMENTATION);
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::null(),
            &page,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    (result.0 as usize) > 32
}

/// Windows Shell adapter for the explicit, non-install PDP fallback action.
#[allow(dead_code)]
pub(crate) struct WinStorePageOpener;

impl StorePageOpener for WinStorePageOpener {
    #[allow(unsafe_code)]
    fn open_store_page(&self, request: &StorePageRequest) -> Result<(), StorePageOpenError> {
        let uri = HSTRING::from(request.uri());
        let result = unsafe {
            ShellExecuteW(
                None,
                PCWSTR::null(),
                &uri,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        ((result.0 as usize) > 32)
            .then_some(())
            .ok_or(StorePageOpenError::LaunchRejected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remedy_without_a_page_opens_nothing() {
        assert!(!open_prerequisite_page(
            PrerequisiteRemedy::RepairMicrosoftStore
        ));
        assert!(!open_prerequisite_page(
            PrerequisiteRemedy::MoveToWritableDirectory
        ));
    }
}
