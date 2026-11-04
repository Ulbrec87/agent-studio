//! A small multi-line text input (`tui-textarea` 0.7 targets ratatui 0.29, so it cannot be used
//! with ratatui 0.30). Single-line mode is the same widget with newlines disabled.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, PartialEq)]
pub struct TextArea {
    lines: Vec<String>,
    row: usize,
    /// Cursor column as a char index in `lines[row]`.
    col: usize,
    multiline: bool,
    /// Horizontal scroll (display columns), kept by `render`.
    hscroll: usize,
    vscroll: usize,
}

impl Default for TextArea {
    fn default() -> Self {
        Self::single_line("")
    }
}

impl TextArea {
    pub fn single_line(text: &str) -> Self {
        let mut t = Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
            multiline: false,
            hscroll: 0,
            vscroll: 0,
        };
        t.set_text(text);
        t
    }

    pub fn multi_line() -> Self {
        Self {
            multiline: true,
            ..Self::single_line("")
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn clear(&mut self) {
        self.set_text("");
    }

    /// Replace the content; the cursor goes to the end.
    pub fn set_text(&mut self, text: &str) {
        let text = if self.multiline {
            text.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            text.replace(['\r', '\n'], " ")
        };
        self.lines = text.split('\n').map(str::to_string).collect();
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].chars().count();
        self.hscroll = 0;
        self.vscroll = 0;
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    fn byte_at(&self, row: usize, col: usize) -> usize {
        self.lines[row]
            .char_indices()
            .nth(col)
            .map_or(self.lines[row].len(), |(i, _)| i)
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines[row].chars().count()
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' || c == '\r' {
            self.insert_newline();
            return;
        }
        let at = self.byte_at(self.row, self.col);
        self.lines[self.row].insert(at, c);
        self.col += 1;
    }

    pub fn insert_str(&mut self, text: &str) {
        for c in text.chars() {
            self.insert_char(c);
        }
    }

    pub fn insert_newline(&mut self) {
        if !self.multiline {
            return;
        }
        let at = self.byte_at(self.row, self.col);
        let rest = self.lines[self.row].split_off(at);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            let at = self.byte_at(self.row, self.col - 1);
            self.lines[self.row].remove(at);
            self.col -= 1;
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.line_len(self.row);
            self.lines[self.row].push_str(&line);
        }
    }

    pub fn delete(&mut self) {
        if self.col < self.line_len(self.row) {
            let at = self.byte_at(self.row, self.col);
            self.lines[self.row].remove(at);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    /// Ctrl+W: delete the word before the cursor.
    pub fn delete_word_back(&mut self) {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let mut start = self.col;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        if start == self.col {
            self.backspace();
            return;
        }
        let from = self.byte_at(self.row, start);
        let to = self.byte_at(self.row, self.col);
        self.lines[self.row].replace_range(from..to, "");
        self.col = start;
    }

    /// Ctrl+U: delete to the start of the line.
    pub fn delete_to_line_start(&mut self) {
        let to = self.byte_at(self.row, self.col);
        self.lines[self.row].replace_range(..to, "");
        self.col = 0;
    }

    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.line_len(self.row);
        }
    }

    pub fn move_right(&mut self) {
        if self.col < self.line_len(self.row) {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// False when already on the first line.
    pub fn move_up(&mut self) -> bool {
        if self.row == 0 {
            return false;
        }
        self.row -= 1;
        self.col = self.col.min(self.line_len(self.row));
        true
    }

    /// False when already on the last line.
    pub fn move_down(&mut self) -> bool {
        if self.row + 1 >= self.lines.len() {
            return false;
        }
        self.row += 1;
        self.col = self.col.min(self.line_len(self.row));
        true
    }

    pub fn move_home(&mut self) {
        self.col = 0;
    }

    pub fn move_end(&mut self) {
        self.col = self.line_len(self.row);
    }

    /// Handle an editing key. Returns whether it was consumed (Enter, Esc, Tab and friends are
    /// left to the caller; Up/Down are consumed only when they can move inside the text).
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
                'w' => self.delete_word_back(),
                'u' => self.delete_to_line_start(),
                'a' => self.move_home(),
                'e' => self.move_end(),
                'b' => self.move_left(),
                'f' => self.move_right(),
                'h' => self.backspace(),
                'd' => self.delete(),
                'j' if self.multiline => self.insert_newline(),
                _ => return false,
            },
            KeyCode::Char(c) if !alt => self.insert_char(c),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Home => self.move_home(),
            KeyCode::End => self.move_end(),
            KeyCode::Up => return self.multiline && self.move_up(),
            KeyCode::Down => return self.multiline && self.move_down(),
            _ => return false,
        }
        true
    }

    /// Draw into `area` and return the screen position of the cursor.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, style: Style) -> Option<(u16, u16)> {
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let height = usize::from(area.height);
        let width = usize::from(area.width);
        if self.row < self.vscroll {
            self.vscroll = self.row;
        } else if self.row >= self.vscroll + height {
            self.vscroll = self.row + 1 - height;
        }
        // Keep the cursor visible horizontally.
        let cursor_x: usize = self.lines[self.row]
            .chars()
            .take(self.col)
            .map(|c| c.width().unwrap_or(0))
            .sum();
        if cursor_x < self.hscroll {
            self.hscroll = cursor_x;
        } else if cursor_x >= self.hscroll + width {
            self.hscroll = cursor_x + 1 - width;
        }
        for (i, line) in self
            .lines
            .iter()
            .skip(self.vscroll)
            .take(height)
            .enumerate()
        {
            let y = area.y + u16::try_from(i).unwrap_or(0);
            let mut x = 0usize;
            for c in line.chars() {
                let w = c.width().unwrap_or(0);
                if x >= self.hscroll + width {
                    break;
                }
                if x >= self.hscroll && w > 0 && x + w <= self.hscroll + width {
                    let sx = area.x + u16::try_from(x - self.hscroll).unwrap_or(0);
                    if let Some(cell) = buf.cell_mut((sx, y)) {
                        cell.set_char(c);
                        cell.set_style(style);
                    }
                }
                x += w;
            }
        }
        let cx = area.x + u16::try_from(cursor_x - self.hscroll).unwrap_or(0);
        let cy = area.y + u16::try_from(self.row - self.vscroll).unwrap_or(0);
        Some((cx, cy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn edits_a_line() {
        let mut t = TextArea::single_line("");
        t.insert_str("héllo wörld");
        assert_eq!(t.text(), "héllo wörld");
        t.handle_key(&ctrl('w'));
        assert_eq!(t.text(), "héllo ");
        t.handle_key(&key(KeyCode::Backspace));
        t.handle_key(&key(KeyCode::Home));
        t.handle_key(&key(KeyCode::Delete));
        assert_eq!(t.text(), "éllo");
        t.handle_key(&key(KeyCode::End));
        t.insert_char('!');
        assert_eq!(t.text(), "éllo!");
        t.handle_key(&ctrl('u'));
        assert!(t.is_empty());
    }

    #[test]
    fn single_line_ignores_newlines_and_does_not_consume_enter() {
        let mut t = TextArea::single_line("a\nb");
        assert_eq!(t.text(), "a b");
        t.insert_newline();
        assert_eq!(t.line_count(), 1);
        assert!(!t.handle_key(&key(KeyCode::Enter)));
        assert!(!t.handle_key(&key(KeyCode::Up)));
        assert!(!t.handle_key(&key(KeyCode::Tab)));
    }

    #[test]
    fn multiline_navigation_and_joining() {
        let mut t = TextArea::multi_line();
        t.insert_str("one\ntwo\nthree");
        assert_eq!(t.line_count(), 3);
        assert_eq!(t.cursor(), (2, 5));
        assert!(t.handle_key(&key(KeyCode::Up)));
        assert_eq!(t.cursor(), (1, 3));
        t.move_home();
        t.backspace();
        assert_eq!(t.text(), "onetwo\nthree");
        assert!(t.handle_key(&key(KeyCode::Down)));
        assert!(
            !t.handle_key(&key(KeyCode::Down)),
            "last line: not consumed"
        );
        t.set_text("x\ny");
        t.handle_key(&key(KeyCode::Up));
        t.move_end();
        t.delete();
        assert_eq!(t.text(), "xy");
    }

    #[test]
    fn renders_with_horizontal_scroll() {
        let mut t = TextArea::single_line("abcdefghij");
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 1));
        let cursor = t.render(Rect::new(0, 0, 5, 1), &mut buf, Style::default());
        let shown: String = (0..5).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert_eq!(shown, "ghij ");
        assert_eq!(cursor, Some((4, 0)));
    }
}
