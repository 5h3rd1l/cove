use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use crate::adapters::{KnownSessions, adapter_for};
use crate::config::{AGENT_ORDER, is_agent};
use crate::model::Session;
use crate::query::{Filter, parse_query};
use crate::search::SearchEngine;

use super::images::AgentImages;
use super::preview::render_preview_lines;
use super::text::char_to_byte_idx;
use super::theme::Theme;
use super::workspaces::{ALL, LineEdit, NONE, TRASH, Workspaces};

const DATE_SUGGESTIONS: [&str; 4] = ["today", "yesterday", "week", "month"];

pub(super) const PENDING_SEARCH_STATUS: &str = "searching; press again when results update";

pub(super) enum ScanMessage {
    Progress {
        elapsed: Duration,
        new_or_modified: usize,
        deleted: usize,
        total: usize,
    },
    Finished {
        elapsed: Duration,
        new_or_modified: usize,
        deleted: usize,
        total: usize,
    },
    Failed {
        elapsed: Duration,
        error: String,
    },
}

pub(super) struct SearchRequest {
    pub(super) generation: u64,
    pub(super) query: String,
    pub(super) agent_filter: Option<String>,
    pub(super) directory_filter: Option<String>,
    pub(super) preserve_selection: Option<(String, String)>,
    pub(super) reload_index: bool,
    pub(super) limit: usize,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum PendingAction {
    Resume,
    Copy,
}

#[derive(Debug, Clone)]
pub(super) struct YoloModal {
    pub(super) action: PendingAction,
    pub(super) session: Session,
    pub(super) selected: bool,
}

/// A destructive step waiting for a yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConfirmAction {
    PurgeChat {
        id: String,
        title: String,
    },
    PurgeWorkspace {
        id: String,
        name: String,
        chats: usize,
    },
}

/// Which pane the keyboard is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Search,
    Chats,
    Sidebar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EditTarget {
    Chat { id: String, original: String },
    Workspace(String),
}

#[derive(Debug, Clone)]
pub(super) struct Edit {
    pub(super) target: EditTarget,
    pub(super) line: LineEdit,
}

/// "Move chat to workspace" dialog.
#[derive(Debug, Clone)]
pub(super) struct Picker {
    pub(super) session_id: String,
    /// (workspace id or `NONE`, label)
    pub(super) items: Vec<(String, String)>,
    pub(super) selected: usize,
}

/// A scratch chat (`Ctrl+I`): a brand-new, throwaway session with an agent.
/// `known_before` is the (agent, id) pairs already indexed at request time,
/// so whatever new session the agent writes while it runs can be found and
/// hidden again the moment it exits.
pub(super) struct ScratchRequest {
    pub(super) command: Vec<String>,
    pub(super) directory: String,
    pub(super) agent: &'static str,
    known_before: KnownSessions,
}

/// A chat being dragged with the mouse.
#[derive(Debug, Clone)]
pub(super) struct Drag {
    pub(super) session_id: String,
    pub(super) title: String,
    pub(super) start: (u16, u16),
    pub(super) pos: (u16, u16),
    pub(super) active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RowKind {
    All,
    Workspace,
    Unsorted,
    /// The Deleted view.
    Deleted,
    /// A deleted workspace, listed under Deleted so it can be restored.
    DeletedWorkspace,
    New,
}

#[derive(Debug, Clone)]
pub(super) struct SidebarRow {
    pub(super) kind: RowKind,
    /// Scope id: `ALL`, `NONE` or a workspace id (empty for `New`).
    pub(super) scope: String,
    pub(super) name: String,
    pub(super) count: usize,
    /// Color slot, for workspace rows.
    pub(super) color: Option<usize>,
}

impl SidebarRow {
    pub(super) fn droppable(&self) -> bool {
        matches!(
            self.kind,
            RowKind::Workspace | RowKind::Unsorted | RowKind::Deleted
        )
    }
}

struct PreviewCache {
    session_id: String,
    mtime: f64,
    query: String,
    lines: Vec<ratatui::text::Line<'static>>,
}

pub(super) struct AppState {
    pub(super) engine: SearchEngine,
    preview_cache: RefCell<Option<PreviewCache>>,
    pub(super) visible: Vec<Session>,
    pub(super) query: String,
    pub(super) cursor: usize,
    pub(super) selected: usize,
    pub(super) preview_scroll: u16,
    pub(super) agent_filter: Option<String>,
    pub(super) directory_filter: Option<String>,
    pub(super) yolo: bool,
    pub(super) scanning: bool,
    pub(super) status: String,
    pub(super) refresh_status: String,
    pub(super) last_search_ms: f64,
    pub(super) show_preview: bool,
    pub(super) show_help: bool,
    pub(super) modal: Option<YoloModal>,
    pub(super) images: Option<AgentImages>,
    pub(super) theme: Theme,
    pub(super) ws: Workspaces,
    live_ids: HashSet<String>,
    counts: BTreeMap<String, usize>,
    pub(super) show_sidebar: bool,
    results_top: Cell<usize>,
    pub(super) focus: Focus,
    sidebar_on_screen: Cell<bool>,
    pub(super) edit: Option<Edit>,
    pub(super) picker: Option<Picker>,
    pub(super) confirm: Option<ConfirmAction>,
    scratch_requested: Option<ScratchRequest>,
    pub(super) drag: Option<Drag>,
    pub(super) last_click: Option<(std::time::Instant, u16, u16)>,
    search_generation: u64,
    applied_search_generation: u64,
    search_requested: bool,
    search_preserve_selection: Option<(String, String)>,
    search_reload_requested: bool,
}

impl AppState {
    /// Rendering the preview lowercases and highlights the full session
    /// content, so the result is cached per (session, mtime, query) instead
    /// of being recomputed on every frame while the user types.
    pub(super) fn preview_lines(&self, session: &Session) -> Vec<ratatui::text::Line<'static>> {
        let mut cache = self.preview_cache.borrow_mut();
        if let Some(cached) = cache.as_ref()
            && cached.session_id == session.id
            && cached.mtime == session.mtime
            && cached.query == self.query
        {
            return cached.lines.clone();
        }
        let lines = render_preview_lines(session, &self.query, &self.theme);
        *cache = Some(PreviewCache {
            session_id: session.id.clone(),
            mtime: session.mtime,
            query: self.query.clone(),
            lines: lines.clone(),
        });
        lines
    }

