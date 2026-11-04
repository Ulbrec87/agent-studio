//! Grid view: project tabs, agent tiles, broadcast bar. The overview placeholder lives here too.
//!
//! Tiles show a *window* of the agent's live screen instead of resizing the agent's PTY (that
//! would reflow Claude's UI in the Chat view): the rows end at the last line with content (or the
//! cursor) so the bottom of the conversation and the prompt are visible, and the columns start at
//! the left edge unless the cursor is further right.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Widget};

use crate::app::{App, Focus, SubState};
use crate::grid::{self, RUNNING_TAB};
use crate::sidebar::{self, Visual};
use crate::ui::layout::Layout;

fn visual_color(app: &App, v: Visual) -> ratatui::style::Color {
    let t = &app.theme;
    match v {
        Visual::Running => t.running,
        Visual::Starting => t.starting,
        Visual::Waiting => t.waiting,
        Visual::Idle => t.idle,
        Visual::Errored => t.error,
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}\u{2026}")
}

/// Draw the whole grid view; returns the cursor position when a tile has the keyboard.
pub fn render(app: &mut App, l: &Layout, buf: &mut Buffer) -> Option<(u16, u16)> {
    render_tabs(app, l.title, buf);
    let mut cursor = None;
    let layout = app.grid_layout();
    if layout.tiles.is_empty() {
        render_empty(app, l.terminal, buf);
    }
    let running_tab = app.grid.tab == RUNNING_TAB;
    for tile in &layout.tiles {
        let Some(id) = app.grid.tiles.get(tile.index) else {
            continue;
        };
        let focused = app.selected.as_deref() == Some(id.as_str());
        if let Some(c) = render_tile(app, tile.rect, id, focused, running_tab, buf) {
            cursor = Some(c);
        }
    }
    if let Some(bar) = l.input
        && app.focus == Focus::Broadcast
    {
        let t = app.theme;
        let n = app.broadcast_targets().len();
        let hint = format!(
            " Broadcast to {n} running agent{} \u{00B7} Enter send \u{00B7} Alt+Enter newline \u{00B7} Esc close ",
            if n == 1 { "" } else { "s" }
        );
        let modal = app.modal.is_some();
        let c = super::chat::render_bar(&t, &mut app.broadcast, bar, buf, !modal, &hint);
        cursor = c.or(cursor);
    }
    cursor
}

fn render_tabs(app: &App, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let t = &app.theme;
    let base = Style::default().fg(t.text_secondary).bg(t.bg);
    for x in area.x..area.right() {
        if let Some(c) = buf.cell_mut((x, area.y)) {
            c.set_symbol(" ");
            c.set_style(base);
        }
    }
    let tabs = app.grid_tabs();
    let active = app.grid_tab_index();
    // The right edge shows the layout mode; the tabs get the rest.
    let mut info = format!(
        "cols {}",
        match app.config.grid_columns {
            as_core::GridColumns::Auto => "auto",
            as_core::GridColumns::Two => "2",
            as_core::GridColumns::Three => "3",
        }
    );
    if app.grid.only_running && app.grid.tab != RUNNING_TAB {
        info.push_str(" \u{00B7} running only");
    }
    info.push(' ');
    let info_w = u16::try_from(info.chars().count()).unwrap_or(0);
    let tabs_w = area.width.saturating_sub(info_w + 1);
    for (i, off, w) in grid::tab_slots(&tabs, active, tabs_w) {
        let tab = &tabs[i];
        let style = if i == active {
            Style::default()
                .fg(t.accent_text)
                .bg(t.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.text_secondary).bg(t.surface_raised)
        };
        let text = format!(" {} {} ", tab.label, tab.count);
        buf.set_string(area.x + off, area.y, clip(&text, usize::from(w)), style);
    }
    if area.width > info_w {
        buf.set_string(
            area.right() - info_w,
            area.y,
            &info,
            Style::default().fg(t.text_muted).bg(t.bg),
        );
    }
}

fn render_empty(app: &App, area: Rect, buf: &mut Buffer) {
    let t = &app.theme;
    let blank = Style::default().fg(t.text).bg(t.bg);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_symbol(" ");
                c.set_style(blank);
            }
        }
    }
    if area.height == 0 {
        return;
    }
    let text = if app.agents.is_empty() {
        format!(
            "No agents yet \u{2014} {} n creates one",
            app.prefix_label()
        )
    } else if app.grid.tab == RUNNING_TAB {
        "No agent is running".to_string()
    } else {
        "No agents in this tab".to_string()
    };
    let y = area.y + area.height / 2;
    let w = u16::try_from(text.chars().count())
        .unwrap_or(0)
        .min(area.width);
    buf.set_string(
        area.x + (area.width - w) / 2,
        y,
        clip(&text, usize::from(area.width)),
        Style::default().fg(t.text_muted).bg(t.bg),
    );
}

