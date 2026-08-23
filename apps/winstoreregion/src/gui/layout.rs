//! DPI-aware placement of every control.
//!
//! The vertical rhythm is a flow rather than a table of fixed coordinates:
//! panels follow one another from the top, the command row is anchored to the
//! bottom of the client area, and the status box takes whatever is left between
//! them. That is what lets one layout hold together on a laptop screen and on a
//! large one, and what keeps a size change from having to be re-measured
//! control by control.

use crate::gui::ids::BASE_DPI_I32;
use crate::gui::state::WindowChrome;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Controls::LVM_SETCOLUMNWIDTH;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CB_GETITEMHEIGHT, GetClientRect, GetWindowRect, MoveWindow, SendMessageW,
};

/// Rows the region list shows before it scrolls.
///
/// The list holds every nation Windows knows unless a search narrowed it, so it
/// usually scrolls; this is only how much of it is visible at once.
const VISIBLE_REGION_ROWS: i32 = 12;

/// Logical size the window is designed for, and its smallest useful size.
///
/// Chosen so the whole window fits a 1366x768 screen at 125%: 1040x576 logical
/// is 1300x720 physical there, which clears the taskbar. Below this the two
/// columns start clipping, because this layout places content rather than
/// reflowing it.
pub(super) const DESIGN_WIDTH: i32 = 1_040;
pub(super) const DESIGN_HEIGHT: i32 = 576;

/// Outer margin of the window content.
const MARGIN: i32 = 20;

/// Inset from a panel's outline to the content inside it.
const PADDING: i32 = 16;

/// Space between the two content columns.
const COLUMN_GAP: i32 = 12;

/// Space between controls that belong together.
const COMPACT_GAP: i32 = 8;

/// Space between stacked panels.
///
/// Ten rather than twelve because four of these gaps stack up between the tabs
/// and the command row, and at 100% the status box needs the difference.
const PANEL_GAP: i32 = 10;

/// Heights of a label, a clickable row, and an edit field.
const LABEL_HEIGHT: i32 = 20;
const ROW_HEIGHT: i32 = 24;
const FIELD_HEIGHT: i32 = 26;

/// Height of the command buttons along the bottom.
const BUTTON_HEIGHT: i32 = 34;

/// Heights of the two panels of the installation tab.
const SOURCE_PANEL_HEIGHT: i32 = 164;
const REGION_PANEL_HEIGHT: i32 = 110;

/// Smallest status box worth showing: two lines of wrapped text.
const MIN_STATUS_HEIGHT: i32 = 40;

/// Height of the installation progress bar.
///
/// Its row is reserved whether or not the bar is shown, so the status box does
/// not change size when an installation starts and the text under the user's
/// eyes does not jump.
const PROGRESS_HEIGHT: i32 = 16;

/// Where the vertical flow puts everything between the tabs and the bottom.
///
/// Kept apart from the placement itself so the arithmetic can be checked at a
/// scaling nobody has a monitor for at hand. It proves rectangles, not
/// readability: whether a caption still fits inside its button at 150 % is a
/// question about fonts and only an eye can answer it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VerticalFlow {
    tabs_y: i32,
    tabs_height: i32,
    panel_top: i32,
    source_panel_height: i32,
    region_panel_y: i32,
    region_panel_height: i32,
    status_y: i32,
    status_height: i32,
    /// Top of the progress bar, between the status box and the command row.
    progress_y: i32,
    progress_height: i32,
    /// Top of the command row, which is anchored to the bottom of the client.
    commands_y: i32,
    button_height: i32,
}

