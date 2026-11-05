//! A scrollable unified-diff view: flattens parsed [`DiffFile`]s into rows (file header, hunk
//! header, context/add/delete lines with both line numbers) for the renderer.

use as_core::git::{DiffFile, DiffLineKind};

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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiffView {
    pub title: String,
    pub rows: Vec<DiffRow>,
    pub scroll: usize,
    /// Row index of every hunk header.
    pub hunks: Vec<usize>,
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
        view
    }

    pub fn from_text(title: &str, diff: &str, show_file_headers: bool) -> Self {
        Self::from_files(
            title,
            &as_core::git::parse_unified_diff(diff),
            show_file_headers,
        )
    }

    pub fn max_scroll(&self, height: usize) -> usize {
        self.rows.len().saturating_sub(height.max(1))
    }

    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll(height));
    }

    /// Jump to the next hunk header after the current top row.
    pub fn next_hunk(&mut self, height: usize) {
        if let Some(&row) = self.hunks.iter().find(|&&r| r > self.scroll) {
            self.scroll = row.min(self.max_scroll(height));
        }
    }

    pub fn prev_hunk(&mut self) {
        if let Some(&row) = self.hunks.iter().rev().find(|&&r| r < self.scroll) {
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
