//! Which way the interface reads, and what the window does about it.
//!
//! Arabic and Hebrew are not only a translation. The window itself has to read
//! from the right: the badge belongs in the top-right corner, the command row
//! starts there, a checkbox puts its box on the right of its caption, and a
//! scrollbar moves to the left edge. Windows can lay a window out that way for
//! anyone who asks, and `WS_EX_LAYOUTRTL` is the ask — it turns the client
//! coordinate system around, so every placement in `layout.rs` is measured from
//! the right edge instead of the left without one line of its arithmetic
//! changing. Mirroring by hand would have meant rewriting all of it and then
//! still owing the scrollbars, the drop-down arrows and the table columns.
//!
//! Two things deliberately keep reading left to right inside the mirrored
//! window. The application badge, because a mirrored logo is a defect in every
//! language; and the two fields that hold a Store address, a Product ID or a
//! file path, because their contents are ASCII by contract and a right-aligned
//! URL puts its slashes and colons where nobody looks for them.

use crate::gui::controls::grouping_panels;
use crate::gui::ids::WindowLong;
use crate::gui::layout::layout_controls;
use crate::gui::state::WindowChrome;
use crate::gui::strings::{Language, TextDirection};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOW, FillRect, GetDC, GetSysColorBrush, RDW_ALLCHILDREN, RDW_ERASE, RDW_ERASENOW,
    RDW_FRAME, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow, ReleaseDC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DrawMenuBar, GW_CHILD, GW_HWNDNEXT, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetClientRect,
    GetWindow, GetWindowLongPtrW, MB_RIGHT, MB_RTLREADING, MESSAGEBOX_STYLE, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos,
    WS_EX_LAYOUTRTL, WS_EX_LEFTSCROLLBAR, WS_EX_RIGHT, WS_EX_RTLREADING,
};
use windows::Win32::UI::WindowsAndMessaging::{ES_CENTER, ES_RIGHT};

/// Lay the window out the way the chosen language reads.
///
/// Called from a render, and it costs nothing on the renders that do not need
/// it: a forty-market search renders the window forty times and the direction
/// changes on none of them. Only a language change with a different direction
/// gets past the first line, and then everything has to move, so the layout
/// cache is dropped rather than trusted.
#[allow(unsafe_code)]
pub(super) unsafe fn apply_interface_direction(
    window: HWND,
    chrome: &WindowChrome,
    language: Language,
) {
    let direction = language.direction();
    if chrome.layout_direction.get() == Some(direction) {
        return;
    }
    chrome.layout_direction.set(Some(direction));
    let mirrored = direction == TextDirection::RightToLeft;
    // The window turns around first, and everything moves before any control is
    // told which way it now reads. The order is the whole trick. A control given
    // its new direction while still standing in its old place repaints there and
    // then, putting its scrollbar on the new side of the old rectangle; the
    // layout then moves the control away and that scrollbar stays behind. It
    // stays for good, because it lands inside one of the grouping panels, and
    // those paint nothing at all — they exist to be transparent — so the window
    // is clipped out of exactly the area that would have to be repainted to
    // clear it. Moving first means every such repaint happens where the control
    // actually is.
    unsafe { set_mirrored(window, mirrored) };
    chrome.last_layout.set(None);
    unsafe { layout_controls(window, chrome) };
    // Children are given their direction one by one rather than left to inherit
    // it: inheritance happens once, when a child is created, and by the time a
    // language is chosen every child already exists.
    for child in unsafe { descendants_of(window) } {
        unsafe { set_mirrored(child, mirrored && !reads_left_to_right(chrome, child)) };
    }
    let _ = unsafe { DrawMenuBar(window) };
    unsafe { wipe_grouping_panels(chrome) };
    // The outlines this window paints around its own panels are part of the
    // client area, not of any child, so a child-only repaint would leave them
    // on the side the window used to read from.
    let _ = unsafe {
        RedrawWindow(
            Some(window),
            None,
            None,
            RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN | RDW_ERASENOW | RDW_UPDATENOW,
        )
    };
}

/// The extra message-box flags the chosen language needs.
///
/// A message box is a window this application does not create, so the only say
/// it has over it is at the call. Without these two flags every dialog in an
/// Arabic interface would come up reading the other way from the window that
/// raised it.
pub(super) fn message_box_direction(language: Language) -> MESSAGEBOX_STYLE {
    match language.direction() {
        TextDirection::LeftToRight => MESSAGEBOX_STYLE::default(),
        TextDirection::RightToLeft => MB_RTLREADING | MB_RIGHT,
    }
}

