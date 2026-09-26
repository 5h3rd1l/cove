//! Clicks, double-clicks and drag-and-drop. The wheel is handled in `tui.rs`.

use std::time::{Duration, Instant};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::layout::{self, contains};
use super::render::{confirm_buttons, picker_rect, result_columns, results_rows_area};
use super::state::{AppState, Drag, Focus, RowKind};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

pub(super) fn handle_pointer(state: &mut AppState, mouse: MouseEvent, area: Rect) -> bool {
    let (column, row) = (mouse.column, mouse.row);
    let pressed = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));

    if state.picker.is_some() {
        return pressed && click_picker(state, area, column, row);
    }
    if state.confirm.is_some() {
        return pressed && click_confirm(state, area, column, row);
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => press(state, area, column, row),
        MouseEventKind::Drag(MouseButton::Left) => drag(state, column, row),
        MouseEventKind::Up(MouseButton::Left) => release(state, area, column, row),
        _ => false,
    }
}

fn click_picker(state: &mut AppState, area: Rect, column: u16, row: u16) -> bool {
    let Some(picker) = state.picker.take() else {
        return false;
    };
    let rect = picker_rect(area, picker.items.len());
    let index = row.checked_sub(rect.y + 1).map(usize::from);
    match index {
        Some(i) if contains(rect, column, row) && i < picker.items.len() => {
            state.move_session_to(&picker.session_id, &picker.items[i].0);
        }
        // Clicks on the border, padding or outside close the dialog.
        _ => {}
    }
    true
}

fn click_confirm(state: &mut AppState, area: Rect, column: u16, row: u16) -> bool {
    let (_, _, erase) = confirm_buttons(area);
    // Only the red button confirms; any other click cancels.
    state.answer_confirm(contains(erase, column, row));
    true
}

fn press(state: &mut AppState, area: Rect, column: u16, row: u16) -> bool {
    let now = Instant::now();
    let double = matches!(
        state.last_click,
        Some((at, c, r)) if now.duration_since(at) <= DOUBLE_CLICK && c == column && r == row
    );
    state.last_click = if double {
        None
    } else {
        Some((now, column, row))
    };
    // A click anywhere else finishes a rename.
    state.commit_edit();
    state.drag = None;

    let layout = layout::app(area, state.show_preview, state.show_sidebar);

    if contains(layout.search, column, row) {
        state.focus = Focus::Search;
        return true;
    }

    if let Some(sidebar) = layout.sidebar
        && let Some(index) = layout::sidebar_row_at(sidebar, column, row)
    {
        state.focus = Focus::Sidebar;
        press_sidebar(state, sidebar, index, column, double);
        return true;
    }

    let rows_area = results_rows_area(layout.main.results());
    if contains(rows_area, column, row) && !state.visible.is_empty() {
        let (top, _) = state.results_window(rows_area.height as usize);
        let index = top + (row - rows_area.y) as usize;
        if index < state.visible.len() {
            // The ✕ / ↺ at the row's right edge only shows on the selected row.
            state.focus = Focus::Chats;
            let was_selected = index == state.selected;
            let right = rows_area.right();
            let deleted_view = state.in_deleted_view();
            // Live view: ✕ deletes. Deleted view: ↺ restores, red ✕ erases.
            let on_glyph = was_selected && !deleted_view && column + 2 >= right;
            let on_restore =
                was_selected && deleted_view && column + 5 >= right && column + 3 < right;
            let on_erase = was_selected && deleted_view && column + 2 >= right;
            state.selected = index;
            state.preview_scroll = 0;
            let columns = result_columns(rows_area.width, state.shows_workspace_column());
            let title_start = rows_area.x + columns.title_x;
            let on_title = column >= title_start && column < title_start + columns.title_w;
            if on_glyph || on_restore {
                state.toggle_delete_selected();
            } else if on_erase {
                state.request_purge_selected_chat();
            } else if double && on_title {
                state.begin_chat_edit();
            } else {
                let session = &state.visible[index];
                state.drag = Some(Drag {
                    session_id: session.id.clone(),
                    title: state.title_of(session).to_string(),
                    start: (column, row),
                    pos: (column, row),
                    active: false,
                });
            }
            return true;
        }
    }
    false
}

fn press_sidebar(state: &mut AppState, sidebar: Rect, index: usize, column: u16, double: bool) {
    let rows = state.sidebar_rows();
    let Some(row) = rows.get(index) else {
        return;
    };
    match row.kind {
        RowKind::New => state.new_workspace(),
        RowKind::DeletedWorkspace
            if layout::sidebar_restore_column(sidebar, column) && row.scope == state.ws.scope() =>
        {
            state.restore_workspace(&row.scope);
        }
        RowKind::DeletedWorkspace
            if layout::sidebar_purge_column(sidebar, column) && row.scope == state.ws.scope() =>
        {
            state.request_purge_open_workspace();
        }
        RowKind::Workspace
            if layout::sidebar_delete_column(sidebar, column) && row.scope == state.ws.scope() =>
        {
            state.delete_workspace(&row.scope);
        }
        RowKind::Workspace if double => {
            state.set_scope(&row.scope);
            state.begin_workspace_edit(&row.scope, None);
        }
        _ => state.set_scope(&row.scope),
    }
}

fn drag(state: &mut AppState, column: u16, row: u16) -> bool {
    let Some(drag) = state.drag.as_mut() else {
        return false;
    };
    drag.pos = (column, row);
    if (column, row) != drag.start {
        drag.active = true;
    }
    true
}

fn release(state: &mut AppState, area: Rect, column: u16, row: u16) -> bool {
    let Some(drag) = state.drag.take() else {
        return false;
    };
    if drag.active {
        let layout = layout::app(area, state.show_preview, state.show_sidebar);
        if let Some(sidebar) = layout.sidebar
            && let Some(index) = layout::sidebar_row_at(sidebar, column, row)
            && let Some(target) = state.sidebar_rows().get(index)
            && target.droppable()
        {
            state.move_session_to(&drag.session_id, &target.scope);
        }
    }
    true
}