    pub(super) fn new(
        query: String,
        agent_filter: Option<String>,
        directory_filter: Option<String>,
        yolo: bool,
        engine: SearchEngine,
        images: Option<AgentImages>,
        theme: Theme,
    ) -> Self {
        #[cfg(test)]
        let ws = Workspaces::in_memory();
        #[cfg(not(test))]
        let ws = Workspaces::load();
        let mut state = Self {
            engine,
            preview_cache: RefCell::new(None),
            visible: Vec::new(),
            cursor: query.chars().count(),
            query,
            selected: 0,
            preview_scroll: 0,
            agent_filter,
            directory_filter,
            yolo,
            scanning: true,
            status: String::new(),
            refresh_status: "refreshing session stores".to_string(),
            last_search_ms: 0.0,
            show_preview: true,
            show_help: false,
            modal: None,
            images,
            theme,
            ws,
            live_ids: HashSet::new(),
            counts: BTreeMap::new(),
            show_sidebar: true,
            results_top: Cell::new(0),
            focus: Focus::Chats,
            sidebar_on_screen: Cell::new(false),
            edit: None,
            picker: None,
            confirm: None,
            scratch_requested: None,
            drag: None,
            last_click: None,
            search_generation: 0,
            applied_search_generation: 0,
            search_requested: false,
            search_preserve_selection: None,
            search_reload_requested: false,
        };
        state.refresh_live();
        state.refresh_search();
        state
    }

    pub(super) fn refresh_search(&mut self) {
        self.refresh_search_inner(false);
    }

    fn refresh_search_inner(&mut self, preserve_selection: bool) {
        let selected_session = preserve_selection
            .then(|| self.selected_session_key())
            .flatten();
        self.search_requested = false;
        self.search_preserve_selection = None;
        self.search_reload_requested = false;
        self.search_generation = self.search_generation.saturating_add(1);
        self.applied_search_generation = self.search_generation;
        let start = Instant::now();
        let agent_filter = self.effective_agent_filter();
        let directory_filter = self.effective_directory_filter();
        let found = self.engine.search(
            &self.query,
            agent_filter.as_deref(),
            directory_filter.as_deref(),
            self.search_limit(),
        );
        self.visible = self.scoped(found);
        self.last_search_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.update_selection_after_search(selected_session.as_ref());
        self.preview_scroll = 0;
    }

    pub(super) fn request_search(&mut self) {
        self.search_preserve_selection = None;
        self.search_requested = true;
    }

    pub(super) fn request_search_preserving_selection(&mut self, reload_index: bool) {
        self.search_reload_requested |= reload_index;
        if self.search_preserve_selection.is_none()
            && !self.search_requested
            && self.applied_search_generation == self.search_generation
        {
            self.search_preserve_selection = self.selected_session_key();
        }
        self.search_requested = true;
    }

    pub(super) fn take_search_request(&mut self) -> Option<SearchRequest> {
        if !self.search_requested {
            return None;
        }
        self.search_requested = false;
        self.search_generation = self.search_generation.saturating_add(1);
        let preserve_selection = self.search_preserve_selection.clone();
        let reload_index = self.search_reload_requested;
        self.search_reload_requested = false;
        Some(SearchRequest {
            generation: self.search_generation,
            query: self.query.clone(),
            agent_filter: self.effective_agent_filter(),
            directory_filter: self.effective_directory_filter(),
            preserve_selection,
            reload_index,
            limit: self.search_limit(),
        })
    }

    pub(super) fn apply_search_result(
        &mut self,
        generation: u64,
        visible: Vec<Session>,
        elapsed_ms: f64,
        preserve_selection: Option<&(String, String)>,
    ) -> bool {
        if generation != self.search_generation {
            return false;
        }
        let selected_session = preserve_selection.and_then(|_| self.selected_session_key());
        self.visible = self.scoped(visible);
        self.last_search_ms = elapsed_ms;
        self.applied_search_generation = generation;
        self.update_selection_after_search(selected_session.as_ref());
        self.search_preserve_selection = None;
        self.preview_scroll = 0;
        if self.status == PENDING_SEARCH_STATUS {
            self.status.clear();
        }
        true
    }

