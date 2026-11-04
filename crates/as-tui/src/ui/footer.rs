//! Footer: current mode, prefix-pending indicator and key hints.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::{App, Focus, Modal};

/// `(mode label, hints)` for the footer.
pub fn mode_and_hints(app: &App) -> (&'static str, String) {
    let p = app.prefix_label();
    if app.prefix_pending {
        return (
            "PREFIX",
            "n new \u{00B7} x kill \u{00B7} R restart \u{00B7} d remove \u{00B7} r rename \u{00B7} y yolo \u{00B7} j/k agents \u{00B7} 1-9 jump \u{00B7} s sidebar \u{00B7} [ scroll \u{00B7} i input \u{00B7} ? help \u{00B7} q quit".into(),
        );
    }
    match &app.modal {
        Some(Modal::NewAgent(_)) => {
            return ("NEW AGENT", "Tab/\u{2193} next \u{00B7} Shift+Tab prev \u{00B7} \u{2190}/\u{2192} change \u{00B7} Enter create \u{00B7} Esc cancel".into());
        }
        Some(Modal::Rename { .. }) => return ("RENAME", "Enter save \u{00B7} Esc cancel".into()),
        Some(Modal::ConfirmRemove { .. }) => {
            return ("CONFIRM", "y remove \u{00B7} n cancel".into());
        }
        Some(Modal::Quit { .. }) => {
            return ("QUIT", "\u{2190}/\u{2192} choose \u{00B7} Enter confirm \u{00B7} d detach \u{00B7} s stop all \u{00B7} Esc cancel".into());
        }
        Some(Modal::Help) => return ("HELP", "any key closes".into()),
        None => {}
    }
    if app.scroll_mode {
        return (
            "SCROLL",
            "\u{2191}/\u{2193} line \u{00B7} PgUp/PgDn page \u{00B7} g/G top/bottom \u{00B7} Esc/q leave".into(),
        );
    }
    match app.focus {
        Focus::Terminal => (
            "TERMINAL",
            format!("{p} ? help \u{00B7} {p} n new \u{00B7} {p} s sidebar \u{00B7} {p} q quit"),
        ),
        Focus::App => (
            "APP",
            "j/k move \u{00B7} Enter attach \u{00B7} Space fold \u{00B7} / filter \u{00B7} n new \u{00B7} Esc back".into(),
        ),
        Focus::Input => (
            "INPUT",
            "Enter send \u{00B7} Alt+Enter newline \u{00B7} Esc terminal".into(),
        ),
    }
}

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let t = &app.theme;
    let bar = Style::default().fg(t.text_secondary).bg(t.surface_raised);
    let (mode, hints) = mode_and_hints(app);
    let mode_style = if app.prefix_pending {
        Style::default().fg(t.accent_text).bg(t.starting)
    } else {
        Style::default().fg(t.accent_text).bg(t.accent)
    };
    let mut spans = vec![
        Span::styled(format!(" {mode} "), mode_style.add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(hints, Style::default().fg(t.text_secondary)),
    ];
    if app.prefix_pending {
        spans.insert(
            1,
            Span::styled(
                format!(" {} \u{2026}", app.prefix_label()),
                Style::default().fg(t.starting).add_modifier(Modifier::BOLD),
            ),
        );
    }
    Line::from(spans).style(bar).render(area, buf);
}
