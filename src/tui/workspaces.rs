//! Workspaces and custom chat names, kept in a small sidecar file so the
//! agents' own session stores (and the search index) are never modified.
//!
//! The file is `~/.local/state/frw/state.json` (override with
//! `COVE_STATE_FILE`). It is only written when something changes.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::APP_NAME;
use crate::model::Session;

/// Pseudo scope: every chat.
pub(super) const ALL: &str = "*";
/// Pseudo scope: chats that are in no workspace.
pub(super) const NONE: &str = "";
/// Pseudo scope: chats (and workspaces) that were deleted. Nothing is ever
/// removed from the agents' own stores; "deleted" only hides a chat here.
pub(super) const TRASH: &str = "!deleted";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Workspace {
    pub(super) id: String,
    pub(super) name: String,
    #[serde(default)]
    pub(super) deleted: bool,
    /// Permanently deleted: gone from every view for good.
    #[serde(default)]
    pub(super) purged: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    workspaces: Vec<Workspace>,
    #[serde(default)]
    assign: BTreeMap<String, String>,
    #[serde(default)]
    names: BTreeMap<String, String>,
    /// Deleted chats: session id -> `""` if the chat itself was deleted, or
    /// the id of the workspace that was deleted with it.
    #[serde(default)]
    trash: BTreeMap<String, String>,
    /// Chats deleted for good. Their files are untouched; frw just never
    /// shows them again.
    #[serde(default)]
    purged: BTreeSet<String>,
    #[serde(default = "all_scope")]
    scope: String,
}

fn all_scope() -> String {
    ALL.to_string()
}

impl Default for StateFile {
    fn default() -> Self {
        Self {
            workspaces: Vec::new(),
            assign: BTreeMap::new(),
            names: BTreeMap::new(),
            trash: BTreeMap::new(),
            purged: BTreeSet::new(),
            scope: all_scope(),
        }
    }
}

pub(super) struct Workspaces {
    path: Option<PathBuf>,
    data: StateFile,
}

impl Workspaces {
    #[cfg_attr(test, allow(dead_code))]
    pub(super) fn load() -> Self {
        let pinned = std::env::var_os("COVE_STATE_FILE").is_some();
        Self::load_from(state_path(), (!pinned).then(legacy_state_path).flatten())
    }

    /// Read `path`; if there is nothing there yet, adopt the state an earlier
    /// version left at `legacy` (and save it to the new place).
    fn load_from(path: Option<PathBuf>, legacy: Option<PathBuf>) -> Self {
        let mut data = path.as_ref().and_then(|path| read_state(path));
        let migrated = data.is_none() && legacy.is_some();
        if migrated {
            data = legacy.as_ref().and_then(|path| read_state(path));
        }
        let migrated = migrated && data.is_some();
        let mut store = Self {
            path,
            data: data.unwrap_or_default(),
        };
        store.normalize_scope();
        if migrated {
            let _ = store.save();
        }
        store
    }

