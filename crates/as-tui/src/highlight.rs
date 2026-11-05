//! Syntax highlighting of the Files preview and of diff lines.
//!
//! With the `highlight` cargo feature this wraps `syntect` (pure-Rust regex engine, embedded
//! syntaxes and themes); without it every function reports "no highlighting" and callers draw
//! plain text. Styles are foreground-only: the background always comes from the app theme.

use std::time::{Duration, Instant};

use ratatui::style::Style;

use crate::theme::Theme;

/// A run of text with a (foreground-only) style.
pub type StyledSpan = (Style, String);

/// Files bigger than this are shown plain.
pub const MAX_BYTES: usize = 512 * 1024;
/// Lines highlighted beyond the last visible one.
pub const MARGIN: usize = 60;
/// Longest a single frame may spend highlighting.
pub const FRAME_BUDGET: Duration = Duration::from_millis(30);

/// Highlighting is compiled in.
pub const fn available() -> bool {
    cfg!(feature = "highlight")
}

/// Start loading the syntax and theme dumps on a background thread, so the first highlighted
/// frame does not pay for it.
pub fn warm() {
    #[cfg(feature = "highlight")]
    imp::warm();
}

/// Highlights one line at a time, carrying the parser state from line to line.
pub struct LineHighlighter {
    #[cfg(feature = "highlight")]
    inner: imp::Inner,
}

impl LineHighlighter {
    /// A highlighter for the language of `path` (by extension, then by file name), colored for
    /// `theme`; `None` when the language is unknown or highlighting is not compiled in.
    pub fn for_path(path: &str, theme: &Theme) -> Option<Self> {
        #[cfg(feature = "highlight")]
        {
            imp::Inner::new(path, theme).map(|inner| Self { inner })
        }
        #[cfg(not(feature = "highlight"))]
        {
            let _ = (path, theme);
            None
        }
    }

    /// Spans of the next line (`text` without its line ending).
    pub fn line(&mut self, text: &str) -> Vec<StyledSpan> {
        #[cfg(feature = "highlight")]
        {
            self.inner.line(text)
        }
        #[cfg(not(feature = "highlight"))]
        {
            vec![(Style::default(), text.to_string())]
        }
    }
}

impl std::fmt::Debug for LineHighlighter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LineHighlighter")
    }
}

/// Highlighted lines of one text, produced lazily and in order.
///
/// Cloning gives an empty cache (the parser state cannot be copied); equality ignores it.
#[derive(Debug, Default)]
pub struct LazyLines {
    hl: Option<LineHighlighter>,
    spans: Vec<Option<Vec<StyledSpan>>>,
    done: usize,
    total: usize,
    enabled: bool,
}

impl Clone for LazyLines {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for LazyLines {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl LazyLines {
    /// Prepare for `total` lines of the file at `path`, `bytes` long. Not enabled (plain text) for
    /// unknown languages, files over [`MAX_BYTES`] or builds without the feature.
    pub fn new(path: &str, total: usize, bytes: usize, theme: &Theme) -> Self {
        let hl = if bytes <= MAX_BYTES {
            LineHighlighter::for_path(path, theme)
        } else {
            None
        };
        Self {
            enabled: hl.is_some(),
            hl,
            spans: Vec::new(),
            done: 0,
            total,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Highlight lines up to `through` (exclusive) of `lines`, within the frame budget. Returns
    /// `true` once everything asked for is done.
    pub fn ensure<S: AsRef<str>>(&mut self, lines: &[S], through: usize) -> bool {
        let Some(hl) = &mut self.hl else {
            return true;
        };
        let end = through.min(self.total).min(lines.len());
        let start = Instant::now();
        while self.done < end {
            let text = lines[self.done].as_ref();
            self.spans.push(Some(hl.line(text)));
            self.done += 1;
            if start.elapsed() >= FRAME_BUDGET && self.done < end {
                return false;
            }
        }
        true
    }

    pub fn get(&self, line: usize) -> Option<&[StyledSpan]> {
        self.spans.get(line)?.as_deref()
    }
}

#[cfg(feature = "highlight")]
mod imp {
    use std::sync::OnceLock;

    use as_core::ThemeId;
    use ratatui::style::{Color, Style};
    use syntect::easy::HighlightLines;
    use syntect::highlighting::{Theme as SynTheme, ThemeSet};
    use syntect::parsing::{SyntaxReference, SyntaxSet};

    use super::StyledSpan;
    use crate::theme::{Theme, rgb_to_indexed};

    struct Assets {
        syntaxes: SyntaxSet,
        themes: ThemeSet,
    }

    static ASSETS: OnceLock<Assets> = OnceLock::new();

    fn assets() -> &'static Assets {
        ASSETS.get_or_init(|| Assets {
            syntaxes: SyntaxSet::load_defaults_newlines(),
            themes: ThemeSet::load_defaults(),
        })
    }

    pub fn warm() {
        if ASSETS.get().is_none() {
            // Failing to spawn only costs the warm-up.
            let _ = std::thread::Builder::new()
                .name("highlight-warm".into())
                .spawn(|| {
                    assets();
                });
        }
    }

    /// Light themes get a light syntect theme, dark and midnight a dark one.
    fn theme_name(id: ThemeId) -> &'static str {
        match id {
            ThemeId::Light => "InspiredGitHub",
            ThemeId::Dark => "base16-eighties.dark",
            ThemeId::Midnight => "base16-ocean.dark",
        }
    }