/// Clear the two panels that never clear themselves.
///
/// The grouping panels paint nothing, by design, and the window is clipped out
/// of them because they are children of it. Between the two, a pixel drawn
/// inside a panel and then vacated has nobody to erase it and stays for the
/// life of the window. Turning the interface around vacates plenty: the
/// application card crosses the panel and leaves its scrollbar behind on the
/// far side. So the panels are wiped by hand, once, and the repaint that
/// follows puts back the outlines and the controls that belong there.
#[allow(unsafe_code)]
unsafe fn wipe_grouping_panels(chrome: &WindowChrome) {
    let Some(controls) = chrome.controls.as_ref() else {
        return;
    };
    for panel in grouping_panels(controls) {
        let mut area = RECT::default();
        if unsafe { GetClientRect(panel, &raw mut area) }.is_err() {
            continue;
        }
        let device_context = unsafe { GetDC(Some(panel)) };
        if device_context.is_invalid() {
            continue;
        }
        let _ = unsafe {
            FillRect(
                device_context,
                &raw const area,
                GetSysColorBrush(COLOR_WINDOW),
            )
        };
        let _ = unsafe { ReleaseDC(Some(panel), device_context) };
    }
}

/// Children whose own contents keep reading left to right in a mirrored window.
#[allow(unsafe_code)]
fn reads_left_to_right(chrome: &WindowChrome, child: HWND) -> bool {
    chrome.controls.as_ref().is_some_and(|controls| {
        child == controls.brand_badge || child == controls.input || child == controls.file_path
    })
}

/// Every window under this one, its own children's children included.
///
/// The headings of a table are a window of their own, owned by the table, and
/// they are the reason this does not stop at the first level. A control created
/// inside a window that already reads right to left inherits that; a control
/// that is told afterwards does not pass anything on, because inheritance
/// happens once. The Journal table proved it: its columns turned around and its
/// four headings stayed where they were, so every heading named the column on
/// the far side of the table from it.
#[allow(unsafe_code)]
unsafe fn descendants_of(window: HWND) -> Vec<HWND> {
    let mut found = Vec::new();
    let mut pending = vec![window];
    while let Some(parent) = pending.pop() {
        let mut child = unsafe { GetWindow(parent, GW_CHILD) };
        while let Ok(handle) = child {
            if handle.is_invalid() {
                break;
            }
            found.push(handle);
            pending.push(handle);
            child = unsafe { GetWindow(handle, GW_HWNDNEXT) };
        }
    }
    found
}

