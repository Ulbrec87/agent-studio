//! Chat view: title line, the agent's terminal, optional input bar.

use as_core::paths::tildify;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::app::{App, Focus, SubState};
use crate::sidebar;

pub fn render_title(app: &App, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let t = &app.theme;
    let base = Style::default().fg(t.text_secondary).bg(t.bg);
    let Some(a) = app.selected_agent() else {
        Line::styled(" Agent Studio", base).render(area, buf);
        return;
    };
    let v = sidebar::visual(a.status, app.is_waiting(a));
    let color = match v {
        sidebar::Visual::Running => t.running,
        sidebar::Visual::Starting => t.starting,
        sidebar::Visual::Waiting => t.waiting,
        sidebar::Visual::Idle => t.idle,
        sidebar::Visual::Errored => t.error,
    };
    let sep = Span::styled(" \u{00B7} ", Style::default().fg(t.text_muted));
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            a.name.clone(),
            Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        ),
        sep.clone(),
        Span::raw(a.provider.as_str()),
        sep.clone(),
        Span::raw(a.model.clone()),
        sep.clone(),
        Span::styled(
            format!("{} {}", v.glyph(), v.label()),
            Style::default().fg(color),
        ),
    ];
    if a.yolo {
        spans.push(sep.clone());
        spans.push(Span::styled(
            "YOLO",
            Style::default().fg(t.yolo).add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(sep);
    spans.push(Span::styled(
        tildify(std::path::Path::new(&a.project_dir)),
        Style::default().fg(t.text_muted),
    ));
    if let Some(branch) = &a.worktree_branch {
        spans.push(Span::styled(
            format!(" ({branch})"),
            Style::default().fg(t.text_muted),
        ));
    }
    Line::from(spans).style(base).render(area, buf);
}

/// Draws the terminal area and returns the cursor position when the terminal has focus.
pub fn render_terminal(app: &App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    if area.is_empty() {
        return None;
    }
    let t = &app.theme;
    let blank = Style::default().fg(t.text).bg(t.bg);
    let Some(id) = app.selected.as_ref() else {
        fill(buf, area, blank);
        render_empty_state(app, area, buf);
        return None;
    };
    let Some(term) = app.terms.get(id) else {
        fill(buf, area, blank);
        return None;
    };
    term.emu.render(area, buf, t.term_palette());
    match &term.sub {
        SubState::Requested(_) => {
            let msg = Line::styled(
                " attaching\u{2026} ",
                Style::default().fg(t.text_muted).bg(t.term_bg),
            );
            msg.render(Rect { height: 1, ..area }, buf);
        }
        SubState::Failed => {
            let msg = Line::styled(
                " could not attach to this agent ",
                Style::default().fg(t.error).bg(t.term_bg),
            );
            msg.render(Rect { height: 1, ..area }, buf);
        }
        SubState::Live => {}
    }
    if app.scroll_mode {
        let off = term.emu.scrollback();
        let label = format!(" scroll: {off}/{} ", term.emu.scrollback_len());
        let w = u16::try_from(label.chars().count())
            .unwrap_or(0)
            .min(area.width);
        buf.set_string(
            area.right() - w,
            area.y,
            label,
            Style::default()
                .fg(t.accent_text)
                .bg(t.accent)
                .add_modifier(Modifier::BOLD),
        );
    }
    if app.focus == Focus::Terminal && !app.scroll_mode && app.modal.is_none() {
        let (col, row) = term.emu.cursor()?;
        if col < area.width && row < area.height {
            return Some((area.x + col, area.y + row));
        }
    }
    None
}

fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_symbol(" ");
                c.set_style(style);
            }
        }
    }
}

fn render_empty_state(app: &App, area: Rect, buf: &mut Buffer) {
    let t = &app.theme;
    let prefix = app.prefix_label();
    let lines = vec![
        Line::styled(
            "No agents yet",
            Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Press ", Style::default().fg(t.text_secondary)),
            Span::styled(
                format!("{prefix} n"),
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to create one", Style::default().fg(t.text_secondary)),
        ]),
        Line::from(vec![
            Span::styled(format!("{prefix} ?"), Style::default().fg(t.accent)),
            Span::styled(
                " shows every key binding",
                Style::default().fg(t.text_muted),
            ),
        ]),
    ];
    let h = u16::try_from(lines.len()).unwrap_or(4);
    if area.height < h {
        return;
    }
    let target = Rect {
        y: area.y + (area.height - h) / 2,
        height: h,
        ..area
    };
    Paragraph::new(lines)
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().bg(t.bg))
        .render(target, buf);
}

/// The input bar; returns the cursor position when it has focus.
pub fn render_input(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let t = app.theme;
    let focused = app.focus == Focus::Input;
    let modal = app.modal.is_some();
    let hint = " Enter send \u{00B7} Alt+Enter newline \u{00B7} Esc terminal ";
    render_bar(&t, &mut app.input, area, buf, focused && !modal, hint)
}

/// A rule with a hint on top and a text area below (chat input, broadcast bar).
pub fn render_bar(
    t: &crate::theme::Theme,
    input: &mut crate::widgets::TextArea,
    area: Rect,
    buf: &mut Buffer,
    focused: bool,
    hint: &str,
) -> Option<(u16, u16)> {
    let rule_style = Style::default()
        .fg(if focused { t.accent } else { t.border })
        .bg(t.bg);
    let rule: String = "\u{2500}".repeat(usize::from(area.width));
    buf.set_string(area.x, area.y, &rule, rule_style);
    if focused && area.width as usize > hint.chars().count() + 4 {
        buf.set_string(
            area.x + 2,
            area.y,
            hint,
            Style::default().fg(t.text_muted).bg(t.bg),
        );
    }
    let text_area = Rect {
        y: area.y + 1,
        height: area.height - 1,
        x: area.x + 2,
        width: area.width.saturating_sub(2),
    };
    for y in text_area.y..text_area.bottom() {
        buf.set_string(
            area.x,
            y,
            "\u{203A} ",
            Style::default().fg(t.accent).bg(t.input_bg),
        );
        for x in text_area.x..text_area.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_symbol(" ");
                c.set_style(Style::default().bg(t.input_bg));
            }
        }
    }
    let cursor = input.render(text_area, buf, Style::default().fg(t.text).bg(t.input_bg));
    if focused { cursor } else { None }
}