    /// Languages syntect's default set lacks, mapped to a close relative.
    fn alias(ext: &str) -> Option<&'static str> {
        Some(match ext {
            "ts" | "tsx" | "jsx" | "mjs" | "cjs" | "vue" | "svelte" => "js",
            "yml" => "yaml",
            "toml" | "cfg" | "conf" | "ini" | "env" => "properties",
            "jsonc" | "json5" => "json",
            "zsh" | "fish" => "sh",
            "kt" | "kts" => "java",
            "hpp" | "cc" | "cxx" | "hh" => "cpp",
            _ => return None,
        })
    }

    fn find_syntax(set: &'static SyntaxSet, path: &str) -> Option<&'static SyntaxReference> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let ext = name.rsplit_once('.').map(|(_, e)| e);
        ext.and_then(|e| set.find_syntax_by_extension(e))
            .or_else(|| set.find_syntax_by_extension(name))
            .or_else(|| {
                ext.map(str::to_ascii_lowercase).and_then(|e| {
                    set.find_syntax_by_extension(&e)
                        .or_else(|| alias(&e).and_then(|a| set.find_syntax_by_extension(a)))
                })
            })
            .filter(|s| s.name != "Plain Text")
    }

    pub struct Inner {
        lines: HighlightLines<'static>,
        set: &'static SyntaxSet,
        truecolor: bool,
    }

    impl Inner {
        pub fn new(path: &str, theme: &Theme) -> Option<Self> {
            let a = assets();
            let syntax = find_syntax(&a.syntaxes, path)?;
            let syn_theme: &'static SynTheme = a.themes.themes.get(theme_name(theme.id))?;
            Some(Self {
                lines: HighlightLines::new(syntax, syn_theme),
                set: &a.syntaxes,
                truecolor: matches!(theme.bg, Color::Rgb(..)),
            })
        }

