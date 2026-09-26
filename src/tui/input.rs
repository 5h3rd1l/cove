use anyhow::Result;
use arboard::Clipboard;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::adapters::adapter_for;

use super::TuiExit;
use super::state::{AppState, Focus, PENDING_SEARCH_STATUS, PendingAction, YoloModal};
use super::text::{shell_join, shell_quote};

pub(super) fn handle_key(state: &mut AppState, key: KeyEvent) -> Result<Option<TuiExit>> {
    if state.show_help {
        if matches!(key.code, KeyCode::F(1) | KeyCode::Esc) {
            state.show_help = false;
        }
        return Ok(None);
    }
    if key.code == KeyCode::F(1) {
        state.show_help = true;
        return Ok(None);
    }
    if state.modal.is_some() {
        return handle_modal_key(state, key);
    }
    if state.edit.is_some() {
        handle_edit_key(state, key);
        return Ok(None);
    }
    if state.picker.is_some() {
        handle_picker_key(state, key);
        return Ok(None);
    }
    if state.confirm.is_some() {
        // Only an explicit y confirms; anything else (Esc, n, Enter) cancels.
        state.answer_confirm(matches!(key.code, KeyCode::Char('y' | 'Y')));
        return Ok(None);
    }

    match handle_focus_key(state, key) {
        Nav::Handled => return Ok(None),
        Nav::Quit => return Ok(Some(TuiExit::Quit)),
        Nav::Continue => {}
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('a'), KeyModifiers::CONTROL) => state.cursor = 0,
        (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
            state.cursor = state.cursor.saturating_sub(1);
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => state.delete(),
        (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
            state.cursor = state.query.chars().count();
        }
        (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
            state.cursor = (state.cursor + 1).min(state.query.chars().count());
        }
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => state.delete_to_start(),
        (KeyCode::Char('w'), KeyModifiers::CONTROL) => state.delete_previous_word(),
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(Some(TuiExit::Quit)),
        (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
            if let Some(exit) = begin_action(state, PendingAction::Copy)? {
                return Ok(Some(exit));
            }
        }
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => state.show_preview = !state.show_preview,
        (KeyCode::Esc, _) => return Ok(Some(TuiExit::Quit)),
        (KeyCode::Enter, _) => {
            if let Some(exit) = begin_action(state, PendingAction::Resume)? {
                return Ok(Some(exit));
            }
        }
        (KeyCode::Up, modifiers) if modifiers.contains(KeyModifiers::ALT) => state.cycle_scope(-1),
        (KeyCode::Down, modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            state.cycle_scope(1);
        }
        (KeyCode::F(9), _) => erase_for_good(state),
        (KeyCode::Delete, modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            erase_for_good(state);
        }
        (KeyCode::F(8), _) => state.toggle_delete_selected(),
        (KeyCode::Char('x'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            state.toggle_delete_selected();
        }
        (KeyCode::Char('u'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            state.restore_selected_workspace();
        }
        (KeyCode::F(2), _) => state.begin_chat_edit(),
        (KeyCode::F(3), _) => state.open_picker(),
        (KeyCode::F(4), _) => state.new_workspace(),
        (KeyCode::Char('m'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            state.open_picker();
        }
        (KeyCode::Char('n'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            state.new_workspace();
        }
        (KeyCode::Char('r'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            state.begin_current_workspace_edit();
        }
        (KeyCode::Char('d'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            let scope = state.ws.scope().to_string();
            if state.ws.name_of(&scope).is_some() {
                state.delete_workspace(&scope);
            } else {
                state.status = "open a workspace first (Alt+↑/↓), then delete it".to_string();
            }
        }
        (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
            state.show_sidebar = !state.show_sidebar;
            if !state.show_sidebar && state.focus == Focus::Sidebar {
                state.focus = Focus::Chats;
            }
        }
        (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::CONTROL) => state.move_selection(-1),
        (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => state.move_selection(1),
        (KeyCode::PageUp, _) => state.move_selection(-10),
        (KeyCode::PageDown, _) => state.move_selection(10),
        (KeyCode::Tab, _) => {
            if !state.accept_suggestion() {
                state.cycle_agent(false);
            }
        }
        (KeyCode::BackTab, _) => state.cycle_agent(true),
        (KeyCode::Backspace, _) => state.backspace(),
        (KeyCode::Delete, _) => state.delete(),
        (KeyCode::Left, _) => state.cursor = state.cursor.saturating_sub(1),
        (KeyCode::Right, _) => state.cursor = (state.cursor + 1).min(state.query.chars().count()),
        (KeyCode::Home, _) => state.cursor = 0,
        (KeyCode::End, _) => state.cursor = state.query.chars().count(),
        (KeyCode::Char('+'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            state.scroll_preview(-3);
        }
        (KeyCode::Char('-'), modifiers) if modifiers.contains(KeyModifiers::ALT) => {
            state.scroll_preview(3);
        }
        (KeyCode::Char(ch), KeyModifiers::NONE) | (KeyCode::Char(ch), KeyModifiers::SHIFT)
            if ch != '\n' && ch != '\r' =>
        {
            state.insert_char(ch);
        }
        _ => {}
    }

    Ok(None)
}

/// F9: delete permanently. Acts on the workspace list or on the chat,
/// depending on the focused pane, and always asks first.
fn erase_for_good(state: &mut AppState) {
    if state.focus == Focus::Sidebar {
        state.request_purge_open_workspace();
    } else {
        state.request_purge_selected_chat();
    }
}

enum Nav {
    /// The key did its job.
    Handled,
    Quit,
    /// Not a navigation key for the focused pane; the general handler runs.
    Continue,
}

/// Keys that depend on which pane has the keyboard. The search box keeps
/// `fr`'s editing keys; the chat list and the workspace list move with the
/// arrows, and typing in either still searches (the pane switches to Search).
fn handle_focus_key(state: &mut AppState, key: KeyEvent) -> Nav {
    let plain = key.modifiers == KeyModifiers::NONE;
    let typing = matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT);
    let ctrl = key.modifiers == KeyModifiers::CONTROL;
    let query_edit = ctrl
        && matches!(
            key.code,
            KeyCode::Char('a' | 'b' | 'd' | 'e' | 'f' | 'u' | 'w')
        );
    const FAR: isize = 1_000_000;

    match state.focus {
        Focus::Search => match key.code {
            KeyCode::Esc if plain => {
                state.focus = Focus::Chats;
                Nav::Handled
            }
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown if plain => {
                state.focus = Focus::Chats;
                Nav::Continue
            }
            _ => Nav::Continue,
        },
        Focus::Chats => match key.code {
            KeyCode::Char('/') if typing => {
                state.focus = Focus::Search;
                Nav::Handled
            }
            KeyCode::Left if plain => {
                state.focus_sidebar();
                Nav::Handled
            }
            KeyCode::Right if plain => Nav::Handled,
            KeyCode::Home if plain => {
                state.move_selection(-FAR);
                Nav::Handled
            }
            KeyCode::End if plain => {
                state.move_selection(FAR);
                Nav::Handled
            }
            KeyCode::Delete if plain => {
                state.delete_key_on_chat();
                Nav::Handled
            }
            KeyCode::Esc if plain => Nav::Quit,
            KeyCode::Backspace if plain => {
                state.focus = Focus::Search;
                Nav::Continue
            }
            KeyCode::Char(_) if typing || query_edit => {
                state.focus = Focus::Search;
                Nav::Continue
            }
            _ => Nav::Continue,
        },
        Focus::Sidebar => match key.code {
            KeyCode::Up if plain => {
                state.step_scope(-1, None);
                Nav::Handled
            }
            KeyCode::Down if plain => {
                state.step_scope(1, None);
                Nav::Handled
            }
            KeyCode::Char('k') if ctrl => {
                state.step_scope(-1, None);
                Nav::Handled
            }
            KeyCode::Char('j') if ctrl => {
                state.step_scope(1, None);
                Nav::Handled
            }
            KeyCode::PageUp | KeyCode::Home if plain => {
                state.step_scope(0, Some(false));
                Nav::Handled
            }
            KeyCode::PageDown | KeyCode::End if plain => {
                state.step_scope(0, Some(true));
                Nav::Handled
            }
            KeyCode::Right | KeyCode::Enter if plain => {
                state.focus = Focus::Chats;
                Nav::Handled
            }
            KeyCode::Left if plain => Nav::Handled,
            KeyCode::Char('/') if typing => {
                state.focus = Focus::Search;
                Nav::Handled
            }
            KeyCode::Delete if plain => {
                state.delete_key_on_workspace();
                Nav::Handled
            }
            KeyCode::F(8) if plain => {
                state.delete_or_restore_open_workspace();
                Nav::Handled
            }
            KeyCode::F(2) if plain => {
                state.begin_current_workspace_edit();
                Nav::Handled
            }
            KeyCode::Esc if plain => Nav::Quit,
            KeyCode::Backspace if plain => {
                state.focus = Focus::Search;
                Nav::Continue
            }
            KeyCode::Char(_) if typing || query_edit => {
                state.focus = Focus::Search;
                Nav::Continue
            }
            _ => Nav::Continue,
        },
    }
}

fn begin_action(state: &mut AppState, action: PendingAction) -> Result<Option<TuiExit>> {
    if state.search_pending() {
        state.status = PENDING_SEARCH_STATUS.to_string();
        return Ok(None);
    }
    let Some(session) = state.selected_session().cloned() else {
        return Ok(None);
    };

    let supports_yolo = adapter_for(&session.agent)
        .as_ref()
        .is_some_and(|adapter| adapter.supports_yolo());
    if state.yolo || session.yolo || !supports_yolo {
        return finish_action(state, action, state.yolo || session.yolo, session);
    }

    state.modal = Some(YoloModal {
        action,
        session,
        selected: false,
    });
    Ok(None)
}

fn handle_modal_key(state: &mut AppState, key: KeyEvent) -> Result<Option<TuiExit>> {
    let Some(modal) = state.modal.as_mut() else {
        return Ok(None);
    };

    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => state.modal = None,
        (KeyCode::Left, _) => modal.selected = false,
        (KeyCode::Right, _) => modal.selected = true,
        (KeyCode::Tab | KeyCode::BackTab, _) => {
            modal.selected = !modal.selected;
        }
        (KeyCode::Char('y') | KeyCode::Char('Y'), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            let action = modal.action;
            let session = modal.session.clone();
            state.modal = None;
            return finish_action(state, action, true, session);
        }
        (KeyCode::Char('n') | KeyCode::Char('N'), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            let action = modal.action;
            let session = modal.session.clone();
            state.modal = None;
            return finish_action(state, action, false, session);
        }
        (KeyCode::Enter, _) => {
            let yolo = modal.selected;
            let action = modal.action;
            let session = modal.session.clone();
            state.modal = None;
            return finish_action(state, action, yolo, session);
        }
        _ => {}
    }

    Ok(None)
}

fn finish_action(
    state: &mut AppState,
    action: PendingAction,
    yolo: bool,
    session: crate::model::Session,
) -> Result<Option<TuiExit>> {
    let Some(adapter) = adapter_for(&session.agent) else {
        state.status = "No resume command available for selected session".to_string();
        return Ok(None);
    };
    let command = adapter.resume_command(&session, yolo);
    match action {
        PendingAction::Resume => Ok(Some(TuiExit::Resume {
            command,
            directory: session.directory,
        })),
        PendingAction::Copy => {
            let command = shell_join(&command);
            let full = if session.directory.is_empty() {
                command
            } else {
                format!("cd {} && {}", shell_quote(&session.directory), command)
            };
            match Clipboard::new().and_then(|mut clipboard| clipboard.set_text(full.clone())) {
                Ok(()) => state.status = format!("copied: {full}"),
                Err(_) => state.status = format!("clipboard unavailable: {full}"),
            }
            Ok(None)
        }
    }
}

fn handle_edit_key(state: &mut AppState, key: KeyEvent) {
    let Some(edit) = state.edit.as_mut() else {
        return;
    };
    let line = &mut edit.line;
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => state.edit = None,
        (KeyCode::Enter, _) => state.commit_edit(),
        (KeyCode::Left, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => line.left(),
        (KeyCode::Right, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => line.right(),
        (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => line.home(),
        (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => line.end(),
        (KeyCode::Backspace, _) => line.backspace(),
        (KeyCode::Delete, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => line.delete(),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => line.delete_to_start(),
        (KeyCode::Char('w'), KeyModifiers::CONTROL) => line.delete_word(),
        (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => line.insert(ch),
        _ => {}
    }
}

fn handle_picker_key(state: &mut AppState, key: KeyEvent) {
    let Some(picker) = state.picker.as_mut() else {
        return;
    };
    let last = picker.items.len().saturating_sub(1);
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => state.picker = None,
        (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
            picker.selected = picker.selected.saturating_sub(1);
        }
        (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
            picker.selected = (picker.selected + 1).min(last);
        }
        (KeyCode::Enter, _) => {
            let session_id = picker.session_id.clone();
            let target = picker.items[picker.selected].0.clone();
            state.picker = None;
            state.move_session_to(&session_id, &target);
        }
        _ => {}
    }
}