    pub(super) fn apply_search_error(&mut self, generation: u64, error: &str) -> bool {
        if generation != self.search_generation {
            return false;
        }
        self.applied_search_generation = generation;
        self.search_preserve_selection = None;
        self.status = format!("search failed: {error}");
        true
    }

    // ---- workspaces ----------------------------------------------------

    /// Chats in a workspace can be anywhere in the ranking, so a workspace
    /// view fetches far more than the default 100 before filtering.
    fn search_limit(&self) -> usize {
        if self.ws.scope() == ALL && !self.ws.has_hidden() {
            100
        } else {
            10_000
        }
    }

    /// Keep what belongs in the open view. Deleted chats only show in the
    /// Deleted view.
    fn scoped(&self, mut sessions: Vec<Session>) -> Vec<Session> {
        let scope = self.ws.scope();
        sessions.retain(|session| !self.ws.is_purged(&session.id));
        if scope == TRASH {
            sessions.retain(|session| self.ws.is_trashed(&session.id));
            return sessions;
        }
        if self.ws.deleted_name_of(scope).is_some() {
            // Looking inside a deleted workspace, without restoring it.
            sessions.retain(|session| self.ws.trashed_with(&session.id) == Some(scope));
            return sessions;
        }
        sessions.retain(|session| !self.ws.is_trashed(&session.id));
        if scope == ALL {
            sessions.truncate(100);
        } else {
            sessions.retain(|session| self.ws.ws_of(&session.id) == scope);
        }
        sessions
    }

    /// Re-read which chats exist, for the sidebar counts.
    pub(super) fn refresh_live(&mut self) {
        self.live_ids = self
            .engine
            .search("", None, None, 100_000)
            .into_iter()
            .map(|session| session.id)
            .filter(|id| !self.ws.is_purged(id))
            .collect();
        self.recount();
    }

    fn recount(&mut self) {
        self.counts = self.ws.counts(&self.live_ids);
    }

    pub(super) fn title_of<'a>(&'a self, session: &'a Session) -> &'a str {
        self.ws.display_title(session)
    }

    /// A chat's workspace as (name, color slot), if it is in one.
    pub(super) fn ws_tag(&self, session: &Session) -> Option<(&str, usize)> {
        let id = self.ws.ws_of(&session.id);
        Some((self.ws.name_of(id)?, self.ws.color_index(id)?))
    }

    /// The results list shows a Workspace column where chats of several
    /// workspaces mix.
    /// Viewing Deleted, or a deleted workspace inside it.
    pub(super) fn in_deleted_view(&self) -> bool {
        let scope = self.ws.scope();
        scope == TRASH || self.ws.deleted_name_of(scope).is_some()
    }

    pub(super) fn shows_workspace_column(&self) -> bool {
        matches!(self.ws.scope(), ALL | TRASH)
    }

    pub(super) fn sidebar_rows(&self) -> Vec<SidebarRow> {
        let count = |key: &str| self.counts.get(key).copied().unwrap_or(0);
        let plain = |kind, scope: &str, name: &str, count| SidebarRow {
            kind,
            scope: scope.to_string(),
            name: name.to_string(),
            count,
            color: None,
        };
        let mut rows = vec![plain(RowKind::All, ALL, "All chats", count(ALL))];
        for ws in self.ws.list() {
            rows.push(SidebarRow {
                color: self.ws.color_index(&ws.id),
                ..plain(RowKind::Workspace, &ws.id, &ws.name, count(&ws.id))
            });
        }
        rows.push(plain(RowKind::Unsorted, NONE, "Unsorted", count(NONE)));
        rows.push(plain(RowKind::Deleted, TRASH, "Deleted", count(TRASH)));
        if self.in_deleted_view() {
            for ws in self.ws.deleted_list() {
                rows.push(SidebarRow {
                    color: self.ws.color_index(&ws.id),
                    ..plain(RowKind::DeletedWorkspace, &ws.id, &ws.name, count(&ws.id))
                });
            }
        }
        rows.push(plain(RowKind::New, "", "+ New workspace", 0));
        rows
    }

