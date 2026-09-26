use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui_image::{Image as TuiImage, protocol::Protocol};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::config::{AGENTS, APP_NAME, APP_TAGLINE};
use crate::model::Session;

use super::layout::{self, MainLayout};
use super::state::{
    AppState, ConfirmAction, EditTarget, Focus, PendingAction, Picker, RowKind, YoloModal,
};
use super::text::{
    age_style, display_width_until, line_width, search_query_spans, time_ago, truncate,
};
use super::theme::Theme;
/// Panels are drawn as a single rule along the top, with the sides and bottom
/// left as breathing room, so nothing looks boxed in.
const AIRY: ratatui::symbols::border::Set<'static> = ratatui::symbols::border::Set {
    top_left: "─",
    top_right: "─",
    bottom_left: " ",
    bottom_right: " ",
    vertical_left: " ",
    vertical_right: " ",
    horizontal_top: "─",
    horizontal_bottom: " ",
};

const SEARCH_PLACEHOLDER: &str = "Search chats, messages and folders. Try agent:claude";

pub(super) fn draw(frame: &mut Frame, state: &AppState) {
    let area = frame.area();
    let layout = layout::app(area, state.show_preview, state.show_sidebar);

    state.note_sidebar_on_screen(layout.sidebar.is_some());
    draw_header(frame, layout.header, state);
    draw_search(frame, layout.search, state);
    draw_filters(frame, layout.filters, state);
    if let Some(sidebar) = layout.sidebar {
        draw_sidebar(frame, sidebar, state);
    }
    draw_main(frame, layout.main, state);
    draw_footer(frame, layout.footer, state);

    draw_drag_ghost(frame, area, state);

    if state.show_help {
        draw_help_modal(frame, area, &state.theme);
    } else if let Some(modal) = &state.modal {
        draw_yolo_modal(frame, area, modal, &state.theme);
    } else if let Some(picker) = &state.picker {
        draw_picker(frame, area, picker, state);
    } else if let Some(action) = &state.confirm {
        draw_confirm(frame, area, action, &state.theme);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, state: &AppState) {
    frame.render_widget(Paragraph::new(header_line(state, area.width)), area);
}

fn header_line(state: &AppState, width: u16) -> Line<'static> {
    let theme = &state.theme;
    let width = width as usize;
    let scope = state.ws.scope();
    let mut left = vec![Span::styled(
        format!("◆ {APP_NAME}"),
        Style::new().bold().fg(theme.accent),
    )];
    if scope == super::workspaces::ALL {
        if width >= 90 {
            left.push(Span::styled(
                format!("  {APP_TAGLINE}"),
                Style::new().fg(theme.muted),
            ));
        }
    } else {
        let color = state
            .ws
            .color_index(scope)
            .map_or(theme.secondary, |i| theme.ws_color(i));
        left.push(Span::styled("  ›  ", Style::new().fg(theme.muted)));
        left.push(Span::styled(
            state.ws.scope_label(scope),
            Style::new().bold().fg(color),
        ));
    }

    let total = state
        .engine
        .count_for_agent(state.count_agent_filter().as_deref());
    let shown = state.visible.len();
    let counts = if shown == total {
        format!("{total} chats")
    } else {
        format!("{shown} of {total} chats")
    };
    let mut right = Vec::new();
    if state.refresh_status.starts_with("refresh failed") {
        right.push(Span::styled(
            truncate(&state.refresh_status, 40),
            Style::new().fg(theme.error),
        ));
        right.push(Span::raw("   "));
    } else if state.scanning {
        right.push(Span::styled("◌ syncing", Style::new().fg(theme.warning)));
        right.push(Span::raw("   "));
    }
    right.push(Span::styled(counts, Style::new().fg(theme.muted)));

    let used = line_width(&Line::from(left.clone())) + line_width(&Line::from(right.clone()));
    let pad = width.saturating_sub(used);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend(right);
    Line::from(spans)
}

