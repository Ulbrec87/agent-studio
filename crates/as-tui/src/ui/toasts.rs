//! Toast notifications, bottom-right. Bodies wrap to the toast width (up to [`MAX_BODY_LINES`]
//! lines, then an ellipsis); the update banner sits at the bottom of the stack until dismissed.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, ToastKind};

pub const TOAST_WIDTH: u16 = 44;
/// Body lines of one toast; more is cut with an ellipsis (the full text is in the log of the
/// feature that raised it).
pub const MAX_BODY_LINES: usize = 6;

/// Break `text` into lines of at most `width` columns: at spaces when possible, inside a word
/// longer than the width otherwise. At most `max_lines` lines; the last one ends with `…` when
/// text was cut.
pub fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for word in text.split_whitespace() {
        let w = word.width();
        if cur_w > 0 && cur_w + 1 + w <= width {
            cur.push(' ');
            cur.push_str(word);
            cur_w += 1 + w;
            continue;
        }
        if cur_w > 0 {
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        if w <= width {
            cur.push_str(word);
            cur_w = w;
            continue;
        }
        // A word wider than the line: split it by columns.
        for c in word.chars() {
            let cw = c.width().unwrap_or(0);
            if cur_w + cw > width {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            cur.push(c);
            cur_w += cw;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            while last.width() + 1 > width {
                last.pop();
            }
            last.push('\u{2026}');
        }
    }
    lines
}

/// Rows a toast with this body needs (borders + title + wrapped body).
pub fn height_for(body: &str, width: u16) -> u16 {
    let inner = usize::from(width.saturating_sub(2));
    let lines = wrap(body, inner, MAX_BODY_LINES).len();
    u16::try_from(lines).unwrap_or(0) + 3
}

struct Card<'a> {
    glyph: &'a str,
    color: ratatui::style::Color,
    title: &'a str,
    body: &'a str,
}

fn draw(app: &App, card: &Card, area: Rect, buf: &mut Buffer) {
    let t = &app.theme;
    Clear.render(area, buf);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(card.color))
        .style(Style::default().fg(t.text).bg(t.surface_raised));
    let inner = block.inner(area);
    block.render(area, buf);
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{} ", card.glyph), Style::default().fg(card.color)),
        Span::styled(
            card.title.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ])];
    for l in wrap(card.body, usize::from(inner.width), MAX_BODY_LINES) {
        lines.push(Line::styled(l, Style::default().fg(t.text_secondary)));
    }
    Paragraph::new(lines).render(inner, buf);
}

pub fn render(app: &App, screen: Rect, buf: &mut Buffer) {
    let t = &app.theme;
    let width = TOAST_WIDTH.min(screen.width);
    // Stack upwards above the footer, newest at the bottom; the update banner is the lowest.
    let mut bottom = screen.bottom().saturating_sub(1);
    let mut place = |height: u16| -> Option<Rect> {
        if height == 0 || bottom < screen.y + height {
            return None;
        }
        let area = Rect {
            x: screen.right() - width,
            y: bottom - height,
            width,
            height,
        };
        bottom -= height;
        Some(area)
    };
    if let Some(info) = app.update_banner() {
        let body = format!(
            "{} (Esc dismisses)",
            if app.update_hint().is_empty() {
                info.url.as_str()
            } else {
                app.update_hint()
            }
        );
        let title = format!("Update available: {}", info.version);
        if let Some(area) = place(height_for(&body, width)) {
            let card = Card {
                glyph: "\u{2191}",
                color: t.accent,
                title: &title,
                body: &body,
            };
            draw(app, &card, area, buf);
        }
    }
    for toast in app.toasts.iter().rev() {
        let Some(area) = place(height_for(&toast.body, width)) else {
            break;
        };
        let (glyph, color) = match toast.kind {
            ToastKind::Info => ("\u{25CF}", t.text_secondary),
            ToastKind::Success => ("\u{2713}", t.running),
            ToastKind::Warn => ("\u{25D0}", t.starting),
            ToastKind::Error => ("\u{2716}", t.error),
        };
        let card = Card {
            glyph,
            color,
            title: &toast.title,
            body: &toast.body,
        };
        draw(app, &card, area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_spaces() {
        assert_eq!(
            wrap("the quick brown fox jumps", 10, 6),
            ["the quick", "brown fox", "jumps"]
        );
        assert_eq!(wrap("", 10, 3), [""]);
        assert_eq!(wrap("short", 10, 3), ["short"]);
    }

    #[test]
    fn splits_a_long_word_and_never_exceeds_the_width() {
        let lines = wrap("aaaaaaaaaaaaaaaaaaaaaaaa", 10, 6);
        assert_eq!(lines, ["aaaaaaaaaa", "aaaaaaaaaa", "aaaa"]);
        for l in wrap(
            "x /home/someone/a/very/long/path/that/goes/on and on",
            12,
            9,
        ) {
            assert!(l.width() <= 12, "{l:?}");
        }
    }

    #[test]
    fn cuts_with_an_ellipsis_after_the_max_lines() {
        let lines = wrap("one two three four five six seven eight", 5, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines[2].ends_with('\u{2026}'), "{lines:?}");
        assert!(lines[2].width() <= 5);
    }

    #[test]
    fn wide_characters_count_two_columns() {
        let lines = wrap("\u{4f60}\u{597d}\u{4f60}\u{597d}\u{4f60}\u{597d}", 4, 6);
        assert!(lines.iter().all(|l| l.width() <= 4), "{lines:?}");
    }

    #[test]
    fn height_grows_with_the_body_up_to_the_cap() {
        assert_eq!(height_for("short", 44), 4); // borders + title + 1 line (the old fixed size)
        assert!(height_for(&"word ".repeat(60), 44) <= u16::try_from(MAX_BODY_LINES).unwrap() + 3);
        assert!(height_for(&"word ".repeat(60), 44) > height_for("short", 44));
    }
}
