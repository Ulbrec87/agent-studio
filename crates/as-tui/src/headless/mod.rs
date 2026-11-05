//! Headless runs panel (Hydra `HeadlessPanel.tsx`): state of the list and of the log viewer.
//! Drawing is `ui::headless`, keys and daemon traffic are `app::headless_ui`.

pub mod render;

use as_core::headless::{HeadlessRun, HeadlessRunLog, StatusFilter, run_matches};

use crate::widgets::TextArea;
use render::{Kind, Row};

/// Log lines kept in the viewer (the daemon is asked for this many at most).
pub const LOG_TAIL_LINES: usize = 1400;
pub const LOG_MAX_CHARS: usize = 240_000;

/// The log of the run being viewed.
#[derive(Debug, Clone, PartialEq)]
pub struct LogView {
    pub run_id: String,
    /// Raw log lines (what the daemon stores).
    lines: Vec<String>,
    /// Lines the whole log has / the viewer got.
    pub total_lines: usize,
    pub truncated: bool,
    /// Shown untouched instead of rendered.
    pub raw: bool,
    pub loaded: bool,
    pub error: Option<String>,
    rows: Vec<Row>,
    /// Display rows scrolled up from the bottom: 0 follows the end of the log.
    pub up: usize,
    /// Width of the last draw, to anchor the view when lines arrive while scrolled up.
    pub width: usize,
}

