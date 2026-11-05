//! Color themes: Hydra's three palettes (`src/styles/tokens.css`) as ratatui colors.

use as_core::ThemeId;
use ratatui::style::Color;

use crate::term::TermPalette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Indexed256,
}

impl ColorMode {
    /// From `COLORTERM` (`truecolor` / `24bit`).
    pub fn detect() -> Self {
        Self::from_colorterm(std::env::var("COLORTERM").ok().as_deref())
    }

    pub fn from_colorterm(value: Option<&str>) -> Self {
        match value {
            Some(v) if v.contains("truecolor") || v.contains("24bit") => ColorMode::TrueColor,
            _ => ColorMode::Indexed256,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub id: ThemeId,
    pub bg: Color,
    pub surface_raised: Color,
    pub surface_hover: Color,
    pub sidebar: Color,
    pub accent: Color,
    pub accent_text: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub border: Color,
    pub running: Color,
    pub idle: Color,
    pub error: Color,
    pub starting: Color,
    pub waiting: Color,
    /// Usage providers: Claude stays terracotta in every theme (Hydra `--color-provider-*`).
    pub claude: Color,
    pub codex: Color,
    pub yolo: Color,
    pub input_bg: Color,
    pub term_bg: Color,
    pub term_fg: Color,
    /// Background of added / removed diff lines.
    pub diff_add_bg: Color,
    pub diff_del_bg: Color,
}

/// Nearest xterm-256 color of an RGB value (6x6x6 cube or the gray ramp).
pub fn rgb_to_indexed(r: u8, g: u8, b: u8) -> u8 {
    fn level(v: u8) -> u8 {
        match v {
            0..=47 => 0,
            48..=114 => 1,
            v => (v - 35) / 40,
        }
    }
    const STEPS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = 16 + 36 * ri + 6 * gi + bi;
    let cube_rgb = (
        STEPS[usize::from(ri)],
        STEPS[usize::from(gi)],
        STEPS[usize::from(bi)],
    );
    let avg = (i32::from(r) + i32::from(g) + i32::from(b)) / 3;
    let gray_i = ((avg - 8).max(0) / 10).min(23);
    let gray = 8 + 10 * gray_i;
    let dist = |c: (i32, i32, i32)| {
        (c.0 - i32::from(r)).pow(2) + (c.1 - i32::from(g)).pow(2) + (c.2 - i32::from(b)).pow(2)
    };
    if dist((gray, gray, gray)) < dist(cube_rgb) {
        u8::try_from(232 + gray_i).unwrap_or(255)
    } else {
        cube
    }
}

fn hex(mode: ColorMode, rgb: u32) -> Color {
    let (r, g, b) = (
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
    );
    match mode {
        ColorMode::TrueColor => Color::Rgb(r, g, b),
        ColorMode::Indexed256 => Color::Indexed(rgb_to_indexed(r, g, b)),
    }
}

impl Theme {
    pub fn new(id: ThemeId, mode: ColorMode) -> Self {
        let h = |v: u32| hex(mode, v);
        match id {
            ThemeId::Midnight => Self {
                id,
                bg: h(0x191919),
                surface_raised: h(0x212121),
                surface_hover: h(0x252525),
                sidebar: h(0x191919),
                accent: h(0xFFFFFF),
                accent_text: h(0x191919),
                text: h(0xE8E8E8),
                text_secondary: h(0xA0A0A0),
                text_muted: h(0x666666),
                border: h(0x2a2a2a),
                running: h(0x7EE787),
                idle: h(0x777777),
                error: h(0xFF6B6B),
                starting: h(0xFFD166),
                waiting: h(0x79B8FF),
                claude: h(0xE08B6D),
                codex: h(0x64A6F0),
                yolo: h(0xFF6B6B),
                input_bg: h(0x191919),
                term_bg: h(0x191919),
                term_fg: h(0xE8E8E8),
                diff_add_bg: h(0x1E3A27),
                diff_del_bg: h(0x442426),
            },
            ThemeId::Dark => Self {
                id,
                bg: h(0x262624),
                surface_raised: h(0x3b3b38),
                surface_hover: h(0x46463f),
                sidebar: h(0x2b2b29),
                accent: h(0xE08B6D),
                accent_text: h(0x262624),
                text: h(0xE8E4E0),
                text_secondary: h(0xA8A49E),
                text_muted: h(0x6B6760),
                border: h(0x3b3b38),
                running: h(0x66BB6A),
                idle: h(0x757575),
                error: h(0xEF5350),
                starting: h(0xFFA726),
                waiting: h(0x64A6F0),
                claude: h(0xE08B6D),
                codex: h(0x64A6F0),
                yolo: h(0xFF5252),
                input_bg: h(0x30302e),
                term_bg: h(0x262624),
                term_fg: h(0xE8E4E0),
                diff_add_bg: h(0x2B4A30),
                diff_del_bg: h(0x572B2B),
            },
            ThemeId::Light => Self {
                id,
                bg: h(0xFAF6F1),
                surface_raised: h(0xFFFFFF),
                surface_hover: h(0xF5F0EA),
                sidebar: h(0xF0EBE4),
                accent: h(0xD97757),
                accent_text: h(0xFFFFFF),
                text: h(0x2D2420),
                text_secondary: h(0x7A6F66),
                text_muted: h(0xB0A79E),
                border: h(0xE5DFD8),
                running: h(0x4CAF50),
                idle: h(0x9E9E9E),
                error: h(0xE53935),
                starting: h(0xFF9800),
                waiting: h(0x4A86D9),
                claude: h(0xD97757),
                codex: h(0x4A86D9),
                yolo: h(0xE53935),
                input_bg: h(0xFFFFFF),
                // Hydra keeps the terminal dark in every theme.
                term_bg: h(0x262624),
                term_fg: h(0xE8E4E0),
                diff_add_bg: h(0xDDF3E0),
                diff_del_bg: h(0xFBDCDC),
            },
        }
    }

    pub fn term_palette(&self) -> TermPalette {
        TermPalette {
            fg: self.term_fg,
            bg: self.term_bg,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_truecolor() {
        assert_eq!(
            ColorMode::from_colorterm(Some("truecolor")),
            ColorMode::TrueColor
        );
        assert_eq!(
            ColorMode::from_colorterm(Some("24bit")),
            ColorMode::TrueColor
        );
        assert_eq!(ColorMode::from_colorterm(Some("")), ColorMode::Indexed256);
        assert_eq!(ColorMode::from_colorterm(None), ColorMode::Indexed256);
    }

    #[test]
    fn rgb_maps_to_indexed() {
        assert_eq!(rgb_to_indexed(0, 0, 0), 16);
        assert_eq!(rgb_to_indexed(255, 255, 255), 231);
        assert_eq!(rgb_to_indexed(255, 0, 0), 196);
        // A mid gray prefers the gray ramp.
        let g = rgb_to_indexed(0x77, 0x77, 0x77);
        assert!((232..=255).contains(&g), "{g}");
    }

    #[test]
    fn palettes_follow_hydra() {
        let m = Theme::new(ThemeId::Midnight, ColorMode::TrueColor);
        assert_eq!(m.bg, Color::Rgb(0x19, 0x19, 0x19));
        assert_eq!(m.running, Color::Rgb(0x7E, 0xE7, 0x87));
        let d = Theme::new(ThemeId::Dark, ColorMode::TrueColor);
        assert_eq!(d.accent, Color::Rgb(0xE0, 0x8B, 0x6D));
        let l = Theme::new(ThemeId::Light, ColorMode::Indexed256);
        assert!(matches!(l.bg, Color::Indexed(_)));
    }
}
