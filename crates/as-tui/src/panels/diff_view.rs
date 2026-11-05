//! A scrollable unified-diff view: flattens parsed [`DiffFile`]s into rows (file header, hunk
//! header, context/add/delete lines with both line numbers) for the renderer.

use as_core::ThemeId;
use as_core::git::{DiffFile, DiffLineKind};

use crate::highlight::{FRAME_BUDGET, LineHighlighter, MAX_BYTES, StyledSpan};
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// `path` line of a file (multi-file diffs).
    File,
    Hunk,
    Context,
    Add,
    Del,
    /// `\ No newline at end of file`.
    Meta,
    /// Plain message (binary file, empty diff, error).
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: RowKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
}

/// One row of the side-by-side layout; the numbers index [`DiffView::rows`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitRow {
    /// File header, hunk header, note or `\ No newline` line: spans both columns.
    Full(usize),
    /// `left` is a context/removed row (old file), `right` a context/added row (new file).
    /// A context row appears on both sides. `None` pads the shorter side of a change.
    Pair {
        left: Option<usize>,
        right: Option<usize>,
    },
}

/// The side-by-side form of a diff, derived from the unified rows by [`pair_rows`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SplitLayout {
    pub rows: Vec<SplitRow>,
    /// Index of every hunk header in `rows`.
    pub hunks: Vec<usize>,
    /// For every unified row, the split row that shows it.
    pub split_of: Vec<usize>,
    /// For every split row, the first unified row it shows.
    pub unified_of: Vec<usize>,
}

/// Pair the removed and added runs of every hunk: the n-th removed line meets the n-th added line,
/// the shorter side is padded. Context rows sit on both sides; headers span the whole width.
pub fn pair_rows(rows: &[DiffRow]) -> SplitLayout {
    let mut out = SplitLayout::default();
    let mut dels: Vec<usize> = Vec::new();
    let mut adds: Vec<usize> = Vec::new();

    fn flush(out: &mut SplitLayout, dels: &mut Vec<usize>, adds: &mut Vec<usize>) {
        for i in 0..dels.len().max(adds.len()) {
            let (l, r) = (dels.get(i).copied(), adds.get(i).copied());
            let first = l.into_iter().chain(r).min().unwrap_or(0);
            out.rows.push(SplitRow::Pair { left: l, right: r });
            out.unified_of.push(first);
            for u in l.into_iter().chain(r) {
                out.split_of[u] = out.rows.len() - 1;
            }
        }
        dels.clear();
        adds.clear();
    }

    out.split_of = vec![0; rows.len()];
    for (i, row) in rows.iter().enumerate() {
        match row.kind {
            RowKind::Del => dels.push(i),
            RowKind::Add => adds.push(i),
            kind => {
                flush(&mut out, &mut dels, &mut adds);
                let split = if kind == RowKind::Context {
                    SplitRow::Pair {
                        left: Some(i),
                        right: Some(i),
                    }
                } else {
                    SplitRow::Full(i)
                };
                if kind == RowKind::Hunk {
                    out.hunks.push(out.rows.len());
                }
                out.rows.push(split);
                out.unified_of.push(i);
                out.split_of[i] = out.rows.len() - 1;
            }
        }
    }
    flush(&mut out, &mut dels, &mut adds);
    out
}

/// Lazy syntax highlighting of the lines of a diff.
///
/// The old file's lines (context + removed) and the new file's (context + added) are two separate
/// streams, each with its own parser state, restarted at every hunk. Cloning gives an empty cache
/// and equality ignores it (the parser state cannot be copied).
#[derive(Debug, Default)]
pub struct DiffHighlight {
    path: Option<String>,
    theme: Option<ThemeId>,
    old: Option<LineHighlighter>,
    new: Option<LineHighlighter>,
    spans: Vec<Option<Vec<StyledSpan>>>,
    done: usize,
    disabled: bool,
}

impl Clone for DiffHighlight {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            ..Self::default()
        }
    }
}

impl PartialEq for DiffHighlight {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for DiffHighlight {}

impl DiffHighlight {
    fn reset(&mut self) {
        let path = self.path.take();
        *self = Self {
            path,
            ..Self::default()
        };
    }

