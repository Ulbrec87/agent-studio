//! Toast notifications, bottom-right.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};

use crate::app::{App, ToastKind};

pub const TOAST_WIDTH: u16 = 44;
pub const TOAST_HEIGHT: u16 = 4;

pub fn render(app: &App, screen: Rect, buf: &mut Buffer) {
    let t = &app.theme;
    let width = TOAST_WIDTH.min(screen.width);
    // Stack upwards above the footer, newest at the bottom.
    let mut bottom = screen.bottom().saturating_sub(1);
    for toast in app.toasts.iter().rev() {
        if bottom < screen.y + TOAST_HEIGHT {
            break;
        }
        let area = Rect {
            x: screen.right() - width,
            y: bottom - TOAST_HEIGHT,
            width,
            height: TOAST_HEIGHT,
        };
        bottom -= TOAST_HEIGHT;
        let (glyph, color) = match toast.kind {
            ToastKind::Info => ("\u{25CF}", t.text_secondary),
            ToastKind::Success => ("\u{2713}", t.running),
            ToastKind::Warn => ("\u{25D0}", t.starting),
            ToastKind::Error => ("\u{2716}", t.error),
        };
        Clear.render(area, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(color))
            .style(Style::default().fg(t.text).bg(t.surface_raised));
        let inner = block.inner(area);
        block.render(area, buf);
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(format!("{glyph} "), Style::default().fg(color)),
                Span::styled(
                    toast.title.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::styled(toast.body.clone(), Style::default().fg(t.text_secondary)),
        ])
        .render(inner, buf);
    }
}