impl LogView {
    pub fn new(run_id: &str) -> Self {
        Self {
            run_id: run_id.to_string(),
            lines: Vec::new(),
            total_lines: 0,
            truncated: false,
            raw: false,
            loaded: false,
            error: None,
            rows: Vec::new(),
            up: 0,
            width: 100,
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn follow(&self) -> bool {
        self.up == 0
    }

    pub fn set_log(&mut self, log: HeadlessRunLog) {
        self.lines = if log.content.is_empty() {
            Vec::new()
        } else {
            log.content.split('\n').map(str::to_string).collect()
        };
        self.total_lines = log.total_lines;
        self.truncated = log.truncated;
        self.loaded = true;
        self.error = None;
        self.rebuild();
    }

    pub fn failed(&mut self, error: String) {
        self.loaded = true;
        self.error = Some(error);
    }

    fn rebuild(&mut self) {
        self.rows = render::render_log(&self.lines, self.raw, self.truncated);
    }

    pub fn toggle_raw(&mut self) {
        self.raw = !self.raw;
        self.rebuild();
    }

    /// One more line from the live stream.
    pub fn push_line(&mut self, line: &str) {
        self.total_lines += 1;
        self.lines.push(line.to_string());
        if self.lines.len() > LOG_TAIL_LINES * 2 {
            let cut = self.lines.len() - LOG_TAIL_LINES;
            self.lines.drain(..cut);
            self.truncated = true;
            self.rebuild();
            return;
        }
        let new = render::render_line(line, self.raw);
        if self.up > 0 {
            // Keep what the user reads where it is.
            let added: usize = new
                .iter()
                .map(|r| render::wrap(&r.text, self.width).len())
                .sum();
            self.up += added;
        }
        self.rows.extend(new);
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.up = self.up.saturating_add(n);
    }

    pub fn scroll_down(&mut self, n: usize) {
        self.up = self.up.saturating_sub(n);
    }

    pub fn to_end(&mut self) {
        self.up = 0;
    }

    pub fn to_top(&mut self) {
        self.up = usize::MAX / 2;
    }

    /// Does the log contain any row of this kind (used by tests and the title)?
    pub fn has(&self, kind: Kind) -> bool {
        self.rows.iter().any(|r| r.kind == kind)
    }
}

#[derive(Debug, Clone)]
pub struct HeadlessUi {
    /// Every run, newest first (filters apply on screen).
    pub runs: Vec<HeadlessRun>,
    pub loaded: bool,
    pub error: Option<String>,
    pub query: TextArea,
    pub filtering: bool,
    pub status: StatusFilter,
    /// Index into [`HeadlessUi::visible`].
    pub cursor: usize,
    /// The log viewer, once a run was opened.
    pub view: Option<LogView>,
}

impl Default for HeadlessUi {
    fn default() -> Self {
        Self {
            runs: Vec::new(),
            loaded: false,
            error: None,
            query: TextArea::single_line(""),
            filtering: false,
            status: StatusFilter::All,
            cursor: 0,
            view: None,
        }
    }
}

fn started(run: &HeadlessRun) -> i64 {
    chrono::DateTime::parse_from_rfc3339(&run.started_at).map_or(0, |d| d.timestamp_millis())
}

impl HeadlessUi {
    /// Indices into `runs` that pass the status chip and the search.
    pub fn visible(&self) -> Vec<usize> {
        let query = self.query.text().trim().to_lowercase();
        self.runs
            .iter()
            .enumerate()
            .filter(|(_, r)| self.status.matches(r.status))
            .filter(|(_, r)| query.is_empty() || run_matches(r, &query))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn selected(&self) -> Option<&HeadlessRun> {
        let visible = self.visible();
        visible.get(self.cursor).map(|i| &self.runs[*i])
    }

    pub fn clamp_cursor(&mut self) {
        let n = self.visible().len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
    }

    pub fn set_runs(&mut self, runs: Vec<HeadlessRun>) {
        let keep = self.selected().map(|r| r.id.clone());
        self.replace_runs(runs, keep);
    }

    fn replace_runs(&mut self, mut runs: Vec<HeadlessRun>, keep: Option<String>) {
        runs.sort_by(|a, b| started(b).cmp(&started(a)).then_with(|| b.id.cmp(&a.id)));
        self.runs = runs;
        self.loaded = true;
        self.error = None;
        if let Some(id) = keep
            && let Some(pos) = self.visible().iter().position(|i| self.runs[*i].id == id)
        {
            self.cursor = pos;
        }
        self.clamp_cursor();
    }

    /// A run started or changed state: insert or replace it.
    pub fn upsert(&mut self, run: HeadlessRun) {
        let keep = self.selected().map(|r| r.id.clone());
        let mut runs = std::mem::take(&mut self.runs);
        match runs.iter_mut().find(|r| r.id == run.id) {
            Some(existing) => *existing = run,
            None => runs.push(run),
        }
        let loaded = self.loaded;
        self.replace_runs(runs, keep);
        self.loaded = loaded;
    }

    pub fn run(&self, id: &str) -> Option<&HeadlessRun> {
        self.runs.iter().find(|r| r.id == id)
    }

    pub fn cycle_status(&mut self) {
        let all = StatusFilter::CYCLE;
        let pos = all.iter().position(|s| *s == self.status).unwrap_or(0);
        self.status = all[(pos + 1) % all.len()];
        self.cursor = 0;
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let n = self.visible().len();
        if n == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = (self.cursor as isize + delta).clamp(0, n as isize - 1) as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use as_core::headless::HeadlessRunStatus;

    fn run(id: &str, at: &str, status: HeadlessRunStatus, prompt: &str) -> HeadlessRun {
        HeadlessRun {
            id: id.into(),
            prompt: prompt.into(),
            started_at: at.into(),
            status,
            model: "sonnet".into(),
            ..Default::default()
        }
    }

    #[test]
    fn filters_sorts_and_keeps_the_selection() {
        let mut ui = HeadlessUi::default();
        ui.set_runs(vec![
            run(
                "a",
                "2026-01-01T00:00:00Z",
                HeadlessRunStatus::Completed,
                "fix bug",
            ),
            run(
                "b",
                "2026-03-01T00:00:00Z",
                HeadlessRunStatus::Running,
                "write docs",
            ),
            run(
                "c",
                "2026-02-01T00:00:00Z",
                HeadlessRunStatus::Errored,
                "fix tests",
            ),
        ]);
        let ids = |ui: &HeadlessUi| -> Vec<String> {
            ui.visible()
                .iter()
                .map(|i| ui.runs[*i].id.clone())
                .collect()
        };
        assert_eq!(ids(&ui), ["b", "c", "a"]);
        ui.query.set_text("FIX");
        assert_eq!(ids(&ui), ["c", "a"]);
        ui.status = StatusFilter::Errored;
        assert_eq!(ids(&ui), ["c"]);
        ui.status = StatusFilter::All;
        ui.query.clear();
        ui.cursor = 1;
        assert_eq!(ui.selected().unwrap().id, "c");
        // A new run on top moves the list, not the selection.
        ui.upsert(run(
            "d",
            "2026-04-01T00:00:00Z",
            HeadlessRunStatus::Running,
            "new",
        ));
        assert_eq!(ui.selected().unwrap().id, "c");
        // A status change replaces the row.
        ui.upsert(run(
            "b",
            "2026-03-01T00:00:00Z",
            HeadlessRunStatus::Completed,
            "write docs",
        ));
        assert_eq!(ui.run("b").unwrap().status, HeadlessRunStatus::Completed);
        assert_eq!(ui.runs.len(), 4);
    }

    #[test]
    fn status_chip_cycles() {
        let mut ui = HeadlessUi::default();
        for expected in [
            StatusFilter::Running,
            StatusFilter::Completed,
            StatusFilter::Errored,
            StatusFilter::Canceled,
            StatusFilter::All,
        ] {
            ui.cycle_status();
            assert_eq!(ui.status, expected);
        }
    }

    #[test]
    fn log_follows_the_end_and_anchors_when_scrolled() {
        let mut view = LogView::new("r1");
        view.set_log(HeadlessRunLog {
            run_id: "r1".into(),
            content: "one\ntwo".into(),
            total_lines: 2,
            returned_lines: 2,
            truncated: false,
        });
        assert!(view.follow());
        view.push_line("three");
        assert_eq!(view.rows().len(), 3);
        assert!(view.follow());
        view.scroll_up(1);
        assert!(!view.follow());
        view.push_line("four");
        assert_eq!(view.up, 2, "the reader keeps their place");
        view.to_end();
        assert!(view.follow());
        assert_eq!(view.total_lines, 4);
        view.toggle_raw();
        assert!(view.raw);
    }
}
