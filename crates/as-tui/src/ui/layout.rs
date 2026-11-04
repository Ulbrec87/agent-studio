//! Screen layout: header, sidebar, chat pane (title, terminal, input bar), footer.

use ratatui::layout::Rect;

/// Below this width the sidebar is hidden.
pub const MIN_WIDTH_FOR_SIDEBAR: u16 = 60;
pub const MIN_SIDEBAR: u16 = 18;
pub const MAX_SIDEBAR: u16 = 60;
pub const DEFAULT_SIDEBAR: u16 = 28;
/// Maximum text lines of the input bar.
pub const MAX_INPUT_LINES: u16 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutParams {
    pub sidebar_width: u16,
    /// Text lines of the input bar; 0 hides it.
    pub input_lines: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub header: Rect,
    /// Includes the separator column on its right edge.
    pub sidebar: Option<Rect>,
    pub title: Rect,
    pub terminal: Rect,
    /// The input bar including its top rule.
    pub input: Option<Rect>,
    pub footer: Rect,
}

pub fn compute(area: Rect, params: LayoutParams) -> Layout {
    let header = Rect {
        height: area.height.min(1),
        ..area
    };
    let footer_h = u16::from(area.height > 2);
    let footer = Rect {
        y: area.y + area.height.saturating_sub(footer_h),
        height: footer_h,
        ..area
    };
    let body_y = area.y + header.height;
    let body_h = area.height.saturating_sub(header.height + footer_h);
    let body = Rect {
        y: body_y,
        height: body_h,
        ..area
    };

    let (sidebar, main) = if area.width >= MIN_WIDTH_FOR_SIDEBAR {
        let w = params
            .sidebar_width
            .clamp(MIN_SIDEBAR, MAX_SIDEBAR)
            .min(area.width / 2);
        (
            Some(Rect { width: w, ..body }),
            Rect {
                x: body.x + w,
                width: body.width - w,
                ..body
            },
        )
    } else {
        (None, body)
    };

    let title = Rect {
        height: main.height.min(1),
        ..main
    };
    let mut rest = Rect {
        y: main.y + title.height,
        height: main.height - title.height,
        ..main
    };
    let input = if params.input_lines > 0 && rest.height > 4 {
        let h = (params.input_lines.min(MAX_INPUT_LINES) + 1).min(rest.height - 3);
        let r = Rect {
            y: rest.y + rest.height - h,
            height: h,
            ..rest
        };
        rest.height -= h;
        Some(r)
    } else {
        None
    };
    Layout {
        header,
        sidebar,
        title,
        terminal: rest,
        input,
        footer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(input: u16) -> LayoutParams {
        LayoutParams {
            sidebar_width: 28,
            input_lines: input,
        }
    }

    #[test]
    fn splits_the_screen() {
        let l = compute(Rect::new(0, 0, 100, 30), params(0));
        assert_eq!(l.header, Rect::new(0, 0, 100, 1));
        assert_eq!(l.footer, Rect::new(0, 29, 100, 1));
        assert_eq!(l.sidebar, Some(Rect::new(0, 1, 28, 28)));
        assert_eq!(l.title, Rect::new(28, 1, 72, 1));
        assert_eq!(l.terminal, Rect::new(28, 2, 72, 27));
        assert_eq!(l.input, None);
    }

    #[test]
    fn input_bar_takes_rows_from_the_terminal() {
        let l = compute(Rect::new(0, 0, 100, 30), params(1));
        assert_eq!(l.input, Some(Rect::new(28, 27, 72, 2)));
        assert_eq!(l.terminal.height, 25);
        let l = compute(Rect::new(0, 0, 100, 30), params(9));
        assert_eq!(l.input.map(|r| r.height), Some(MAX_INPUT_LINES + 1));
    }

    #[test]
    fn narrow_screens_drop_the_sidebar() {
        let l = compute(Rect::new(0, 0, 50, 20), params(0));
        assert_eq!(l.sidebar, None);
        assert_eq!(l.terminal.x, 0);
        assert_eq!(l.terminal.width, 50);
    }

    #[test]
    fn tiny_screens_do_not_underflow() {
        for (w, h) in [(0, 0), (1, 1), (10, 2), (5, 3), (60, 4)] {
            let l = compute(Rect::new(0, 0, w, h), params(3));
            assert!(l.terminal.right() <= w && l.terminal.bottom() <= h);
        }
    }

    #[test]
    fn sidebar_width_is_clamped() {
        let l = compute(
            Rect::new(0, 0, 70, 20),
            LayoutParams {
                sidebar_width: 200,
                input_lines: 0,
            },
        );
        assert_eq!(l.sidebar.map(|r| r.width), Some(35));
    }
}