/// Lay the vertical flow out for one client height and one scaling.
fn vertical_flow(client_height: i32, dpi: i32) -> VerticalFlow {
    let scale = |value: i32| value * dpi / BASE_DPI_I32;
    let margin = scale(MARGIN);
    let tabs_y = scale(70);
    let tabs_height = scale(30);
    let panel_top = tabs_y + tabs_height + scale(PANEL_GAP);
    let button_height = scale(BUTTON_HEIGHT);
    // The command row sits on the bottom edge and the status box fills what is
    // left above it, so both follow the window instead of a coordinate that
    // suited exactly one window height.
    let commands_y = (client_height - margin - button_height).max(panel_top);
    let source_panel_height = scale(SOURCE_PANEL_HEIGHT);
    let region_panel_height = scale(REGION_PANEL_HEIGHT);
    let region_panel_y = panel_top + source_panel_height + scale(PANEL_GAP);
    let status_y = region_panel_y + region_panel_height + scale(PANEL_GAP);
    let progress_height = scale(PROGRESS_HEIGHT);
    let progress_y = (commands_y - scale(PANEL_GAP) - progress_height).max(status_y);
    let status_height = (progress_y - scale(PANEL_GAP) - status_y).max(scale(MIN_STATUS_HEIGHT));
    VerticalFlow {
        tabs_y,
        tabs_height,
        panel_top,
        source_panel_height,
        region_panel_y,
        region_panel_height,
        status_y,
        status_height,
        progress_y,
        progress_height,
        commands_y,
        button_height,
    }
}

/// Left edge and width of every button on the command row, in tab order.
///
/// The first three follow one another from the left margin; the last is pinned
/// to the right edge, so the space between the two groups is what tells whether
/// the row still fits.
fn command_row(client_width: i32, dpi: i32) -> [(i32, i32); 4] {
    let scale = |value: i32| value * dpi / BASE_DPI_I32;
    let margin = scale(MARGIN);
    let compact_gap = scale(COMPACT_GAP);
    let install_width = scale(180);
    let restore_width = scale(300);
    let store_width = scale(200);
    let details_width = scale(170);
    [
        (margin, install_width),
        (margin + install_width + compact_gap, restore_width),
        (
            margin + install_width + restore_width + 2 * compact_gap,
            store_width,
        ),
        (
            (client_width - margin - details_width).max(margin),
            details_width,
        ),
    ]
}

