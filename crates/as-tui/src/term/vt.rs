//! [`TerminalEmulator`] on top of the `vt100` crate.

use std::num::NonZeroU16;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{SCROLLBACK_LINES, TermPalette, TerminalEmulator, render};

/// Collects the answers vt100 does not give itself (queries the CLIs send at startup, e.g. Codex
/// asks for the cursor position and waits for the reply).
#[derive(Default)]
struct Replies {
    out: Vec<u8>,
    bell: bool,
}

impl vt100::Callbacks for Replies {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, first) {
            // DSR: status report / cursor position report.
            (None, 'n', 5) => self.out.extend_from_slice(b"\x1b[0n"),
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                self.out
                    .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            // Primary device attributes: a VT220-ish terminal with ANSI color.
            (None, 'c', 0) => self.out.extend_from_slice(b"\x1b[?62;22c"),
            // Secondary device attributes.
            (Some(b'>'), 'c', 0) => self.out.extend_from_slice(b"\x1b[>0;0;0c"),
            _ => {}
        }
    }
}

pub struct Vt100Emulator {
    parser: vt100::Parser<Replies>,
}

fn nz(v: u16) -> NonZeroU16 {
    NonZeroU16::new(v.max(1)).unwrap_or(NonZeroU16::MIN)
}

impl Vt100Emulator {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                nz(rows),
                nz(cols),
                SCROLLBACK_LINES,
                Replies::default(),
            ),
        }
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }
}

impl TerminalEmulator for Vt100Emulator {
    fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.parser.set_size(nz(rows), nz(cols));
    }

    fn size(&self) -> (u16, u16) {
        let (rows, cols) = self.parser.screen().size();
        (cols.get(), rows.get())
    }

    fn set_scrollback(&mut self, rows: usize) {
        self.parser.screen_mut().set_scrollback(rows);
    }

    fn scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    fn scrollback_len(&self) -> usize {
        self.parser.screen().scrollback_len()
    }

    fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    fn cursor(&self) -> Option<(u16, u16)> {
        let screen = self.parser.screen();
        if screen.hide_cursor() || screen.scrollback() > 0 {
            return None;
        }
        let (row, col) = screen.cursor_position();
        Some((col, row))
    }

    fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.parser.callbacks_mut().out)
    }

    fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.parser.callbacks_mut().bell)
    }

    fn render(&self, area: Rect, buf: &mut Buffer, palette: TermPalette) {
        render::render_screen(self.parser.screen(), area, buf, palette);
    }

    fn render_region(
        &self,
        area: Rect,
        buf: &mut Buffer,
        palette: TermPalette,
        row_off: u16,
        col_off: u16,
    ) {
        render::render_region(self.parser.screen(), area, buf, palette, row_off, col_off);
    }

    fn content_rows(&self) -> u16 {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let (cursor_row, _) = screen.cursor_position();
        for row in (0..rows.get()).rev() {
            if row < cursor_row {
                break;
            }
            let blank =
                (0..cols.get()).all(|col| screen.cell(row, col).is_none_or(|c| !c.has_contents()));
            if !blank {
                return row + 1;
            }
        }
        cursor_row + 1
    }

    fn cursor_raw(&self) -> (u16, u16) {
        let (row, col) = self.parser.screen().cursor_position();
        (col, row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_cursor_position_and_device_queries() {
        let mut emu = Vt100Emulator::new(20, 5);
        emu.process(b"ab\x1b[6n");
        assert_eq!(emu.take_replies(), b"\x1b[1;3R");
        emu.process(b"\x1b[5n\x1b[c");
        assert_eq!(emu.take_replies(), b"\x1b[0n\x1b[?62;22c");
        assert!(emu.take_replies().is_empty());
    }

    #[test]
    fn tracks_modes_cursor_and_bell() {
        let mut emu = Vt100Emulator::new(10, 3);
        assert!(!emu.application_cursor() && !emu.bracketed_paste());
        emu.process(b"\x1b[?1h\x1b[?2004h\x07hi");
        assert!(emu.application_cursor() && emu.bracketed_paste());
        assert!(emu.take_bell());
        assert!(!emu.take_bell());
        assert_eq!(emu.cursor(), Some((2, 0)));
        emu.process(b"\x1b[?25l");
        assert_eq!(emu.cursor(), None);
    }

    #[test]
    fn content_rows_follow_the_text_and_the_cursor() {
        let mut emu = Vt100Emulator::new(20, 10);
        assert_eq!(emu.content_rows(), 1);
        emu.process(b"one\r\ntwo\r\nthree");
        assert_eq!(emu.content_rows(), 3);
        emu.process(b"\x1b[8;1Hlow");
        assert_eq!(emu.content_rows(), 8);
        emu.process(b"\x1b[2J\x1b[5;3H");
        assert_eq!(
            emu.content_rows(),
            5,
            "the cursor row counts on a blank screen"
        );
        assert_eq!(emu.cursor_raw(), (2, 4));
    }

    #[test]
    fn scrollback_and_resize() {
        let mut emu = Vt100Emulator::new(10, 3);
        for i in 0..10 {
            emu.process(format!("line{i}\r\n").as_bytes());
        }
        assert!(emu.scrollback_len() >= 7);
        emu.set_scrollback(4);
        assert_eq!(emu.scrollback(), 4);
        assert_eq!(emu.cursor(), None);
        emu.set_scrollback(0);
        emu.resize(20, 6);
        assert_eq!(emu.size(), (20, 6));
    }
}