fn draw_search(frame: &mut Frame, area: Rect, state: &AppState) {
    let focused = state.focus == Focus::Search;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(AIRY)
        .border_style(Style::new().fg(if focused {
            state.theme.accent
        } else {
            state.theme.panel_border
        }))
        .title(" Search ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let prompt = Span::styled(" / ", Style::new().fg(state.theme.accent).bold());
    let input_width = inner.width.saturating_sub(3) as usize;
    let mut spans = vec![prompt];
    if state.query.is_empty() {
        if input_width > 0 {
            spans.push(Span::styled(
                truncate(
                    if focused {
                        SEARCH_PLACEHOLDER
                    } else {
                        "Press / or just start typing"
                    },
                    input_width,
                ),
                Style::new().fg(state.theme.muted).italic(),
            ));
        }
    } else {
        let (visible_query, visible_cursor) =
            search_input_view(&state.query, state.cursor, input_width);
        spans.extend(search_query_spans(&visible_query, &state.theme));
        if visible_cursor == visible_query.chars().count()
            && let Some(suffix) = state.suggestion_suffix()
        {
            let remaining = input_width.saturating_sub(visible_query.width());
            if remaining > 0 {
                spans.push(Span::styled(
                    truncate(&suffix, remaining),
                    Style::new().fg(state.theme.muted),
                ));
            }
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);

    let cursor_x =
        inner.x + 3 + search_input_view(&state.query, state.cursor, input_width).1 as u16;
    if focused && cursor_x < inner.right() {
        frame.set_cursor_position((cursor_x, inner.y));
    }
}

pub(super) fn search_input_view(query: &str, cursor: usize, width: usize) -> (String, usize) {
    if width == 0 {
        return (String::new(), 0);
    }

    let cursor_col = display_width_until(query, cursor);
    let start_col = cursor_col.saturating_sub(width.saturating_sub(1));
    let mut current_col = 0usize;
    let mut start_char = 0usize;
    for (idx, ch) in query.chars().enumerate() {
        let next_col = current_col + ch.width().unwrap_or(0);
        if next_col > start_col {
            start_char = idx;
            break;
        }
        current_col = next_col;
        start_char = idx + 1;
    }

    let actual_start_col = display_width_until(query, start_char);
    let cursor_in_view = cursor_col.saturating_sub(actual_start_col);
    let mut out = String::new();
    for ch in query.chars().skip(start_char) {
        let ch_width = ch.width().unwrap_or(0);
        if out.width() + ch_width > width {
            break;
        }
        out.push(ch);
    }

    (out, cursor_in_view.min(width.saturating_sub(1)))
}

fn draw_filters(frame: &mut Frame, area: Rect, state: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let active_agents = state.active_agent_filters();
    let available_agents = state.agent_filters_with_sessions();
    let tabs: Vec<_> = available_agents
        .iter()
        .map(|(agent, count)| {
            let config = AGENTS.get(agent).expect("known agent");
            AgentFilterTab {
                label: config.badge,
                count: *count,
                has_icon: state
                    .images
                    .as_ref()
                    .is_some_and(|images| images.row.contains_key(*agent)),
                active: active_agents.iter().any(|active| active == agent),
            }
        })
        .collect();
    let filter_layout = plan_filter_layout(area.width, &tabs, state.all_agent_filter_active());
    let mut x = area.x;
    if filter_layout.show_all {
        x = draw_filter_tab(
            frame,
            area,
            x,
            "All",
            None,
            state.all_agent_filter_active(),
            state.theme.foreground,
            None,
            &state.theme,
        );
    }
    for (index, (agent, _)) in available_agents.iter().enumerate() {
        if !filter_layout.visible_agents.contains(&index) {
            continue;
        }
        let config = AGENTS.get(agent).expect("known agent");
        let count = filter_layout.show_counts.then_some(tabs[index].count);
        let active = active_agents.iter().any(|active| active == agent);
        let icon = filter_layout
            .show_icons
            .then(|| {
                state
                    .images
                    .as_ref()
                    .and_then(|images| images.row.get(*agent))
            })
            .flatten();
        let label = if filter_layout.show_labels || icon.is_none() {
            config.badge
        } else {
            ""
        };
        x = draw_filter_tab(
            frame,
            area,
            x,
            label,
            count,
            active,
            state.theme.agent_color(config),
            icon,
            &state.theme,
        );
    }
}

#[derive(Clone, Copy)]
struct AgentFilterTab<'a> {
    label: &'a str,
    count: usize,
    has_icon: bool,
    active: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct FilterLayout {
    show_all: bool,
    show_counts: bool,
    show_icons: bool,
    show_labels: bool,
    visible_agents: std::ops::Range<usize>,
}

fn plan_filter_layout(width: u16, tabs: &[AgentFilterTab<'_>], all_active: bool) -> FilterLayout {
    for (show_counts, show_icons, show_labels) in [
        (true, true, true),
        (false, true, true),
        (false, false, true),
        (false, true, false),
    ] {
        let total_width = filter_tab_width("All", None, false)
            + tabs
                .iter()
                .map(|tab| {
                    let label = filter_tab_label(tab, show_icons, show_labels);
                    filter_tab_width(
                        label,
                        show_counts.then_some(tab.count),
                        show_icons && tab.has_icon,
                    )
                })
                .sum::<u16>();
        if total_width <= width {
            return FilterLayout {
                show_all: true,
                show_counts,
                show_icons,
                show_labels,
                visible_agents: 0..tabs.len(),
            };
        }
    }

    let show_counts = false;
    let show_icons = true;
    let show_labels = false;
    let tab_widths: Vec<_> = tabs
        .iter()
        .map(|tab| {
            filter_tab_width(
                filter_tab_label(tab, show_icons, show_labels),
                None,
                tab.has_icon,
            )
        })
        .collect();
    let all_width = filter_tab_width("All", None, false);
    let active_index = tabs.iter().position(|tab| tab.active);
    let show_all =
        all_active || active_index.is_none_or(|index| all_width + tab_widths[index] <= width);
    let available = width.saturating_sub(if show_all { all_width } else { 0 });
    let anchor = active_index.unwrap_or(0);
    let mut start = anchor;
    let mut used = tab_widths.get(anchor).copied().unwrap_or(0);
    while start > 0 && used.saturating_add(tab_widths[start - 1]) <= available {
        start -= 1;
        used = used.saturating_add(tab_widths[start]);
    }
    let mut end = (anchor + 1).min(tabs.len());
    while end < tabs.len() && used.saturating_add(tab_widths[end]) <= available {
        used = used.saturating_add(tab_widths[end]);
        end += 1;
    }

    FilterLayout {
        show_all,
        show_counts,
        show_icons,
        show_labels,
        visible_agents: start..end,
    }
}

fn filter_tab_label<'a>(
    tab: &'a AgentFilterTab<'a>,
    show_icons: bool,
    show_labels: bool,
) -> &'a str {
    if show_labels || !show_icons || !tab.has_icon {
        tab.label
    } else {
        ""
    }
}

fn filter_tab_width(label: &str, count: Option<usize>, has_icon: bool) -> u16 {
    (label.width() + count_suffix(count).width()) as u16 + if has_icon { 5 } else { 3 }
}

fn compact_count(count: usize) -> String {
    if count < 1_000 {
        return count.to_string();
    }
    if count < 10_000 {
        let tenths = (count + 50) / 100;
        return if tenths.is_multiple_of(10) {
            format!("{}k", tenths / 10)
        } else {
            format!("{}.{}k", tenths / 10, tenths % 10)
        };
    }
    if count < 1_000_000 {
        return format!("{}k", (count + 500) / 1_000);
    }
    format!("{}m", (count + 500_000) / 1_000_000)
}

fn count_suffix(count: Option<usize>) -> String {
    match count {
        Some(0) | None => String::new(),
        Some(count) => format!(" · {}", compact_count(count)),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_filter_tab(
    frame: &mut Frame,
    area: Rect,
    x: u16,
    label: &str,
    count: Option<usize>,
    active: bool,
    color: Color,
    icon: Option<&Protocol>,
    theme: &Theme,
) -> u16 {
    let has_icon = icon.is_some();
    let suffix = count_suffix(count);
    let label_width = (label.width() + suffix.width()) as u16;
    let tab_width = filter_tab_width(label, count, has_icon);
    if x >= area.right() {
        return x.saturating_add(tab_width);
    }

    let background_style = if active {
        Style::new().bg(theme.filter_selected_bg)
    } else {
        Style::new()
    };
    let label_style = if active {
        Style::new()
            .fg(color)
            .bg(theme.filter_selected_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(color)
    };
    let count_style = if active {
        Style::new()
            .fg(theme.secondary)
            .bg(theme.filter_selected_bg)
    } else {
        Style::new().fg(theme.muted)
    };
    let label_line = || {
        Line::from(vec![
            Span::styled(label.to_string(), label_style),
            Span::styled(suffix.clone(), count_style),
        ])
    };

    let visible_width = tab_width.min(area.right().saturating_sub(x));
    frame.render_widget(
        Paragraph::new(" ".repeat(visible_width as usize)).style(background_style),
        Rect::new(x, area.y, visible_width, 1),
    );
    if active {
        frame.render_widget(
            Paragraph::new("▌").style(
                Style::new()
                    .fg(color)
                    .bg(theme.filter_selected_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Rect::new(x, area.y, 1.min(visible_width), 1),
        );
    }

    if let Some(protocol) = icon {
        if x + 1 < area.right() {
            let icon_width = 2.min(area.right().saturating_sub(x + 1));
            frame.render_widget(
                TuiImage::new(protocol).allow_clipping(true),
                Rect::new(x + 1, area.y, icon_width, 1),
            );
        }
        if x + 4 < area.right() {
            frame.render_widget(
                Paragraph::new(label_line()),
                Rect::new(x + 4, area.y, label_width.min(area.right() - (x + 4)), 1),
            );
        }
    } else if x + 1 < area.right() {
        frame.render_widget(
            Paragraph::new(label_line()),
            Rect::new(x + 1, area.y, label_width.min(area.right() - (x + 1)), 1),
        );
    }

    x.saturating_add(tab_width)
}

fn draw_main(frame: &mut Frame, layout: MainLayout, state: &AppState) {
    draw_results(frame, layout.results(), state);
    if let Some(preview) = layout.preview() {
        draw_preview(frame, preview, state);
    }
}

/// Where the result rows are inside the results panel (below the border and
/// the column header). Shared by drawing and mouse hit-testing.
pub(super) fn results_inner(results: Rect) -> Rect {
    Block::default()
        .borders(Borders::ALL)
        .inner(results)
        .inner(Margin {
            horizontal: 1,
            vertical: 0,
        })
}

pub(super) fn results_rows_area(results: Rect) -> Rect {
    let inner = results_inner(results);
    Rect::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    )
}

fn draw_results(frame: &mut Frame, area: Rect, state: &AppState) {
    let theme = &state.theme;
    let scope = state.ws.scope();
    let title = match scope {
        super::workspaces::ALL => Line::from(" Chats "),
        _ => {
            let color = state
                .ws
                .color_index(scope)
                .map_or(theme.secondary, |i| theme.ws_color(i));
            Line::from(vec![
                Span::styled(" ● ", Style::new().fg(color)),
                Span::raw(format!("{} ", state.ws.scope_label(scope))),
            ])
        }
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(AIRY)
        .border_style(Style::new().fg(if state.focus == Focus::Chats {
            theme.accent
        } else {
            theme.panel_border
        }))
        .title(title)
        .title_style(Style::new().fg(theme.secondary).bold());
    frame.render_widget(block, area);

    let inner = results_inner(area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let columns = result_columns(inner.width, state.shows_workspace_column());
    draw_result_header(frame, inner, columns, theme);

    let rows_area = results_rows_area(area);

    if state.visible.is_empty() {
        draw_empty_state(frame, rows_area, state);
        return;
    }

    let max_rows = rows_area.height as usize;
    if max_rows == 0 {
        return;
    }
    let (start, end) = state.results_window(max_rows);

    for (screen_row, session) in state.visible[start..end].iter().enumerate() {
        let row_y = rows_area.y + screen_row as u16;
        let selected = start + screen_row == state.selected;
        draw_result_row(frame, rows_area, row_y, columns, session, selected, state);
    }
}

fn draw_empty_state(frame: &mut Frame, area: Rect, state: &AppState) {
    use super::workspaces::{ALL, NONE, TRASH};
    let theme = &state.theme;
    let (headline, hint) = if !state.query.trim().is_empty() {
        (
            "No chats match",
            "Try fewer words, or press Ctrl+U to clear",
        )
    } else {
        match state.ws.scope() {
            ALL => ("No chats found", ""),
            NONE => (
                "Everything is filed",
                "Chats that are in no workspace show up here",
            ),
            TRASH => (
                "Nothing deleted",
                "Deleted chats and workspaces wait here to be restored",
            ),
            _ if state.in_deleted_view() => (
                "This deleted workspace is empty",
                "Restore it with the ↺ on its row, F8 or Alt+U",
            ),
            _ => (
                "Nothing here yet",
                "Drag a chat onto this workspace, or press F3 on a chat",
            ),
        }
    };
    let mut lines = vec![Line::styled(
        headline,
        Style::new().fg(theme.secondary).bold(),
    )];
    if !hint.is_empty() {
        lines.push(Line::styled(hint, Style::new().fg(theme.muted).italic()));
    }
    let top = area.height.saturating_sub(lines.len() as u16) / 3;
    let rect = Rect::new(
        area.x,
        area.y + top,
        area.width,
        area.height.saturating_sub(top),
    );
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), rect);
}

#[derive(Clone, Copy)]
pub(super) struct ResultColumns {
    agent_x: u16,
    agent_w: u16,
    pub(super) title_x: u16,
    pub(super) title_w: u16,
    ws_x: u16,
    ws_w: u16,
    dir_x: u16,
    dir_w: u16,
    turns_x: u16,
    turns_w: u16,
    age_x: u16,
    age_w: u16,
}

pub(super) fn result_columns(width: u16, show_workspace: bool) -> ResultColumns {
    let (agent_w, mut dir_w, turns_w, age_w): (u16, u16, u16, u16) = if width >= 100 {
        (15, 32, 7, 10)
    } else if width >= 72 {
        (13, 22, 6, 9)
    } else {
        (13, 0, 5, 8)
    };
    // Where chats of several workspaces mix, a colored Workspace column tells
    // them apart; it takes its room from the directory.
    let ws_w = match (show_workspace, width) {
        (true, 100..) => 16,
        (true, 72..) => 12,
        _ => 0,
    };
    if ws_w > 0 {
        // Titles matter most: the directory shrinks, and goes at mid widths.
        dir_w = if width >= 100 {
            dir_w.saturating_sub(ws_w + 2).max(10)
        } else {
            0
        };
    }
    let ws_gap = if ws_w > 0 { ws_w + 1 } else { 0 };
    let fixed = agent_w + dir_w + turns_w + age_w + ws_gap + 4;
    let title_w = width.saturating_sub(fixed).max(16);
    let agent_x = 0;
    let title_x = agent_x + agent_w + 1;
    let ws_x = title_x + title_w + 1;
    let dir_x = ws_x + ws_gap;
    let turns_x = dir_x + dir_w + 1;
    let age_x = turns_x + turns_w + 1;
    ResultColumns {
        agent_x,
        agent_w,
        title_x,
        title_w,
        ws_x,
        ws_w,
        dir_x,
        dir_w,
        turns_x,
        turns_w,
        age_x,
        age_w,
    }
}

fn agent_badge(agent: &str) -> &str {
    AGENTS
        .get(agent)
        .map(|config| config.badge)
        .unwrap_or(agent)
}

fn draw_result_header(frame: &mut Frame, inner: Rect, columns: ResultColumns, theme: &Theme) {
    let style = Style::new().fg(theme.secondary).bold();
    draw_cell(
        frame,
        inner,
        columns.agent_x,
        0,
        columns.agent_w,
        "  Agent",
        style,
    );
    draw_cell(
        frame,
        inner,
        columns.title_x,
        0,
        columns.title_w,
        "Chat",
        style,
    );
    if columns.ws_w > 0 {
        draw_cell(
            frame,
            inner,
            columns.ws_x,
            0,
            columns.ws_w,
            "Workspace",
            style,
        );
    }
    if columns.dir_w > 0 {
        draw_cell(
            frame,
            inner,
            columns.dir_x,
            0,
            columns.dir_w,
            "Folder",
            style,
        );
    }
    draw_cell(
        frame,
        inner,
        columns.turns_x,
        0,
        columns.turns_w,
        "Msgs",
        style,
    );
    draw_cell(
        frame,
        inner,
        columns.age_x,
        0,
        columns.age_w,
        "Active",
        style,
    );
}

fn draw_result_row(
    frame: &mut Frame,
    rows_area: Rect,
    row_y: u16,
    columns: ResultColumns,
    session: &Session,
    selected: bool,
    state: &AppState,
) {
    let row_style = if selected {
        Style::new()
            .bg(state.theme.selected_bg)
            .fg(state.theme.selected_fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    frame.render_widget(
        Paragraph::new(" ".repeat(rows_area.width as usize)).style(row_style),
        Rect::new(rows_area.x, row_y, rows_area.width, 1),
    );

    let agent_config = AGENTS.get(session.agent.as_str());
    let agent_color = agent_config
        .map(|agent| state.theme.agent_color(agent))
        .unwrap_or(state.theme.foreground);
    let agent_label = agent_badge(&session.agent);
    let pointer = if selected { "▌" } else { " " };
    draw_cell(
        frame,
        rows_area,
        0,
        row_y - rows_area.y,
        1,
        pointer,
        row_style.fg(state.theme.accent),
    );

    let label_x = if let Some(protocol) = state
        .images
        .as_ref()
        .filter(|_| !state.overlay_open())
        .and_then(|images| images.row.get(&session.agent))
    {
        frame.render_widget(
            TuiImage::new(protocol).allow_clipping(true),
            Rect::new(rows_area.x + 2, row_y, 2, 1),
        );
        5
    } else {
        2
    };

    draw_cell(
        frame,
        rows_area,
        label_x,
        row_y - rows_area.y,
        columns.agent_w.saturating_sub(label_x),
        &truncate(
            agent_label,
            columns.agent_w.saturating_sub(label_x) as usize,
        ),
        row_style.fg(agent_color).add_modifier(Modifier::BOLD),
    );
    match state.edit.as_ref() {
        Some(edit) if matches!(&edit.target, EditTarget::Chat { id, .. } if *id == session.id) => {
            let (text, cursor) =
                search_input_view(&edit.line.text, edit.line.cursor, columns.title_w as usize);
            draw_cell(
                frame,
                rows_area,
                columns.title_x,
                row_y - rows_area.y,
                columns.title_w,
                &text,
                Style::new()
                    .fg(state.theme.accent)
                    .add_modifier(Modifier::UNDERLINED),
            );
            frame.set_cursor_position((rows_area.x + columns.title_x + cursor as u16, row_y));
        }
        _ => draw_cell(
            frame,
            rows_area,
            columns.title_x,
            row_y - rows_area.y,
            columns.title_w,
            &truncate(state.title_of(session), columns.title_w as usize),
            row_style,
        ),
    }
    if columns.ws_w > 0 && columns.ws_x < rows_area.width {
        let width = columns.ws_w.min(rows_area.width - columns.ws_x);
        let cell = Rect::new(rows_area.x + columns.ws_x, row_y, width, 1);
        let line = match state.ws_tag(session) {
            Some((name, index)) => Line::from(vec![
                Span::styled("● ", row_style.fg(state.theme.ws_color(index))),
                Span::styled(
                    truncate(name, columns.ws_w.saturating_sub(2) as usize),
                    row_style.fg(state.theme.secondary),
                ),
            ]),
            None => Line::styled("–", row_style.fg(state.theme.muted)),
        };
        frame.render_widget(Paragraph::new(line), cell);
    }
    if columns.dir_w > 0 {
        draw_cell(
            frame,
            rows_area,
            columns.dir_x,
            row_y - rows_area.y,
            columns.dir_w,
            &truncate(&session.display_directory(), columns.dir_w as usize),
            row_style.fg(state.theme.muted),
        );
    }
    draw_cell(
        frame,
        rows_area,
        columns.turns_x,
        row_y - rows_area.y,
        columns.turns_w,
        &session.message_count.to_string(),
        row_style,
    );
    draw_cell(
        frame,
        rows_area,
        columns.age_x,
        row_y - rows_area.y,
        columns.age_w,
        &time_ago(session.timestamp),
        age_style(session.timestamp, &state.theme).bg(row_style.bg.unwrap_or(Color::Reset)),
    );
    if selected && rows_area.width > 6 {
        let y = row_y - rows_area.y;
        if state.in_deleted_view() {
            // ↺ brings it back; the red ✕ deletes it for good (asks first).
            draw_cell(
                frame,
                rows_area,
                rows_area.width - 4,
                y,
                1,
                "↺",
                row_style.fg(state.theme.accent),
            );
            draw_cell(
                frame,
                rows_area,
                rows_area.width - 1,
                y,
                1,
                "✕",
                row_style.fg(state.theme.error),
            );
        } else {
            draw_cell(
                frame,
                rows_area,
                rows_area.width - 1,
                y,
                1,
                "✕",
                row_style.fg(state.theme.muted),
            );
        }
    }
}

fn draw_cell(frame: &mut Frame, area: Rect, x: u16, y: u16, width: u16, text: &str, style: Style) {
    if width == 0 || x >= area.width || y >= area.height {
        return;
    }
    frame.render_widget(
        Paragraph::new(truncate(text, width as usize)).style(style),
        Rect::new(area.x + x, area.y + y, width.min(area.width - x), 1),
    );
}

fn draw_preview(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(AIRY)
        .border_style(Style::new().fg(state.theme.panel_border))
        .title(" Preview ")
        .title_style(Style::new().fg(state.theme.secondary).bold());
    let inner = block.inner(area).inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    frame.render_widget(block, area);

    let Some(session) = state.selected_session() else {
        frame.render_widget(
            Paragraph::new("No session selected").style(Style::new().fg(state.theme.muted)),
            inner,
        );
        return;
    };

    let agent_color = AGENTS
        .get(session.agent.as_str())
        .map(|agent| state.theme.agent_color(agent))
        .unwrap_or(state.theme.foreground);
    let mut header_lines = vec![
        Line::from(vec![
            Span::styled(&session.agent, Style::new().fg(agent_color).bold()),
            Span::raw("  "),
            Span::styled(state.title_of(session), Style::new().bold()),
        ]),
        Line::from(vec![
            Span::styled(
                session.display_directory(),
                Style::new().fg(state.theme.muted),
            ),
            Span::raw("  "),
            Span::styled(
                session.timestamp.format("%Y-%m-%d %H:%M").to_string(),
                Style::new().fg(state.theme.muted),
            ),
        ]),
    ];

    if let Some((name, index)) = state.ws_tag(session) {
        header_lines[1].spans.insert(
            0,
            Span::styled(
                format!("● {name}  "),
                Style::new().fg(state.theme.ws_color(index)),
            ),
        );
    }

    let mut body_area = inner;
    if let Some(protocol) = state
        .images
        .as_ref()
        .filter(|_| !state.overlay_open())
        .and_then(|images| images.preview.get(&session.agent))
        && inner.width > 48
        && inner.height > 7
    {
        let logo_area = Rect::new(inner.right().saturating_sub(8), inner.y, 8, 4);
        let text_area = Rect::new(inner.x, inner.y, inner.width.saturating_sub(9), 3);
        frame.render_widget(Paragraph::new(Text::from(header_lines.clone())), text_area);
        frame.render_widget(TuiImage::new(protocol).allow_clipping(true), logo_area);
        body_area = Rect::new(
            inner.x,
            inner.y + 4,
            inner.width,
            inner.height.saturating_sub(4),
        );
    }

    let mut lines = if body_area.y == inner.y {
        let mut lines = header_lines;
        lines.push(Line::raw(""));
        lines
    } else {
        Vec::new()
    };

    lines.extend(state.preview_lines(session).into_iter().take(220));

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((state.preview_scroll, 0)),
        body_area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, state: &AppState) {
    frame.render_widget(
        Paragraph::new(footer_line(
            &state.status,
            area.width,
            &state.theme,
            state.in_deleted_view(),
        )),
        area,
    );
}

fn shortcut_footer(theme: &Theme, compact: bool, deleted_view: bool) -> Line<'static> {
    let mut spans = vec![
        Span::styled(" Enter ", Style::new().fg(theme.accent).bold()),
        Span::raw(" resume  "),
        Span::styled(" Ctrl+Y ", Style::new().fg(theme.key_fg)),
        Span::raw(" copy  "),
        Span::styled(" Tab ", Style::new().fg(theme.key_fg)),
        Span::raw(" agent  "),
        Span::styled(" Ctrl+P ", Style::new().fg(theme.key_fg)),
        Span::raw(" preview  "),
        Span::styled(" F1 ", Style::new().fg(theme.key_fg)),
        Span::raw(" help  "),
        Span::styled(" Esc ", Style::new().fg(theme.key_fg)),
        Span::raw(" quit"),
    ];
    if !compact {
        // Workspace shortcuts go right after the resume/copy pair.
        let mut extra = vec![
            Span::styled(" F2 ", Style::new().fg(theme.key_fg)),
            Span::raw(" rename  "),
            Span::styled(" F3 ", Style::new().fg(theme.key_fg)),
            Span::raw(" move  "),
            Span::styled(" F8 ", Style::new().fg(theme.key_fg)),
            Span::raw(if deleted_view {
                " restore  "
            } else {
                " delete  "
            }),
        ];
        if deleted_view {
            extra.push(Span::styled(" F9 ", Style::new().fg(theme.error).bold()));
            extra.push(Span::raw(" erase  "));
        }
        spans.splice(4..4, extra);
    }
    Line::from(spans)
}

fn footer_line(status: &str, width: u16, theme: &Theme, deleted_view: bool) -> Line<'static> {
    let full = shortcut_footer(theme, false, deleted_view);
    if status.trim().is_empty() {
        return full;
    }

    // With a status showing, drop the extra shortcuts before truncating it.
    let width = width as usize;
    let shortcuts = if line_width(&full) + status.width() + 2 <= width {
        full
    } else {
        shortcut_footer(theme, true, deleted_view)
    };
    let shortcut_width = line_width(&shortcuts);
    if width <= shortcut_width + 4 {
        return Line::from(Span::styled(
            truncate(status, width),
            Style::new().fg(theme.warning),
        ));
    }

    let status_width = width.saturating_sub(shortcut_width + 2);
    let status = truncate(status, status_width);
    let mut spans = vec![
        Span::styled(status, Style::new().fg(theme.warning)),
        Span::raw("  "),
    ];
    spans.extend(shortcuts.spans);
    Line::from(spans)
}

fn draw_help_modal(frame: &mut Frame, area: Rect, theme: &Theme) {
    let popup = centered_rect(78, 35, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent))
        .title(format!(" {APP_NAME} · keyboard shortcuts "));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let section = |name: &'static str| Line::styled(name, Style::new().bold().fg(theme.accent));
    let text = vec![
        section("Search"),
        help_row(
            theme,
            "  Type|Filter titles, messages, paths (agent:claude)",
        ),
        help_row(
            theme,
            "  ← → Home End|Move the cursor (also Ctrl+B / F / A / E)",
        ),
        help_row(theme, "  Ctrl+W / Ctrl+U|Delete word / to start"),
        help_row(theme, "  Tab / Shift+Tab|Complete / cycle agent"),
        Line::raw(""),
        section("Chats"),
        help_row(theme, "  ← →|Move between the workspace list and the chats"),
        help_row(theme, "  /|Jump to search (typing anywhere also searches)"),
        help_row(
            theme,
            "  ↑ ↓, Ctrl+K / Ctrl+J|Move selection (Page Up / Down: by 10)",
        ),
        help_row(theme, "  Enter|Resume session"),
        help_row(theme, "  Ctrl+Y|Copy resume command"),
        help_row(theme, "  Ctrl+P|Toggle preview (Alt++ / Alt+- scrolls it)"),
        Line::raw(""),
        section("Workspaces"),
        help_row(theme, "  F2 or double-click a name|Rename the chat"),
        help_row(theme, "  F3 / Alt+M or drag|Move the chat to a workspace"),
        help_row(
            theme,
            "  F8 / Alt+X|Delete the chat (in Deleted: restore it)",
        ),
        help_row(theme, "  Alt+↑ / Alt+↓|Previous / next workspace"),
        help_row(theme, "  F4 / Alt+N|New workspace"),
        help_row(theme, "  Alt+R or double-click|Rename the open workspace"),
        help_row(
            theme,
            "  Alt+D or ×|Delete the open workspace and its chats",
        ),
        help_row(theme, "  Alt+U|Restore the workspace of a deleted chat"),
        help_row(
            theme,
            "  Delete in Deleted, or F9|Delete for good (asks first)",
        ),
        help_row(theme, "  Ctrl+O|Show / hide the sidebar"),
        help_row(
            theme,
            "  Deleted keeps chats and workspaces: ↺ restores them, F9 erases for good",
        ),
        Line::raw(""),
        section("General"),
        help_row(theme, "  Mouse wheel|Scroll under the pointer"),
        help_row(
            theme,
            "  Esc|Back to the list, then quit (Ctrl+C quits at once)",
        ),
        Line::raw(""),
        Line::styled(
            "F1 or Esc closes this help",
            Style::new().fg(theme.muted).italic(),
        ),
    ];
    frame.render_widget(Paragraph::new(text), inner);
}

/// A help line written as "  keys|description": keys are picked out and the
/// descriptions line up. Lines without a `|` are plain notes.
fn help_row(theme: &Theme, line: &'static str) -> Line<'static> {
    let body = line.trim_start();
    match body.split_once('|') {
        Some((keys, rest)) => Line::from(vec![
            Span::styled(
                format!("  {keys:<27}"),
                Style::new().fg(theme.secondary).bold(),
            ),
            Span::raw(rest.trim_start()),
        ]),
        None => Line::styled(format!("  {body}"), Style::new().fg(theme.muted).italic()),
    }
}

fn draw_yolo_modal(frame: &mut Frame, area: Rect, modal: &YoloModal, theme: &Theme) {
    let popup = centered_rect(48, 8, area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.warning))
        .title(" Yolo mode ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let text = vec![
        Line::from(yolo_modal_prompt(modal.action)),
        Line::raw(""),
        Line::from(vec![
            button_span(" No ", !modal.selected, theme),
            Span::raw("  "),
            button_span(" Yolo ", modal.selected, theme),
        ])
        .alignment(Alignment::Center),
    ];
    frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), inner);
}

fn yolo_modal_prompt(action: PendingAction) -> &'static str {
    match action {
        PendingAction::Resume => "Resume with auto-approve / skip-permissions flags?",
        PendingAction::Copy => "Copy command with auto-approve / skip-permissions flags?",
    }
}

fn button_span(label: &'static str, selected: bool, theme: &Theme) -> Span<'static> {
    if selected {
        Span::styled(label, Style::new().fg(theme.warning).bold().underlined())
    } else {
        Span::styled(label, Style::new().fg(theme.secondary))
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(area.width.saturating_sub(width) / 2),
            Constraint::Length(width.min(area.width)),
            Constraint::Min(0),
        ])
        .split(area);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(area.height.saturating_sub(height) / 2),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(horizontal[1]);
    vertical[1]
}

fn draw_sidebar(frame: &mut Frame, area: Rect, state: &AppState) {
    let focused = state.focus == Focus::Sidebar;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(AIRY)
        .border_style(Style::new().fg(if focused {
            state.theme.accent
        } else {
            state.theme.panel_border
        }))
        .title(" Workspaces ")
        .title_style(Style::new().fg(state.theme.secondary).bold());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 12 {
        return;
    }

    let theme = &state.theme;
    let scope = state.ws.scope();
    let hover = state
        .drag
        .as_ref()
        .filter(|drag| drag.active)
        .and_then(|drag| layout::sidebar_row_at(area, drag.pos.0, drag.pos.1));
    let rows = state.sidebar_rows();

    for (i, row) in rows.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let y = i as u16;
        let selected = row.kind != RowKind::New && row.scope == scope;
        let hovered = hover == Some(i) && row.droppable();
        let style = if hovered {
            Style::new().fg(theme.accent).bold().underlined()
        } else if selected {
            Style::new()
                .bg(theme.selected_bg)
                .fg(theme.selected_fg)
                .bold()
        } else {
            Style::new()
        };
        frame.render_widget(
            Paragraph::new(" ".repeat(inner.width as usize)).style(style),
            Rect::new(inner.x, inner.y + y, inner.width, 1),
        );

        if row.kind == RowKind::New {
            // A button, not a row of text.
            let width = inner.width.saturating_sub(4).min(20);
            draw_cell(
                frame,
                inner,
                2,
                y,
                width,
                " + New workspace",
                Style::new()
                    .fg(theme.accent)
                    .bg(theme.filter_selected_bg)
                    .bold(),
            );
            continue;
        }

        if selected {
            let bar = if focused { theme.accent } else { theme.muted };
            draw_cell(frame, inner, 0, y, 1, "▌", style.fg(bar));
        }
        let (icon, icon_color) = match row.kind {
            RowKind::All => ("◈", theme.secondary),
            RowKind::Workspace => (
                "●",
                row.color.map_or(theme.secondary, |i| theme.ws_color(i)),
            ),
            RowKind::Unsorted => ("○", theme.secondary),
            RowKind::Deleted => ("⊘", theme.muted),
            RowKind::DeletedWorkspace => {
                ("●", row.color.map_or(theme.muted, |i| theme.ws_color(i)))
            }
            RowKind::New => ("+", theme.accent),
        };
        let icon_style = if hovered { style } else { style.fg(icon_color) };
        draw_cell(frame, inner, 2, y, 1, icon, icon_style);

        let count = row.count.to_string();
        // Deleted workspaces carry two glyphs on the right, so the count moves.
        let glyph_room = if row.kind == RowKind::DeletedWorkspace {
            5
        } else {
            3
        };
        let count_x = inner.width.saturating_sub(count.len() as u16 + glyph_room);
        let name_w = count_x.saturating_sub(5);
        let muted_row = matches!(row.kind, RowKind::Deleted | RowKind::DeletedWorkspace);
        let editing = state
            .edit
            .as_ref()
            .filter(|edit| matches!(&edit.target, EditTarget::Workspace(id) if *id == row.scope));
        if let Some(edit) = editing {
            let (text, cursor) =
                search_input_view(&edit.line.text, edit.line.cursor, name_w as usize);
            let (text, edit_style) = if text.is_empty() {
                (
                    "name…".to_string(),
                    Style::new().fg(theme.muted).italic().underlined(),
                )
            } else {
                (text, Style::new().fg(theme.accent).underlined())
            };
            draw_cell(frame, inner, 4, y, name_w, &text, edit_style);
            frame.set_cursor_position((inner.x + 4 + cursor as u16, inner.y + y));
        } else {
            let name_style = if muted_row && !hovered {
                style.fg(theme.muted)
            } else {
                style
            };
            draw_cell(
                frame,
                inner,
                4,
                y,
                name_w,
                &truncate(&row.name, name_w as usize),
                name_style,
            );
        }
        draw_cell(
            frame,
            inner,
            count_x,
            y,
            count.len() as u16,
            &count,
            if hovered {
                style
            } else {
                style.fg(theme.muted).remove_modifier(Modifier::BOLD)
            },
        );
        if selected {
            match row.kind {
                RowKind::Workspace => {
                    draw_cell(
                        frame,
                        inner,
                        inner.width - 2,
                        y,
                        1,
                        "×",
                        style.fg(theme.muted),
                    );
                }
                RowKind::DeletedWorkspace => {
                    draw_cell(
                        frame,
                        inner,
                        inner.width - 3,
                        y,
                        1,
                        "↺",
                        style.fg(theme.accent),
                    );
                    draw_cell(
                        frame,
                        inner,
                        inner.width - 1,
                        y,
                        1,
                        "✕",
                        style.fg(theme.error),
                    );
                }
                _ => {}
            }
        }
    }

    // A quiet hint at the bottom while there is room.
    if inner.height > rows.len() as u16 + 2 {
        draw_cell(
            frame,
            inner,
            2,
            inner.height - 1,
            inner.width - 2,
            if focused {
                "↑↓ pick · → chats"
            } else {
                "← here · drag to file"
            },
            Style::new().fg(theme.muted).italic(),
        );
    }
}