#[allow(clippy::too_many_lines, unsafe_code)]
pub(super) unsafe fn layout_controls(window: HWND, chrome: &WindowChrome) {
    let Some(controls) = chrome.controls.as_ref() else {
        return;
    };
    let mut client = RECT::default();
    if unsafe { GetClientRect(window, &raw mut client) }.is_err() {
        return;
    }
    let dpi = i32::try_from(unsafe { GetDpiForWindow(window) }).unwrap_or(BASE_DPI_I32);
    // Same client area and same scaling produce the same rectangles, so there
    // is nothing to move and nothing to repaint.
    let geometry = (client.right - client.left, client.bottom - client.top, dpi);
    if chrome.last_layout.get() == Some(geometry) {
        return;
    }
    chrome.last_layout.set(Some(geometry));
    let scale = |value: i32| value * dpi / BASE_DPI_I32;
    let margin = scale(MARGIN);
    let width = (client.right - client.left - 2 * margin).max(scale(320));
    let gap = scale(COLUMN_GAP);
    let compact_gap = scale(COMPACT_GAP);
    // Panels are drawn at `margin`; their content starts one padding further
    // in. Placing content at `margin` too is what put every label and field on
    // top of the panel outline.
    let padding = scale(PADDING);
    let content_left = margin + padding;
    let content_width = (width - 2 * padding).max(scale(280));
    let column_width = ((content_width - gap) / 2).max(scale(150));
    let right_column = content_left + column_width + gap;
    let language_width = scale(180);
    let language_x = (client.right - margin - language_width).max(margin);
    let header_text_x = margin + scale(60);
    let header_text_width = (language_x - scale(72) - header_text_x).max(scale(220));

    // Header.
    let _ = unsafe {
        MoveWindow(
            controls.brand_badge,
            margin,
            scale(14),
            scale(44),
            scale(44),
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.title,
            header_text_x,
            scale(14),
            header_text_width,
            scale(24),
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.subtitle,
            header_text_x,
            scale(40),
            header_text_width,
            scale(LABEL_HEIGHT),
            true,
        )
    };
    // A label is sized for the longest language it has to hold, not for the one
    // it happens to start in: the English caption does not fit a width chosen
    // for the Russian one.
    let language_label_width = scale(96);
    let _ = unsafe {
        MoveWindow(
            controls.language_label,
            language_x - language_label_width - scale(8),
            scale(22),
            language_label_width,
            scale(LABEL_HEIGHT),
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.language,
            language_x,
            scale(18),
            language_width,
            scale(FIELD_HEIGHT),
            true,
        )
    };

    // Tabs, and the top of everything that follows them. The whole vertical
    // flow is one calculation so it can be checked at a DPI nobody has a screen
    // for handy.
    let flow = vertical_flow(client.bottom - client.top, dpi);
    let VerticalFlow {
        tabs_y,
        tabs_height,
        panel_top,
        source_panel_height,
        region_panel_y,
        region_panel_height,
        status_y,
        status_height,
        progress_y,
        progress_height,
        commands_y,
        button_height,
    } = flow;
    let _ = unsafe { MoveWindow(controls.tabs, margin, tabs_y, width, tabs_height, true) };

    // Journal tab: the table on top, its details filling down to the commands.
    // Both tabs use the same split, so switching between them moves nothing.
    let table_area = (commands_y - scale(PANEL_GAP) - panel_top).max(scale(200));
    let table_height = (table_area * 3 / 5).max(scale(120));
    let _ = unsafe {
        MoveWindow(
            controls.journal_list,
            margin,
            panel_top,
            width,
            table_height,
            true,
        )
    };
    for (column, share) in [(0, 22), (1, 30), (2, 22), (3, 26)] {
        let column_width = width * share / 100;
        let _ = unsafe {
            SendMessageW(
                controls.journal_list,
                LVM_SETCOLUMNWIDTH,
                Some(WPARAM(column)),
                Some(LPARAM(isize::try_from(column_width).unwrap_or(0))),
            )
        };
    }
    let journal_details_y = panel_top + table_height + compact_gap;
    let _ = unsafe {
        MoveWindow(
            controls.journal_details,
            margin,
            journal_details_y,
            width,
            (commands_y - scale(PANEL_GAP) - journal_details_y).max(scale(120)),
            true,
        )
    };
    // Updates tab: the same two rectangles the journal uses, so a tab switch
    // moves nothing on screen.
    // Updates tab: the same split as the journal.
    let _ = unsafe {
        MoveWindow(
            controls.updates_list,
            margin,
            panel_top,
            width,
            table_height,
            true,
        )
    };
    // Columns follow the width of the table, so nothing is cut off when the
    // window is resized: name widest, then the two versions, then the id.
    for (column, share) in [(0, 34), (1, 18), (2, 22), (3, 26)] {
        let column_width = width * share / 100;
        let _ = unsafe {
            SendMessageW(
                controls.updates_list,
                LVM_SETCOLUMNWIDTH,
                Some(WPARAM(column)),
                Some(LPARAM(isize::try_from(column_width).unwrap_or(0))),
            )
        };
    }
    let updates_details_y = panel_top + table_height + compact_gap;
    let _ = unsafe {
        MoveWindow(
            controls.updates_details,
            margin,
            updates_details_y,
            width,
            (commands_y - scale(PANEL_GAP) - updates_details_y).max(scale(80)),
            true,
        )
    };
    let updates_button_width = ((width - compact_gap) / 4).max(scale(140));
    for (index, control) in [controls.updates_refresh, controls.updates_open]
        .into_iter()
        .enumerate()
    {
        let offset = i32::try_from(index).unwrap_or(0) * (updates_button_width + compact_gap);
        let _ = unsafe {
            MoveWindow(
                control,
                margin + offset,
                commands_y,
                updates_button_width,
                button_height,
                true,
            )
        };
    }
    let journal_button_width = ((width - 4 * compact_gap) / 5).max(scale(110));
    for (index, control) in [
        controls.journal_open_store,
        controls.journal_repeat,
        controls.journal_copy_id,
        controls.journal_delete,
        controls.journal_clear,
    ]
    .into_iter()
    .enumerate()
    {
        let offset = i32::try_from(index).unwrap_or(0) * (journal_button_width + compact_gap);
        let _ = unsafe {
            MoveWindow(
                control,
                margin + offset,
                commands_y,
                journal_button_width,
                button_height,
                true,
            )
        };
    }
    let _ = unsafe {
        MoveWindow(
            controls.tab_content,
            margin,
            panel_top,
            width,
            (commands_y - scale(PANEL_GAP) - panel_top).max(scale(120)),
            true,
        )
    };

    // Installation tab: the source panel, with the card beside its fields.
    let _ = unsafe {
        MoveWindow(
            controls.source_panel,
            margin,
            panel_top,
            width,
            source_panel_height,
            true,
        )
    };
    let source_content_y = panel_top + padding;
    let _ = unsafe {
        MoveWindow(
            controls.source_title,
            content_left,
            source_content_y,
            column_width,
            scale(LABEL_HEIGHT),
            true,
        )
    };
    let hint_y = source_content_y + scale(LABEL_HEIGHT) + scale(4);
    let _ = unsafe {
        MoveWindow(
            controls.source_hint,
            content_left,
            hint_y,
            column_width,
            scale(32),
            true,
        )
    };
    // The two sources exclude each other, so they read as one choice side by
    // side, and the pair costs one row instead of two.
    let sources_y = hint_y + scale(32) + scale(4);
    let source_choice_width = ((column_width - compact_gap) / 2).max(scale(110));
    let _ = unsafe {
        MoveWindow(
            controls.source_link,
            content_left,
            sources_y,
            source_choice_width,
            scale(ROW_HEIGHT),
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.source_file,
            content_left + source_choice_width + compact_gap,
            sources_y,
            source_choice_width,
            scale(ROW_HEIGHT),
            true,
        )
    };
    let input_label_y = sources_y + scale(ROW_HEIGHT) + scale(4);
    let _ = unsafe {
        MoveWindow(
            controls.input_label,
            content_left,
            input_label_y,
            column_width,
            scale(LABEL_HEIGHT),
            true,
        )
    };
    let input_y = input_label_y + scale(LABEL_HEIGHT) + scale(2);
    let field_row_height = scale(FIELD_HEIGHT);
    let source_action_width = scale(120);
    let clear_width = scale(78);
    let _ = unsafe {
        MoveWindow(
            controls.input,
            content_left,
            input_y,
            (column_width - source_action_width - compact_gap).max(scale(120)),
            field_row_height,
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.file_path,
            content_left,
            input_y,
            (column_width - source_action_width - clear_width - 2 * compact_gap).max(scale(80)),
            field_row_height,
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.clear_file,
            content_left + column_width - source_action_width - clear_width - compact_gap,
            input_y,
            clear_width,
            field_row_height,
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.source_action,
            content_left + column_width - source_action_width,
            input_y,
            source_action_width,
            field_row_height,
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.app_card,
            right_column,
            source_content_y,
            column_width,
            source_panel_height - 2 * padding,
            true,
        )
    };

    // Region panel: the two regions, then the availability row beneath them.
    let _ = unsafe {
        MoveWindow(
            controls.region_panel,
            margin,
            region_panel_y,
            width,
            region_panel_height,
            true,
        )
    };
    let region_label_y = region_panel_y + scale(12);
    let region_field_y = region_label_y + scale(LABEL_HEIGHT) + scale(2);
    let _ = unsafe {
        MoveWindow(
            controls.current_region_label,
            content_left,
            region_label_y,
            column_width,
            scale(LABEL_HEIGHT),
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.temporary_region_label,
            right_column,
            region_label_y,
            column_width,
            scale(LABEL_HEIGHT),
            true,
        )
    };
    // A combobox keeps its own closed height and spends the height it is given
    // on the dropped list. Asking for one height therefore sized the list, not
    // the field: the list showed barely one region out of two hundred, and the
    // field ended up shorter than the box beside it. So the field is measured
    // after placement, the list is sized to a readable number of rows, and the
    // current-region box copies the measured height.
    let _ = unsafe {
        MoveWindow(
            controls.temporary_region,
            right_column,
            region_field_y,
            column_width,
            scale(52),
            true,
        )
    };
    let field_height =
        unsafe { window_height(controls.temporary_region) }.unwrap_or(field_row_height);
    let item_height = i32::try_from(
        unsafe { SendMessageW(controls.temporary_region, CB_GETITEMHEIGHT, None, None) }.0,
    )
    .unwrap_or(scale(18))
    .max(scale(12));
    let _ = unsafe {
        MoveWindow(
            controls.temporary_region,
            right_column,
            region_field_y,
            column_width,
            field_height + item_height * VISIBLE_REGION_ROWS + scale(4),
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.current_region,
            content_left,
            region_field_y,
            column_width,
            field_height,
            true,
        )
    };
    // The availability row sits in the space the region panel already had below
    // its two fields, so asking where an application is offered costs the window
    // no extra height.
    let availability_y = region_field_y + field_height + compact_gap;
    // The button carries a whole sentence, and a clipped one reads as a defect;
    // the second button needs only two words.
    let find_width = (column_width * 13 / 20).max(scale(180));
    let remaining_width = (column_width - find_width - compact_gap).max(scale(120));
    let _ = unsafe {
        MoveWindow(
            controls.find_region,
            content_left,
            availability_y,
            find_width,
            scale(FIELD_HEIGHT),
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.check_remaining,
            content_left + find_width + compact_gap,
            availability_y,
            remaining_width,
            scale(FIELD_HEIGHT),
            true,
        )
    };
    let show_all_width = (column_width * 2 / 5).max(scale(140));
    let _ = unsafe {
        MoveWindow(
            controls.show_all_regions,
            right_column,
            availability_y,
            show_all_width,
            scale(FIELD_HEIGHT),
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.availability_status,
            right_column + show_all_width + compact_gap,
            availability_y,
            (column_width - show_all_width - compact_gap).max(scale(120)),
            scale(FIELD_HEIGHT + 6),
            true,
        )
    };

    // Status, then the command row on the bottom edge.
    let _ = unsafe {
        MoveWindow(
            controls.status,
            margin,
            status_y,
            width,
            status_height,
            true,
        )
    };
    let _ = unsafe {
        MoveWindow(
            controls.progress,
            margin,
            progress_y,
            width,
            progress_height,
            true,
        )
    };
    let commands = command_row(client.right - client.left, dpi);
    for (control, (left, button_width)) in [
        controls.install,
        controls.restore,
        controls.open_store_page,
        controls.details,
    ]
    .into_iter()
    .zip(commands)
    {
        let _ = unsafe { MoveWindow(control, left, commands_y, button_width, button_height, true) };
    }
    let _ = unsafe {
        MoveWindow(
            controls.drop_overlay,
            margin,
            panel_top,
            width,
            source_panel_height,
            true,
        )
    };
    // The outlines are painted by the window around these rectangles, so a move
    // leaves the old ones behind until the client area is asked to repaint.
    let _ = unsafe { InvalidateRect(Some(window), None, true) };
}