    /// A store that never touches disk (tests).
    #[cfg(test)]
    pub(super) fn in_memory() -> Self {
        Self {
            path: None,
            data: StateFile::default(),
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    fn normalize_scope(&mut self) {
        let scope = &self.data.scope;
        let known = scope == ALL
            || scope == NONE
            || scope == TRASH
            || self.name_of(scope).is_some()
            || self.deleted_name_of(scope).is_some();
        if !known {
            self.data.scope = all_scope();
        }
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(&self.data)?)?;
        fs::rename(tmp, path)
    }

    /// Workspaces that are not deleted, in creation order.
    pub(super) fn list(&self) -> impl Iterator<Item = &Workspace> {
        self.data
            .workspaces
            .iter()
            .filter(|w| !w.deleted && !w.purged)
    }

    pub(super) fn deleted_list(&self) -> impl Iterator<Item = &Workspace> {
        self.data
            .workspaces
            .iter()
            .filter(|w| w.deleted && !w.purged)
    }

    pub(super) fn scope(&self) -> &str {
        &self.data.scope
    }

    pub(super) fn set_scope(&mut self, scope: &str) -> io::Result<()> {
        if self.data.scope == scope {
            return Ok(());
        }
        self.data.scope = scope.to_string();
        self.save()
    }

    /// Stable color slot for a workspace: its creation-order position, which
    /// deleting never changes.
    pub(super) fn color_index(&self, ws_id: &str) -> Option<usize> {
        self.data.workspaces.iter().position(|w| w.id == ws_id)
    }

    /// Name of a workspace that exists (deleted ones do not).
    pub(super) fn name_of(&self, ws_id: &str) -> Option<&str> {
        self.list().find(|w| w.id == ws_id).map(|w| w.name.as_str())
    }

    pub(super) fn deleted_name_of(&self, ws_id: &str) -> Option<&str> {
        self.deleted_list()
            .find(|w| w.id == ws_id)
            .map(|w| w.name.as_str())
    }

    /// Label for a scope, for the results title and the picker.
    pub(super) fn scope_label(&self, scope: &str) -> String {
        match scope {
            ALL => "All chats".to_string(),
            NONE => "Unsorted".to_string(),
            TRASH => "Deleted".to_string(),
            id => match (self.name_of(id), self.deleted_name_of(id)) {
                (Some(name), _) => name.to_string(),
                (None, Some(name)) => format!("{name} (deleted)"),
                (None, None) => "?".to_string(),
            },
        }
    }

    /// The workspace a chat is in, or `NONE`.
    pub(super) fn ws_of(&self, session_id: &str) -> &str {
        match self.data.assign.get(session_id) {
            Some(ws) if self.name_of(ws).is_some() => ws,
            _ => NONE,
        }
    }

    pub(super) fn add(&mut self, name: &str) -> io::Result<String> {
        let mut seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ u64::from(std::process::id());
        let id = loop {
            let candidate = format!("{:08x}", seed as u32);
            if self.name_of(&candidate).is_none() {
                break candidate;
            }
            seed = seed.wrapping_add(1);
        };
        self.data.workspaces.push(Workspace {
            id: id.clone(),
            name: name.to_string(),
            deleted: false,
            purged: false,
        });
        self.save()?;
        Ok(id)
    }

    pub(super) fn rename(&mut self, ws_id: &str, name: &str) -> io::Result<()> {
        if let Some(ws) = self.data.workspaces.iter_mut().find(|w| w.id == ws_id) {
            ws.name = name.to_string();
        }
        self.save()
    }

    /// Soft-delete a workspace. Its chats are deleted with it and keep their
    /// assignment, so restoring the workspace brings them all back.
    pub(super) fn delete(&mut self, ws_id: &str) -> io::Result<()> {
        if let Some(ws) = self
            .data
            .workspaces
            .iter_mut()
            .find(|w| w.id == ws_id && !w.deleted)
        {
            ws.deleted = true;
        }
        let members: Vec<String> = self
            .data
            .assign
            .iter()
            .filter(|(_, ws)| *ws == ws_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in members {
            self.data
                .trash
                .entry(id)
                .or_insert_with(|| ws_id.to_string());
        }
        if self.data.scope == ws_id {
            self.data.scope = all_scope();
        }
        self.save()
    }

    /// Bring back a deleted workspace and the chats deleted with it.
    pub(super) fn restore_workspace(&mut self, ws_id: &str) -> io::Result<()> {
        if let Some(ws) = self.data.workspaces.iter_mut().find(|w| w.id == ws_id) {
            ws.deleted = false;
        }
        self.data.trash.retain(|_, reason| reason != ws_id);
        self.save()
    }

    pub(super) fn is_trashed(&self, session_id: &str) -> bool {
        self.data.trash.contains_key(session_id)
    }

    /// The deleted workspace a trashed chat was deleted with, if any.
    pub(super) fn trashed_with(&self, session_id: &str) -> Option<&str> {
        self.data
            .trash
            .get(session_id)
            .map(String::as_str)
            .filter(|ws| self.deleted_name_of(ws).is_some())
    }

    pub(super) fn is_purged(&self, session_id: &str) -> bool {
        self.data.purged.contains(session_id)
    }

    /// Chats or workspaces are hidden, so views must look past them.
    pub(super) fn has_hidden(&self) -> bool {
        !self.data.trash.is_empty() || !self.data.purged.is_empty()
    }

    /// Delete a chat for good. It never shows again; nothing on disk changes.
    pub(super) fn purge_chat(&mut self, session_id: &str) -> io::Result<()> {
        self.data.purged.insert(session_id.to_string());
        self.data.trash.remove(session_id);
        self.data.assign.remove(session_id);
        self.data.names.remove(session_id);
        self.save()
    }

    /// Chats that were deleted together with this workspace.
    pub(super) fn trashed_in(&self, ws_id: &str) -> Vec<String> {
        self.data
            .trash
            .iter()
            .filter(|(_, reason)| *reason == ws_id)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Delete a deleted workspace for good, with the chats deleted along with
    /// it. The workspace keeps its slot so other workspaces keep their colors.
    pub(super) fn purge_workspace(&mut self, ws_id: &str) -> io::Result<()> {
        for id in self.trashed_in(ws_id) {
            self.data.purged.insert(id.clone());
            self.data.trash.remove(&id);
            self.data.assign.remove(&id);
            self.data.names.remove(&id);
        }
        if let Some(ws) = self.data.workspaces.iter_mut().find(|w| w.id == ws_id) {
            ws.deleted = true;
            ws.purged = true;
        }
        self.data.assign.retain(|_, ws| ws != ws_id);
        if self.data.scope == ws_id {
            self.data.scope = all_scope();
        }
        self.save()
    }

    pub(super) fn trash_chat(&mut self, session_id: &str) -> io::Result<()> {
        self.data.trash.entry(session_id.to_string()).or_default();
        self.save()
    }

    /// Restore a chat to the workspace it was in, or to Unsorted if that
    /// workspace is deleted.
    pub(super) fn restore_chat(&mut self, session_id: &str) -> io::Result<()> {
        self.data.trash.remove(session_id);
        self.save()
    }

    /// Moving a chat anywhere also restores it.
    pub(super) fn move_session(&mut self, session_id: &str, ws_id: &str) -> io::Result<()> {
        self.data.trash.remove(session_id);
        if ws_id == NONE {
            self.data.assign.remove(session_id);
        } else {
            self.data
                .assign
                .insert(session_id.to_string(), ws_id.to_string());
        }
        self.save()
    }

    /// An empty `name` removes the custom name.
    pub(super) fn rename_chat(&mut self, session_id: &str, name: &str) -> io::Result<()> {
        if name.is_empty() {
            self.data.names.remove(session_id);
        } else {
            self.data
                .names
                .insert(session_id.to_string(), name.to_string());
        }
        self.save()
    }

    pub(super) fn display_title<'a>(&'a self, session: &'a Session) -> &'a str {
        self.data
            .names
            .get(&session.id)
            .map_or(session.title.as_str(), String::as_str)
    }

    /// Chats per workspace id, `NONE`, `ALL` and `TRASH`, counting only chats
    /// that still exist. Deleted chats count only under `TRASH` (and under the
    /// deleted workspace they went with).
    pub(super) fn counts(&self, live: &HashSet<String>) -> BTreeMap<String, usize> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for id in live.iter().filter(|id| !self.is_purged(id)) {
            match self.data.trash.get(id) {
                Some(reason) => {
                    *counts.entry(TRASH.to_string()).or_default() += 1;
                    if !reason.is_empty() {
                        *counts.entry(reason.clone()).or_default() += 1;
                    }
                }
                None => {
                    *counts.entry(ALL.to_string()).or_default() += 1;
                    *counts.entry(self.ws_of(id).to_string()).or_default() += 1;
                }
            }
        }
        counts
    }
}

#[cfg_attr(test, allow(dead_code))]
fn state_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("COVE_STATE_FILE") {
        return Some(PathBuf::from(path));
    }
    state_base().map(|dir| dir.join(APP_NAME).join("state.json"))
}

/// Before the rebrand the app was called `frw` and kept its state there.
#[cfg_attr(test, allow(dead_code))]
fn legacy_state_path() -> Option<PathBuf> {
    state_base().map(|dir| dir.join("frw").join("state.json"))
}

#[cfg_attr(test, allow(dead_code))]
fn state_base() -> Option<PathBuf> {
    dirs::state_dir().or_else(|| dirs::home_dir().map(|home| home.join(".local/state")))
}

fn read_state(path: &std::path::Path) -> Option<StateFile> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Single-line text editor used for renaming chats and workspaces.
#[derive(Debug, Clone)]
pub(super) struct LineEdit {
    pub(super) text: String,
    /// Cursor position in characters.
    pub(super) cursor: usize,
}

impl LineEdit {
    pub(super) fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.chars().count(),
        }
    }

    fn byte(&self, char_idx: usize) -> usize {
        super::text::char_to_byte_idx(&self.text, char_idx)
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub(super) fn insert(&mut self, ch: char) {
        let at = self.byte(self.cursor);
        self.text.insert(at, ch);
        self.cursor += 1;
    }

    fn delete_range(&mut self, start: usize, end: usize) {
        let end = end.min(self.len());
        if start >= end {
            return;
        }
        let (a, b) = (self.byte(start), self.byte(end));
        self.text.replace_range(a..b, "");
        self.cursor = start;
    }

    pub(super) fn backspace(&mut self) {
        self.delete_range(self.cursor.saturating_sub(1), self.cursor);
    }

    pub(super) fn delete(&mut self) {
        self.delete_range(self.cursor, self.cursor + 1);
    }

    pub(super) fn delete_to_start(&mut self) {
        self.delete_range(0, self.cursor);
    }

    pub(super) fn delete_word(&mut self) {
        let chars: Vec<char> = self.text.chars().collect();
        let mut start = self.cursor.min(chars.len());
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        self.delete_range(start, self.cursor);
    }

    pub(super) fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(super) fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.len());
    }

    pub(super) fn home(&mut self) {
        self.cursor = 0;
    }

    pub(super) fn end(&mut self) {
        self.cursor = self.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Workspaces {
        Workspaces::in_memory()
    }

    #[test]
    fn workspaces_assign_rename_and_delete() {
        let mut ws = store();
        let work = ws.add("Work").unwrap();
        let play = ws.add("Play").unwrap();
        assert_ne!(work, play);

        ws.move_session("s1", &work).unwrap();
        ws.move_session("s2", &work).unwrap();
        ws.move_session("s3", &play).unwrap();
        assert_eq!(ws.ws_of("s1"), work);
        assert_eq!(ws.ws_of("nope"), NONE);

        let live: HashSet<String> = ["s1", "s2", "s3", "s4"].map(String::from).into();
        let counts = ws.counts(&live);
        assert_eq!(counts[ALL], 4);
        assert_eq!(counts[&work], 2);
        assert_eq!(counts[&play], 1);
        assert_eq!(counts[NONE], 1);

        ws.rename(&work, "Job").unwrap();
        assert_eq!(ws.name_of(&work), Some("Job"));

        ws.set_scope(&work).unwrap();
        ws.delete(&work).unwrap();
        assert_eq!(ws.ws_of("s1"), NONE, "a deleted workspace has no members");
        assert_eq!(ws.scope(), ALL, "deleting the open workspace resets scope");
    }

    #[test]
    fn deleting_and_restoring_a_chat() {
        let mut ws = store();
        let work = ws.add("Work").unwrap();
        ws.move_session("s1", &work).unwrap();
        let live: HashSet<String> = ["s1", "s2"].map(String::from).into();

        ws.trash_chat("s1").unwrap();
        assert!(ws.is_trashed("s1"));
        let counts = ws.counts(&live);
        assert_eq!(counts[TRASH], 1);
        assert_eq!(counts[ALL], 1, "deleted chats leave All");
        assert_eq!(counts.get(&work), None, "and their workspace");

        ws.restore_chat("s1").unwrap();
        assert_eq!(ws.ws_of("s1"), work, "restored to where it was");
        assert!(!ws.is_trashed("s1"));
        assert_eq!(ws.counts(&live)[&work], 1);
    }

    #[test]
    fn deleted_workspace_takes_its_chats_along_and_restores_them() {
        let mut ws = store();
        let work = ws.add("Work").unwrap();
        ws.move_session("s1", &work).unwrap();
        ws.move_session("s2", &work).unwrap();
        ws.trash_chat("s2").unwrap(); // deleted on its own first
        ws.delete(&work).unwrap();

        assert!(ws.name_of(&work).is_none());
        assert_eq!(ws.deleted_name_of(&work), Some("Work"));
        assert!(ws.is_trashed("s1") && ws.is_trashed("s2"));
        assert_eq!(ws.trashed_with("s1"), Some(work.as_str()));
        assert_eq!(ws.trashed_with("s2"), None, "s2 was deleted by itself");

        ws.restore_workspace(&work).unwrap();
        assert_eq!(ws.name_of(&work), Some("Work"));
        assert!(!ws.is_trashed("s1"), "came back with its workspace");
        assert!(
            ws.is_trashed("s2"),
            "individually deleted chat stays deleted"
        );
        assert_eq!(ws.ws_of("s1"), work);
    }

    #[test]
    fn restoring_a_chat_of_a_deleted_workspace_lands_in_unsorted() {
        let mut ws = store();
        let work = ws.add("Work").unwrap();
        ws.move_session("s1", &work).unwrap();
        ws.delete(&work).unwrap();
        ws.restore_chat("s1").unwrap();
        assert_eq!(ws.ws_of("s1"), NONE);
    }

    #[test]
    fn moving_a_deleted_chat_restores_it_there() {
        let mut ws = store();
        let work = ws.add("Work").unwrap();
        ws.trash_chat("s1").unwrap();
        ws.move_session("s1", &work).unwrap();
        assert!(!ws.is_trashed("s1"));
        assert_eq!(ws.ws_of("s1"), work);
    }

    #[test]
    fn custom_names_override_and_reset() {
        let mut ws = store();
        let session = Session::new(
            "s1",
            "claude",
            "Original",
            "/tmp",
            chrono::Local::now(),
            "",
            1,
        );
        assert_eq!(ws.display_title(&session), "Original");
        ws.rename_chat("s1", "Mine").unwrap();
        assert_eq!(ws.display_title(&session), "Mine");
        ws.rename_chat("s1", "").unwrap();
        assert_eq!(ws.display_title(&session), "Original");
    }

    #[test]
    fn state_round_trips_and_reads_python_version_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            r#"{"workspaces":[{"id":"ab12","name":"Work"}],"assign":{"s1":"ab12"},"names":{"s1":"Mine"},"scope":"ab12"}"#,
        )
        .unwrap();
        let mut ws = Workspaces {
            path: Some(path.clone()),
            data: serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap(),
        };
        assert_eq!(ws.scope(), "ab12");
        assert_eq!(ws.ws_of("s1"), "ab12");
        ws.move_session("s2", "ab12").unwrap();
        let reread: StateFile =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(reread.assign.len(), 2);
    }

    #[test]
    fn line_edit_handles_unicode_and_word_delete() {
        let mut edit = LineEdit::new("héllo wörld");
        edit.delete_word();
        assert_eq!(edit.text, "héllo ");
        edit.home();
        edit.delete();
        assert_eq!(edit.text, "éllo ");
        edit.end();
        edit.insert('!');
        edit.left();
        edit.backspace();
        assert_eq!(edit.text, "éllo!");
    }

    #[test]
    fn purging_a_chat_hides_it_everywhere_for_good() {
        let mut ws = store();
        let work = ws.add("Work").unwrap();
        ws.move_session("s1", &work).unwrap();
        ws.rename_chat("s1", "Mine").unwrap();
        ws.trash_chat("s1").unwrap();
        let live: HashSet<String> = ["s1", "s2"].map(String::from).into();

        ws.purge_chat("s1").unwrap();
        assert!(ws.is_purged("s1"));
        assert!(!ws.is_trashed("s1"), "no longer even in Deleted");
        let counts = ws.counts(&live);
        assert_eq!(counts.get(TRASH), None);
        assert_eq!(counts[ALL], 1, "only s2 is left");
        assert_eq!(ws.ws_of("s1"), NONE);
    }

    #[test]
    fn purging_a_workspace_takes_its_own_chats_but_not_separately_deleted_ones() {
        let mut ws = store();
        let work = ws.add("Work").unwrap();
        let play = ws.add("Play").unwrap();
        ws.move_session("s1", &work).unwrap();
        ws.move_session("s2", &work).unwrap();
        ws.trash_chat("s2").unwrap(); // deleted on its own earlier
        ws.delete(&work).unwrap();

        ws.purge_workspace(&work).unwrap();
        assert!(ws.is_purged("s1"), "deleted with the workspace, so erased");
        assert!(!ws.is_purged("s2"));
        assert!(ws.is_trashed("s2"), "still in Deleted");
        assert!(ws.deleted_name_of(&work).is_none(), "gone from Deleted too");
        assert!(ws.name_of(&work).is_none());
        assert_eq!(ws.color_index(&play), Some(1), "colors do not shift");
        assert_eq!(ws.scope(), ALL);
    }

    #[test]
    fn purged_state_round_trips_and_old_files_still_load() {
        let old: StateFile = serde_json::from_str(
            r#"{"workspaces":[{"id":"a1","name":"W","deleted":true}],"trash":{"s":"a1"}}"#,
        )
        .unwrap();
        assert!(old.purged.is_empty());
        assert!(!old.workspaces[0].purged);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut ws = Workspaces {
            path: Some(path.clone()),
            data: StateFile::default(),
        };
        ws.purge_chat("gone").unwrap();
        let again: StateFile =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(again.purged.contains("gone"));
    }

    #[test]
    fn state_from_the_old_frw_folder_moves_to_the_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("frw/state.json");
        let new = dir.path().join("cove/state.json");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(
            &old,
            r#"{"workspaces":[{"id":"ab12","name":"Cheezious"}],"assign":{"s1":"ab12"},"scope":"ab12"}"#,
        )
        .unwrap();

        let ws = Workspaces::load_from(Some(new.clone()), Some(old.clone()));
        assert_eq!(ws.name_of("ab12"), Some("Cheezious"));
        assert_eq!(ws.scope(), "ab12");
        assert!(new.exists(), "saved to the new place");
        assert!(old.exists(), "the old file is left alone");

        // From now on the new file wins, even if the old one changes.
        std::fs::write(&old, r#"{"workspaces":[]}"#).unwrap();
        let again = Workspaces::load_from(Some(new), Some(old));
        assert_eq!(again.name_of("ab12"), Some("Cheezious"));
    }

    #[test]
    fn nothing_to_migrate_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspaces::load_from(
            Some(dir.path().join("cove/state.json")),
            Some(dir.path().join("frw/state.json")),
        );
        assert!(ws.list().next().is_none());
        assert!(
            !dir.path().join("cove/state.json").exists(),
            "nothing written"
        );
    }
}