    fn report<T>(&mut self, result: std::io::Result<T>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.status = format!("could not save workspaces: {error}");
                None
            }
        }
    }

    pub(super) fn set_scope(&mut self, scope: &str) {
        if self.ws.scope() == scope {
            return;
        }
        let result = self.ws.set_scope(scope);
        self.report(result);
        self.request_search();
    }

    /// Previous / next workspace in sidebar order (All, workspaces, Unsorted).
    pub(super) fn cycle_scope(&mut self, delta: isize) {
        let scopes: Vec<String> = self
            .sidebar_rows()
            .into_iter()
            .filter(|row| row.kind != RowKind::New)
            .map(|row| row.scope)
            .collect();
        let current = scopes
            .iter()
            .position(|scope| scope == self.ws.scope())
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(scopes.len() as isize) as usize;
        self.set_scope(&scopes[next]);
    }

    /// Arrow keys in the workspace list: like `cycle_scope` but without
    /// wrapping. `to` jumps to the first (`Some(false)`) or last row.
    pub(super) fn step_scope(&mut self, delta: isize, to: Option<bool>) {
        let scopes: Vec<String> = self
            .sidebar_rows()
            .into_iter()
            .filter(|row| row.kind != RowKind::New)
            .map(|row| row.scope)
            .collect();
        let last = scopes.len() as isize - 1;
        let current = scopes
            .iter()
            .position(|scope| scope == self.ws.scope())
            .unwrap_or(0) as isize;
        let next = match to {
            Some(false) => 0,
            Some(true) => last,
            None => (current + delta).clamp(0, last),
        };
        self.set_scope(&scopes[next as usize]);
    }

    // ---- focus ---------------------------------------------------------

    /// Called while drawing: whether the sidebar actually fits on screen.
    pub(super) fn note_sidebar_on_screen(&self, on_screen: bool) {
        self.sidebar_on_screen.set(on_screen);
    }

    pub(super) fn focus_sidebar(&mut self) {
        if self.show_sidebar && self.sidebar_on_screen.get() {
            self.focus = Focus::Sidebar;
        } else {
            self.status = "the workspace list is hidden (Ctrl+O shows it)".to_string();
        }
    }

    /// The Delete key on a chat: delete it, or in the Deleted view erase it
    /// for good (after asking). Restoring is F8.
    pub(super) fn delete_key_on_chat(&mut self) {
        if self.in_deleted_view() {
            self.request_purge_selected_chat();
        } else {
            self.toggle_delete_selected();
        }
    }

    /// The Delete key with the workspace list focused: delete the open
    /// workspace, or erase it for good if it is already deleted.
    pub(super) fn delete_key_on_workspace(&mut self) {
        let scope = self.ws.scope().to_string();
        if self.ws.name_of(&scope).is_some() {
            self.delete_workspace(&scope);
        } else if self.ws.deleted_name_of(&scope).is_some() {
            self.request_purge_open_workspace();
        } else {
            self.status = "pick a workspace first (↑/↓)".to_string();
        }
    }

    /// F8 with the workspace list focused: delete the open workspace,
    /// or restore it when it is a deleted one.
    pub(super) fn delete_or_restore_open_workspace(&mut self) {
        let scope = self.ws.scope().to_string();
        if self.ws.name_of(&scope).is_some() {
            self.delete_workspace(&scope);
        } else if self.ws.deleted_name_of(&scope).is_some() {
            self.restore_workspace(&scope);
        } else {
            self.status = "pick a workspace first (↑/↓)".to_string();
        }
    }

    pub(super) fn begin_chat_edit(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let line = LineEdit::new(self.ws.display_title(session));
        self.edit = Some(Edit {
            target: EditTarget::Chat {
                id: session.id.clone(),
                original: session.title.clone(),
            },
            line,
        });
    }

    pub(super) fn begin_workspace_edit(&mut self, ws_id: &str, initial: Option<&str>) {
        let Some(name) = self.ws.name_of(ws_id) else {
            return;
        };
        let line = LineEdit::new(initial.unwrap_or(name));
        self.show_sidebar = true;
        self.edit = Some(Edit {
            target: EditTarget::Workspace(ws_id.to_string()),
            line,
        });
    }

    /// Rename the workspace that is currently open, if it is a real one.
    pub(super) fn begin_current_workspace_edit(&mut self) {
        let scope = self.ws.scope().to_string();
        if self.ws.name_of(&scope).is_some() {
            self.begin_workspace_edit(&scope, None);
        } else {
            self.status = "open a workspace first (Alt+↑/↓), then rename it".to_string();
        }
    }

    pub(super) fn commit_edit(&mut self) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        let text = edit.line.text.trim().to_string();
        match edit.target {
            EditTarget::Chat { id, original } => {
                let name = if text == original {
                    String::new()
                } else {
                    text
                };
                let result = self.ws.rename_chat(&id, &name);
                self.report(result);
            }
            EditTarget::Workspace(id) => {
                if !text.is_empty() {
                    let result = self.ws.rename(&id, &text);
                    self.report(result);
                }
            }
        }
    }

    pub(super) fn new_workspace(&mut self) {
        let added = self.ws.add("New workspace");
        let Some(id) = self.report(added) else {
            return;
        };
        self.recount();
        self.set_scope(&id);
        self.begin_workspace_edit(&id, Some(""));
    }

    /// Move a chat to a workspace, Unsorted or Deleted. Moving a deleted chat
    /// anywhere else restores it.
    pub(super) fn move_session_to(&mut self, session_id: &str, ws_id: &str) {
        let was_deleted = self.ws.is_trashed(session_id);
        let result = if ws_id == TRASH {
            self.ws.trash_chat(session_id)
        } else {
            self.ws.move_session(session_id, ws_id)
        };
        if self.report(result).is_none() {
            return;
        }
        self.recount();
        let place = self.ws.scope_label(ws_id);
        self.status = match (ws_id == TRASH, was_deleted) {
            (true, _) => "moved to Deleted (restore it from there)".to_string(),
            (false, true) => format!("restored to {place}"),
            (false, false) => format!("moved to {place}"),
        };
        self.request_search_preserving_selection(false);
    }

    /// F8: delete the selected chat, or restore it when viewing Deleted.
    pub(super) fn toggle_delete_selected(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let id = session.id.clone();
        if self.ws.is_trashed(&id) {
            let result = self.ws.restore_chat(&id);
            if self.report(result).is_some() {
                self.recount();
                self.status = format!("restored to {}", self.ws.scope_label(self.ws.ws_of(&id)));
                self.request_search_preserving_selection(false);
            }
        } else {
            self.move_session_to(&id, TRASH);
        }
    }

    /// `Ctrl+I` / `Alt+I`: ask to open a scratch chat — a brand-new session
    /// with an agent, for a quick aside that should not linger in the chat
    /// list. Uses the selected chat's agent so it stays contextual, or the
    /// first agent with any indexed history, or `claude` as a last resort.
    /// `tui.rs` services the request: it owns the terminal, so it is the one
    /// that suspends Cove, runs the agent, and resumes.
    pub(super) fn request_scratch(&mut self) {
        let agent = self
            .selected_session()
            .map(|session| session.agent.clone())
            .or_else(|| {
                self.agent_filters_with_sessions()
                    .first()
                    .map(|(agent, _)| (*agent).to_string())
            })
            .unwrap_or_else(|| "claude".to_string());
        let Some(adapter) = adapter_for(&agent) else {
            self.status = format!("no adapter for agent {agent}");
            return;
        };
        let directory = std::env::current_dir()
            .ok()
            .and_then(|path| path.to_str().map(str::to_string))
            .unwrap_or_default();
        let known_before = self.engine.known_sessions().unwrap_or_default();
        self.scratch_requested = Some(ScratchRequest {
            command: adapter.new_session_command(&directory, self.yolo),
            directory,
            agent: adapter.name(),
            known_before,
        });
    }

    pub(super) fn take_scratch_request(&mut self) -> Option<ScratchRequest> {
        self.scratch_requested.take()
    }

    /// Called after the scratch chat's process exits: finds whatever new
    /// session(s) that agent wrote while it ran and hides them the same way
    /// `F9` does, so they never show up in Cove. It cannot know for certain
    /// that a new session belongs to this run rather than something else
    /// happening with the same agent at the same moment, but that is rare
    /// enough to accept for a quick-aside feature — and nothing on disk is
    /// ever touched either way, so the worst case is a chat staying hidden
    /// that a purist would have kept.
    pub(super) fn finish_scratch(&mut self, request: ScratchRequest) {
        let _ = self.engine.refresh_incremental();
        let _ = self.engine.reload();
        let after = self.engine.known_sessions().unwrap_or_default();
        let new_ids = new_sessions_for(request.agent, &request.known_before, &after);
        let hidden = new_ids.len();
        for id in &new_ids {
            let _ = self.ws.purge_chat(id);
        }
        self.refresh_live();
        self.status = if hidden == 0 {
            "scratch chat closed".to_string()
        } else {
            format!(
                "scratch chat closed and hidden ({hidden} new session{})",
                if hidden == 1 { "" } else { "s" }
            )
        };
        self.request_search_preserving_selection(false);
    }

    /// F9: ask before deleting the selected chat for good. Only chats that are
    /// already in Deleted can be.
    pub(super) fn request_purge_selected_chat(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        if !self.ws.is_trashed(&session.id) {
            self.status = "delete it first (F8); then it can be erased from Deleted".to_string();
            return;
        }
        self.confirm = Some(ConfirmAction::PurgeChat {
            id: session.id.clone(),
            title: self.ws.display_title(session).to_string(),
        });
    }

    /// Ask before deleting the open (deleted) workspace for good.
    pub(super) fn request_purge_open_workspace(&mut self) {
        let scope = self.ws.scope().to_string();
        let Some(name) = self.ws.deleted_name_of(&scope).map(str::to_string) else {
            self.status = "open a deleted workspace first, then erase it".to_string();
            return;
        };
        let chats = self.ws.trashed_in(&scope).len();
        self.confirm = Some(ConfirmAction::PurgeWorkspace {
            id: scope,
            name,
            chats,
        });
    }

    /// The answer to the confirmation dialog.
    pub(super) fn answer_confirm(&mut self, yes: bool) {
        let Some(action) = self.confirm.take() else {
            return;
        };
        if !yes {
            return;
        }
        let result = match &action {
            ConfirmAction::PurgeChat { id, .. } => self.ws.purge_chat(id),
            ConfirmAction::PurgeWorkspace { id, .. } => self.ws.purge_workspace(id),
        };
        if self.report(result).is_none() {
            return;
        }
        self.recount();
        match action {
            ConfirmAction::PurgeChat { .. } => {
                self.status = "deleted for good".to_string();
                self.request_search_preserving_selection(false);
            }
            ConfirmAction::PurgeWorkspace { name, chats, .. } => {
                self.status = format!("deleted “{name}” and {chats} chat(s) for good");
                self.request_search();
            }
        }
    }

    /// Restore the deleted workspace the selected chat was deleted with,
    /// together with everything else deleted with it.
    pub(super) fn restore_selected_workspace(&mut self) {
        let open = self.ws.scope().to_string();
        if self.ws.deleted_name_of(&open).is_some() {
            self.restore_workspace(&open);
            return;
        }
        let Some(ws_id) = self
            .selected_session()
            .and_then(|session| self.ws.trashed_with(&session.id))
            .map(str::to_string)
        else {
            self.status = "this chat was not deleted with a workspace".to_string();
            return;
        };
        self.restore_workspace(&ws_id);
    }

    pub(super) fn restore_workspace(&mut self, ws_id: &str) {
        let name = self
            .ws
            .deleted_name_of(ws_id)
            .unwrap_or("workspace")
            .to_string();
        let result = self.ws.restore_workspace(ws_id);
        if self.report(result).is_none() {
            return;
        }
        self.recount();
        self.status = format!("restored workspace “{name}”");
        self.request_search_preserving_selection(false);
    }

    pub(super) fn open_picker(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let current = self.ws.ws_of(&session.id).to_string();
        let mut items: Vec<(String, String)> = self
            .ws
            .list()
            .map(|ws| (ws.id.clone(), ws.name.clone()))
            .collect();
        items.push((NONE.to_string(), "No workspace".to_string()));
        let selected = items.iter().position(|(id, _)| *id == current).unwrap_or(0);
        self.picker = Some(Picker {
            session_id: session.id.clone(),
            items,
            selected,
        });
    }

    /// Delete a workspace and its chats. Nothing is lost: they all move to
    /// Deleted and can be restored.
    pub(super) fn delete_workspace(&mut self, ws_id: &str) {
        let Some(name) = self.ws.name_of(ws_id).map(str::to_string) else {
            return;
        };
        let chats = self.counts.get(ws_id).copied().unwrap_or(0);
        let was_open = self.ws.scope() == ws_id;
        let result = self.ws.delete(ws_id);
        if self.report(result).is_none() {
            return;
        }
        self.recount();
        self.status = format!("deleted “{name}” with {chats} chat(s); restore from Deleted");
        if was_open {
            self.request_search();
        } else {
            self.request_search_preserving_selection(false);
        }
    }

    fn selected_session_key(&self) -> Option<(String, String)> {
        self.selected_session()
            .map(|session| (session.agent.clone(), session.id.clone()))
    }

    fn update_selection_after_search(&mut self, selected_session: Option<&(String, String)>) {
        if self.visible.is_empty() {
            self.selected = 0;
        } else if let Some((agent, id)) = selected_session {
            self.selected = self
                .visible
                .iter()
                .position(|session| session.agent == *agent && session.id == *id)
                .unwrap_or_else(|| self.selected.min(self.visible.len() - 1));
        } else {
            self.selected = 0;
        }
    }

    pub(super) fn search_pending(&self) -> bool {
        self.search_requested || self.applied_search_generation != self.search_generation
    }

    /// A dialog is on screen. Terminal images are not drawn under it, since
    /// their cells would show through.
    pub(super) fn overlay_open(&self) -> bool {
        self.show_help || self.modal.is_some() || self.picker.is_some() || self.confirm.is_some()
    }

    /// The slice of results on screen. The top only moves when the selection
    /// leaves the window, so clicking a row does not make the list jump.
    pub(super) fn results_window(&self, max_rows: usize) -> (usize, usize) {
        let len = self.visible.len();
        if len == 0 || max_rows == 0 {
            return (0, 0);
        }
        let mut top = self.results_top.get();
        if self.selected < top {
            top = self.selected;
        } else if self.selected >= top + max_rows {
            top = self.selected + 1 - max_rows;
        }
        top = top.min(len.saturating_sub(max_rows));
        self.results_top.set(top);
        (top, (top + max_rows).min(len))
    }

    pub(super) fn selected_session(&self) -> Option<&Session> {
        self.visible.get(self.selected)
    }

    pub(super) fn move_selection(&mut self, delta: isize) {
        if self.visible.is_empty() {
            self.selected = 0;
            return;
        }
        let max = self.visible.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
        self.preview_scroll = 0;
    }

    pub(super) fn scroll_preview(&mut self, delta: isize) {
        if delta < 0 {
            self.preview_scroll = self
                .preview_scroll
                .saturating_sub(delta.unsigned_abs() as u16);
        } else {
            self.preview_scroll = self.preview_scroll.saturating_add(delta as u16);
        }
    }

    pub(super) fn active_agent_filter(&self) -> Option<String> {
        if query_has_agent_filter(&self.query) {
            single_query_agent_filter(&self.query)
        } else {
            self.agent_filter.clone()
        }
    }

    pub(super) fn active_agent_filters(&self) -> Vec<String> {
        if query_has_agent_filter(&self.query) {
            query_agent_filters(&self.query)
        } else {
            self.agent_filter.iter().cloned().collect()
        }
    }

    pub(super) fn all_agent_filter_active(&self) -> bool {
        self.agent_filter.is_none() && !query_has_agent_filter(&self.query)
    }

    pub(super) fn agent_filters_with_sessions(&self) -> Vec<(&'static str, usize)> {
        AGENT_ORDER
            .iter()
            .filter_map(|agent| {
                let count = self.engine.count_for_agent(Some(agent));
                (count > 0).then_some((*agent, count))
            })
            .collect()
    }

    pub(super) fn count_agent_filter(&self) -> Option<String> {
        if let Some(agent) = single_query_agent_filter(&self.query) {
            return Some(agent);
        }
        if query_has_agent_filter(&self.query) {
            return None;
        }
        self.agent_filter.clone()
    }

    pub(super) fn suggestion_suffix(&self) -> Option<String> {
        let suggestion = search_suggestion(&self.query, self.cursor)?;
        suggestion
            .strip_prefix(&self.query)
            .filter(|suffix| !suffix.is_empty())
            .map(ToString::to_string)
    }

    pub(super) fn accept_suggestion(&mut self) -> bool {
        let Some(suggestion) = search_suggestion(&self.query, self.cursor) else {
            return false;
        };
        self.cursor = suggestion.chars().count();
        self.query = suggestion;
        self.clear_explicit_filter_if_query_has_agent();
        self.request_search();
        true
    }

    pub(super) fn cycle_agent(&mut self, reverse: bool) {
        let available = self.agent_filters_with_sessions();
        let active = self.active_agent_filter();
        let current = active
            .as_deref()
            .and_then(|agent| {
                available
                    .iter()
                    .position(|(candidate, _)| *candidate == agent)
            })
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let len = available.len() + 1;
        let next = if reverse {
            (current + len - 1) % len
        } else {
            (current + 1) % len
        };
        let next_agent = if next == 0 {
            None
        } else {
            Some(available[next - 1].0.to_string())
        };
        self.query = update_agent_in_query(&self.query, next_agent.as_deref());
        self.cursor = self.query.chars().count();
        self.agent_filter = None;
        self.request_search();
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        let byte_idx = char_to_byte_idx(&self.query, self.cursor);
        self.query.insert(byte_idx, ch);
        self.cursor += 1;
        self.clear_explicit_filter_if_query_has_agent();
        self.request_search();
    }

    pub(super) fn backspace(&mut self) {
        self.delete_char_range(self.cursor.saturating_sub(1), self.cursor);
    }

    pub(super) fn delete(&mut self) {
        self.delete_char_range(self.cursor, self.cursor.saturating_add(1));
    }

    pub(super) fn delete_to_start(&mut self) {
        self.delete_char_range(0, self.cursor);
    }

    pub(super) fn delete_previous_word(&mut self) {
        let chars: Vec<_> = self.query.chars().collect();
        let mut start = self.cursor.min(chars.len());
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        self.delete_char_range(start, self.cursor);
    }

    fn delete_char_range(&mut self, start: usize, end: usize) {
        let char_count = self.query.chars().count();
        let start = start.min(char_count);
        let end = end.min(char_count);
        if start >= end {
            return;
        }
        let start_byte = char_to_byte_idx(&self.query, start);
        let end_byte = char_to_byte_idx(&self.query, end);
        self.query.replace_range(start_byte..end_byte, "");
        self.cursor = start;
        self.clear_explicit_filter_if_query_has_agent();
        self.request_search();
    }

    fn effective_agent_filter(&self) -> Option<String> {
        if query_has_agent_filter(&self.query) {
            None
        } else {
            self.agent_filter.clone()
        }
    }

    fn effective_directory_filter(&self) -> Option<String> {
        if query_has_directory_filter(&self.query) {
            None
        } else {
            self.directory_filter.clone()
        }
    }

    fn clear_explicit_filter_if_query_has_agent(&mut self) {
        if query_has_agent_filter(&self.query) {
            self.agent_filter = None;
        }
    }
}