/// Give one window the layout its contents should read in.
///
/// The frame change is not optional and it is not tidiness. A window caches
/// what its own borders and scrollbars cost it, and writing an extended style
/// does not disturb that cache: without `SWP_FRAMECHANGED` a mirrored box goes
/// on reserving space for the scrollbar it used to have on the right while
/// drawing the new one on the left, and every framed box in the window ends up
/// with two of them.
#[allow(unsafe_code)]
unsafe fn set_mirrored(handle: HWND, mirrored: bool) {
    let extended = unsafe { GetWindowLongPtrW(handle, GWL_EXSTYLE) };
    let extended_now = mirrored_ex_style(extended, mirrored);
    let ordinary = unsafe { GetWindowLongPtrW(handle, GWL_STYLE) };
    let ordinary_now = if unsafe { is_text_box(handle) } {
        aligned_style(ordinary, mirrored)
    } else {
        ordinary
    };
    if extended_now == extended && ordinary_now == ordinary {
        return;
    }
    if extended_now != extended {
        unsafe { SetWindowLongPtrW(handle, GWL_EXSTYLE, extended_now) };
    }
    if ordinary_now != ordinary {
        unsafe { SetWindowLongPtrW(handle, GWL_STYLE, ordinary_now) };
    }
    let _ = unsafe {
        SetWindowPos(
            handle,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

/// Whether this control is a text box, and therefore aligns its own text.
#[allow(unsafe_code)]
unsafe fn is_text_box(handle: HWND) -> bool {
    let mut class = [0_u16; 16];
    let written = usize::try_from(unsafe { GetClassNameW(handle, &mut class) }).unwrap_or(0);
    String::from_utf16_lossy(&class[..written]).eq_ignore_ascii_case("Edit")
}

/// One extended style laid out the way this direction needs it.
///
/// Asking for a mirrored window is one bit; taking the request back is four.
/// A text box does not keep `WS_EX_LAYOUTRTL`: it reads the request, spends it
/// on the three styles that say what it actually means for a box of text —
/// right-aligned, right-to-left reading, scrollbar on the left — and the bit
/// that was asked for is gone. Clearing only that bit therefore clears nothing,
/// and the box stays mirrored in a language that reads the other way. That is
/// exactly what it did: Russian chosen after Arabic kept its scrollbars on the
/// left. So the whole set goes on and the whole set comes off.
fn mirrored_ex_style(current: WindowLong, mirrored: bool) -> WindowLong {
    let requested = WindowLong::try_from(WS_EX_LAYOUTRTL.0).unwrap_or(0);
    let spent = [
        WS_EX_LAYOUTRTL,
        WS_EX_RIGHT,
        WS_EX_RTLREADING,
        WS_EX_LEFTSCROLLBAR,
    ]
    .into_iter()
    .fold(0, |mask, style| {
        mask | WindowLong::try_from(style.0).unwrap_or(0)
    });
    if mirrored {
        current | requested
    } else {
        current & !spent
    }
}

/// One text box's ordinary style, aligned the way this direction reads.
///
/// The extended styles are not enough for a text box, and finding that out cost
/// a round trip through Arabic and back. A box asked to mirror does not merely
/// record the request: it writes `ES_RIGHT` into its ordinary style, where no
/// amount of clearing extended styles will ever reach it. The window came back
/// to Russian with its layout the right way round and every sentence in the
/// card and the status box still pushed against the right edge.
fn aligned_style(current: WindowLong, mirrored: bool) -> WindowLong {
    let alignment = WindowLong::try_from(ES_CENTER | ES_RIGHT).unwrap_or(0);
    let right = WindowLong::try_from(ES_RIGHT).unwrap_or(0);
    // Left is the absence of the other two, which is how these boxes were made.
    if mirrored {
        (current & !alignment) | right
    } else {
        current & !alignment
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{ES_MULTILINE, ES_READONLY, WS_EX_CLIENTEDGE};

    #[test]
    fn mirroring_leaves_every_style_that_is_not_about_direction_alone() {
        let border = WindowLong::try_from(WS_EX_CLIENTEDGE.0).unwrap_or(0);
        let bit = WindowLong::try_from(WS_EX_LAYOUTRTL.0).unwrap_or(0);
        assert_eq!(mirrored_ex_style(border, true), border | bit);
        assert_eq!(mirrored_ex_style(border | bit, false), border);
        // Applying the same direction twice must not keep rewriting the style:
        // that is what lets a render leave an unchanged window alone.
        assert_eq!(mirrored_ex_style(border | bit, true), border | bit);
        assert_eq!(mirrored_ex_style(border, false), border);
    }

    #[test]
    fn a_text_box_is_aligned_by_its_ordinary_style_and_put_back_the_same_way() {
        let made = WindowLong::try_from(ES_MULTILINE | ES_READONLY).unwrap_or(0);
        let right = WindowLong::try_from(ES_RIGHT).unwrap_or(0);
        assert_eq!(aligned_style(made, true), made | right);
        // The round trip is the whole point: what a box was made with is what it
        // gets back, not whatever the mirrored state left in it.
        assert_eq!(aligned_style(made | right, false), made);
        assert_eq!(aligned_style(made, false), made);
    }

    #[test]
    fn going_back_takes_away_what_a_text_box_spent_the_request_on() {
        // What a text box is actually holding after it has been mirrored: the
        // bit that was asked for is gone, and three others stand in its place.
        let spent = WindowLong::try_from((WS_EX_RIGHT | WS_EX_RTLREADING | WS_EX_LEFTSCROLLBAR).0)
            .unwrap_or(0);
        assert_eq!(mirrored_ex_style(spent, false), 0);
        let border = WindowLong::try_from(WS_EX_CLIENTEDGE.0).unwrap_or(0);
        assert_eq!(mirrored_ex_style(border | spent, false), border);
    }

    #[test]
    fn a_dialog_reads_the_way_the_language_it_is_written_in_does() {
        assert_eq!(
            message_box_direction(Language::English),
            MESSAGEBOX_STYLE::default()
        );
        assert_eq!(
            message_box_direction(Language::Arabic),
            MB_RTLREADING | MB_RIGHT
        );
    }
}
