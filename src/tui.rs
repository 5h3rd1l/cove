use std::io::{self, Stdout};
use std::panic;
use std::process::Command;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, KeyboardEnhancementFlags,
    MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

use crate::index::{INDEX_REFRESH_BATCH_SIZE, SessionIndex};
use crate::model::Session;
use crate::search::SearchEngine;

mod images;
mod input;
mod layout;
mod mouse;
mod preview;
mod render;
mod state;
mod text;
mod theme;
mod workspaces;

use images::AgentImages;
use input::handle_key;
use layout::ScrollTarget;
use render::draw;
use state::{AppState, ScanMessage, ScratchRequest, SearchRequest, handle_scan_message};

pub use images::ImageProtocol;
pub use theme::ThemeMode;

pub enum TuiExit {
    Quit,
    Resume {
        command: Vec<String>,
        directory: String,
    },
}

pub fn run_tui(
    query: String,
    agent_filter: Option<String>,
    directory_filter: Option<String>,
    yolo: bool,
    image_protocol: Option<ImageProtocol>,
    theme_mode: ThemeMode,
) -> Result<TuiExit> {
    let theme = theme_mode.resolve();
    let engine = SearchEngine::open_default()?;
    let (scan_tx, scan_rx) = mpsc::channel();
    thread::spawn(move || {
        let start = Instant::now();
        let progress_tx = scan_tx.clone();
        let refreshed = SessionIndex::open_default().and_then(|index| {
            index.refresh_incremental_streaming(INDEX_REFRESH_BATCH_SIZE, |summary| {
                let _ = progress_tx.send(ScanMessage::Progress {
                    elapsed: start.elapsed(),
                    new_or_modified: summary.new_or_modified,
                    deleted: summary.deleted,
                    total: summary.sessions,
                });
            })
        });
        let message = match refreshed {
            Ok(summary) => ScanMessage::Finished {
                elapsed: start.elapsed(),
                new_or_modified: summary.new_or_modified,
                deleted: summary.deleted,
                total: summary.sessions,
            },
            Err(error) => ScanMessage::Failed {
                elapsed: start.elapsed(),
                error: format!("{error:#}"),
            },
        };
        let _ = scan_tx.send(message);
    });

    install_panic_hook();
    let mut terminal = setup_terminal()?;
    let images = image_protocol.and_then(AgentImages::load);
    let mut state = AppState::new(
        query,
        agent_filter,
        directory_filter,
        yolo,
        engine,
        images,
        theme,
    );
    let result = run_loop(&mut terminal, &mut state, scan_rx);
    restore_terminal(&mut terminal)?;
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
    scan_rx: Receiver<ScanMessage>,
) -> Result<TuiExit> {
    let (search_tx, search_rx) = spawn_search_worker(state.engine.clone());
    let mut needs_draw = true;
    let mut overlay_was_open = false;
    loop {
        let mut latest_scan_message = None;
        while let Ok(message) = scan_rx.try_recv() {
            let finished = matches!(
                message,
                ScanMessage::Finished { .. } | ScanMessage::Failed { .. }
            );
            latest_scan_message = Some(message);
            if finished {
                break;
            }
        }
        if let Some(message) = latest_scan_message {
            handle_scan_message(state, message);
            start_search_if_requested(state, &search_tx);
            needs_draw = true;
        }

        while let Ok(result) = search_rx.try_recv() {
            let applied = match result.visible {
                Ok(visible) => state.apply_search_result(
                    result.generation,
                    visible,
                    result.elapsed_ms,
                    result.preserve_selection.as_ref(),
                ),
                Err(error) => state.apply_search_error(result.generation, &error),
            };
            if applied {
                needs_draw = true;
            }
        }

        if needs_draw {
            // Agent logos are terminal images whose cells confuse the frame
            // diff, so a dialog opening or closing repaints everything.
            if state.overlay_open() != overlay_was_open {
                overlay_was_open = state.overlay_open();
                terminal.clear()?;
            }
            terminal.draw(|frame| draw(frame, state))?;
            needs_draw = false;
        }

        if event::poll(Duration::from_millis(24))? {
            match event::read()? {
                // Terminals using the Kitty keyboard protocol also emit
                // Release/Repeat events; acting on them doubles keystrokes.
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if let Some(exit) = handle_key(state, key)? {
                        return Ok(exit);
                    }
                    start_search_if_requested(state, &search_tx);
                    if let Some(request) = state.take_scratch_request() {
                        run_scratch(terminal, state, request)?;
                        overlay_was_open = state.overlay_open();
                    }
                    needs_draw = true;
                }
                Event::Resize(_, _) => {
                    terminal.autoresize()?;
                    needs_draw = true;
                }
                Event::Mouse(mouse) => {
                    let size = terminal.size()?;
                    let area = Rect::new(0, 0, size.width, size.height);
                    if handle_mouse(state, mouse, area) {
                        needs_draw = true;
                    }
                    start_search_if_requested(state, &search_tx);
                }
                _ => {}
            }
        }
    }
}

/// Runs a scratch chat (`Ctrl+I`): leaves Cove's screen entirely, runs the
/// agent with the terminal it would normally get, waits for it to exit, then
/// comes back to Cove exactly where the user left it.
fn run_scratch(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
    request: ScratchRequest,
) -> Result<()> {
    suspend_terminal()?;

    let mut command = Command::new(&request.command[0]);
    command.args(&request.command[1..]);
    if !request.directory.is_empty() {
        command.current_dir(&request.directory);
    }
    let outcome = command.status();

    resume_terminal(terminal)?;

    match outcome {
        Ok(_status) => state.finish_scratch(request),
        Err(error) => {
            state.status = format!("could not start {}: {error}", request.command[0]);
        }
    }
    Ok(())
}

/// Leaves Cove's screen so a child process can use the terminal normally,
/// the same modes `restore_terminal` releases on exit.
fn suspend_terminal() -> Result<()> {
    disable_raw_mode()?;
    let mut stdout = io::stdout();
    if KEYBOARD_ENHANCEMENT.load(Ordering::Relaxed) {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    execute!(
        stdout,
        DisableMouseCapture,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    Ok(())
}

/// The other half of `suspend_terminal`: back to the modes `setup_terminal`
/// establishes, plus a full repaint, since whatever ran in between could
/// have left anything on screen.
fn resume_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        SetTitle(crate::config::APP_NAME)
    )?;
    if KEYBOARD_ENHANCEMENT.load(Ordering::Relaxed) {
        let _ = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    execute!(stdout, EnableMouseCapture)?;
    terminal.clear()?;
    Ok(())
}

struct SearchResult {
    generation: u64,
    visible: std::result::Result<Vec<Session>, String>,
    elapsed_ms: f64,
    preserve_selection: Option<(String, String)>,
}

fn start_search_if_requested(state: &mut AppState, tx: &Sender<SearchRequest>) {
    let Some(request) = state.take_search_request() else {
        return;
    };
    let _ = tx.send(request);
}

fn spawn_search_worker(engine: SearchEngine) -> (Sender<SearchRequest>, Receiver<SearchResult>) {
    let (request_tx, request_rx) = mpsc::channel::<SearchRequest>();
    let (result_tx, result_rx) = mpsc::channel::<SearchResult>();
    thread::spawn(move || {
        let mut engine = engine;
        while let Ok(request) = request_rx.recv() {
            let result = run_search(&mut engine, latest_search_request(&request_rx, request));
            if result_tx.send(result).is_err() {
                break;
            }
        }
    });
    (request_tx, result_rx)
}

fn latest_search_request(
    request_rx: &Receiver<SearchRequest>,
    mut request: SearchRequest,
) -> SearchRequest {
    let mut reload_index = request.reload_index;
    while let Ok(latest) = request_rx.try_recv() {
        reload_index |= latest.reload_index;
        request = latest;
    }
    request.reload_index = reload_index;
    request
}

fn run_search(engine: &mut SearchEngine, request: SearchRequest) -> SearchResult {
    let start = Instant::now();
    let visible = (|| {
        if request.reload_index {
            engine.reload()?;
        }
        engine.search_result(
            &request.query,
            request.agent_filter.as_deref(),
            request.directory_filter.as_deref(),
            request.limit,
        )
    })()
    .map_err(|error| format!("{error:#}"));
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    SearchResult {
        generation: request.generation,
        visible,
        elapsed_ms,
        preserve_selection: request.preserve_selection,
    }
}

const MOUSE_SCROLL_LINES: isize = 3;

fn handle_mouse(state: &mut AppState, mouse: MouseEvent, area: Rect) -> bool {
    if state.modal.is_some() || state.show_help {
        return false;
    }

    let delta = match mouse.kind {
        MouseEventKind::ScrollUp => -MOUSE_SCROLL_LINES,
        MouseEventKind::ScrollDown => MOUSE_SCROLL_LINES,
        _ => return mouse::handle_pointer(state, mouse, area),
    };
    if state.picker.is_some() || state.confirm.is_some() {
        return false;
    }

    match layout::scroll_target(
        area,
        state.show_preview,
        state.show_sidebar,
        mouse.column,
        mouse.row,
    ) {
        Some(ScrollTarget::Results) => {
            state.move_selection(delta);
            true
        }
        Some(ScrollTarget::Preview) => {
            state.scroll_preview(delta);
            true
        }
        None => false,
    }
}

/// Whether the terminal accepted the Kitty keyboard protocol at startup.
/// Set once in `setup_terminal`; read wherever the terminal is left or
/// re-entered, since it decides whether to push or pop the enhancement
/// flags there too. Without it, `Ctrl+I` is indistinguishable from a bare
/// Tab on most terminals (see `input.rs`).
static KEYBOARD_ENHANCEMENT: AtomicBool = AtomicBool::new(false);

/// Restore the terminal before the default panic handler runs. Without this,
/// the panic message prints into the alternate screen and is erased, and the
/// shell is left in raw mode.
fn install_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let original = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            restore_terminal_modes();
            original(info);
        }));
    });
}