fn query_agent_filters(query: &str) -> Vec<String> {
    parse_query(query)
        .agent
        .map(valid_included_agents)
        .unwrap_or_default()
}

fn valid_included_agents(filter: Filter) -> Vec<String> {
    filter
        .include
        .into_iter()
        .filter_map(normalize_agent)
        .fold(Vec::new(), |mut agents, agent| {
            if !agents.contains(&agent) {
                agents.push(agent);
            }
            agents
        })
}

fn single_query_agent_filter(query: &str) -> Option<String> {
    let filter = parse_query(query).agent?;
    if !filter.exclude.is_empty() {
        return None;
    }
    let agents = valid_included_agents(filter);
    match agents.as_slice() {
        [agent] => Some(agent.clone()),
        _ => None,
    }
}

fn normalize_agent(agent: String) -> Option<String> {
    let agent = agent.to_ascii_lowercase();
    is_agent(&agent).then_some(agent)
}

fn query_has_agent_filter(query: &str) -> bool {
    parse_query(query).agent.is_some()
}

fn query_has_directory_filter(query: &str) -> bool {
    parse_query(query).directory.is_some()
}

fn update_agent_in_query(query: &str, agent: Option<&str>) -> String {
    let mut parts: Vec<_> = query
        .split_whitespace()
        .map(ToString::to_string)
        .filter(|part| !is_agent_keyword(part))
        .collect();
    if let Some(agent) = agent {
        parts.push(format!("agent:{agent}"));
    }
    parts.join(" ")
}

