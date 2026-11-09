//! Pure parser for unified diffs (`git diff`, `gh pr diff`, plain `diff -u`).
//!
//! Tolerant: unknown header lines are kept in [`DiffFile::header`], malformed hunks never panic.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub enum DiffLineKind {
    Context,
    Add,
    Del,
    /// `\ No newline at end of file`.
    Meta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// Line content without the leading `+`/`-`/space marker (`Meta`: the whole line).
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    /// The whole `@@ -a,b +c,d @@ section` line.
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub enum DiffFileStatus {
    #[default]
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    /// `None` for a new file.
    pub old_path: Option<String>,
    /// `None` for a deleted file.
    pub new_path: Option<String>,
    pub status: DiffFileStatus,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    /// Header lines before the first hunk (`diff --git`, `index`, modes, `---`/`+++`).
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
}

impl DiffFile {
    /// New path, else the old one.
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or("")
    }
}

/// One file's slice of a multi-file diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct FileDiffText {
    pub path: String,
    pub patch: String,
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let Some(inner) = s.strip_prefix('"').and_then(|r| r.strip_suffix('"')) else {
        return s.to_string();
    };
    let bytes = inner.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 >= bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        i += 1;
        match bytes[i] {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'"' => out.push(b'"'),
            b'\\' => out.push(b'\\'),
            d @ b'0'..=b'7' => {
                let mut v = u32::from(d - b'0');
                let mut n = 1;
                while n < 3 && i + 1 < bytes.len() && matches!(bytes[i + 1], b'0'..=b'7') {
                    i += 1;
                    v = v * 8 + u32::from(bytes[i] - b'0');
                    n += 1;
                }
                out.push(v as u8);
            }
            other => {
                out.push(b'\\');
                out.push(other);
            }
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `a/x b/x` (the part after `diff --git `) into `(old, new)` without their prefixes.
fn split_git_header_paths(rest: &str) -> Option<(String, String)> {
    let rest = rest.trim_end_matches('\r');
    if let Some(after_quote) = rest.strip_prefix('"') {
        // Quoted names: `"a/x y" "b/x y"` or mixed.
        let first_end = after_quote
            .char_indices()
            .scan(false, |esc, (i, c)| {
                let end = !*esc && c == '"';
                *esc = !*esc && c == '\\';
                Some((i, end))
            })
            .find(|(_, end)| *end)
            .map(|(i, _)| i + 2)?;
        let (a, b) = rest.split_at(first_end);
        return Some((strip_ab(&unquote(a)), strip_ab(&unquote(b))));
    }
    // Same path on both sides (no rename): `a/P b/P` has length 2|P| + 5.
    if rest.len() >= 5 && (rest.len() - 5).is_multiple_of(2) {
        let p = (rest.len() - 5) / 2;
        if let (Some(a), Some(b)) = (rest.get(..p + 2), rest.get(p + 3..))
            && rest.get(p + 2..p + 3) == Some(" ")
            && a.starts_with("a/")
            && b.starts_with("b/")
            && a[2..] == b[2..]
        {
            return Some((a[2..].to_string(), b[2..].to_string()));
        }
    }
    let idx = rest.find(" b/").or_else(|| rest.find(" \""))?;
    let (a, b) = rest.split_at(idx);
    Some((strip_ab(&unquote(a)), strip_ab(&unquote(b))))
}

fn strip_ab(p: &str) -> String {
    p.strip_prefix("a/")
        .or_else(|| p.strip_prefix("b/"))
        .unwrap_or(p)
        .to_string()
}

/// Path of a `---`/`+++` line: no timestamp/tab suffix, `/dev/null` is `None`.
fn marker_path(rest: &str, git_style: bool) -> Option<String> {
    let rest = rest.trim_end_matches('\r');
    let raw = if rest.starts_with('"') {
        rest.to_string()
    } else {
        rest.split('\t').next().unwrap_or("").to_string()
    };
    let p = unquote(&raw);
    if p == "/dev/null" {
        return None;
    }
    if git_style {
        Some(strip_ab(&p))
    } else {
        Some(p)
    }
}

fn parse_range(s: &str, sign: char) -> Option<(u32, u32)> {
    let s = s.strip_prefix(sign)?;
    match s.split_once(',') {
        Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ ")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(' ')?;
    let (old_start, old_lines) = parse_range(old, '-')?;
    let (new_start, new_lines) = parse_range(new, '+')?;
    Some(Hunk {
        header: line.trim_end_matches('\r').to_string(),
        old_start,
        old_lines,
        new_start,
        new_lines,
        lines: Vec::new(),
    })
}

struct Parser {
    files: Vec<DiffFile>,
    cur: Option<DiffFile>,
    hunk: Option<Hunk>,
    old_no: u32,
    new_no: u32,
    old_rem: u32,
    new_rem: u32,
    git_style: bool,
}

impl Parser {
    fn flush_hunk(&mut self) {
        if let (Some(h), Some(f)) = (self.hunk.take(), self.cur.as_mut()) {
            f.hunks.push(h);
        }
    }

    fn flush_file(&mut self) {
        self.flush_hunk();
        if let Some(f) = self.cur.take() {
            self.files.push(f);
        }
    }

    fn hunk_line(&mut self, line: &str) {
        let Some(h) = self.hunk.as_mut() else { return };
        let Some(f) = self.cur.as_mut() else { return };
        let (kind, text) = match line.as_bytes().first() {
            Some(b'+') => (DiffLineKind::Add, &line[1..]),
            Some(b'-') => (DiffLineKind::Del, &line[1..]),
            Some(b'\\') => (DiffLineKind::Meta, line),
            Some(b' ') => (DiffLineKind::Context, &line[1..]),
            _ => (DiffLineKind::Context, line),
        };
        let (old_no, new_no) = match kind {
            DiffLineKind::Context => {
                let r = (Some(self.old_no), Some(self.new_no));
                self.old_no += 1;
                self.new_no += 1;
                self.old_rem = self.old_rem.saturating_sub(1);
                self.new_rem = self.new_rem.saturating_sub(1);
                r
            }
            DiffLineKind::Add => {
                let r = (None, Some(self.new_no));
                self.new_no += 1;
                self.new_rem = self.new_rem.saturating_sub(1);
                f.additions += 1;
                r
            }
            DiffLineKind::Del => {
                let r = (Some(self.old_no), None);
                self.old_no += 1;
                self.old_rem = self.old_rem.saturating_sub(1);
                f.deletions += 1;
                r
            }
            DiffLineKind::Meta => (None, None),
        };
        h.lines.push(DiffLine {
            kind,
            old_no,
            new_no,
            text: text.to_string(),
        });
    }

    fn header_line(&mut self, line: &str) {
        let git_style = self.git_style;
        let Some(f) = self.cur.as_mut() else { return };
        let l = line.trim_end_matches('\r');
        if let Some(p) = l.strip_prefix("--- ") {
            f.old_path = marker_path(p, git_style);
            if f.old_path.is_none() && f.status == DiffFileStatus::Modified {
                f.status = DiffFileStatus::Added;
            }
        } else if let Some(p) = l.strip_prefix("+++ ") {
            f.new_path = marker_path(p, git_style);
            if f.new_path.is_none() && f.status == DiffFileStatus::Modified {
                f.status = DiffFileStatus::Deleted;
            }
        } else if l.starts_with("new file mode") {
            f.status = DiffFileStatus::Added;
            f.old_path = None;
        } else if l.starts_with("deleted file mode") {
            f.status = DiffFileStatus::Deleted;
            f.new_path = None;
        } else if let Some(p) = l.strip_prefix("rename from ") {
            f.status = DiffFileStatus::Renamed;
            f.old_path = Some(unquote(p));
        } else if let Some(p) = l.strip_prefix("rename to ") {
            f.status = DiffFileStatus::Renamed;
            f.new_path = Some(unquote(p));
        } else if let Some(p) = l.strip_prefix("copy from ") {
            f.status = DiffFileStatus::Copied;
            f.old_path = Some(unquote(p));
        } else if let Some(p) = l.strip_prefix("copy to ") {
            f.status = DiffFileStatus::Copied;
            f.new_path = Some(unquote(p));
        } else if l.starts_with("Binary files ") || l.starts_with("GIT binary patch") {
            f.binary = true;
        }
        if !f.binary || l.starts_with("Binary files ") || l.starts_with("GIT binary patch") {
            f.header.push(l.to_string());
        }
    }

    fn feed(&mut self, line: &str) {
        let in_hunk = self.hunk.is_some();
        if in_hunk && (self.old_rem > 0 || self.new_rem > 0 || line.starts_with('\\')) {
            self.hunk_line(line);
            return;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            self.flush_file();
            self.git_style = true;
            let (old, new) = split_git_header_paths(rest).unzip();
            self.cur = Some(DiffFile {
                old_path: old,
                new_path: new,
                header: vec![line.trim_end_matches('\r').to_string()],
                ..DiffFile::default()
            });
            return;
        }
        if line.starts_with("@@ ") {
            self.flush_hunk();
            if self.cur.is_none() {
                self.cur = Some(DiffFile::default());
            }
            if let Some(h) = parse_hunk_header(line) {
                self.old_no = h.old_start;
                self.new_no = h.new_start;
                self.old_rem = h.old_lines;
                self.new_rem = h.new_lines;
                self.hunk = Some(h);
            }
            return;
        }
        // A `--- ` after hunks (or without any file yet) starts a plain (non-git) file diff.
        if line.starts_with("--- ")
            && self
                .cur
                .as_ref()
                .is_none_or(|f| !f.hunks.is_empty() || self.hunk.is_some())
        {
            self.flush_file();
            self.git_style = false;
            self.cur = Some(DiffFile::default());
        }
        self.flush_hunk();
        self.header_line(line);
    }
}

/// Parse unified diff text (one or many files) into structured files.
pub fn parse_unified_diff(text: &str) -> Vec<DiffFile> {
    let mut p = Parser {
        files: Vec::new(),
        cur: None,
        hunk: None,
        old_no: 0,
        new_no: 0,
        old_rem: 0,
        new_rem: 0,
        git_style: false,
    };
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        if line.is_empty() && lines.peek().is_none() {
            break;
        }
        p.feed(line);
    }
    p.flush_file();
    p.files
}

/// Parse the output of `gh pr diff` (same format as `git diff`).
pub fn parse_pr_diff(full_diff: &str) -> Vec<DiffFile> {
    parse_unified_diff(full_diff)
}

/// Split a multi-file `git diff` into one raw patch per file (each starts at `diff --git`).
pub fn split_diff_by_file(full_diff: &str) -> Vec<FileDiffText> {
    let mut starts: Vec<usize> = Vec::new();
    let mut offset = 0;
    for line in full_diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            starts.push(offset);
        }
        offset += line.len();
    }
    let mut out = Vec::new();
    for (i, &s) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(full_diff.len());
        let patch = full_diff[s..end]
            .strip_suffix('\n')
            .unwrap_or(&full_diff[s..end]);
        let path = parse_unified_diff(patch)
            .first()
            .map(|f| f.path().to_string())
            .unwrap_or_default();
        out.push(FileDiffText {
            path,
            patch: patch.to_string(),
        });
    }
    out
}

/// The raw patch of `path` inside a multi-file diff (empty when absent). Matches the new or old path.
pub fn extract_file_diff(full_diff: &str, path: &str) -> String {
    let wanted = path.replace('\\', "/");
    for chunk in split_diff_by_file(full_diff) {
        if chunk.path == wanted {
            return chunk.patch;
        }
        let renamed_from = parse_unified_diff(&chunk.patch)
            .first()
            .and_then(|f| f.old_path.clone());
        if renamed_from.as_deref() == Some(wanted.as_str()) {
            return chunk.patch;
        }
    }
    String::new()
}