/// The dialog and its two buttons: (dialog, cancel, delete for good).
pub(super) fn confirm_buttons(area: Rect) -> (Rect, Rect, Rect) {
    let popup = centered_rect(62, 9, area);
    let y = popup.bottom().saturating_sub(2);
    let cancel = Rect::new(popup.x + 6, y, 14, 1);
    let erase = Rect::new(popup.right().saturating_sub(26), y, 20, 1);
    (popup, cancel, erase)
}

fn draw_confirm(frame: &mut Frame, area: Rect, action: &ConfirmAction, theme: &Theme) {
    let (popup, cancel, erase) = confirm_buttons(area);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.error))
        .title(" Delete for good ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let (headline, detail) = match action {
        ConfirmAction::PurgeChat { title, .. } => (
            format!("Delete “{}” for good?", truncate(title, 38)),
            "It will never show up in frw again.".to_string(),
        ),
        ConfirmAction::PurgeWorkspace { name, chats, .. } => (
            format!("Delete “{}” for good?", truncate(name, 38)),
            format!("The workspace and its {chats} chat(s) will never show again."),
        ),
    };
    let text = vec![
        Line::from(headline).style(Style::new().bold()),
        Line::styled(detail, Style::new().fg(theme.secondary)),
        Line::styled(
            "The chat files on disk are not touched.",
            Style::new().fg(theme.muted).italic(),
        ),
    ];
    frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), inner);
    frame.render_widget(
        Paragraph::new(" Cancel (n) ").style(Style::new().fg(theme.secondary)),
        cancel,
    );
    frame.render_widget(
        Paragraph::new(" Delete for good (y) ")
            .style(Style::new().fg(theme.error).bold().underlined()),
        erase,
    );
}