        pub fn line(&mut self, text: &str) -> Vec<StyledSpan> {
            let mut with_nl = String::with_capacity(text.len() + 1);
            with_nl.push_str(text);
            with_nl.push('\n');
            let Ok(ranges) = self.lines.highlight_line(&with_nl, self.set) else {
                return vec![(Style::default(), text.to_string())];
            };
            let mut out: Vec<StyledSpan> = Vec::with_capacity(ranges.len());
            for (style, piece) in ranges {
                let piece = piece.trim_end_matches(['\n', '\r']);
                if piece.is_empty() {
                    continue;
                }
                let c = style.foreground;
                let color = if self.truecolor {
                    Color::Rgb(c.r, c.g, c.b)
                } else {
                    Color::Indexed(rgb_to_indexed(c.r, c.g, c.b))
                };
                let st = Style::default().fg(color);
                match out.last_mut() {
                    Some((last, s)) if *last == st => s.push_str(piece),
                    _ => out.push((st, piece.to_string())),
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use as_core::ThemeId;

    use super::*;
    use crate::theme::ColorMode;

    fn theme(id: ThemeId) -> Theme {
        Theme::new(id, ColorMode::TrueColor)
    }

    #[test]
    fn unknown_language_is_plain() {
        assert!(LineHighlighter::for_path("notes.zzzz", &theme(ThemeId::Dark)).is_none());
        let lazy = LazyLines::new("notes.zzzz", 3, 30, &theme(ThemeId::Dark));
        assert!(!lazy.is_enabled());
    }

    #[test]
    fn big_files_are_plain() {
        let lazy = LazyLines::new("main.rs", 10, MAX_BYTES + 1, &theme(ThemeId::Dark));
        assert!(!lazy.is_enabled());
    }

    #[test]
    fn plain_lazy_lines_have_no_spans() {
        let mut lazy = LazyLines::new("x.zzzz", 2, 4, &theme(ThemeId::Dark));
        assert!(lazy.ensure(&["a", "b"], 2));
        assert!(lazy.get(0).is_none());
    }

    #[cfg(not(feature = "highlight"))]
    #[test]
    fn without_the_feature_nothing_highlights() {
        assert!(!available());
        assert!(LineHighlighter::for_path("main.rs", &theme(ThemeId::Dark)).is_none());
    }

    #[cfg(feature = "highlight")]
    mod with_syntect {
        use ratatui::style::Color;

        use super::*;

        fn colors(spans: &[StyledSpan]) -> Vec<Option<Color>> {
            spans.iter().map(|(s, _)| s.fg).collect()
        }

        #[test]
        fn rust_keywords_get_their_own_color() {
            let mut hl = LineHighlighter::for_path("src/main.rs", &theme(ThemeId::Midnight))
                .expect("rust is in the default syntaxes");
            let spans = hl.line("fn main() { let x = 1; }");
            let text: String = spans.iter().map(|(_, t)| t.as_str()).collect();
            assert_eq!(text, "fn main() { let x = 1; }");
            assert!(spans.len() > 3, "{spans:?}");
            assert!(spans.iter().all(|(s, _)| s.bg.is_none()), "foreground only");
            let c = colors(&spans);
            assert!(c.iter().any(|x| *x != c[0]), "several colors: {c:?}");
        }

        #[test]
        fn markdown_and_json_are_known_and_toml_maps_to_a_relative() {
            let t = theme(ThemeId::Dark);
            for p in [
                "README.md",
                "a/b/c.json",
                "Cargo.toml",
                "x.py",
                "Makefile",
                "a.ts",
            ] {
                assert!(LineHighlighter::for_path(p, &t).is_some(), "{p}");
            }
        }

        #[test]
        fn light_and_dark_themes_differ() {
            let l = LineHighlighter::for_path("a.rs", &theme(ThemeId::Light))
                .map(|mut h| colors(&h.line("fn a() {}")));
            let d = LineHighlighter::for_path("a.rs", &theme(ThemeId::Midnight))
                .map(|mut h| colors(&h.line("fn a() {}")));
            assert_ne!(l, d);
        }

        #[test]
        fn state_carries_over_multi_line_constructs() {
            let mut hl = LineHighlighter::for_path("a.rs", &theme(ThemeId::Dark)).unwrap();
            let first = hl.line("/* start");
            let second = hl.line("still comment fn let */");
            assert_eq!(colors(&first)[0], colors(&second)[0], "same comment color");
            assert_eq!(second.len(), 1, "no keyword split inside the comment");
        }

        #[test]
        fn indexed_terminals_get_indexed_colors() {
            let t = Theme::new(ThemeId::Dark, ColorMode::Indexed256);
            let mut hl = LineHighlighter::for_path("a.rs", &t).unwrap();
            let spans = hl.line("fn a() {}");
            assert!(
                spans
                    .iter()
                    .all(|(s, _)| matches!(s.fg, Some(Color::Indexed(_))))
            );
        }

        #[test]
        fn lazy_lines_fill_up_to_the_requested_line() {
            let lines: Vec<String> = (0..100).map(|i| format!("let v{i} = {i};")).collect();
            let mut lazy = LazyLines::new("a.rs", lines.len(), 1000, &theme(ThemeId::Dark));
            assert!(lazy.is_enabled());
            // The frame budget may split the work (slow debug builds): keep asking.
            while !lazy.ensure(&lines, 10) {}
            assert!(lazy.get(9).is_some());
            assert!(lazy.get(10).is_none(), "lazy: nothing beyond the window");
            while !lazy.ensure(&lines, 1000) {}
            assert!(lazy.get(99).is_some());
            assert!(lazy.clone().get(0).is_none(), "clones start empty");
        }
    }
}
