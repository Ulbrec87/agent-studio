//! Sidebar: projects with their agents.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::{App, Focus};
use crate::sidebar::{self, Row, RowKey, SortKey, StatusChip};

/// First visible row so that the cursor row stays on screen.
pub fn scroll_offset(cursor: Option<usize>, height: u16) -> usize {
    match cursor {
        Some(c) => (c + 1).saturating_sub(usize::from(height.max(1))),
        None => 0,
    }
}

/// Rows below the agent list: status chips and the search line (just the search line when tiny).
pub fn footer_height(sidebar_height: u16) -> u16 {
    if sidebar_height >= 5 { 2 } else { 1 }
}

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let t = &app.theme;
    let base = Style::default().fg(t.text).bg(t.sidebar);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_symbol(" ");
                c.set_style(base);
            }
        }
    }
    // Separator on the right edge.
    let sep_x = area.right().saturating_sub(1);
    for y in area.y..area.bottom() {
        if let Some(c) = buf.cell_mut((sep_x, y)) {
            c.set_symbol("\u{2502}");
            c.set_style(Style::default().fg(t.border).bg(t.sidebar));
        }
    }
    let inner = Rect {
        width: area.width.saturating_sub(1),
        ..area
    };
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let footer_h = footer_height(inner.height);
    let list_h = inner.height - footer_h;
    let rows = app.sidebar_rows();
    let cursor = app.sidebar_cursor_index(&rows);
    let offset = scroll_offset(cursor, list_h);
    let sidebar_focused = app.focus == Focus::App;

    if rows.is_empty() {
        let msg = if app.sidebar.filter.is_empty() && app.sidebar.status == StatusChip::All {
            "No agents yet"
        } else {
            "No match"
        };
        Line::styled(
            format!(" {msg}"),
            Style::default().fg(t.text_muted).bg(t.sidebar),
        )
        .render(Rect { height: 1, ..inner }, buf);
    }

    for (i, row) in rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(usize::from(list_h))
    {
        let y = inner.y + u16::try_from(i - offset).unwrap_or(0);
        let line_area = Rect {
            y,
            height: 1,
            ..inner
        };
        let key = sidebar::row_key(&app.agents, row);
        let is_cursor = cursor == Some(i);
        let is_selected = matches!(&key, RowKey::Agent(id) if app.selected.as_deref() == Some(id));
        let mut row_style = base;
        if is_selected {
            row_style = row_style.bg(t.surface_raised);
        }
        if is_cursor && sidebar_focused {
            row_style = row_style.bg(t.surface_hover).add_modifier(Modifier::BOLD);
        }
        fill(buf, line_area, row_style);
        let spans = match row {
            Row::Header {
                key,
                count,
                collapsed,
            } => {
                let arrow = if *collapsed { "\u{25B8}" } else { "\u{25BE}" };
                let count_text = format!(" {count}");
                // A small chip names the Claude profile of the project unless it is the default.
                let chip = (key != sidebar::MANAGER_GROUP)
                    .then(|| app.profile_chip(key))
                    .flatten()
                    .map(|label| format!(" [{label}]"));
                let chip_w = chip.as_ref().map_or(0, |c| c.chars().count());
                // " ▾ " in front and the count behind: the path gives way, from the left.
                let room = usize::from(inner.width)
                    .saturating_sub(3 + count_text.chars().count() + chip_w + 1);
                let mut spans = vec![
                    Span::styled(format!(" {arrow} "), Style::default().fg(t.text_muted)),
                    Span::styled(
                        fit_left(&sidebar::group_label(key), room),
                        Style::default()
                            .fg(t.text_secondary)
                            .add_modifier(Modifier::BOLD),
                    ),
                ];
                if let Some(chip) = chip {
                    spans.push(Span::styled(chip, Style::default().fg(t.accent)));
                }
                spans.push(Span::styled(count_text, Style::default().fg(t.text_muted)));
                spans
            }
            Row::Agent(idx) => {
                let a = &app.agents[*idx];
                let v = sidebar::visual(a.status, app.is_waiting(a));
                let color = match v {
                    sidebar::Visual::Running => t.running,
                    sidebar::Visual::Starting => t.starting,
                    sidebar::Visual::Waiting => t.waiting,
                    sidebar::Visual::Idle => t.idle,
                    sidebar::Visual::Errored => t.error,
                };
                let name_style = if is_selected {
                    Style::default().fg(t.text).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(t.text)
                };
                let mut spans = vec![
                    Span::raw("   "),
                    Span::styled(v.glyph(), Style::default().fg(color)),
                    Span::raw(" "),
                    Span::styled(a.name.clone(), name_style),
                ];
                if a.yolo {
                    spans.push(Span::styled(
                        " !",
                        Style::default().fg(t.yolo).add_modifier(Modifier::BOLD),
                    ));
                }
                spans
            }
        };
        let line = Line::from(spans).style(row_style);
        // Right-aligned status label for agents when there is room.
        if let Row::Agent(idx) = row {
            let a = &app.agents[*idx];
            let v = sidebar::visual(a.status, app.is_waiting(a));
            let label = v.label();
            let used: usize = line
                .spans
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            let label_w = label.len() + 1;
            if used + label_w < usize::from(line_area.width) {
                let x = line_area.right() - u16::try_from(label_w).unwrap_or(0);
                buf.set_string(x, y, label, row_style.fg(t.text_muted));
            }
        }
        line.render(line_area, buf);
    }

    // Status chips (when there is room) above the search line.
    if footer_h == 2 {
        let chips_area = Rect {
            y: inner.bottom() - 2,
            height: 1,
            ..inner
        };
        let mut spans = vec![Span::raw(" ")];
        for chip in StatusChip::ALL {
            let active = chip == app.sidebar.status;
            let style = if active {
                Style::default()
                    .fg(t.accent_text)
                    .bg(t.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.text_muted)
            };
            spans.push(Span::styled(chip.short().to_string(), style));
            spans.push(Span::raw(" "));
        }
        Line::from(spans).style(base).render(chips_area, buf);
    }
    // Search line at the bottom, with the sort order on the right.
    let filter_area = Rect {
        y: inner.bottom() - 1,
        height: 1,
        ..inner
    };
    let sort_label = format!("\u{2195}{} ", app.sidebar.sort.label());
    let sort_w = u16::try_from(sort_label.chars().count()).unwrap_or(0);
    let filter_line = if app.sidebar.filtering || !app.sidebar.filter.is_empty() {
        let cursor = if app.sidebar.filtering {
            "\u{2588}"
        } else {
            ""
        };
        Line::from(vec![
            Span::styled(" / ", Style::default().fg(t.accent)),
            Span::styled(
                format!("{}{cursor}", app.sidebar.filter),
                Style::default().fg(t.text),
            ),
        ])
    } else {
        Line::styled(" / search\u{2026}", Style::default().fg(t.text_muted))
    };
    filter_line.style(base).render(filter_area, buf);
    if inner.width > sort_w + 14 {
        buf.set_string(
            inner.right() - sort_w,
            filter_area.y,
            sort_label,
            Style::default()
                .fg(if app.sidebar.sort == SortKey::Created {
                    t.text_muted
                } else {
                    t.accent
                })
                .bg(t.sidebar),
        );
    }
}

fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    for x in area.x..area.right() {
        if let Some(c) = buf.cell_mut((x, area.y)) {
            c.set_style(style);
        }
    }
}

/// Keep the tail of `text` within `max` columns, marking the cut with an ellipsis.
pub fn fit_left(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let tail: String = chars[chars.len() - (max - 1)..].iter().collect();
    format!("\u{2026}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_paths_lose_their_head() {
        assert_eq!(fit_left("~/a/b/project", 20), "~/a/b/project");
        assert_eq!(fit_left("~/a/b/project", 8), "\u{2026}project");
        assert_eq!(fit_left("abc", 0), "");
    }

    #[test]
    fn scroll_offset_keeps_the_cursor_visible() {
        assert_eq!(scroll_offset(None, 5), 0);
        assert_eq!(scroll_offset(Some(3), 5), 0);
        assert_eq!(scroll_offset(Some(9), 5), 5);
    }
}