fn is_agent_keyword(token: &str) -> bool {
    token
        .strip_prefix('-')
        .unwrap_or(token)
        .starts_with("agent:")
}

fn search_suggestion(query: &str, cursor: usize) -> Option<String> {
    if cursor != query.chars().count() {
        return None;
    }
    let cursor_idx = char_to_byte_idx(query, cursor);
    let before_cursor = &query[..cursor_idx];
    let token_start = before_cursor
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    let token = &before_cursor[token_start..];
    let (negated, token) = token
        .strip_prefix('-')
        .map(|token| ("-", token))
        .unwrap_or(("", token));
    if let Some(value) = token.strip_prefix("agent:") {
        return complete_keyword(query, token_start, negated, "agent:", value, &AGENT_ORDER);
    }
    if let Some(value) = token.strip_prefix("date:") {
        return complete_keyword(
            query,
            token_start,
            negated,
            "date:",
            value,
            &DATE_SUGGESTIONS,
        );
    }
    None
}

fn complete_keyword(
    query: &str,
    token_start: usize,
    negated: &str,
    keyword: &str,
    value: &str,
    values: &[&str],
) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let (value_prefix, partial) = value
        .strip_prefix('!')
        .map(|partial| ("!", partial))
        .unwrap_or(("", value));
    if partial.is_empty() {
        return None;
    }
    let partial = partial.to_ascii_lowercase();
    let completion = values.iter().find(|candidate| {
        candidate.starts_with(&partial) && candidate.to_ascii_lowercase() != partial
    })?;
    Some(format!(
        "{}{}{}{}{}",
        &query[..token_start],
        negated,
        keyword,
        value_prefix,
        completion
    ))
}

