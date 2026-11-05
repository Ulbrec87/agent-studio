//! Files panel: read-only tree + preview + fuzzy search (Hydra's editor panel without Monaco;
//! editing opens `$EDITOR`). Open files are re-read when their mtime changes (polled about
//! once a second: no file watchers).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use as_core::fs::{FileContent, FsEntry, SearchHit};
use as_proto::{Request, Response};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Outcome, Reply, nav, req, step};
use crate::app::Effect;
use crate::highlight::{LazyLines, MARGIN};
use crate::theme::Theme;
use crate::widgets::TextArea;

/// How often the open file's mtime is checked.
pub const POLL_MS: u64 = 1000;
const SEARCH_LIMIT: usize = 100;
/// Highlighted files kept besides the open one (per path, mtime and theme).
const HL_CACHE: usize = 4;

type HlKey = (String, u64, as_core::ThemeId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenFile {
    pub path: String,
    pub lines: Vec<String>,
    /// `Some(text)` instead of lines for binary / too large files and errors.
    pub placeholder: Option<String>,
    pub scroll: usize,
    pub mtime: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchUi {
    pub active: bool,
    pub query: TextArea,
    pub hits: Vec<SearchHit>,
    pub cursor: usize,
    seq: u64,
}

impl Default for SearchUi {
    fn default() -> Self {
        Self {
            active: false,
            query: TextArea::single_line(""),
            hits: Vec::new(),
            cursor: 0,
            seq: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FilesUi {
    pub root: String,
    pub show_hidden: bool,
    pub dirs: HashMap<String, Vec<FsEntry>>,
    pub expanded: BTreeSet<String>,
    pub cursor: usize,
    pub file: Option<OpenFile>,
    pub preview_focus: bool,
    pub error: Option<String>,
    pub search: SearchUi,
    /// Rows the preview shows (set by the renderer).
    pub preview_height: usize,
    /// Lazy highlighting of the open file and a few recent ones.
    hl: LazyLines,
    hl_key: Option<HlKey>,
    hl_cache: Vec<(HlKey, LazyLines)>,
    file_seq: u64,
    last_poll_ms: u64,
    /// A key asked for the open-in picker with this absolute path.
    pub want_open_in: Option<String>,
}

impl FilesUi {
    pub fn open(&mut self, root: &str) -> Vec<Effect> {
        if self.root != root {
            *self = Self {
                show_hidden: self.show_hidden,
                preview_height: self.preview_height,
                root: root.to_string(),
                ..Self::default()
            };
        }
        self.search.active = false;
        self.reload_all()
    }

    /// List the root and every expanded folder again.
    fn reload_all(&self) -> Vec<Effect> {
        let mut fx = vec![self.list_request("")];
        fx.extend(self.expanded.iter().map(|d| self.list_request(d)));
        fx
    }

    fn list_request(&self, path: &str) -> Effect {
        req(
            format!("fs:dir:{path}"),
            Request::FsReadDir {
                project_dir: self.root.clone(),
                path: path.to_string(),
                show_hidden: self.show_hidden,
            },
        )
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        self.walk("", 0, &mut out);
        out
    }

    fn walk(&self, dir: &str, depth: usize, out: &mut Vec<Row>) {
        let Some(entries) = self.dirs.get(dir) else {
            return;
        };
        for e in entries {
            let expanded = e.is_dir && self.expanded.contains(&e.path);
            out.push(Row {
                path: e.path.clone(),
                name: e.name.clone(),
                depth,
                is_dir: e.is_dir,
                expanded,
            });
            if expanded {
                self.walk(&e.path, depth + 1, out);
            }
        }
    }

    pub fn selected(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.cursor)
    }

    /// Absolute path of a project-relative one.
    pub fn abs(&self, rel: &str) -> String {
        let mut p = Path::new(&self.root).to_path_buf();
        for part in rel.split('/').filter(|s| !s.is_empty()) {
            p.push(part);
        }
        p.to_string_lossy().into_owned()
    }

    /// Show a file in the preview (expands its folders and puts the cursor on it).
    pub fn open_file(&mut self, rel: &str) -> Vec<Effect> {
        let mut fx = Vec::new();
        let mut prefix = String::new();
        for part in rel
            .split('/')
            .take(rel.split('/').count().saturating_sub(1))
        {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if self.expanded.insert(prefix.clone()) || !self.dirs.contains_key(&prefix) {
                fx.push(self.list_request(&prefix));
            }
        }
        if let Some(i) = self.rows().iter().position(|r| r.path == rel) {
            self.cursor = i;
        }
        self.file_seq += 1;
        self.file = Some(OpenFile {
            path: rel.to_string(),
            lines: Vec::new(),
            placeholder: Some("Loading\u{2026}".into()),
            scroll: 0,
            mtime: 0,
        });
        fx.push(req(
            format!("fs:file:{}", self.file_seq),
            Request::FsReadFile {
                project_dir: self.root.clone(),
                path: rel.to_string(),
            },
        ));
        fx
    }

    /// Highlight the lines the preview shows (`height` rows) plus a margin, lazily; the result is
    /// cached per (path, mtime, theme). `false`: more to do, draw again.
    pub fn highlight_visible(&mut self, theme: &Theme, height: usize) -> bool {
        let Some(f) = self.file.as_ref().filter(|f| f.placeholder.is_none()) else {
            return true;
        };
        let key: HlKey = (f.path.clone(), f.mtime, theme.id);
        if self.hl_key.as_ref() != Some(&key) {
            if let Some(old_key) = self.hl_key.take() {
                let old = std::mem::take(&mut self.hl);
                if old.is_enabled() {
                    self.hl_cache.retain(|(k, _)| *k != old_key);
                    self.hl_cache.push((old_key, old));
                    if self.hl_cache.len() > HL_CACHE {
                        self.hl_cache.remove(0);
                    }
                }
            }
            self.hl = match self.hl_cache.iter().position(|(k, _)| *k == key) {
                Some(i) => self.hl_cache.remove(i).1,
                None => {
                    let bytes: usize = f.lines.iter().map(|l| l.len() + 1).sum();
                    LazyLines::new(&f.path, f.lines.len(), bytes, theme)
                }
            };
            self.hl_key = Some(key);
        }
        self.hl.ensure(&f.lines, f.scroll + height + MARGIN)
    }

    /// Highlighted spans of line `i` of the open file.
    pub fn spans(&self, i: usize) -> Option<&[crate::highlight::StyledSpan]> {
        self.hl.get(i)
    }

    /// Time-driven work: check the open file's mtime.
    pub fn poll(&mut self, now_ms: u64) -> Vec<Effect> {
        let Some(f) = &self.file else {
            return Vec::new();
        };
        if now_ms.saturating_sub(self.last_poll_ms) < POLL_MS {
            return Vec::new();
        }
        self.last_poll_ms = now_ms;
        vec![req(
            format!("fs:mtime:{}", f.path),
            Request::FsMtime {
                project_dir: self.root.clone(),
                path: f.path.clone(),
            },
        )]
    }

    pub fn on_reply(&mut self, key: &str, result: Reply) -> Vec<Effect> {
        if let Some(dir) = key.strip_prefix("fs:dir:") {
            match result {
                Ok(Response::FsEntries(entries)) => {
                    self.error = None;
                    self.dirs.insert(dir.to_string(), entries);
                    let len = self.rows().len();
                    self.cursor = self.cursor.min(len.saturating_sub(1));
                }
                Err(e) => {
                    self.error = Some(e);
                    self.expanded.remove(dir);
                }
                Ok(_) => {}
            }
        } else if let Some(seq) = key.strip_prefix("fs:file:") {
            if seq == self.file_seq.to_string()
                && let Some(f) = &mut self.file
            {
                match result {
                    Ok(Response::FileContent(c)) => apply_content(f, &c),
                    Err(e) => {
                        f.lines.clear();
                        f.placeholder = Some(e);
                    }
                    Ok(_) => {}
                }
            }
        } else if let Some(path) = key.strip_prefix("fs:mtime:") {
            if let (Ok(Response::Number(m)), Some(f)) = (result, &self.file)
                && f.path == path
                && f.mtime != 0
                && m != f.mtime
            {
                let (scroll, p) = (f.scroll, f.path.clone());
                let fx = self.open_file(&p);
                if let Some(f) = &mut self.file {
                    f.scroll = scroll;
                }
                return fx;
            }
        } else if let Some(seq) = key.strip_prefix("fs:search:")
            && seq == self.search.seq.to_string()
            && let Ok(Response::SearchHits(hits)) = result
        {
            self.search.hits = hits;
            self.search.cursor = 0;
        }
        Vec::new()
    }

    pub fn start_search(&mut self) -> Vec<Effect> {
        self.search.active = true;
        self.search.query.clear();
        self.search.hits.clear();
        self.search_request()
    }

    fn search_request(&mut self) -> Vec<Effect> {
        self.search.seq += 1;
        vec![req(
            format!("fs:search:{}", self.search.seq),
            Request::FsSearch {
                project_dir: self.root.clone(),
                query: self.search.query.text(),
                limit: Some(SEARCH_LIMIT),
            },
        )]
    }

    fn search_key(&mut self, key: &KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Esc => self.search.active = false,
            KeyCode::Enter => {
                if let Some(hit) = self.search.hits.get(self.search.cursor).cloned() {
                    self.search.active = false;
                    self.preview_focus = true;
                    return self.open_file(&hit.path);
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                self.search.cursor = step(self.search.cursor, self.search.hits.len(), 1);
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.search.cursor = step(self.search.cursor, self.search.hits.len(), -1);
            }
            KeyCode::Char('n' | 'j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search.cursor = step(self.search.cursor, self.search.hits.len(), 1);
            }
            KeyCode::Char('p' | 'k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search.cursor = step(self.search.cursor, self.search.hits.len(), -1);
            }
            _ => {
                let before = self.search.query.text();
                self.search.query.handle_key(key);
                if self.search.query.text() != before {
                    return self.search_request();
                }
            }
        }
        Vec::new()
    }

    pub fn key(&mut self, key: &KeyEvent) -> (Outcome, Vec<Effect>) {
        if self.search.active {
            return (Outcome::Stay, self.search_key(key));
        }
        if self.preview_focus && self.file.is_some() {
            if key.code == KeyCode::Char('q') {
                return (Outcome::Close, Vec::new());
            }
            return (Outcome::Stay, self.preview_key(key));
        }
        let rows = self.rows();
        let selected = rows.get(self.cursor).cloned();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return (Outcome::Close, Vec::new()),
            KeyCode::Tab => self.preview_focus = self.file.is_some(),
            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                self.dirs.clear();
                return (Outcome::Stay, self.reload_all());
            }
            KeyCode::Char('f' | '/') => return (Outcome::Stay, self.start_search()),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(r) = selected {
                    if r.is_dir {
                        return (Outcome::Stay, self.toggle_dir(&r.path, !r.expanded));
                    }
                    self.preview_focus = true;
                    return (Outcome::Stay, self.open_file(&r.path));
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if let Some(r) = selected {
                    if r.is_dir && r.expanded {
                        return (Outcome::Stay, self.toggle_dir(&r.path, false));
                    }
                    // Go to the parent folder.
                    if let Some(parent) = r.path.rsplit_once('/').map(|(p, _)| p.to_string())
                        && let Some(i) = rows.iter().position(|x| x.path == parent)
                    {
                        self.cursor = i;
                    }
                }
            }
            KeyCode::Char('E') => {
                if let Some(r) = selected.filter(|r| !r.is_dir) {
                    let abs = self.abs(&r.path);
                    return (
                        Outcome::Stay,
                        vec![Effect::Suspend(crate::suspend::editor_command_from_env(
                            Path::new(&abs),
                        ))],
                    );
                }
            }
            KeyCode::Char('y') => {
                if let Some(r) = selected {
                    return (Outcome::Stay, vec![Effect::Clipboard(self.abs(&r.path))]);
                }
            }
            KeyCode::Char('o') => {
                self.want_open_in =
                    Some(selected.map_or_else(|| self.root.clone(), |r| self.abs(&r.path)));
            }
            _ => {
                if let Some(d) = nav(key, 15) {
                    self.cursor = step(self.cursor, rows.len(), d);
                }
            }
        }
        (Outcome::Stay, Vec::new())
    }

    fn toggle_dir(&mut self, path: &str, open: bool) -> Vec<Effect> {
        if open {
            self.expanded.insert(path.to_string());
            if !self.dirs.contains_key(path) {
                return vec![self.list_request(path)];
            }
        } else {
            self.expanded.remove(path);
            let len = self.rows().len();
            self.cursor = self.cursor.min(len.saturating_sub(1));
        }
        Vec::new()
    }

    fn preview_key(&mut self, key: &KeyEvent) -> Vec<Effect> {
        let height = self.preview_height.max(1);
        let Some(path) = self.file.as_ref().map(|f| f.path.clone()) else {
            return Vec::new();
        };
        let abs = self.abs(&path);
        match key.code {
            KeyCode::Esc | KeyCode::Tab | KeyCode::Left | KeyCode::Char('h') => {
                self.preview_focus = false;
            }
            KeyCode::Char('E') => {
                return vec![Effect::Suspend(crate::suspend::editor_command_from_env(
                    Path::new(&abs),
                ))];
            }
            KeyCode::Char('y') => return vec![Effect::Clipboard(abs)],
            KeyCode::Char('o') => self.want_open_in = Some(abs),
            KeyCode::Char('f' | '/') => return self.start_search(),
            _ => {
                if let (Some(d), Some(f)) = (nav(key, height.saturating_sub(1)), &mut self.file) {
                    let max = f.lines.len().saturating_sub(height);
                    f.scroll = f.scroll.saturating_add_signed(d).min(max);
                }
            }
        }
        Vec::new()
    }
}

fn apply_content(f: &mut OpenFile, c: &FileContent) {
    match c {
        FileContent::Text { text, mtime_ms, .. } => {
            f.lines = text.lines().map(str::to_string).collect();
            f.placeholder = text.is_empty().then(|| "(empty file)".to_string());
            f.mtime = *mtime_ms;
        }
        FileContent::Binary { size } => {
            f.lines.clear();
            f.placeholder = Some(format!("binary file ({size} bytes)"));
        }
        FileContent::TooLarge { size } => {
            f.lines.clear();
            f.placeholder = Some(format!(
                "file too large to preview ({} KiB); press E to open it in your editor",
                size / 1024
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, dir: bool) -> FsEntry {
        FsEntry {
            name: path.rsplit('/').next().unwrap_or(path).into(),
            path: path.into(),
            is_dir: dir,
            is_symlink: false,
            size: 0,
        }
    }

    fn k(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn ui() -> FilesUi {
        let mut ui = FilesUi {
            root: std::env::temp_dir().to_string_lossy().into_owned(),
            ..Default::default()
        };
        ui.on_reply(
            "fs:dir:",
            Ok(Response::FsEntries(vec![
                entry("src", true),
                entry("README.md", false),
            ])),
        );
        ui
    }

    #[test]
    fn expanding_loads_and_flattens_the_tree() {
        let mut ui = ui();
        assert_eq!(ui.rows().len(), 2);
        let (_, fx) = ui.key(&k(KeyCode::Enter));
        assert!(matches!(&fx[..], [Effect::Req { key, .. }] if key == "fs:dir:src"));
        ui.on_reply(
            "fs:dir:src",
            Ok(Response::FsEntries(vec![entry("src/main.rs", false)])),
        );
        let rows = ui.rows();
        assert_eq!(
            rows.iter()
                .map(|r| (r.name.as_str(), r.depth))
                .collect::<Vec<_>>(),
            [("src", 0), ("main.rs", 1), ("README.md", 0)]
        );
        // Left collapses, Left again on a child goes to the parent.
        ui.cursor = 1;
        ui.key(&k(KeyCode::Left));
        assert_eq!(ui.cursor, 0);
        ui.key(&k(KeyCode::Left));
        assert_eq!(ui.rows().len(), 2);
    }

    #[test]
    fn opening_a_file_requests_content_and_applies_it() {
        let mut ui = ui();
        ui.cursor = 1;
        let (_, fx) = ui.key(&k(KeyCode::Enter));
        let key = match &fx[..] {
            [
                Effect::Req {
                    key,
                    req: Request::FsReadFile { path, .. },
                },
            ] if path == "README.md" => key.clone(),
            other => panic!("{other:?}"),
        };
        assert!(ui.preview_focus);
        ui.on_reply(
            &key,
            Ok(Response::FileContent(FileContent::Text {
                text: "a\nb\n".into(),
                truncated: false,
                size: 4,
                mtime_ms: 7,
            })),
        );
        let f = ui.file.as_ref().unwrap();
        assert_eq!(
            (f.lines.len(), f.mtime, f.placeholder.clone()),
            (2, 7, None)
        );
        // A stale answer is ignored.
        ui.on_reply(
            "fs:file:0",
            Ok(Response::FileContent(FileContent::Binary { size: 1 })),
        );
        assert_eq!(ui.file.as_ref().unwrap().lines.len(), 2);
        ui.on_reply(
            &key,
            Ok(Response::FileContent(FileContent::Binary { size: 9 })),
        );
        assert!(
            ui.file
                .as_ref()
                .unwrap()
                .placeholder
                .as_deref()
                .is_some_and(|p| p.contains("binary"))
        );
    }

    #[test]
    fn polling_reloads_when_the_mtime_changed_and_keeps_the_scroll() {
        let mut ui = ui();
        ui.open_file("README.md");
        let key = format!("fs:file:{}", ui.file_seq);
        let text = |m| {
            Response::FileContent(FileContent::Text {
                text: "x\n".repeat(50),
                truncated: false,
                size: 1,
                mtime_ms: m,
            })
        };
        ui.on_reply(&key, Ok(text(5)));
        ui.file.as_mut().unwrap().scroll = 9;
        assert!(ui.poll(10_000).len() == 1);
        assert!(ui.poll(10_500).is_empty(), "polled at most once a second");
        assert!(
            ui.on_reply("fs:mtime:README.md", Ok(Response::Number(5)))
                .is_empty()
        );
        let fx = ui.on_reply("fs:mtime:README.md", Ok(Response::Number(6)));
        assert_eq!(fx.len(), 1);
        assert_eq!(ui.file.as_ref().unwrap().scroll, 9);
    }

    #[test]
    fn search_sends_queries_and_enter_opens_the_hit() {
        let mut ui = ui();
        let fx = ui.start_search();
        assert_eq!(fx.len(), 1);
        let (_, fx) = ui.key(&k(KeyCode::Char('m')));
        let key = match &fx[..] {
            [
                Effect::Req {
                    key,
                    req: Request::FsSearch { query, .. },
                },
            ] if query == "m" => key.clone(),
            other => panic!("{other:?}"),
        };
        ui.on_reply(
            &key,
            Ok(Response::SearchHits(vec![SearchHit {
                path: "src/main.rs".into(),
                name: "main.rs".into(),
                score: 9,
            }])),
        );
        let (_, fx) = ui.key(&k(KeyCode::Enter));
        assert!(!ui.search.active);
        assert!(fx.iter().any(|e| matches!(e, Effect::Req { req: Request::FsReadFile { path, .. }, .. } if path == "src/main.rs")));
        assert!(
            ui.expanded.contains("src"),
            "the folders of the hit are expanded"
        );
    }

    #[test]
    fn hidden_toggle_reloads_expanded_folders_and_keys_dispatch() {
        let mut ui = ui();
        ui.expanded.insert("src".into());
        let (_, fx) = ui.key(&k(KeyCode::Char('.')));
        assert!(ui.show_hidden);
        assert!(fx.len() >= 2);
        assert!(ui.expanded.contains("src"));
        ui.on_reply(
            "fs:dir:",
            Ok(Response::FsEntries(vec![
                entry("src", true),
                entry("README.md", false),
            ])),
        );
        ui.cursor = 1;
        let (_, fx) = ui.key(&k(KeyCode::Char('y')));
        assert!(matches!(&fx[..], [Effect::Clipboard(p)] if p.ends_with("README.md")));
        ui.key(&k(KeyCode::Char('o')));
        assert!(ui.want_open_in.is_some());
        let (_, fx) = ui.key(&k(KeyCode::Char('E')));
        assert!(matches!(&fx[..], [Effect::Suspend(_)]));
        let (out, _) = ui.key(&k(KeyCode::Char('q')));
        assert_eq!(out, Outcome::Close);
    }
}
