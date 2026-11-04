//! Header bar: app name, view tabs, YOLO state, agent counts, waiting badge, connection.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use as_core::ViewMode;

use crate::app::{App, ConnState, running_count};
use crate::palette::view_label;

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let t = &app.theme;
    let bar = Style::default().fg(t.text).bg(t.surface_raised);
    let mut left = vec![
        Span::styled(
            " Agent Studio ",
            Style::default()
                .fg(t.accent_text)
                .bg(t.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    for view in [ViewMode::Chat, ViewMode::Grid, ViewMode::Overview] {
        let style = if view == app.view {
            Style::default()
                .fg(t.text)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(t.text_muted)
        };
        left.push(Span::styled(format!(" {} ", view_label(view)), style));
        // Hydra's amber badge: how many agents need you, visible from every view.
        let needs_you = app.overview.needs_you_count();
        if view == ViewMode::Overview && needs_you > 0 {
            left.push(Span::styled(
                format!(" {needs_you} "),
                Style::default()
                    .fg(t.accent_text)
                    .bg(t.starting)
                    .add_modifier(Modifier::BOLD),
            ));
            left.push(Span::raw(" "));
        }
    }
    let mut right: Vec<Span> = Vec::new();

    match app.conn {
        ConnState::Connected => {}
        ConnState::Reconnecting { attempt } => {
            left.push(Span::raw("  "));
            left.push(Span::styled(
                format!(" daemon disconnected \u{2014} reconnecting (attempt {attempt}) "),
                Style::default()
                    .fg(t.accent_text)
                    .bg(t.error)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }

    let waiting = app.waiting_count();
    if waiting > 0 {
        right.push(Span::styled(
            format!("\u{26A0} {waiting} waiting"),
            Style::default().fg(t.starting).add_modifier(Modifier::BOLD),
        ));
        right.push(Span::raw("  "));
    }
    let total = app.agents.len();
    let running = running_count(&app.agents);
    right.push(Span::styled(
        format!("\u{25CF} {running}/{total} running"),
        Style::default().fg(if running > 0 { t.running } else { t.text_muted }),
    ));
    right.push(Span::raw("  "));
    let chip = crate::ui::usage::chip_spans(app);
    let chip_w: usize = chip.iter().map(|s| s.content.width()).sum();
    let left_w: usize = left.iter().map(|s| s.content.width()).sum();
    let right_w: usize = right.iter().map(|s| s.content.width()).sum();
    // The chip only shows when the whole bar still fits.
    if chip_w > 0 && left_w + right_w + chip_w + 14 <= usize::from(area.width) {
        right.extend(chip);
        right.push(Span::raw("  "));
    }
    if app.config.global_yolo {
        right.push(Span::styled(
            " YOLO ",
            Style::default()
                .fg(t.accent_text)
                .bg(t.yolo)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        right.push(Span::styled("YOLO off", Style::default().fg(t.text_muted)));
    }
    right.push(Span::raw(" "));

    Line::from(left).style(bar).render(area, buf);
    let right_line = Line::from(right).style(bar);
    let w = u16::try_from(right_line.width())
        .unwrap_or(0)
        .min(area.width);
    right_line.render(
        Rect {
            x: area.right() - w,
            width: w,
            ..area
        },
        buf,
    );
}