fn render_tile(
    app: &App,
    rect: Rect,
    id: &str,
    focused: bool,
    show_project: bool,
    buf: &mut Buffer,
) -> Option<(u16, u16)> {
    let t = &app.theme;
    let agent = app.agent(id)?;
    let attached = focused && app.focus == Focus::Terminal && app.modal.is_none();
    let v = sidebar::visual(agent.status, app.is_waiting(agent));
    let status_color = visual_color(app, v);
    let border = if attached {
        t.running
    } else if focused {
        t.accent
    } else if agent.yolo {
        t.yolo
    } else if v == Visual::Waiting {
        t.waiting
    } else {
        t.border
    };
    let mut border_style = Style::default().fg(border);
    if focused {
        border_style = border_style.add_modifier(Modifier::BOLD);
    }
    // Header: status glyph, name, provider . model, YOLO, project (Running tab).
    let budget = usize::from(rect.width.saturating_sub(4));
    let extra = format!(" {} \u{00B7} {} ", agent.provider.as_str(), agent.model);
    let yolo_w = if agent.yolo { 6 } else { 0 };
    let project = show_project.then(|| format!(" {} ", grid::basename(&agent.project_dir)));
    let project_w = project.as_ref().map_or(0, |p| p.chars().count());
    let name_room = budget
        .saturating_sub(3 + extra.chars().count() + yolo_w + project_w)
        .max(6);
    let mut spans = vec![
        Span::styled(
            format!(" {} ", v.glyph()),
            Style::default().fg(status_color),
        ),
        Span::styled(
            clip(&agent.name, name_room),
            Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(extra, Style::default().fg(t.text_muted)),
    ];
    if agent.yolo {
        spans.push(Span::styled(
            " YOLO ",
            Style::default()
                .fg(t.accent_text)
                .bg(t.yolo)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(project) = project {
        spans.push(Span::styled(project, Style::default().fg(t.accent)));
    }
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Rounded
        })
        .border_style(border_style)
        .style(Style::default().bg(t.term_bg))
        .title_top(Line::from(spans));
    if attached {
        block = block.title_top(
            Line::styled(
                " typing \u{00B7} prefix Esc to leave ",
                Style::default().fg(t.running),
            )
            .right_aligned(),
        );
    } else if app.grid.expanded.as_deref() == Some(id) {
        block = block.title_top(
            Line::styled(" expanded \u{00B7} z ", Style::default().fg(t.text_muted))
                .right_aligned(),
        );
    }
    let body = block.inner(rect);
    block.render(rect, buf);
    if body.is_empty() {
        return None;
    }
    let palette = t.term_palette();
    let blank = Style::default().fg(t.text_muted).bg(t.term_bg);
    let note = |text: &str, buf: &mut Buffer| {
        buf.set_string(body.x, body.y, clip(text, usize::from(body.width)), blank);
    };
    let Some(term) = app.terms.get(id) else {
        note(" not subscribed", buf);
        return None;
    };
    match &term.sub {
        SubState::Requested(_) => {
            note(" attaching\u{2026}", buf);
            return None;
        }
        SubState::Failed => {
            note(" could not attach", buf);
            return None;
        }
        SubState::Live => {}
    }
    let content = term.emu.content_rows();
    let row_off = content.saturating_sub(body.height);
    let (cursor_col, cursor_row) = term.emu.cursor_raw();
    let col_off = if cursor_col >= body.width {
        cursor_col + 1 - body.width
    } else {
        0
    };
    term.emu.render_region(body, buf, palette, row_off, col_off);
    if !grid::is_active(agent.status) && content <= 1 && cursor_col == 0 {
        note(
            &format!(" {} \u{2014} Enter types here, c opens the chat", v.label()),
            buf,
        );
    }
    if attached {
        let (col, row) = term.emu.cursor()?;
        let (col, row) = (col.checked_sub(col_off)?, row.checked_sub(row_off)?);
        let _ = cursor_row;
        if col < body.width && row < body.height {
            return Some((body.x + col, body.y + row));
        }
    }
    None
}

/// Overview is ported in phase P6; the tab exists already.
pub fn render_overview_placeholder(app: &App, l: &Layout, buf: &mut Buffer) {
    let t = &app.theme;
    let base = Style::default().fg(t.text_muted).bg(t.bg);
    for area in [l.title, l.terminal] {
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                if let Some(c) = buf.cell_mut((x, y)) {
                    c.set_symbol(" ");
                    c.set_style(base);
                }
            }
        }
    }
    if l.title.height > 0 {
        buf.set_string(
            l.title.x,
            l.title.y,
            " Overview",
            Style::default().fg(t.text_secondary).bg(t.bg),
        );
    }
    let area = l.terminal;
    if area.height == 0 {
        return;
    }
    let text = "Overview \u{2014} coming in P6";
    let w = u16::try_from(text.chars().count())
        .unwrap_or(0)
        .min(area.width);
    buf.set_string(
        area.x + (area.width - w) / 2,
        area.y + area.height / 2,
        clip(text, usize::from(area.width)),
        base,
    );
}