/// Height of a control as Windows finally sized it.
///
/// A combobox is the reason this exists: it overrides the height it is given.
#[allow(unsafe_code)]
unsafe fn window_height(control: HWND) -> Option<i32> {
    let mut bounds = RECT::default();
    unsafe { GetWindowRect(control, &raw mut bounds) }.ok()?;
    Some(bounds.bottom - bounds.top)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Non-client width and height a plain overlapped window costs at 100 %.
    ///
    /// The client area is what the layout actually gets, and it is smaller than
    /// the size the window is created with. The exact values differ by theme, so
    /// these are the generous ones: a layout that survives them survives more.
    const NON_CLIENT_WIDTH: i32 = 16;
    const NON_CLIENT_HEIGHT: i32 = 39;

    /// The scalings a user is actually offered by Windows.
    const SCALINGS: [(i32, &str); 5] = [
        (96, "100%"),
        (120, "125%"),
        (144, "150%"),
        (168, "175%"),
        (192, "200%"),
    ];

    /// The client area the window has at its own minimum size for one scaling.
    fn client_at(dpi: i32) -> (i32, i32) {
        let scale = |value: i32| value * dpi / BASE_DPI_I32;
        (
            scale(DESIGN_WIDTH) - scale(NON_CLIENT_WIDTH),
            scale(DESIGN_HEIGHT) - scale(NON_CLIENT_HEIGHT),
        )
    }

    #[test]
    fn the_vertical_flow_keeps_a_usable_status_box_at_every_offered_scaling() {
        for (dpi, name) in SCALINGS {
            let (_, client_height) = client_at(dpi);
            let flow = vertical_flow(client_height, dpi);
            let scale = |value: i32| value * dpi / BASE_DPI_I32;

            assert!(
                flow.status_height >= scale(MIN_STATUS_HEIGHT),
                "{name}: the status box fell below two lines of text"
            );
            assert!(
                flow.status_y + flow.status_height + scale(PANEL_GAP) <= flow.commands_y,
                "{name}: the status box runs into the command row"
            );
            assert!(
                flow.commands_y + flow.button_height <= client_height,
                "{name}: the command row hangs off the bottom of the window"
            );
            assert!(
                flow.panel_top + flow.source_panel_height <= flow.region_panel_y,
                "{name}: the two panels overlap"
            );
            assert!(
                flow.region_panel_y + flow.region_panel_height <= flow.status_y,
                "{name}: the region panel runs into the status box"
            );
            // The progress row is reserved whether or not the bar is shown, so
            // starting an installation never moves the text under the reader.
            assert!(
                flow.status_y + flow.status_height <= flow.progress_y,
                "{name}: the status box runs into the progress bar"
            );
            assert!(
                flow.progress_y + flow.progress_height <= flow.commands_y,
                "{name}: the progress bar runs into the command row"
            );
            assert!(
                flow.progress_height > 0,
                "{name}: the progress bar has no height"
            );
        }
    }

    #[test]
    fn the_command_row_neither_overlaps_nor_leaves_the_window_at_any_scaling() {
        for (dpi, name) in SCALINGS {
            let (client_width, _) = client_at(dpi);
            let row = command_row(client_width, dpi);
            let margin = MARGIN * dpi / BASE_DPI_I32;

            assert!(
                row[0].0 >= margin,
                "{name}: the first button clips the edge"
            );
            for pair in row.windows(2) {
                let (left, width) = pair[0];
                let (next_left, _) = pair[1];
                assert!(
                    left + width <= next_left,
                    "{name}: two command buttons overlap"
                );
            }
            let (last_left, last_width) = row[3];
            assert!(
                last_left + last_width <= client_width - margin,
                "{name}: the details button hangs off the right edge"
            );
        }
    }

    #[test]
    fn a_window_squeezed_below_its_design_height_keeps_its_controls_apart() {
        // Windows can still hand a smaller client area than the minimum track
        // size asks for, on a screen shorter than the layout wants. Nothing may
        // end up on top of anything else when that happens.
        let flow = vertical_flow(200, BASE_DPI_I32);
        assert!(flow.commands_y >= flow.panel_top);
        assert_eq!(flow.status_height, MIN_STATUS_HEIGHT);
    }

    #[test]
    fn the_design_size_still_fits_the_screen_the_layout_was_chosen_for() {
        // 1366x768 at 125 % is the case that fixed the design size, and the
        // taskbar has to keep its room.
        let physical_width = DESIGN_WIDTH * 120 / BASE_DPI_I32;
        let physical_height = DESIGN_HEIGHT * 120 / BASE_DPI_I32;
        assert_eq!((physical_width, physical_height), (1300, 720));
        assert!(physical_height <= 768 - 40, "the taskbar must stay visible");

        // At 150 % the same window is 1560x864, which needs a 1080p screen.
        assert_eq!(
            (
                DESIGN_WIDTH * 144 / BASE_DPI_I32,
                DESIGN_HEIGHT * 144 / BASE_DPI_I32
            ),
            (1560, 864)
        );
    }
}