pub(super) fn handle_scan_message(state: &mut AppState, message: ScanMessage) {
    match message {
        ScanMessage::Progress {
            elapsed,
            new_or_modified,
            deleted,
            total,
        } => {
            state.refresh_status =
                refresh_status("refreshing", total, new_or_modified, deleted, elapsed);
            state.request_search_preserving_selection(true);
        }
        ScanMessage::Finished {
            elapsed,
            new_or_modified,
            deleted,
            total,
        } => {
            let _ = state.engine.reload();
            state.refresh_live();
            state.scanning = false;
            state.refresh_status =
                refresh_status("refreshed", total, new_or_modified, deleted, elapsed);
            state.request_search_preserving_selection(true);
        }
        ScanMessage::Failed { elapsed, error } => {
            state.scanning = false;
            state.refresh_status =
                format!("refresh failed after {}: {error}", elapsed_label(elapsed));
        }
    }
}

fn refresh_status(
    label: &str,
    total: usize,
    new_or_modified: usize,
    deleted: usize,
    elapsed: Duration,
) -> String {
    let mut status = format!("{label}: {total} sessions, {new_or_modified} changed");
    if deleted > 0 {
        status.push_str(&format!(", {deleted} deleted"));
    }
    status.push_str(&format!(", {}", elapsed_label(elapsed)));
    status
}