fn draw_drag_ghost(frame: &mut Frame, area: Rect, state: &AppState) {
    let Some(drag) = state.drag.as_ref().filter(|drag| drag.active) else {
        return;
    };
    let label = format!(" ↪ {} ", truncate(&drag.title, 24));
    let x = drag.pos.0.saturating_add(2);
    // Float above the pointer, so the row being dropped on stays readable.
    let y = if drag.pos.1 > area.y {
        drag.pos.1 - 1
    } else {
        drag.pos.1 + 1
    };
    if x >= area.right() || y >= area.bottom() {
        return;
    }
    let width = (label.width() as u16).min(area.right() - x);
    // Clear what is underneath (no fill, just no text) so the label reads.
    frame.render_widget(Clear, Rect::new(x, y, width, 1));
    frame.render_widget(
        Paragraph::new(label).style(Style::new().fg(state.theme.accent).bold()),
        Rect::new(x, y, width, 1),
    );
}

pub(super) fn picker_rect(area: Rect, items: usize) -> Rect {
    centered_rect(40, (items as u16 + 4).min(area.height), area)
}

fn draw_picker(frame: &mut Frame, area: Rect, picker: &Picker, state: &AppState) {
    let theme = &state.theme;
    let popup = picker_rect(area, picker.items.len());
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent))
        .title(" Move to workspace ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let width = inner.width as usize;
    let mut lines: Vec<Line> = picker
        .items
        .iter()
        .enumerate()
        .map(|(i, (id, name))| {
            let row = if i == picker.selected {
                Style::new()
                    .bg(theme.selected_bg)
                    .fg(theme.selected_fg)
                    .bold()
            } else {
                Style::new()
            };
            let (dot, color) = match state.ws.color_index(id) {
                Some(index) if state.ws.name_of(id).is_some() => ("●", theme.ws_color(index)),
                _ => ("○", theme.secondary),
            };
            let label = truncate(name, width.saturating_sub(5));
            let pad = width.saturating_sub(label.width() + 3);
            Line::from(vec![
                Span::styled(" ", row),
                Span::styled(dot, row.fg(color)),
                Span::styled(format!(" {label}{}", " ".repeat(pad)), row),
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        " Enter move · Esc cancel",
        Style::new().fg(theme.muted).italic(),
    ));
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter_tabs(active: Option<usize>) -> Vec<AgentFilterTab<'static>> {
        [
            "claude", "codex", "copilot", "crush", "opencode", "vibe", "vscode",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, label)| AgentFilterTab {
            label,
            count: 124,
            has_icon: true,
            active: active == Some(index),
        })
        .collect()
    }

    fn filter_layout_width(
        tabs: &[AgentFilterTab<'_>],
        show_counts: bool,
        show_icons: bool,
        show_labels: bool,
    ) -> u16 {
        filter_tab_width("All", None, false)
            + tabs
                .iter()
                .map(|tab| {
                    filter_tab_width(
                        filter_tab_label(tab, show_icons, show_labels),
                        show_counts.then_some(tab.count),
                        show_icons && tab.has_icon,
                    )
                })
                .sum::<u16>()
    }

    #[test]
    fn filter_layout_drops_counts_before_icons() {
        let tabs = filter_tabs(None);
        let icons_only_width = filter_layout_width(&tabs, false, true, true);
        let layout = plan_filter_layout(icons_only_width, &tabs, true);

        assert!(!layout.show_counts);
        assert!(layout.show_icons);
        assert!(layout.show_labels);
        assert_eq!(layout.visible_agents, 0..tabs.len());
    }

    #[test]
    fn filter_layout_drops_icons_after_counts() {
        let tabs = filter_tabs(None);
        let labels_only_width = filter_layout_width(&tabs, false, false, true);
        let layout = plan_filter_layout(labels_only_width, &tabs, true);

        assert!(!layout.show_counts);
        assert!(!layout.show_icons);
        assert!(layout.show_labels);
        assert_eq!(layout.visible_agents, 0..tabs.len());
    }

    #[test]
    fn filter_layout_uses_icon_only_tabs_as_the_last_full_bar_stage() {
        let tabs = filter_tabs(None);
        let icon_only_width = filter_layout_width(&tabs, false, true, false);
        let layout = plan_filter_layout(icon_only_width, &tabs, true);

        assert!(!layout.show_counts);
        assert!(layout.show_icons);
        assert!(!layout.show_labels);
        assert_eq!(layout.visible_agents, 0..tabs.len());
    }

    #[test]
    fn filter_layout_keeps_active_agent_visible_when_tabs_overflow() {
        let tabs = filter_tabs(Some(6));
        let width =
            filter_tab_width("All", None, false) + filter_tab_width(tabs[6].label, None, false);
        let layout = plan_filter_layout(width, &tabs, false);

        assert!(layout.show_all);
        assert!(layout.show_icons);
        assert!(!layout.show_labels);
        assert!(layout.visible_agents.contains(&6));
        assert!(!layout.visible_agents.contains(&0));
    }

    #[test]
    fn filter_layout_prioritizes_active_agent_in_extreme_widths() {
        let tabs = filter_tabs(Some(6));
        let width = filter_tab_width(tabs[6].label, None, false);
        let layout = plan_filter_layout(width, &tabs, false);

        assert!(!layout.show_all);
        assert!(layout.show_icons);
        assert!(!layout.show_labels);
        assert_eq!(layout.visible_agents, 6..7);
    }

    #[test]
    fn agent_filter_counts_are_compact_and_zero_is_hidden() {
        assert_eq!(count_suffix(Some(124)), " · 124");
        assert_eq!(count_suffix(Some(1_249)), " · 1.2k");
        assert_eq!(count_suffix(Some(15_200)), " · 15k");
        assert_eq!(count_suffix(Some(0)), "");
    }

    #[test]
    fn result_rows_use_short_agent_badges() {
        assert_eq!(AGENTS["antigravity"].badge, "agy");
        assert_eq!(agent_badge("antigravity"), "agy");
        assert_eq!(agent_badge("copilot-cli"), "copilot");
        assert_eq!(agent_badge("copilot-vscode"), "vscode");
        assert_eq!(agent_badge("unknown-agent"), "unknown-agent");
    }

    #[test]
    fn narrow_results_reserve_room_for_the_longest_agent_badge() {
        let columns = result_columns(60, false);

        assert_eq!(columns.agent_w, 13);
        assert_eq!(columns.agent_w - 5, "opencode".width() as u16);
    }

    #[test]
    fn footer_renders_status_when_present() {
        let line = footer_line("copied: codex resume abc", 120, &Theme::dark(), false);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(rendered.contains("copied: codex resume abc"));
        assert!(rendered.contains("Enter"));
        assert!(rendered.contains("F1"));
    }

    #[test]
    fn footer_prefers_status_on_narrow_width() {
        let line = footer_line(
            "clipboard unavailable: codex resume abc",
            18,
            &Theme::dark(),
            false,
        );
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(rendered.starts_with("clipboard"));
        assert!(!rendered.contains("Enter"));
    }

    #[test]
    fn yolo_modal_prompt_matches_pending_action() {
        assert_eq!(
            yolo_modal_prompt(PendingAction::Resume),
            "Resume with auto-approve / skip-permissions flags?"
        );
        assert_eq!(
            yolo_modal_prompt(PendingAction::Copy),
            "Copy command with auto-approve / skip-permissions flags?"
        );
    }

    #[test]
    fn search_input_view_keeps_end_cursor_visible() {
        let (visible, cursor) = search_input_view("0123456789abcdef", 16, 6);

        assert_eq!(visible, "bcdef");
        assert_eq!(cursor, 5);
    }

    #[test]
    fn search_input_view_keeps_middle_cursor_visible() {
        let (visible, cursor) = search_input_view("0123456789abcdef", 10, 6);

        assert_eq!(visible, "56789a");
        assert_eq!(cursor, 5);
    }

    #[test]
    fn workspace_column_keeps_titles_readable_and_fits_the_row() {
        for width in [60u16, 72, 90, 100, 140] {
            let plain = result_columns(width, false);
            let with_ws = result_columns(width, true);
            assert_eq!(plain.ws_w, 0);
            if width < 72 {
                assert_eq!(with_ws.ws_w, 0, "too narrow for a workspace column");
            } else {
                assert!(with_ws.ws_w > 0);
                // The title must stay usable, and the last column must fit.
                assert!(with_ws.title_w >= 16, "title at width {width}");
                assert!(
                    with_ws.age_x + with_ws.age_w <= width.max(with_ws.age_x + with_ws.age_w),
                    "layout is well-formed"
                );
                assert!(with_ws.ws_x > with_ws.title_x);
                assert!(with_ws.dir_x >= with_ws.ws_x + with_ws.ws_w);
            }
        }
        // At mid widths the directory makes way for the workspace column.
        assert_eq!(result_columns(90, true).dir_w, 0);
        assert!(result_columns(140, true).dir_w >= 10);
    }
}
