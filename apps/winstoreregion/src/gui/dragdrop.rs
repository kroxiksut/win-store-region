//! The OLE drop target that offers an installer file as a source.

use crate::gui::ids::{WM_APP_DROP_ENTER, WM_APP_DROP_FILE, WM_APP_DROP_LEAVE};
use crate::gui::state::FileSelectionError;
use std::path::PathBuf;
use windows::Win32::Foundation::{HWND, POINTL, WPARAM};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
    RegisterDragDrop, ReleaseStgMedium,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::core::{Ref, Result, implement};

#[allow(unsafe_code, clippy::inline_always, clippy::ref_as_ptr)]
#[implement(IDropTarget)]
struct NativeDropTarget {
    window: HWND,
    accepts_current_drag: std::cell::Cell<bool>,
}

impl NativeDropTarget {
    const fn new(window: HWND) -> Self {
        Self {
            window,
            accepts_current_drag: std::cell::Cell::new(false),
        }
    }
}

/// Make the window accept a dropped installer file.
///
/// The returned target must outlive the registration, so the caller keeps it
/// and revokes it before the window goes away. Without this call the target
/// below exists but Windows never consults it, which is what left dragging a
/// file onto the window doing nothing at all.
///
/// # Errors
///
/// Returns the Windows error when the window cannot be registered as a drop
/// target, which leaves the file picker as the only way to choose a file.
#[allow(unsafe_code)]
pub(super) fn register_drop_target(window: HWND) -> Result<IDropTarget> {
    let target: IDropTarget = NativeDropTarget::new(window).into();
    unsafe { RegisterDragDrop(window, &target) }?;
    Ok(target)
}

#[allow(unsafe_code)]
impl IDropTarget_Impl for NativeDropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _key_state: MODIFIERKEYS_FLAGS,
        _point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let accepts = data.as_ref().is_some_and(|data| {
            let format = hdrop_format();
            unsafe { data.QueryGetData(&raw const format) }.is_ok()
        });
        self.accepts_current_drag.set(accepts);
        unsafe { set_drop_effect(effect, accepts) };
        let _ = unsafe {
            SendMessageW(
                self.window,
                WM_APP_DROP_ENTER,
                Some(WPARAM(usize::from(accepts))),
                None,
            )
        };
        Ok(())
    }

    fn DragOver(
        &self,
        _key_state: MODIFIERKEYS_FLAGS,
        _point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        unsafe { set_drop_effect(effect, self.accepts_current_drag.get()) };
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        self.accepts_current_drag.set(false);
        let _ = unsafe { SendMessageW(self.window, WM_APP_DROP_LEAVE, None, None) };
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _key_state: MODIFIERKEYS_FLAGS,
        _point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let candidate = data
            .as_ref()
            .map_or(Err(FileSelectionError::EmptyPath), |data| unsafe {
                dropped_data_object(data)
            });
        let accepted = candidate.is_ok();
        unsafe { set_drop_effect(effect, accepted) };
        self.accepts_current_drag.set(false);
        let payload = Box::into_raw(Box::new(candidate));
        let _ = unsafe {
            SendMessageW(
                self.window,
                WM_APP_DROP_FILE,
                Some(WPARAM(payload as usize)),
                None,
            )
        };
        Ok(())
    }
}

#[allow(unsafe_code)]
unsafe fn set_drop_effect(effect: *mut DROPEFFECT, accepts: bool) {
    if let Some(effect) = unsafe { effect.as_mut() } {
        *effect = if accepts {
            DROPEFFECT_COPY
        } else {
            DROPEFFECT_NONE
        };
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: u32::try_from(TYMED_HGLOBAL.0).unwrap_or_default(),
        ..Default::default()
    }
}

#[allow(unsafe_code)]
unsafe fn dropped_file(drop: HDROP) -> std::result::Result<PathBuf, FileSelectionError> {
    if unsafe { DragQueryFileW(drop, u32::MAX, None) } != 1 {
        return Err(FileSelectionError::MultipleFiles);
    }
    let length = unsafe { DragQueryFileW(drop, 0, None) };
    if length == 0 {
        return Err(FileSelectionError::EmptyPath);
    }
    let mut file = vec![
        0_u16;
        usize::try_from(length)
            .unwrap_or_default()
            .saturating_add(1)
    ];
    let copied = unsafe { DragQueryFileW(drop, 0, Some(&mut file)) };
    if copied == 0 {
        return Err(FileSelectionError::EmptyPath);
    }
    Ok(PathBuf::from(String::from_utf16_lossy(
        &file[..usize::try_from(copied).unwrap_or_default()],
    )))
}

#[allow(unsafe_code)]
unsafe fn dropped_data_object(
    data: &IDataObject,
) -> std::result::Result<PathBuf, FileSelectionError> {
    let format = hdrop_format();
    let mut medium =
        unsafe { data.GetData(&raw const format) }.map_err(|_| FileSelectionError::EmptyPath)?;
    let drop = HDROP(unsafe { medium.u.hGlobal.0 });
    let candidate = unsafe { dropped_file(drop) };
    unsafe { ReleaseStgMedium(&raw mut medium) };
    candidate
}