    fn start_hunk(&mut self, theme: &Theme) {
        let Some(path) = &self.path else { return };
        self.old = LineHighlighter::for_path(path, theme);
        self.new = LineHighlighter::for_path(path, theme);
        self.disabled = self.old.is_none() || self.new.is_none();
    }

    /// Highlight the rows before `through`; `false` when the frame budget ran out first.
    fn ensure(&mut self, theme: &Theme, rows: &[DiffRow], through: usize) -> bool {
        if self.path.is_none() || self.disabled {
            return true;
        }
        if self.theme != Some(theme.id) {
            self.reset();
            self.theme = Some(theme.id);
            if rows.iter().map(|r| r.text.len() + 1).sum::<usize>() > MAX_BYTES {
                self.disabled = true;
                return true;
            }
            self.start_hunk(theme);
            if self.disabled {
                return true;
            }
        }
        let end = through.min(rows.len());
        let started = std::time::Instant::now();
        while self.done < end {
            let row = &rows[self.done];
            let spans = match row.kind {
                RowKind::File | RowKind::Hunk => {
                    self.start_hunk(theme);
                    None
                }
                RowKind::Context => {
                    let old = self.old.as_mut().map(|h| h.line(&row.text));
                    let new = self.new.as_mut().map(|h| h.line(&row.text));
                    new.or(old)
                }
                RowKind::Add => self.new.as_mut().map(|h| h.line(&row.text)),
                RowKind::Del => self.old.as_mut().map(|h| h.line(&row.text)),
                RowKind::Meta | RowKind::Note => None,
            };
            self.spans.push(spans);
            self.done += 1;
            if started.elapsed() >= FRAME_BUDGET && self.done < end {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiffView {
    pub title: String,
    pub rows: Vec<DiffRow>,
    /// Scroll offset in the rows of the active layout.
    pub scroll: usize,
    /// Row index of every hunk header (unified layout).
    pub hunks: Vec<usize>,
    /// Side-by-side form of `rows` (always kept in sync).
    pub split: SplitLayout,
    /// The side-by-side layout is the active one (the renderer sets it from the config and the
    /// panel width).
    pub split_active: bool,
    pub highlight: DiffHighlight,
}

impl DiffView {
    pub fn note(title: &str, text: &str) -> Self {
        Self {
            title: title.to_string(),
            rows: vec![DiffRow {
                kind: RowKind::Note,
                old_no: None,
                new_no: None,
                text: text.to_string(),
            }],
            split: SplitLayout {
                rows: vec![SplitRow::Full(0)],
                hunks: Vec::new(),
                split_of: vec![0],
                unified_of: vec![0],
            },
            ..Default::default()
        }
    }

    pub fn from_files(title: &str, files: &[DiffFile], show_file_headers: bool) -> Self {
        let mut view = Self {
            title: title.to_string(),
            ..Default::default()
        };
        for file in files {
            if show_file_headers {
                view.rows.push(DiffRow {
                    kind: RowKind::File,
                    old_no: None,
                    new_no: None,
                    text: format!("{}  (+{} -{})", file.path(), file.additions, file.deletions),
                });
            }
            if file.binary {
                view.rows.push(DiffRow {
                    kind: RowKind::Note,
                    old_no: None,
                    new_no: None,
                    text: "binary file".into(),
                });
            }
            for hunk in &file.hunks {
                view.hunks.push(view.rows.len());
                view.rows.push(DiffRow {
                    kind: RowKind::Hunk,
                    old_no: None,
                    new_no: None,
                    text: hunk.header.clone(),
                });
                for l in &hunk.lines {
                    view.rows.push(DiffRow {
                        kind: match l.kind {
                            DiffLineKind::Context => RowKind::Context,
                            DiffLineKind::Add => RowKind::Add,
                            DiffLineKind::Del => RowKind::Del,
                            DiffLineKind::Meta => RowKind::Meta,
                        },
                        old_no: l.old_no,
                        new_no: l.new_no,
                        text: l.text.clone(),
                    });
                }
            }
        }
        if view.rows.is_empty() {
            return Self::note(title, "No textual changes");
        }
        view.split = pair_rows(&view.rows);
        view
    }

    /// Highlight the lines by the language of `path`.
    pub fn with_path(mut self, path: &str) -> Self {
        self.highlight.path = Some(path.to_string());
        self
    }

    pub fn from_text(title: &str, diff: &str, show_file_headers: bool) -> Self {
        Self::from_files(
            title,
            &as_core::git::parse_unified_diff(diff),
            show_file_headers,
        )
    }

    /// Number of rows of the active layout.
    pub fn len(&self) -> usize {
        if self.split_active {
            self.split.rows.len()
        } else {
            self.rows.len()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Hunk header rows of the active layout.
    pub fn hunk_rows(&self) -> &[usize] {
        if self.split_active {
            &self.split.hunks
        } else {
            &self.hunks
        }
    }

    /// Switch the active layout, keeping the top row (or the one that shows it) in view.
    pub fn set_split(&mut self, active: bool) {
        if active == self.split_active {
            return;
        }
        self.scroll = if active {
            self.split.split_of.get(self.scroll).copied().unwrap_or(0)
        } else {
            self.split.unified_of.get(self.scroll).copied().unwrap_or(0)
        };
        self.split_active = active;
    }

    /// Unified index of the last row the viewport of `height` rows shows.
    pub fn last_visible_unified(&self, height: usize) -> usize {
        if !self.split_active {
            return (self.scroll + height)
                .min(self.rows.len())
                .saturating_sub(1);
        }
        self.split
            .rows
            .iter()
            .skip(self.scroll)
            .take(height)
            .flat_map(|r| match *r {
                SplitRow::Full(i) => [Some(i), None],
                SplitRow::Pair { left, right } => [left, right],
            })
            .flatten()
            .max()
            .unwrap_or(0)
    }

    /// Highlight what the viewport (`height` rows) shows plus a margin. `false`: more to do.
    pub fn highlight_visible(&mut self, theme: &Theme, height: usize) -> bool {
        let through = self.last_visible_unified(height) + 1 + crate::highlight::MARGIN;
        self.highlight.ensure(theme, &self.rows, through)
    }

    /// Spans of a unified row, when highlighted.
    pub fn spans_of(&self, row: usize) -> Option<&[StyledSpan]> {
        self.highlight.spans.get(row)?.as_deref()
    }

    pub fn max_scroll(&self, height: usize) -> usize {
        self.len().saturating_sub(height.max(1))
    }

    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll(height));
    }

    /// Jump to the next hunk header after the current top row.
    pub fn next_hunk(&mut self, height: usize) {
        if let Some(&row) = self.hunk_rows().iter().find(|&&r| r > self.scroll) {
            self.scroll = row.min(self.max_scroll(height));
        }
    }

    pub fn prev_hunk(&mut self) {
        if let Some(&row) = self.hunk_rows().iter().rev().find(|&&r| r < self.scroll) {
            self.scroll = row;
        }
    }

    /// Width of the line-number gutter (digits of the largest number).
    pub fn gutter(&self) -> usize {
        let max = self
            .rows
            .iter()
            .flat_map(|r| [r.old_no, r.new_no])
            .flatten()
            .max()
            .unwrap_or(0);
        max.to_string().len().max(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n@@ -10,2 +10,3 @@ fn x\n ten\n+eleven\n twelve\n";

    #[test]
    fn flattens_hunks_with_numbers() {
        let v = DiffView::from_text("a.txt", DIFF, false);
        assert_eq!(v.hunks, vec![0, 5]);
        assert_eq!(v.rows[1].kind, RowKind::Context);
        assert_eq!(
            (v.rows[2].kind, v.rows[2].old_no, v.rows[2].new_no),
            (RowKind::Del, Some(2), None)
        );
        assert_eq!(
            (v.rows[3].kind, v.rows[3].old_no, v.rows[3].new_no),
            (RowKind::Add, None, Some(2))
        );
        assert_eq!(v.rows[0].text, "@@ -1,3 +1,3 @@");
        assert_eq!(v.gutter(), 2);
    }

    #[test]
    fn hunk_navigation_and_scroll_clamp() {
        let mut v = DiffView::from_text("a.txt", DIFF, false);
        v.next_hunk(3);
        assert_eq!(v.scroll, 5.min(v.max_scroll(3)));
        v.scroll = 0;
        v.next_hunk(100);
        assert_eq!(v.scroll, 0, "clamped to the scroll range");
        v.scroll = 6;
        v.prev_hunk();
        assert_eq!(v.scroll, 5);
        v.scroll_by(100, 3);
        assert_eq!(v.scroll, v.max_scroll(3));
        v.scroll_by(-100, 3);
        assert_eq!(v.scroll, 0);
    }

    const CHANGE: &str = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,6 +1,6 @@\n one\n-two\n-three\n-four\n+TWO\n+THREE\n five\n-six\n+SIX\n+seven\n";

    fn pairs(v: &DiffView) -> Vec<(Option<&str>, Option<&str>)> {
        v.split
            .rows
            .iter()
            .map(|r| match r {
                SplitRow::Full(i) => (Some(v.rows[*i].text.as_str()), None),
                SplitRow::Pair { left, right } => (
                    left.map(|i| v.rows[i].text.as_str()),
                    right.map(|i| v.rows[i].text.as_str()),
                ),
            })
            .collect()
    }

    #[test]
    fn pairs_removed_and_added_runs_and_pads_the_shorter_side() {
        let v = DiffView::from_text("a.txt", CHANGE, false);
        assert_eq!(
            pairs(&v),
            vec![
                (Some("@@ -1,6 +1,6 @@"), None),
                (Some("one"), Some("one")),
                (Some("two"), Some("TWO")),
                (Some("three"), Some("THREE")),
                (Some("four"), None),
                (Some("five"), Some("five")),
                (Some("six"), Some("SIX")),
                (None, Some("seven")),
            ]
        );
        assert_eq!(v.split.hunks, vec![0]);
    }

    #[test]
    fn pure_additions_and_deletions_pad_one_side() {
        let v = DiffView::from_text(
            "x",
            "--- a/x\n+++ b/x\n@@ -1,2 +1,3 @@\n a\n+new\n b\n@@ -9,2 +10,1 @@\n-gone\n c\n",
            false,
        );
        let p = pairs(&v);
        assert!(p.contains(&(None, Some("new"))));
        assert!(p.contains(&(Some("gone"), None)));
        assert_eq!(v.split.hunks.len(), 2);
    }

    #[test]
    fn mapping_between_layouts_keeps_the_top_row() {
        let mut v = DiffView::from_text("a.txt", CHANGE, false);
        // Unified rows: 0 hunk, 1 one, 2 two, 3 three, 4 four, 5 TWO, 6 THREE, 7 five ...
        v.scroll = 7;
        v.set_split(true);
        assert_eq!(v.scroll, 5, "five is the sixth split row");
        v.set_split(false);
        assert_eq!(v.scroll, 7);
        v.set_split(true);
        v.scroll = 3;
        v.set_split(false);
        assert_eq!(v.scroll, 3, "pairs map to their first unified row");
    }

    #[test]
    fn navigation_follows_the_active_layout() {
        let mut v = DiffView::from_text("a.txt", DIFF, false);
        v.set_split(true);
        assert_eq!(v.len(), 8);
        assert_eq!(v.hunk_rows(), &[0, 4]);
        v.next_hunk(3);
        assert_eq!(v.scroll, 4.min(v.max_scroll(3)));
        v.prev_hunk();
        assert_eq!(v.scroll, 0);
        v.scroll_by(100, 3);
        assert_eq!(v.scroll, 5);
    }

    #[test]
    fn empty_diff_is_a_note() {
        let v = DiffView::from_text("x", "", false);
        assert_eq!(v.rows[0].kind, RowKind::Note);
        assert!(v.hunks.is_empty());
    }

    #[test]
    fn multi_file_diffs_get_file_rows() {
        let two = format!(
            "{DIFF}diff --git a/b.txt b/b.txt\n--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-x\n+y\n"
        );
        let v = DiffView::from_text("pr", &two, true);
        assert_eq!(v.rows.iter().filter(|r| r.kind == RowKind::File).count(), 2);
        assert_eq!(v.hunks.len(), 3);
    }
}