fn restore_terminal_modes() {
    let mut stdout = io::stdout();
    if KEYBOARD_ENHANCEMENT.load(Ordering::Relaxed) {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        stdout,
        DisableMouseCapture,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = disable_raw_mode();
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    let mut guard = TerminalSetupGuard::default();
    enable_raw_mode()?;
    guard.raw_mode = true;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        SetTitle(crate::config::APP_NAME)
    )?;
    guard.alternate_screen = true;
    // Best-effort: most terminals lack this, and Ctrl+I just behaves like
    // Tab there instead (see input.rs). `supports_keyboard_enhancement`
    // queries the terminal and can fail on an unusual one; treat that the
    // same as "unsupported" rather than failing startup over it.
    if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        KEYBOARD_ENHANCEMENT.store(true, Ordering::Relaxed);
        guard.keyboard_enhancement = true;
    }
    execute!(stdout, EnableMouseCapture)?;
    guard.mouse_capture = true;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    guard.disarm();
    Ok(terminal)
}

#[derive(Default)]
struct TerminalSetupGuard {
    raw_mode: bool,
    alternate_screen: bool,
    keyboard_enhancement: bool,
    mouse_capture: bool,
}

impl TerminalSetupGuard {
    fn disarm(&mut self) {
        self.raw_mode = false;
        self.alternate_screen = false;
        self.keyboard_enhancement = false;
        self.mouse_capture = false;
    }
}

