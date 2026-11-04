//! Draw a `vt100::Screen` into a ratatui `Buffer`, cell by cell.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::TermPalette;

fn convert(color: vt100::Color, default: Color) -> Color {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub fn cell_style(cell: &vt100::Cell, palette: TermPalette) -> Style {
    let mut fg = convert(cell.fgcolor(), palette.fg);
    let mut bg = convert(cell.bgcolor(), palette.bg);
    if cell.inverse() {
        std::mem::swap(&mut fg, &mut bg);
    }
    let mut modifier = Modifier::empty();
    if cell.bold() {
        modifier |= Modifier::BOLD;
    }
    if cell.dim() {
        modifier |= Modifier::DIM;
    }
    if cell.italic() {
        modifier |= Modifier::ITALIC;
    }
    if cell.underline() {
        modifier |= Modifier::UNDERLINED;
    }
    Style::default().fg(fg).bg(bg).add_modifier(modifier)
}

/// Fill `area` with the screen (at its current scrollback offset); cells outside the screen get
/// the default background.
pub fn render_screen(screen: &vt100::Screen, area: Rect, buf: &mut Buffer, palette: TermPalette) {
    render_region(screen, area, buf, palette, 0, 0);
}

/// Like [`render_screen`], but shows the part of the screen that starts at `(row_off, col_off)`
/// (what a grid tile does when the agent's screen is bigger than the tile).
pub fn render_region(
    screen: &vt100::Screen,
    area: Rect,
    buf: &mut Buffer,
    palette: TermPalette,
    row_off: u16,
    col_off: u16,
) {
    let blank = Style::default().fg(palette.fg).bg(palette.bg);
    for row in 0..area.height {
        let y = area.y + row;
        let mut col = 0u16;
        while col < area.width {
            let x = area.x + col;
            let Some(target) = buf.cell_mut((x, y)) else {
                break;
            };
            match screen.cell(row + row_off, col + col_off) {
                Some(cell) if !cell.is_wide_continuation() => {
                    let text = cell.contents();
                    target.set_symbol(if text.is_empty() { " " } else { text });
                    target.set_style(cell_style(cell, palette));
                    if cell.is_wide() && col + 1 < area.width {
                        // The right half is painted by the terminal; keep its style in sync.
                        if let Some(next) = buf.cell_mut((x + 1, y)) {
                            next.set_symbol("");
                            next.set_style(cell_style(cell, palette));
                        }
                        col += 1;
                    }
                }
                _ => {
                    target.set_symbol(" ");
                    target.set_style(blank);
                }
            }
            col += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> TermPalette {
        TermPalette {
            fg: Color::White,
            bg: Color::Black,
        }
    }

    fn screen_of(bytes: &[u8]) -> vt100::Parser {
        let mut p = vt100::Parser::new(
            std::num::NonZeroU16::new(3).unwrap(),
            std::num::NonZeroU16::new(8).unwrap(),
            0,
        );
        p.process(bytes);
        p
    }

    #[test]
    fn renders_text_colors_and_attributes() {
        let p = screen_of(b"\x1b[1;31mred\x1b[0m \x1b[4;38;2;1;2;3mx\x1b[7my\x1b[0m");
        let area = Rect::new(2, 1, 8, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 6));
        render_screen(p.screen(), area, &mut buf, palette());
        let c = &buf[(2, 1)];
        assert_eq!(c.symbol(), "r");
        assert_eq!(c.fg, Color::Indexed(1));
        assert_eq!(c.bg, Color::Black);
        assert!(c.modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(5, 1)].symbol(), " ");
        let x = &buf[(6, 1)];
        assert_eq!(x.fg, Color::Rgb(1, 2, 3));
        assert!(x.modifier.contains(Modifier::UNDERLINED));
        let y = &buf[(7, 1)];
        assert_eq!(
            (y.fg, y.bg),
            (Color::Black, Color::Rgb(1, 2, 3)),
            "inverse swaps"
        );
        // Outside the area nothing is touched.
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let p = screen_of("a日b".as_bytes());
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 3));
        render_screen(p.screen(), Rect::new(0, 0, 8, 3), &mut buf, palette());
        assert_eq!(buf[(0, 0)].symbol(), "a");
        assert_eq!(buf[(1, 0)].symbol(), "日");
        assert_eq!(buf[(3, 0)].symbol(), "b");
    }
}