fn elapsed_label(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs_f64();
    if seconds >= 1.0 {
        format!("{seconds:.1}s")
    } else {
        format!("{:.0}ms", seconds * 1000.0)
    }
}

/// The scratch chat's actual purge decision: which ids of `agent` are in
/// `known_after` but were not already in `known_before`.
fn new_sessions_for(
    agent: &str,
    known_before: &KnownSessions,
    known_after: &KnownSessions,
) -> Vec<String> {
    known_after
        .keys()
        .filter(|(found_agent, id)| {
            found_agent == agent && !known_before.contains_key(&(found_agent.clone(), id.clone()))
        })
        .map(|(_, id)| id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(pairs: &[(&str, &str)]) -> KnownSessions {
        pairs
            .iter()
            .map(|(agent, id)| ((agent.to_string(), id.to_string()), 0.0))
            .collect()
    }

    #[test]
    fn new_sessions_for_only_reports_ids_of_that_agent_absent_before() {
        let before = known(&[("claude", "a"), ("codex", "x")]);
        let after = known(&[("claude", "a"), ("claude", "b"), ("codex", "y")]);

        let mut claude_new = new_sessions_for("claude", &before, &after);
        claude_new.sort();
        assert_eq!(claude_new, vec!["b".to_string()], "a already existed");

        // codex's new session is ignored when checking claude.
        assert!(
            new_sessions_for("claude", &before, &after)
                .iter()
                .all(|id| id != "y")
        );
        assert_eq!(
            new_sessions_for("codex", &before, &after),
            vec!["y".to_string()]
        );
    }

    #[test]
    fn new_sessions_for_finds_nothing_when_nothing_changed() {
        let snapshot = known(&[("claude", "a")]);
        assert!(new_sessions_for("claude", &snapshot, &snapshot).is_empty());
    }
}