impl Drop for TerminalSetupGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        if self.mouse_capture {
            let _ = execute!(stdout, DisableMouseCapture);
        }
        if self.keyboard_enhancement {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        }
        if self.alternate_screen {
            let _ = execute!(stdout, LeaveAlternateScreen);
        }
        if self.raw_mode {
            let _ = disable_raw_mode();
        }
    }
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    if KEYBOARD_ENHANCEMENT.load(Ordering::Relaxed) {
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags)?;
    }
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, Local};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::layout::Rect;
    use tempfile::tempdir;

    use crate::index::SessionIndex;
    use crate::model::Session;
    use crate::search::SearchEngine;

    use super::input::handle_key;
    use super::state::{AppState, SearchRequest};
    use super::theme::Theme;
    use super::{layout, render};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn search_request(generation: u64, query: &str) -> SearchRequest {
        SearchRequest {
            generation,
            query: query.to_string(),
            agent_filter: None,
            directory_filter: None,
            preserve_selection: None,
            reload_index: false,
            limit: 100,
        }
    }

    fn session(id: &str) -> Session {
        session_in(id, "/tmp/fast-resume")
    }

    fn session_in(id: &str, directory: &str) -> Session {
        session_for_agent(id, "codex", directory)
    }

    fn session_for_agent(id: &str, agent: &str, directory: &str) -> Session {
        Session::new(
            id,
            agent,
            format!("Session {id}"),
            directory,
            Local::now(),
            "message",
            1,
        )
    }

    fn test_state(sessions: Vec<Session>) -> AppState {
        test_state_with_directory_filter(sessions, None)
    }

    #[test]
    fn preview_cache_refreshes_when_session_content_changes() {
        let state = test_state(Vec::new());
        let mut session = session("preview-1");
        session.content = "first version of the content".to_string();
        session.mtime = 1.0;

        let first = state.preview_lines(&session);
        let cached = state.preview_lines(&session);
        assert_eq!(format!("{first:?}"), format!("{cached:?}"));

        session.content = "second version of the content".to_string();
        session.mtime = 2.0;
        let refreshed = state.preview_lines(&session);
        assert!(format!("{refreshed:?}").contains("second version"));
    }

    #[test]
    fn pending_search_requests_coalesce_to_latest() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(search_request(2, "second")).unwrap();
        tx.send(search_request(3, "third")).unwrap();

        let latest = super::latest_search_request(&rx, search_request(1, "first"));

        assert_eq!(latest.generation, 3);
        assert_eq!(latest.query, "third");
    }

    #[test]
    fn coalesced_search_requests_preserve_reload() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut refresh = search_request(2, "second");
        refresh.reload_index = true;
        tx.send(refresh).unwrap();
        tx.send(search_request(3, "third")).unwrap();

        let latest = super::latest_search_request(&rx, search_request(1, "first"));

        assert_eq!(latest.generation, 3);
        assert_eq!(latest.query, "third");
        assert!(latest.reload_index);
    }

    fn test_state_with_directory_filter(
        sessions: Vec<Session>,
        directory_filter: Option<String>,
    ) -> AppState {
        test_state_and_index(sessions, directory_filter).0
    }

    fn test_state_and_index(
        sessions: Vec<Session>,
        directory_filter: Option<String>,
    ) -> (AppState, SessionIndex) {
        let temp = tempdir().unwrap();
        let path = temp.keep();
        let index = SessionIndex::open(path.join("index")).unwrap();
        index.rebuild(sessions).unwrap();
        let mut state = AppState::new(
            String::new(),
            None,
            directory_filter,
            false,
            SearchEngine::from_index(index.clone()),
            None,
            Theme::dark(),
        );
        // The mouse tests below assume the two-pane geometry; sidebar tests
        // turn it back on.
        state.show_sidebar = false;
        (state, index)
    }

    fn type_query(state: &mut AppState, query: &str) {
        for ch in query.chars() {
            handle_key(state, key(KeyCode::Char(ch), KeyModifiers::NONE)).unwrap();
        }
    }

    #[test]
    fn plain_j_and_k_type_into_search() {
        let mut state = test_state(Vec::new());

        handle_key(&mut state, key(KeyCode::Char('j'), KeyModifiers::NONE)).unwrap();
        handle_key(&mut state, key(KeyCode::Char('k'), KeyModifiers::NONE)).unwrap();

        assert_eq!(state.query, "jk");
        assert_eq!(state.cursor, 2);
    }

    #[test]
    fn plus_and_minus_type_into_search() {
        let mut state = test_state(Vec::new());

        handle_key(&mut state, key(KeyCode::Char('-'), KeyModifiers::NONE)).unwrap();
        handle_key(&mut state, key(KeyCode::Char('+'), KeyModifiers::SHIFT)).unwrap();

        assert_eq!(state.query, "-+");
        assert_eq!(state.cursor, 2);
    }

    #[test]
    fn ctrl_j_and_ctrl_k_keep_navigation_shortcuts() {
        let mut state = test_state(vec![session("a"), session("b")]);

        handle_key(&mut state, key(KeyCode::Char('j'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(state.selected, 1);
        assert!(state.query.is_empty());

        handle_key(&mut state, key(KeyCode::Char('k'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(state.selected, 0);
        assert!(state.query.is_empty());
    }

    #[test]
    fn f1_opens_help_and_suspends_search_input() {
        let mut state = test_state(Vec::new());

        handle_key(&mut state, key(KeyCode::F(1), KeyModifiers::NONE)).unwrap();
        assert!(state.show_help);

        handle_key(&mut state, key(KeyCode::Char('z'), KeyModifiers::NONE)).unwrap();
        assert!(state.query.is_empty());

        handle_key(&mut state, key(KeyCode::F(1), KeyModifiers::NONE)).unwrap();
        assert!(!state.show_help);
    }

    #[test]
    fn emacs_keys_move_the_search_cursor() {
        let mut state = test_state(Vec::new());
        type_query(&mut state, "alpha βeta");
        let end = state.query.chars().count();

        handle_key(&mut state, key(KeyCode::Char('a'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(state.cursor, 0);

        handle_key(&mut state, key(KeyCode::Char('f'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(state.cursor, 1);

        handle_key(&mut state, key(KeyCode::Char('b'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(state.cursor, 0);

        handle_key(&mut state, key(KeyCode::Char('e'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(state.cursor, end);
    }

    #[test]
    fn emacs_keys_delete_search_text() {
        let mut state = test_state(Vec::new());
        type_query(&mut state, "aβc");
        state.cursor = 1;

        handle_key(&mut state, key(KeyCode::Char('d'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(state.query, "ac");
        assert_eq!(state.cursor, 1);

        state.cursor = state.query.chars().count();
        type_query(&mut state, " beta  ");
        handle_key(&mut state, key(KeyCode::Char('w'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(state.query, "ac ");

        handle_key(&mut state, key(KeyCode::Char('u'), KeyModifiers::CONTROL)).unwrap();
        assert!(state.query.is_empty());
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn alt_plus_and_minus_scroll_preview() {
        let mut state = test_state(Vec::new());
        state.preview_scroll = 3;

        handle_key(
            &mut state,
            key(KeyCode::Char('+'), KeyModifiers::ALT | KeyModifiers::SHIFT),
        )
        .unwrap();
        assert_eq!(state.preview_scroll, 0);

        handle_key(&mut state, key(KeyCode::Char('-'), KeyModifiers::ALT)).unwrap();
        assert_eq!(state.preview_scroll, 3);
        assert!(state.query.is_empty());
    }

    #[test]
    fn tab_accepts_agent_suggestion_before_cycling_filter() {
        let mut state = test_state(Vec::new());

        type_query(&mut state, "agent:c");
        assert_eq!(state.suggestion_suffix().as_deref(), Some("laude"));

        handle_key(&mut state, key(KeyCode::Tab, KeyModifiers::NONE)).unwrap();

        assert_eq!(state.query, "agent:claude");
        assert_eq!(state.cursor, "agent:claude".chars().count());
        assert_eq!(state.active_agent_filter().as_deref(), Some("claude"));
    }

    #[test]
    fn tab_accepts_date_suggestion() {
        let mut state = test_state(Vec::new());

        type_query(&mut state, "date:y");

        assert_eq!(state.suggestion_suffix().as_deref(), Some("esterday"));
        handle_key(&mut state, key(KeyCode::Tab, KeyModifiers::NONE)).unwrap();

        assert_eq!(state.query, "date:yesterday");
    }

    #[test]
    fn filter_tabs_and_cycle_include_only_agents_with_sessions() {
        let mut state = test_state(vec![
            session_for_agent("codex-1", "codex", "/tmp/codex"),
            session_for_agent("claude-1", "claude", "/tmp/claude"),
            session_for_agent("codex-2", "codex", "/tmp/codex"),
        ]);

        assert_eq!(
            state.agent_filters_with_sessions(),
            vec![("claude", 1), ("codex", 2)]
        );

        handle_key(&mut state, key(KeyCode::Tab, KeyModifiers::NONE)).unwrap();

        assert_eq!(state.query, "agent:claude");
        assert_eq!(state.active_agent_filter().as_deref(), Some("claude"));
        let request = state.take_search_request().unwrap();
        assert_eq!(request.query, "agent:claude");
        assert_eq!(request.agent_filter, None);
        assert_eq!(request.directory_filter, None);
    }

    #[test]
    fn filter_cycle_stays_on_all_when_no_agent_has_sessions() {
        let mut state = test_state(Vec::new());

        handle_key(&mut state, key(KeyCode::Tab, KeyModifiers::NONE)).unwrap();

        assert!(state.agent_filters_with_sessions().is_empty());
        assert!(state.query.is_empty());
        assert!(state.all_agent_filter_active());
    }

    #[test]
    fn deleting_cycled_filter_keyword_clears_filter() {
        let mut state = test_state(vec![session("codex-1")]);

        handle_key(&mut state, key(KeyCode::Tab, KeyModifiers::NONE)).unwrap();
        let query_len = state.query.chars().count();
        for _ in 0..query_len {
            handle_key(&mut state, key(KeyCode::Backspace, KeyModifiers::NONE)).unwrap();
        }

        assert!(state.query.is_empty());
        assert_eq!(state.active_agent_filter(), None);
        let request = state.take_search_request().unwrap();
        assert_eq!(request.agent_filter, None);
        assert_eq!(request.directory_filter, None);
    }

    #[test]
    fn reverse_filter_cycle_removes_agent_keyword_for_all() {
        let mut state = test_state(Vec::new());

        type_query(&mut state, "api agent:antigravity");
        handle_key(&mut state, key(KeyCode::BackTab, KeyModifiers::SHIFT)).unwrap();

        assert_eq!(state.query, "api");
        assert_eq!(state.active_agent_filter(), None);
    }

    #[test]
    fn typed_agent_keyword_syncs_filter_without_overriding_query_filter() {
        let mut state = test_state(Vec::new());

        type_query(&mut state, "agent:claude,codex");

        assert_eq!(state.active_agent_filter(), None);
        assert_eq!(
            state.active_agent_filters(),
            vec!["claude".to_string(), "codex".to_string()]
        );
        assert!(!state.all_agent_filter_active());
        let request = state.take_search_request().unwrap();
        assert_eq!(request.query, "agent:claude,codex");
        assert_eq!(request.agent_filter, None);
        assert_eq!(request.directory_filter, None);
    }

    #[test]
    fn negated_agent_keyword_does_not_activate_all_filter() {
        let mut state = test_state(Vec::new());

        type_query(&mut state, "-agent:claude");

        assert!(state.active_agent_filters().is_empty());
        assert!(!state.all_agent_filter_active());
        let request = state.take_search_request().unwrap();
        assert_eq!(request.query, "-agent:claude");
        assert_eq!(request.agent_filter, None);
        assert_eq!(request.directory_filter, None);
    }

    #[test]
    fn cli_directory_filter_limits_tui_search_until_query_overrides_it() {
        let mut state = test_state_with_directory_filter(
            vec![
                session_in("backend", "/work/backend"),
                session_in("frontend", "/work/frontend"),
            ],
            Some("backend".to_string()),
        );

        assert_eq!(state.visible.len(), 1);
        assert_eq!(state.visible[0].id, "backend");

        handle_key(&mut state, key(KeyCode::Char('a'), KeyModifiers::NONE)).unwrap();
        let request = state.take_search_request().unwrap();
        assert_eq!(request.directory_filter.as_deref(), Some("backend"));

        state.query = "dir:frontend".to_string();
        state.cursor = state.query.chars().count();
        state.refresh_search();

        assert_eq!(state.visible.len(), 1);
        assert_eq!(state.visible[0].id, "frontend");
    }

    #[test]
    fn mouse_wheel_over_results_moves_selection() {
        let mut state = test_state((0..10).map(|idx| session(&idx.to_string())).collect());

        assert!(super::handle_mouse(
            &mut state,
            mouse(MouseEventKind::ScrollDown, 10, 6),
            Rect::new(0, 0, 120, 40),
        ));

        assert_eq!(state.selected, 3);
        assert_eq!(state.preview_scroll, 0);
    }

    #[test]
    fn mouse_wheel_over_preview_scrolls_preview() {
        let mut state = test_state(vec![session("a")]);

        assert!(super::handle_mouse(
            &mut state,
            mouse(MouseEventKind::ScrollDown, 100, 6),
            Rect::new(0, 0, 120, 40),
        ));

        assert_eq!(state.selected, 0);
        assert_eq!(state.preview_scroll, 3);
    }

    #[test]
    fn mouse_wheel_outside_main_area_is_ignored() {
        let mut state = test_state(vec![session("a")]);

        assert!(!super::handle_mouse(
            &mut state,
            mouse(MouseEventKind::ScrollDown, 10, 1),
            Rect::new(0, 0, 120, 40),
        ));

        assert_eq!(state.selected, 0);
        assert_eq!(state.preview_scroll, 0);
    }

    #[test]
    fn mouse_wheel_is_ignored_while_modal_is_open() {
        let mut state = test_state((0..10).map(|idx| session(&idx.to_string())).collect());
        handle_key(&mut state, key(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert!(state.modal.is_some());

        assert!(!super::handle_mouse(
            &mut state,
            mouse(MouseEventKind::ScrollDown, 10, 6),
            Rect::new(0, 0, 120, 40),
        ));

        assert_eq!(state.selected, 0);
        assert_eq!(state.preview_scroll, 0);
    }

    #[test]
    fn typing_requests_search_without_blocking_visible_results() {
        let mut state = test_state(vec![session("a")]);

        handle_key(&mut state, key(KeyCode::Char('z'), KeyModifiers::NONE)).unwrap();

        assert_eq!(state.query, "z");
        assert_eq!(state.visible.len(), 1);
        let request = state.take_search_request().unwrap();
        assert_eq!(request.query, "z");
        assert_eq!(request.agent_filter, None);
        assert_eq!(request.directory_filter, None);
        assert!(state.take_search_request().is_none());
    }

    #[test]
    fn stale_search_results_are_ignored() {
        let mut state = test_state(vec![session("a")]);

        handle_key(&mut state, key(KeyCode::Char('a'), KeyModifiers::NONE)).unwrap();
        let stale = state.take_search_request().unwrap();
        handle_key(&mut state, key(KeyCode::Char('b'), KeyModifiers::NONE)).unwrap();
        let latest = state.take_search_request().unwrap();

        assert!(!state.apply_search_result(stale.generation, Vec::new(), 10.0, None));
        assert_eq!(state.visible.len(), 1);
        assert!(state.apply_search_result(latest.generation, Vec::new(), 1.0, None));
        assert!(state.visible.is_empty());
    }

    #[test]
    fn new_search_results_reset_selection_to_top() {
        let mut state = test_state(vec![session("a"), session("b"), session("c")]);
        state.move_selection(2);
        assert_eq!(state.selected, 2);

        handle_key(&mut state, key(KeyCode::Char('z'), KeyModifiers::NONE)).unwrap();
        let request = state.take_search_request().unwrap();
        assert!(state.apply_search_result(
            request.generation,
            vec![session("b"), session("c")],
            1.0,
            None
        ));

        assert_eq!(state.selected, 0);
        assert_eq!(state.selected_session().unwrap().id, "b");
    }

    #[test]
    fn background_refresh_preserves_selected_session_identity() {
        let base = Local::now();
        let mut first = session("a");
        first.timestamp = base + ChronoDuration::seconds(2);
        let mut selected = session("b");
        selected.timestamp = base + ChronoDuration::seconds(1);
        let mut last = session("c");
        last.timestamp = base;
        let (mut state, index) =
            test_state_and_index(vec![first.clone(), selected.clone(), last.clone()], None);
        state.selected = 1;
        assert_eq!(state.selected_session().unwrap().id, "b");

        let mut newer = session("newer");
        newer.timestamp = base + ChronoDuration::seconds(3);
        index
            .rebuild(vec![newer, first, selected.clone(), last])
            .unwrap();

        super::state::handle_scan_message(
            &mut state,
            super::state::ScanMessage::Progress {
                elapsed: Duration::ZERO,
                new_or_modified: 1,
                deleted: 0,
                total: 4,
            },
        );

        assert_eq!(state.selected, 1);
        assert_eq!(state.selected_session().unwrap().id, "b");
        let request = state.take_search_request().unwrap();
        assert!(request.reload_index);
        assert_eq!(
            request.preserve_selection.as_ref(),
            Some(&(selected.agent.clone(), selected.id.clone()))
        );
        state.engine.reload().unwrap();
        let visible = state.engine.search(
            &request.query,
            request.agent_filter.as_deref(),
            request.directory_filter.as_deref(),
            100,
        );
        assert!(state.apply_search_result(
            request.generation,
            visible,
            0.0,
            request.preserve_selection.as_ref()
        ));
        assert_eq!(state.selected, 2);
        assert_eq!(state.selected_session().unwrap().id, "b");
    }

    #[test]
    fn repeated_refresh_messages_preserve_selected_session_identity() {
        let base = Local::now();
        let mut first = session("a");
        first.timestamp = base + ChronoDuration::seconds(2);
        let mut selected = session("b");
        selected.timestamp = base + ChronoDuration::seconds(1);
        let mut last = session("c");
        last.timestamp = base;
        let (mut state, index) =
            test_state_and_index(vec![first.clone(), selected.clone(), last.clone()], None);
        state.selected = 1;

        let mut newer = session("newer");
        newer.timestamp = base + ChronoDuration::seconds(3);
        index
            .rebuild(vec![newer, first, selected.clone(), last])
            .unwrap();

        super::state::handle_scan_message(
            &mut state,
            super::state::ScanMessage::Progress {
                elapsed: Duration::ZERO,
                new_or_modified: 1,
                deleted: 0,
                total: 4,
            },
        );
        let first_request = state.take_search_request().unwrap();
        assert_eq!(
            first_request.preserve_selection.as_ref(),
            Some(&(selected.agent.clone(), selected.id.clone()))
        );

        super::state::handle_scan_message(
            &mut state,
            super::state::ScanMessage::Finished {
                elapsed: Duration::ZERO,
                new_or_modified: 1,
                deleted: 0,
                total: 4,
            },
        );
        let second_request = state.take_search_request().unwrap();
        assert_eq!(
            second_request.preserve_selection.as_ref(),
            Some(&(selected.agent.clone(), selected.id.clone()))
        );

        let visible = state.engine.search(
            &second_request.query,
            second_request.agent_filter.as_deref(),
            second_request.directory_filter.as_deref(),
            100,
        );
        assert!(state.apply_search_result(
            second_request.generation,
            visible,
            0.0,
            second_request.preserve_selection.as_ref()
        ));
        assert_eq!(state.selected_session().unwrap().id, "b");
    }

    #[test]
    fn background_refresh_does_not_undo_newer_navigation() {
        let base = Local::now();
        let mut first = session("a");
        first.timestamp = base + ChronoDuration::seconds(2);
        let mut initially_selected = session("b");
        initially_selected.timestamp = base + ChronoDuration::seconds(1);
        let mut newly_selected = session("c");
        newly_selected.timestamp = base;
        let (mut state, index) = test_state_and_index(
            vec![
                first.clone(),
                initially_selected.clone(),
                newly_selected.clone(),
            ],
            None,
        );
        state.selected = 1;

        let mut newer = session("newer");
        newer.timestamp = base + ChronoDuration::seconds(3);
        index
            .rebuild(vec![newer, first, initially_selected, newly_selected])
            .unwrap();
        super::state::handle_scan_message(
            &mut state,
            super::state::ScanMessage::Progress {
                elapsed: Duration::ZERO,
                new_or_modified: 1,
                deleted: 0,
                total: 4,
            },
        );
        let request = state.take_search_request().unwrap();

        state.move_selection(1);
        assert_eq!(state.selected_session().unwrap().id, "c");
        state.engine.reload().unwrap();
        let visible = state.engine.search(
            &request.query,
            request.agent_filter.as_deref(),
            request.directory_filter.as_deref(),
            100,
        );
        assert!(state.apply_search_result(
            request.generation,
            visible,
            0.0,
            request.preserve_selection.as_ref()
        ));

        assert_eq!(state.selected_session().unwrap().id, "c");
    }

    #[test]
    fn background_refresh_does_not_preserve_selection_over_typed_search() {
        let base = Local::now();
        let mut first = session("a");
        first.timestamp = base + ChronoDuration::seconds(2);
        let mut second = session("c");
        second.timestamp = base + ChronoDuration::seconds(1);
        let mut stale_selection = session("stale");
        stale_selection.timestamp = base;
        let mut state = test_state(vec![first, second, stale_selection]);
        state.move_selection(2);
        assert_eq!(state.selected_session().unwrap().id, "stale");

        handle_key(&mut state, key(KeyCode::Char('b'), KeyModifiers::NONE)).unwrap();
        let typed_request = state.take_search_request().unwrap();
        assert!(typed_request.preserve_selection.is_none());

        super::state::handle_scan_message(
            &mut state,
            super::state::ScanMessage::Progress {
                elapsed: Duration::ZERO,
                new_or_modified: 1,
                deleted: 0,
                total: 3,
            },
        );

        let refresh_request = state.take_search_request().unwrap();
        assert!(refresh_request.reload_index);
        assert!(refresh_request.preserve_selection.is_none());
        assert!(state.apply_search_result(
            refresh_request.generation,
            vec![session("b"), session("c")],
            0.0,
            refresh_request.preserve_selection.as_ref()
        ));
        assert_eq!(state.selected, 0);
        assert_eq!(state.selected_session().unwrap().id, "b");
    }

    #[test]
    fn refresh_messages_do_not_overwrite_footer_status() {
        let mut state = test_state(vec![session("a")]);
        state.status = "copied: codex resume abc".to_string();

        super::state::handle_scan_message(
            &mut state,
            super::state::ScanMessage::Finished {
                elapsed: Duration::from_millis(17_784),
                new_or_modified: 1_964,
                deleted: 0,
                total: 1_964,
            },
        );

        assert_eq!(state.status, "copied: codex resume abc");
        assert_eq!(
            state.refresh_status,
            "refreshed: 1964 sessions, 1964 changed, 17.8s"
        );
    }

    #[test]
    fn failed_refresh_reports_error_without_requesting_a_search() {
        let mut state = test_state(vec![session("a")]);

        super::state::handle_scan_message(
            &mut state,
            super::state::ScanMessage::Failed {
                elapsed: Duration::from_millis(250),
                error: "index writer is locked".to_string(),
            },
        );

        assert!(!state.scanning);
        assert_eq!(
            state.refresh_status,
            "refresh failed after 250ms: index writer is locked"
        );
        assert!(state.take_search_request().is_none());
        assert_eq!(state.selected_session().unwrap().id, "a");
    }

    #[test]
    fn search_errors_keep_visible_results_and_finish_the_generation() {
        let mut state = test_state(vec![session("a")]);
        handle_key(&mut state, key(KeyCode::Char('z'), KeyModifiers::NONE)).unwrap();
        let request = state.take_search_request().unwrap();

        assert!(state.apply_search_error(request.generation, "reload failed"));

        assert_eq!(state.selected_session().unwrap().id, "a");
        assert_eq!(state.status, "search failed: reload failed");
        assert!(!state.search_pending());
    }

    #[test]
    fn actions_wait_for_pending_search_results_before_using_selection() {
        let mut state = test_state(vec![session("a")]);

        handle_key(&mut state, key(KeyCode::Char('z'), KeyModifiers::NONE)).unwrap();
        let stale = state.take_search_request().unwrap();
        let exit = handle_key(&mut state, key(KeyCode::Enter, KeyModifiers::NONE)).unwrap();

        assert!(exit.is_none());
        assert_eq!(state.visible.len(), 1);
        assert!(state.modal.is_none());
        assert!(state.status.contains("searching"));
        assert!(state.apply_search_result(stale.generation, Vec::new(), 10.0, None));
        assert!(state.visible.is_empty());
        assert!(state.status.is_empty());
    }

    #[test]
    fn actions_do_not_use_matching_results_before_redraw() {
        let mut state = test_state(vec![session("a"), session("b")]);
        state.selected = state
            .visible
            .iter()
            .position(|session| session.id == "a")
            .unwrap();
        assert_eq!(state.selected_session().unwrap().id, "a");

        handle_key(&mut state, key(KeyCode::Char('b'), KeyModifiers::NONE)).unwrap();
        let request = state.take_search_request().unwrap();
        let exit = handle_key(&mut state, key(KeyCode::Enter, KeyModifiers::NONE)).unwrap();

        assert!(exit.is_none());
        assert!(state.modal.is_none());
        assert_eq!(state.selected_session().unwrap().id, "a");
        assert!(state.status.contains("searching"));

        assert!(state.apply_search_result(request.generation, vec![session("b")], 10.0, None));
        assert_eq!(state.selected_session().unwrap().id, "b");
        assert!(state.status.is_empty());
    }

    #[test]
    fn yolo_modal_confirms_original_session_after_selection_changes() {
        let mut state = test_state(vec![session("a"), session("b")]);
        let original_id = state.selected_session().unwrap().id.clone();

        let exit = handle_key(&mut state, key(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert!(exit.is_none());
        assert_eq!(state.modal.as_ref().unwrap().session.id, original_id);

        state.selected = if state.selected == 0 { 1 } else { 0 };
        let exit = handle_key(&mut state, key(KeyCode::Char('y'), KeyModifiers::NONE))
            .unwrap()
            .unwrap();

        match exit {
            super::TuiExit::Resume { command, directory } => {
                assert_eq!(
                    command.last().map(String::as_str),
                    Some(original_id.as_str())
                );
                assert_eq!(directory, "/tmp/fast-resume");
            }
            super::TuiExit::Quit => panic!("expected resume exit"),
        }
    }

    #[test]
    fn yolo_modal_arrows_select_buttons_directionally() {
        let mut state = test_state(vec![session("a")]);

        handle_key(&mut state, key(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert!(!state.modal.as_ref().unwrap().selected);

        handle_key(&mut state, key(KeyCode::Left, KeyModifiers::NONE)).unwrap();
        assert!(!state.modal.as_ref().unwrap().selected);

        handle_key(&mut state, key(KeyCode::Right, KeyModifiers::NONE)).unwrap();
        assert!(state.modal.as_ref().unwrap().selected);

        handle_key(&mut state, key(KeyCode::Left, KeyModifiers::NONE)).unwrap();
        assert!(!state.modal.as_ref().unwrap().selected);

        handle_key(&mut state, key(KeyCode::Tab, KeyModifiers::NONE)).unwrap();
        assert!(state.modal.as_ref().unwrap().selected);
    }

    #[test]
    fn ctrl_y_does_not_confirm_yolo_modal() {
        let mut state = test_state(vec![session("a")]);

        handle_key(&mut state, key(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert!(state.modal.is_some());

        let exit = handle_key(&mut state, key(KeyCode::Char('y'), KeyModifiers::CONTROL)).unwrap();

        assert!(exit.is_none());
        assert!(state.modal.is_some());
        assert!(!state.modal.as_ref().unwrap().selected);
    }

    #[test]
    fn enter_resumes_crush_sessions() {
        let mut crush = session("crush-1");
        crush.agent = "crush".to_string();
        let mut state = test_state(vec![crush]);
        state.yolo = true;

        let exit = handle_key(&mut state, key(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap()
            .unwrap();

        match exit {
            super::TuiExit::Resume { command, directory } => {
                assert_eq!(command, vec!["crush", "--yolo", "--session", "crush-1"]);
                assert_eq!(directory, "/tmp/fast-resume");
            }
            super::TuiExit::Quit => panic!("expected resume exit"),
        }
    }

    fn press(state: &mut AppState, column: u16, row: u16) {
        let event = mouse(MouseEventKind::Down(MouseButton::Left), column, row);
        super::handle_mouse(state, event, Rect::new(0, 0, 120, 40));
    }

    const AREA: Rect = Rect::new(0, 0, 120, 40);

    /// Where a chat row is on screen (slot 0 is the first chat).
    fn chat_point(state: &AppState, sidebar: bool, slot: u16) -> (u16, u16) {
        let layout = layout::app(AREA, state.show_preview, sidebar);
        let rows = render::results_rows_area(layout.main.results());
        (rows.x + 2, rows.y + slot)
    }

    /// Where a sidebar row is on screen, found by `pick`.
    fn sidebar_point(
        state: &AppState,
        pick: impl Fn(&super::state::SidebarRow) -> bool,
    ) -> (u16, u16) {
        let sidebar = layout::app(AREA, state.show_preview, true)
            .sidebar
            .expect("the sidebar fits in 120 columns");
        let index = state
            .sidebar_rows()
            .iter()
            .position(pick)
            .expect("that row exists");
        (sidebar.x + 3, sidebar.y + 1 + index as u16)
    }

    fn type_text(state: &mut AppState, text: &str) {
        for ch in text.chars() {
            handle_key(state, key(KeyCode::Char(ch), KeyModifiers::NONE)).unwrap();
        }
    }

    #[test]
    fn dragging_a_chat_onto_a_workspace_moves_it() {
        let mut state = test_state(vec![session("a"), session("b")]);
        state.show_sidebar = true;
        let work = state.ws.add("Work").unwrap();
        state.refresh_live();
        let moved = state.visible[1].id.clone();

        let (from_x, from_y) = chat_point(&state, true, 1);
        let (to_x, to_y) =
            sidebar_point(&state, |row| row.kind == super::state::RowKind::Workspace);
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
        ] {
            let (x, y) = if matches!(kind, MouseEventKind::Down(_)) {
                (from_x, from_y)
            } else {
                (to_x, to_y)
            };
            assert!(super::handle_mouse(&mut state, mouse(kind, x, y), AREA));
        }
        assert!(state.drag.as_ref().is_some_and(|drag| drag.active));
        super::handle_mouse(
            &mut state,
            mouse(MouseEventKind::Up(MouseButton::Left), to_x, to_y),
            AREA,
        );

        assert_eq!(state.ws.ws_of(&moved), work);
        assert!(state.drag.is_none());
        assert_eq!(state.selected, 1, "the click selected the dragged row");
    }

    #[test]
    fn plain_click_selects_without_moving_anything() {
        let mut state = test_state(vec![session("a"), session("b")]);
        state.show_sidebar = true;
        let work = state.ws.add("Work").unwrap();
        state.refresh_live();
        let (x, y) = chat_point(&state, true, 1);
        press(&mut state, x, y);
        super::handle_mouse(
            &mut state,
            mouse(MouseEventKind::Up(MouseButton::Left), x, y),
            AREA,
        );

        assert_eq!(state.selected, 1);
        assert!(state.ws.ws_of("a").is_empty() && state.ws.ws_of("b").is_empty());
        assert!(state.ws.name_of(&work).is_some());
    }

    #[test]
    fn double_clicking_a_title_renames_the_chat() {
        let mut state = test_state(vec![session("a")]);
        let layout = layout::app(Rect::new(0, 0, 120, 40), state.show_preview, false);
        let rows_area = render::results_rows_area(layout.main.results());
        let columns = render::result_columns(rows_area.width, state.shows_workspace_column());
        let (x, y) = (rows_area.x + columns.title_x + 1, rows_area.y);

        press(&mut state, x, y);
        assert!(state.edit.is_none());
        press(&mut state, x, y);
        assert!(
            state.edit.is_some(),
            "second click within 400ms starts editing"
        );

        for _ in 0..40 {
            handle_key(&mut state, key(KeyCode::Backspace, KeyModifiers::NONE)).unwrap();
        }
        type_text(&mut state, "My chat");
        handle_key(&mut state, key(KeyCode::Enter, KeyModifiers::NONE)).unwrap();

        assert!(state.edit.is_none());
        let shown = state.title_of(&state.visible[0]).to_string();
        assert_eq!(shown, "My chat");
        assert_eq!(
            state.visible[0].title, "Session a",
            "the real title is untouched"
        );
    }

    #[test]
    fn renaming_back_to_the_original_or_empty_clears_the_custom_name() {
        let mut state = test_state(vec![session("a")]);
        state.ws.rename_chat("a", "Custom").unwrap();
        handle_key(&mut state, key(KeyCode::F(2), KeyModifiers::NONE)).unwrap();
        for _ in 0..40 {
            handle_key(&mut state, key(KeyCode::Backspace, KeyModifiers::NONE)).unwrap();
        }
        handle_key(&mut state, key(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert_eq!(state.title_of(&state.visible[0]), "Session a");
    }

    #[test]
    fn escape_cancels_a_rename() {
        let mut state = test_state(vec![session("a")]);
        handle_key(&mut state, key(KeyCode::F(2), KeyModifiers::NONE)).unwrap();
        type_text(&mut state, "zzz");
        handle_key(&mut state, key(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert!(state.edit.is_none());
        assert_eq!(state.title_of(&state.visible[0]), "Session a");
    }

    #[test]
    fn workspace_scope_filters_results() {
        let mut state = test_state(vec![session("a"), session("b"), session("c")]);
        let work = state.ws.add("Work").unwrap();
        state.move_session_to("a", &work);
        state.move_session_to("c", &work);

        state.set_scope(&work);
        state.refresh_search();
        let mut ids: Vec<_> = state.visible.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["a", "c"]);

        state.set_scope(super::workspaces::NONE);
        state.refresh_search();
        assert_eq!(state.visible.len(), 1);
        assert_eq!(state.visible[0].id, "b");

        state.set_scope(super::workspaces::ALL);
        state.refresh_search();
        assert_eq!(state.visible.len(), 3);
    }

    #[test]
    fn scratch_uses_the_selected_chats_agent() {
        let mut state = test_state(vec![
            session_for_agent("a", "claude", "/work/app"),
            session_for_agent("b", "codex", "/work/app"),
        ]);
        let codex_row = state
            .visible
            .iter()
            .position(|session| session.agent == "codex")
            .unwrap();
        state.selected = codex_row;

        state.request_scratch();

        let request = state
            .take_scratch_request()
            .expect("a scratch was requested");
        assert_eq!(request.agent, "codex");
        assert_eq!(request.command, vec!["codex".to_string()]);
        assert!(!request.directory.is_empty());
        assert!(state.take_scratch_request().is_none(), "one-shot request");
    }

    #[test]
    fn scratch_falls_back_to_an_indexed_agent_when_nothing_is_selected() {
        let mut state = test_state(vec![session_for_agent("a", "codex", "/work/app")]);
        // A query that matches nothing: no selection, but codex is indexed.
        state.query = "no such chat anywhere".to_string();
        state.refresh_search();
        assert!(state.selected_session().is_none());

        state.request_scratch();

        let request = state
            .take_scratch_request()
            .expect("a scratch was requested");
        assert_eq!(request.agent, "codex");
    }

    #[test]
    fn scratch_falls_back_to_claude_when_nothing_is_indexed() {
        let mut state = test_state(Vec::new());

        state.request_scratch();

        let request = state
            .take_scratch_request()
            .expect("a scratch was requested");
        assert_eq!(request.agent, "claude");
        assert_eq!(request.command, vec!["claude".to_string()]);
    }

    #[test]
    fn picker_moves_the_selected_chat() {
        let mut state = test_state(vec![session("a")]);
        let work = state.ws.add("Work").unwrap();
        handle_key(&mut state, key(KeyCode::F(3), KeyModifiers::NONE)).unwrap();
        assert!(state.picker.is_some());
        // It opens on the chat's current place ("No workspace", the last item).
        handle_key(&mut state, key(KeyCode::Up, KeyModifiers::NONE)).unwrap();
        handle_key(&mut state, key(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert!(state.picker.is_none());
        assert_eq!(state.ws.ws_of("a"), work);
    }

    #[test]
    fn new_workspace_is_named_inline_and_alt_arrows_switch() {
        let mut state = test_state(vec![session("a")]);
        handle_key(&mut state, key(KeyCode::F(4), KeyModifiers::NONE)).unwrap();
        assert!(matches!(
            state.edit.as_ref().map(|edit| &edit.target),
            Some(super::state::EditTarget::Workspace(_))
        ));
        type_text(&mut state, "Optruss");
        handle_key(&mut state, key(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        let id = state.ws.scope().to_string();
        assert_eq!(state.ws.name_of(&id), Some("Optruss"));

        handle_key(&mut state, key(KeyCode::Up, KeyModifiers::ALT)).unwrap();
        assert_eq!(state.ws.scope(), super::workspaces::ALL);
        handle_key(&mut state, key(KeyCode::Down, KeyModifiers::ALT)).unwrap();
        assert_eq!(state.ws.scope(), id);
    }

    #[test]
    fn deleting_a_chat_moves_it_to_deleted_and_restore_brings_it_back() {
        let mut state = test_state(vec![session("a"), session("b")]);
        let work = state.ws.add("Work").unwrap();
        state.move_session_to("a", &work);
        state.set_scope(&work);
        state.refresh_search();
        assert_eq!(state.visible.len(), 1);

        handle_key(&mut state, key(KeyCode::F(8), KeyModifiers::NONE)).unwrap();
        state.refresh_search();
        assert!(state.visible.is_empty(), "gone from its workspace");
        state.set_scope(super::workspaces::ALL);
        state.refresh_search();
        assert_eq!(state.visible.len(), 1, "and from All; only b is left");
        assert_eq!(state.visible[0].id, "b");

        state.set_scope(super::workspaces::TRASH);
        state.refresh_search();
        assert_eq!(state.visible.len(), 1);
        assert_eq!(state.visible[0].id, "a");

        handle_key(&mut state, key(KeyCode::F(8), KeyModifiers::NONE)).unwrap();
        state.refresh_search();
        assert!(state.visible.is_empty(), "restored chats leave Deleted");
        state.set_scope(&work);
        state.refresh_search();
        assert_eq!(state.visible[0].id, "a", "back in its workspace");
    }

    #[test]
    fn deleting_a_workspace_moves_its_chats_to_deleted_and_it_can_be_restored() {
        let mut state = test_state(vec![session("a"), session("b")]);
        let work = state.ws.add("Work").unwrap();
        state.move_session_to("a", &work);

        state.delete_workspace(&work);
        assert!(state.ws.name_of(&work).is_none());
        state.set_scope(super::workspaces::TRASH);
        state.refresh_search();
        assert_eq!(state.visible[0].id, "a");
        let rows = state.sidebar_rows();
        let deleted_ws = rows
            .iter()
            .find(|row| row.kind == super::state::RowKind::DeletedWorkspace)
            .expect("deleted workspaces are listed under Deleted");
        assert_eq!((deleted_ws.name.as_str(), deleted_ws.count), ("Work", 1));

        state.restore_selected_workspace();
        assert_eq!(state.ws.name_of(&work), Some("Work"));
        state.set_scope(&work);
        state.refresh_search();
        assert_eq!(state.visible[0].id, "a");
    }

    #[test]
    fn dragging_onto_deleted_deletes_and_dragging_out_restores() {
        use super::state::RowKind;
        let mut state = test_state(vec![session("a")]);
        state.show_sidebar = true;
        let work = state.ws.add("Work").unwrap();
        state.refresh_live();

        let drag_to = |state: &mut AppState, from: (u16, u16), to: (u16, u16)| {
            for (kind, at) in [
                (MouseEventKind::Down(MouseButton::Left), from),
                (MouseEventKind::Drag(MouseButton::Left), to),
                (MouseEventKind::Up(MouseButton::Left), to),
            ] {
                super::handle_mouse(state, mouse(kind, at.0, at.1), AREA);
            }
        };

        let from = chat_point(&state, true, 0);
        let deleted = sidebar_point(&state, |row| row.kind == RowKind::Deleted);
        drag_to(&mut state, from, deleted);
        assert!(state.ws.is_trashed("a"));

        // In the Deleted view, dropping on a workspace restores into it.
        state.set_scope(super::workspaces::TRASH);
        state.refresh_search();
        // One column over: the same cell twice in a row would be a double-click.
        let from = chat_point(&state, true, 0);
        let from = (from.0 + 1, from.1);
        let target = sidebar_point(&state, |row| row.kind == RowKind::Workspace);
        drag_to(&mut state, from, target);
        assert!(!state.ws.is_trashed("a"));
        assert_eq!(state.ws.ws_of("a"), work);
    }

    #[test]
    fn clicking_the_row_glyph_deletes_only_on_the_selected_row() {
        let mut state = test_state(vec![session("a"), session("b")]);
        let layout = layout::app(AREA, state.show_preview, false);
        let rows_area = render::results_rows_area(layout.main.results());
        let glyph_x = rows_area.right() - 1;
        let y = rows_area.y + 1; // the second chat

        // Row 1 is not selected yet: the first click only selects it.
        press(&mut state, glyph_x, y);
        assert!(!state.ws.is_trashed("a") && !state.ws.is_trashed("b"));
        assert_eq!(state.selected, 1);

        let doomed = state.visible[1].id.clone();
        press(&mut state, glyph_x, y);
        assert!(state.ws.is_trashed(&doomed));
    }

    #[test]
    fn results_window_does_not_jump_when_clicking_a_visible_row() {
        let mut state = test_state((0..30).map(|i| session(&format!("s{i:02}"))).collect());
        state.selected = 25;
        let (top, _) = state.results_window(10);
        assert_eq!(top, 16);
        state.selected = 20;
        assert_eq!(state.results_window(10).0, 16, "still inside the window");
        state.selected = 5;
        assert_eq!(state.results_window(10).0, 5);
    }

    fn press_key(state: &mut AppState, code: KeyCode) -> Option<super::TuiExit> {
        handle_key(state, key(code, KeyModifiers::NONE)).unwrap()
    }

    fn with_sidebar(sessions: Vec<Session>) -> AppState {
        let mut state = test_state(sessions);
        state.show_sidebar = true;
        state.note_sidebar_on_screen(true); // drawing does this in the app
        state
    }

    #[test]
    fn arrows_move_between_the_chat_list_and_the_workspace_list() {
        use super::state::Focus;
        let mut state = with_sidebar(vec![session("a")]);
        let work = state.ws.add("Work").unwrap();
        state.refresh_live();
        assert_eq!(
            state.focus,
            Focus::Chats,
            "the list has the keyboard at start"
        );

        press_key(&mut state, KeyCode::Left);
        assert_eq!(state.focus, Focus::Sidebar);
        press_key(&mut state, KeyCode::Down);
        assert_eq!(
            state.ws.scope(),
            work,
            "moving in the list opens that workspace"
        );
        press_key(&mut state, KeyCode::Down);
        assert_eq!(state.ws.scope(), super::workspaces::NONE);
        press_key(&mut state, KeyCode::Up);
        press_key(&mut state, KeyCode::Up);
        assert_eq!(state.ws.scope(), super::workspaces::ALL);
        press_key(&mut state, KeyCode::Up);
        assert_eq!(
            state.ws.scope(),
            super::workspaces::ALL,
            "no wrap at the top"
        );

        press_key(&mut state, KeyCode::Right);
        assert_eq!(state.focus, Focus::Chats);
        press_key(&mut state, KeyCode::Left);
        press_key(&mut state, KeyCode::Enter);
        assert_eq!(
            state.focus,
            Focus::Chats,
            "Enter also goes back to the chats"
        );
    }

    #[test]
    fn slash_jumps_to_search_and_typing_anywhere_still_searches() {
        use super::state::Focus;
        let mut state = with_sidebar(vec![session("a")]);

        type_text(&mut state, "/");
        assert_eq!(state.focus, Focus::Search);
        assert!(state.query.is_empty(), "the slash is a command, not text");

        // A slash typed in the search box is ordinary text (paths).
        type_text(&mut state, "/tmp");
        assert_eq!(state.query, "/tmp");

        press_key(&mut state, KeyCode::Esc);
        assert_eq!(state.focus, Focus::Chats, "Esc leaves the search box");
        state.query.clear();
        state.cursor = 0;

        press_key(&mut state, KeyCode::Left);
        assert_eq!(state.focus, Focus::Sidebar);
        type_text(&mut state, "x");
        assert_eq!(
            state.focus,
            Focus::Search,
            "typing in the workspace list searches"
        );
        assert_eq!(state.query, "x");
    }

    #[test]
    fn escape_quits_only_from_the_lists() {
        use super::state::Focus;
        let mut state = with_sidebar(vec![session("a")]);
        state.focus = Focus::Search;
        assert!(press_key(&mut state, KeyCode::Esc).is_none());
        assert!(matches!(
            press_key(&mut state, KeyCode::Esc),
            Some(super::TuiExit::Quit)
        ));
    }

    #[test]
    fn search_keeps_its_cursor_keys() {
        use super::state::Focus;
        let mut state = with_sidebar(vec![session("a")]);
        state.focus = Focus::Search;
        type_text(&mut state, "abc");
        press_key(&mut state, KeyCode::Left);
        assert_eq!(state.focus, Focus::Search, "Left edits the query here");
        assert_eq!(state.cursor, 2);
        press_key(&mut state, KeyCode::Down);
        assert_eq!(state.focus, Focus::Chats, "Down goes to the results");
    }

    #[test]
    fn left_says_so_when_the_workspace_list_is_hidden() {
        use super::state::Focus;
        let mut state = test_state(vec![session("a")]);
        press_key(&mut state, KeyCode::Left);
        assert_eq!(state.focus, Focus::Chats);
        assert!(state.status.contains("hidden"));
    }

    #[test]
    fn delete_and_f2_act_on_the_focused_pane() {
        use super::state::{EditTarget, Focus};
        let mut state = with_sidebar(vec![session("a")]);
        let work = state.ws.add("Work").unwrap();
        state.move_session_to("a", &work);

        // In the chat list Delete removes the chat...
        press_key(&mut state, KeyCode::Delete);
        assert!(state.ws.is_trashed("a"));
        state.ws.restore_chat("a").unwrap();
        state.refresh_search();

        // ...in the workspace list, F2 renames the workspace, Delete removes it.
        state.set_scope(&work);
        press_key(&mut state, KeyCode::Left);
        assert_eq!(state.focus, Focus::Sidebar);
        press_key(&mut state, KeyCode::F(2));
        assert!(matches!(
            state.edit.as_ref().map(|e| &e.target),
            Some(EditTarget::Workspace(id)) if *id == work
        ));
        press_key(&mut state, KeyCode::Esc);
        state.focus = Focus::Sidebar;
        press_key(&mut state, KeyCode::Delete);
        assert!(state.ws.name_of(&work).is_none(), "workspace deleted");
    }

    #[test]
    fn a_deleted_workspace_can_be_opened_without_being_restored() {
        let mut state = with_sidebar(vec![session("a"), session("b")]);
        let work = state.ws.add("Work").unwrap();
        state.move_session_to("a", &work);
        state.delete_workspace(&work);
        state.set_scope(super::workspaces::TRASH);
        state.refresh_search();

        let sidebar = layout::app(AREA, state.show_preview, true).sidebar.unwrap();
        let (row_x, y) = sidebar_point(&state, |row| {
            row.kind == super::state::RowKind::DeletedWorkspace
        });

        // Clicking the row opens it and lists its chats; nothing is restored.
        press(&mut state, row_x, y);
        assert_eq!(state.ws.scope(), work);
        assert!(state.ws.name_of(&work).is_none(), "still deleted");
        assert_eq!(state.visible.len(), 1);
        assert_eq!(state.visible[0].id, "a");
        assert!(state.in_deleted_view());
        assert_eq!(state.ws.scope_label(&work), "Work (deleted)");

        // The keyboard reaches it too, and the ↺ on the row restores it.
        press(&mut state, sidebar.right() - 4, y);
        assert_eq!(state.ws.name_of(&work), Some("Work"), "restored on purpose");
        assert!(!state.ws.is_trashed("a"));
    }

    #[test]
    fn alt_u_restores_the_open_deleted_workspace_and_arrows_reach_it() {
        use super::state::Focus;
        let mut state = with_sidebar(vec![session("a")]);
        let work = state.ws.add("Work").unwrap();
        state.move_session_to("a", &work);
        state.delete_workspace(&work);
        state.set_scope(super::workspaces::TRASH);

        state.focus = Focus::Sidebar;
        press_key(&mut state, KeyCode::Down);
        assert_eq!(
            state.ws.scope(),
            work,
            "the deleted workspace is the next row"
        );

        handle_key(&mut state, key(KeyCode::Char('u'), KeyModifiers::ALT)).unwrap();
        assert_eq!(state.ws.name_of(&work), Some("Work"));
    }

    fn open_deleted_view(state: &mut AppState) {
        state.set_scope(super::workspaces::TRASH);
        state.refresh_search();
    }

    #[test]
    fn f9_asks_first_and_only_y_erases_a_deleted_chat() {
        let mut state = test_state(vec![session("a"), session("b")]);
        // Not deleted yet: nothing to confirm.
        press_key(&mut state, KeyCode::F(9));
        assert!(state.confirm.is_none());
        assert!(state.status.contains("delete it first"));

        state.move_session_to("a", super::workspaces::TRASH);
        open_deleted_view(&mut state);
        press_key(&mut state, KeyCode::F(9));
        assert!(state.confirm.is_some());

        // Enter, Esc and n all cancel.
        for code in [KeyCode::Enter, KeyCode::Esc, KeyCode::Char('n')] {
            press_key(&mut state, KeyCode::F(9));
            press_key(&mut state, code);
            assert!(state.confirm.is_none());
            assert!(state.ws.is_trashed("a") && !state.ws.is_purged("a"));
        }

        press_key(&mut state, KeyCode::F(9));
        press_key(&mut state, KeyCode::Char('y'));
        assert!(state.ws.is_purged("a"));
        assert!(!state.ws.is_trashed("a"));
        state.refresh_search();
        assert!(state.visible.is_empty(), "gone from Deleted");
        state.set_scope(super::workspaces::ALL);
        state.refresh_search();
        let ids: Vec<_> = state.visible.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["b"], "and from All");
    }

    #[test]
    fn a_workspace_is_erased_with_its_chats_from_the_workspace_list() {
        use super::state::{ConfirmAction, Focus};
        let mut state = with_sidebar(vec![session("a"), session("b"), session("c")]);
        let work = state.ws.add("Work").unwrap();
        state.move_session_to("a", &work);
        state.move_session_to("b", &work);
        state.delete_workspace(&work);
        state.set_scope(&work);

        state.focus = Focus::Sidebar;
        press_key(&mut state, KeyCode::F(9));
        assert_eq!(
            state.confirm,
            Some(ConfirmAction::PurgeWorkspace {
                id: work.clone(),
                name: "Work".into(),
                chats: 2
            })
        );
        press_key(&mut state, KeyCode::Char('y'));

        assert!(state.ws.is_purged("a") && state.ws.is_purged("b"));
        assert!(!state.ws.is_purged("c"));
        assert!(state.ws.deleted_name_of(&work).is_none());
        assert_eq!(state.ws.scope(), super::workspaces::ALL);
        assert!(
            state
                .sidebar_rows()
                .iter()
                .all(|row| row.kind != super::state::RowKind::DeletedWorkspace),
        );
        state.refresh_search();
        assert_eq!(state.visible.len(), 1);
        assert_eq!(state.visible[0].id, "c");
    }

    #[test]
    fn f9_on_a_live_workspace_says_what_to_do() {
        use super::state::Focus;
        let mut state = with_sidebar(vec![session("a")]);
        let work = state.ws.add("Work").unwrap();
        state.set_scope(&work);
        state.focus = Focus::Sidebar;
        press_key(&mut state, KeyCode::F(9));
        assert!(state.confirm.is_none());
        assert!(state.status.contains("deleted workspace"));
    }

    #[test]
    fn the_red_x_on_a_deleted_chat_row_opens_the_dialog_and_only_its_button_erases() {
        let mut state = test_state(vec![session("a")]);
        state.move_session_to("a", super::workspaces::TRASH);
        open_deleted_view(&mut state);
        let area = Rect::new(0, 0, 120, 40);
        let layout = layout::app(area, state.show_preview, false);
        let rows_area = render::results_rows_area(layout.main.results());
        let y = rows_area.y;

        // The row must be selected before its glyphs react.
        press(&mut state, rows_area.x + 3, y);
        assert!(state.confirm.is_none());
        press(&mut state, rows_area.right() - 1, y);
        assert!(state.confirm.is_some());

        // A click away from the red button cancels.
        press(&mut state, 1, 1);
        assert!(state.confirm.is_none() && !state.ws.is_purged("a"));

        press(&mut state, rows_area.right() - 1, y);
        let (_, _, erase) = render::confirm_buttons(area);
        press(&mut state, erase.x + 2, erase.y);
        assert!(state.ws.is_purged("a"));
    }

    #[test]
    fn the_restore_arrow_on_a_deleted_chat_row_still_restores() {
        let mut state = test_state(vec![session("a")]);
        state.move_session_to("a", super::workspaces::TRASH);
        open_deleted_view(&mut state);
        let layout = layout::app(Rect::new(0, 0, 120, 40), state.show_preview, false);
        let rows_area = render::results_rows_area(layout.main.results());

        press(&mut state, rows_area.x + 3, rows_area.y);
        press(&mut state, rows_area.right() - 4, rows_area.y);
        assert!(!state.ws.is_trashed("a"));
        assert!(!state.ws.is_purged("a"));
    }

    #[test]
    fn the_red_x_on_a_deleted_workspace_row_asks_to_erase_it() {
        let mut state = with_sidebar(vec![session("a")]);
        let work = state.ws.add("Work").unwrap();
        state.move_session_to("a", &work);
        state.delete_workspace(&work);
        state.set_scope(&work); // open it, as a click on its row does

        let sidebar = layout::app(AREA, state.show_preview, true).sidebar.unwrap();
        let (row_x, y) = sidebar_point(&state, |row| {
            row.kind == super::state::RowKind::DeletedWorkspace
        });

        let _ = row_x;
        press(&mut state, sidebar.right() - 2, y);
        assert!(matches!(
            state.confirm,
            Some(super::state::ConfirmAction::PurgeWorkspace { .. })
        ));
        assert!(
            state.ws.deleted_name_of(&work).is_some(),
            "nothing erased yet"
        );
    }

    #[test]
    fn the_delete_key_erases_in_deleted_but_f8_still_restores() {
        use super::state::ConfirmAction;
        let mut state = test_state(vec![session("a"), session("b")]);
        state.move_session_to("a", super::workspaces::TRASH);
        state.move_session_to("b", super::workspaces::TRASH);
        open_deleted_view(&mut state);
        assert_eq!(state.visible.len(), 2);

        // Delete asks to erase the selected chat, and does nothing until y.
        press_key(&mut state, KeyCode::Delete);
        assert!(matches!(
            state.confirm,
            Some(ConfirmAction::PurgeChat { .. })
        ));
        assert!(state.ws.is_trashed("a") && state.ws.is_trashed("b"));
        press_key(&mut state, KeyCode::Esc);
        assert!(state.confirm.is_none());
        assert!(!state.ws.is_purged("a") && state.ws.is_trashed("a"));

        // F8 restores, exactly as before.
        let first = state.visible[0].id.clone();
        press_key(&mut state, KeyCode::F(8));
        assert!(!state.ws.is_trashed(&first), "F8 restored it");
        assert!(!state.ws.is_purged(&first));

        // y after Delete erases the remaining one.
        state.refresh_search();
        let second = state.visible[0].id.clone();
        press_key(&mut state, KeyCode::Delete);
        press_key(&mut state, KeyCode::Char('y'));
        assert!(state.ws.is_purged(&second));
    }

    #[test]
    fn the_delete_key_in_the_workspace_list_erases_a_deleted_workspace_but_deletes_a_live_one() {
        use super::state::{ConfirmAction, Focus};
        let mut state = with_sidebar(vec![session("a")]);
        let work = state.ws.add("Work").unwrap();
        let play = state.ws.add("Play").unwrap();
        state.move_session_to("a", &work);

        // Live workspace: Delete moves it to Deleted (no dialog, recoverable).
        state.set_scope(&play);
        state.focus = Focus::Sidebar;
        press_key(&mut state, KeyCode::Delete);
        assert!(state.confirm.is_none());
        assert!(state.ws.deleted_name_of(&play).is_some());

        // Deleted workspace: Delete now asks to erase it for good.
        state.delete_workspace(&work);
        state.set_scope(&work);
        state.focus = Focus::Sidebar;
        press_key(&mut state, KeyCode::Delete);
        assert!(matches!(
            state.confirm,
            Some(ConfirmAction::PurgeWorkspace { .. })
        ));
        press_key(&mut state, KeyCode::Char('n'));
        assert!(state.ws.deleted_name_of(&work).is_some(), "n keeps it");

        // F8 in the list still restores it.
        press_key(&mut state, KeyCode::F(8));
        assert_eq!(state.ws.name_of(&work), Some("Work"));
    }
}
