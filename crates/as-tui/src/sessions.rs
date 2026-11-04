//! State and logic of the session picker ("Resume session"): the list of a provider's sessions
//! with fuzzy filter, age / project / hidden toggles and hide. No rendering here (`ui::dialogs`).

use as_core::ProviderId;
use as_core::paths::tildify;
use as_core::sessions::SessionSummary;
use chrono::{DateTime, Utc};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::widgets::TextArea;

/// Where the picker was opened from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerOrigin {
    /// `prefix o`: picking creates the agent right away.
    Standalone,
    /// From the New agent dialog: picking fills the dialog.
    Form,
}

/// What the app has to do after a key in the picker.
#[derive(Debug, Clone, PartialEq)]
pub enum PickerAction {
    None,
    Cancel,
    Pick(Box<SessionSummary>),
    Hide {
        session_id: String,
        hidden: bool,
    },
    /// (Re)load the sessions of this provider (`force`: rescan the disk).
    Load {
        provider: ProviderId,
        force: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionPicker {
    pub origin: PickerOrigin,
    pub provider: ProviderId,
    pub loading: bool,
    pub error: Option<String>,
    sessions: Vec<SessionSummary>,
    pub query: TextArea,
    /// Typing into the filter (otherwise keys are commands).
    pub filtering: bool,
    /// Index into [`SessionPicker::visible`].
    pub cursor: usize,
    /// Show every age instead of the last `max_age_days`.
    pub all_ages: bool,
    pub max_age_days: u32,
    /// Only sessions of `project_dir` (and below).
    pub project_only: bool,
    pub project_dir: String,
    pub show_hidden: bool,
    /// Epoch ms the age filter and the age column are computed against.
    pub now_ms: i64,
    visible: Vec<usize>,
}

pub fn normalize_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    let p = p.trim_end_matches('/');
    if cfg!(windows) {
        p.to_lowercase()
    } else {
        p.to_string()
    }
}

/// One line of text: whitespace (newlines included) collapsed.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `now`, `5m`, `3h`, `2d`, `3w`, `4mo`, `2y`.
pub fn format_age(modified_ms: i64, now_ms: i64) -> String {
    let secs = (now_ms - modified_ms).max(0) / 1000;
    match secs {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        86_400..=1_209_599 => format!("{}d", secs / 86_400),
        1_209_600..=5_183_999 => format!("{}w", secs / 604_800),
        5_184_000..=31_535_999 => format!("{}mo", secs / 2_592_000),
        _ => format!("{}y", secs / 31_536_000),
    }
}

fn modified_ms(s: &SessionSummary) -> i64 {
    DateTime::parse_from_rfc3339(&s.modified_at)
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

impl SessionPicker {
    pub fn new(
        origin: PickerOrigin,
        provider: ProviderId,
        project_dir: &str,
        max_age_days: u32,
    ) -> Self {
        Self {
            origin,
            provider,
            loading: true,
            error: None,
            sessions: Vec::new(),
            query: TextArea::single_line(""),
            filtering: false,
            cursor: 0,
            all_ages: max_age_days == 0,
            max_age_days,
            project_only: false,
            project_dir: project_dir.trim().to_string(),
            show_hidden: false,
            now_ms: Utc::now().timestamp_millis(),
            visible: Vec::new(),
        }
    }

    /// The sessions as listed by the daemon (hidden ones flagged); keeps the cursor on the same
    /// session when it is still there.
    pub fn set_sessions(
        &mut self,
        provider: ProviderId,
        result: Result<Vec<SessionSummary>, String>,
    ) {
        if provider != self.provider {
            return; // a stale answer after switching provider
        }
        self.loading = false;
        let keep = self.selected().map(|s| s.session_id.clone());
        match result {
            Ok(list) => {
                self.error = None;
                self.sessions = list;
            }
            Err(error) => {
                self.sessions.clear();
                self.error = Some(error);
            }
        }
        self.refilter_keeping(keep);
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn visible(&self) -> impl Iterator<Item = &SessionSummary> {
        self.visible.iter().map(|i| &self.sessions[*i])
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn selected(&self) -> Option<&SessionSummary> {
        self.visible
            .get(self.cursor)
            .and_then(|i| self.sessions.get(*i))
    }

    /// Hidden sessions that the current toggles leave out (for the footer hint).
    pub fn hidden_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.hidden).count()
    }

    /// Mark a session hidden/unhidden locally (the daemon was asked to persist it).
    pub fn mark_hidden(&mut self, session_id: &str, hidden: bool) {
        for s in &mut self.sessions {
            if s.session_id == session_id {
                s.hidden = hidden;
            }
        }
        self.refilter();
    }

    /// Recompute the visible list from the sessions and the filters; keeps the selected session.
    pub fn refilter(&mut self) {
        let keep = self.selected().map(|s| s.session_id.clone());
        self.refilter_keeping(keep);
    }

    fn refilter_keeping(&mut self, keep: Option<String>) {
        let cutoff = (!self.all_ages && self.max_age_days > 0)
            .then(|| self.now_ms - i64::from(self.max_age_days) * 86_400_000);
        let project = (self.project_only && !self.project_dir.is_empty())
            .then(|| normalize_path(&self.project_dir));
        let mut candidates: Vec<usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                (self.show_hidden || !s.hidden)
                    // Nothing to resume: no prompt and no messages.
                    && !(s.first_prompt.trim().is_empty() && s.message_count == 0)
                    && cutoff.is_none_or(|c| modified_ms(s) >= c)
                    && project.as_ref().is_none_or(|p| {
                        let path = normalize_path(&s.project_path);
                        path == *p || path.starts_with(&format!("{p}/"))
                    })
            })
            .map(|(i, _)| i)
            .collect();
        candidates.sort_by_key(|i| std::cmp::Reverse(modified_ms(&self.sessions[*i])));

        let query = self.query.text();
        let query = query.trim();
        if !query.is_empty() {
            let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
            let mut matcher = Matcher::new(Config::DEFAULT);
            let mut buf = Vec::new();
            let mut scored: Vec<(u32, usize)> = candidates
                .iter()
                .filter_map(|i| {
                    let s = &self.sessions[*i];
                    let hay = format!(
                        "{} {}",
                        tildify(std::path::Path::new(&s.project_path)),
                        one_line(&s.first_prompt)
                    );
                    pattern
                        .score(Utf32Str::new(&hay, &mut buf), &mut matcher)
                        .map(|score| (score, *i))
                })
                .collect();
            // Best match first, newest first among equals (the sort is stable).
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            candidates = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.visible = candidates;
        self.cursor = keep
            .and_then(|id| {
                self.visible
                    .iter()
                    .position(|i| self.sessions[*i].session_id == id)
            })
            .unwrap_or(0)
            .min(self.visible.len().saturating_sub(1));
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let max = self.visible.len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, max) as usize;
    }

    /// Cycle the provider (standalone picker only: the dialog's provider field decides there).
    fn next_provider(&mut self) -> PickerAction {
        let next = match self.provider {
            ProviderId::Claude => ProviderId::Codex,
            _ => ProviderId::Claude,
        };
        self.provider = next;
        self.loading = true;
        self.sessions.clear();
        self.visible.clear();
        self.error = None;
        self.cursor = 0;
        PickerAction::Load {
            provider: next,
            force: false,
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        self.filtering = true;
        self.query.insert_str(text);
        self.refilter_keeping(None);
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> PickerAction {
        if key.kind == KeyEventKind::Release {
            return PickerAction::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.filtering {
            match key.code {
                KeyCode::Esc => {
                    self.query.clear();
                    self.filtering = false;
                    self.refilter_keeping(None);
                    return PickerAction::None;
                }
                KeyCode::Enter => self.filtering = false,
                KeyCode::Down => self.move_cursor(1),
                KeyCode::Up => self.move_cursor(-1),
                KeyCode::Char('n') if ctrl => self.move_cursor(1),
                KeyCode::Char('p') if ctrl => self.move_cursor(-1),
                _ => {
                    // A new query starts at the best match.
                    self.query.handle_key(key);
                    self.refilter_keeping(None);
                    return PickerAction::None;
                }
            }
            self.refilter();
            return PickerAction::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                if self.query.is_empty() {
                    return PickerAction::Cancel;
                }
                self.query.clear();
                self.refilter_keeping(None);
                return PickerAction::None;
            }
            KeyCode::Enter => {
                if let Some(s) = self.selected() {
                    return PickerAction::Pick(Box::new(s.clone()));
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::PageDown | KeyCode::Char('f') if ctrl || key.code == KeyCode::PageDown => {
                self.move_cursor(10);
            }
            KeyCode::PageUp | KeyCode::Char('b') if ctrl || key.code == KeyCode::PageUp => {
                self.move_cursor(-10);
            }
            KeyCode::Home | KeyCode::Char('g') => self.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => {
                self.cursor = self.visible.len().saturating_sub(1);
            }
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Char('a') => self.all_ages = !self.all_ages,
            KeyCode::Char('p') => {
                self.project_only = !self.project_only && !self.project_dir.is_empty()
            }
            KeyCode::Char('s') => self.show_hidden = !self.show_hidden,
            KeyCode::Char('h') => {
                if let Some(s) = self.selected() {
                    let (session_id, hidden) = (s.session_id.clone(), !s.hidden);
                    self.mark_hidden(&session_id, hidden);
                    return PickerAction::Hide { session_id, hidden };
                }
            }
            KeyCode::Char('r') => {
                self.loading = true;
                return PickerAction::Load {
                    provider: self.provider,
                    force: true,
                };
            }
            KeyCode::Tab if self.origin == PickerOrigin::Standalone => return self.next_provider(),
            _ => return PickerAction::None,
        }
        self.refilter();
        PickerAction::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-03-10T12:00:00Z";

    fn now_ms() -> i64 {
        DateTime::parse_from_rfc3339(NOW)
            .unwrap()
            .timestamp_millis()
    }

    pub(crate) fn session(id: &str, project: &str, prompt: &str, modified: &str) -> SessionSummary {
        SessionSummary {
            session_id: id.into(),
            project_path: project.into(),
            first_prompt: prompt.into(),
            modified_at: modified.into(),
            message_count: 3,
            ..Default::default()
        }
    }

    fn picker(max_age: u32) -> SessionPicker {
        let mut p = SessionPicker::new(
            PickerOrigin::Standalone,
            ProviderId::Claude,
            "/work/api",
            max_age,
        );
        p.now_ms = now_ms();
        p.set_sessions(
            ProviderId::Claude,
            Ok(vec![
                session(
                    "old",
                    "/work/api",
                    "ancient refactor",
                    "2026-01-01T00:00:00Z",
                ),
                session(
                    "new",
                    "/work/web",
                    "fix the login form",
                    "2026-03-10T11:00:00Z",
                ),
                session(
                    "mid",
                    "/work/api/sub",
                    "write the docs\nfor billing",
                    "2026-03-08T00:00:00Z",
                ),
                session("hid", "/work/api", "secret", "2026-03-09T00:00:00Z"),
                SessionSummary {
                    hidden: true,
                    ..session("hid2", "/work/api", "hidden prompt", "2026-03-09T01:00:00Z")
                },
                SessionSummary {
                    message_count: 0,
                    ..session("empty", "/work/api", "  ", "2026-03-10T11:30:00Z")
                },
            ]),
        );
        p
    }

    fn ids(p: &SessionPicker) -> Vec<&str> {
        p.visible().map(|s| s.session_id.as_str()).collect()
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn typ(p: &mut SessionPicker, text: &str) {
        for c in text.chars() {
            p.handle_key(&key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn newest_first_limited_by_age_without_hidden_or_empty_sessions() {
        let p = picker(7);
        assert_eq!(ids(&p), ["new", "hid", "mid"]);
        let p = picker(0);
        assert_eq!(ids(&p), ["new", "hid", "mid", "old"], "0 days = every age");
    }

    #[test]
    fn toggles_all_ages_project_and_hidden() {
        let mut p = picker(7);
        p.handle_key(&key(KeyCode::Char('a')));
        assert_eq!(ids(&p), ["new", "hid", "mid", "old"]);
        p.handle_key(&key(KeyCode::Char('p')));
        assert_eq!(
            ids(&p),
            ["hid", "mid", "old"],
            "this project and below, not /work/web"
        );
        p.handle_key(&key(KeyCode::Char('s')));
        assert_eq!(ids(&p), ["hid2", "hid", "mid", "old"]);
        p.handle_key(&key(KeyCode::Char('p')));
        p.handle_key(&key(KeyCode::Char('a')));
        assert_eq!(ids(&p), ["new", "hid2", "hid", "mid"]);
    }

    #[test]
    fn project_filter_needs_a_project() {
        let mut p = SessionPicker::new(PickerOrigin::Standalone, ProviderId::Claude, "", 7);
        p.handle_key(&key(KeyCode::Char('p')));
        assert!(!p.project_only);
    }

    #[test]
    fn h_hides_and_unhides_the_selected_session() {
        let mut p = picker(7);
        assert_eq!(
            p.handle_key(&key(KeyCode::Char('h'))),
            PickerAction::Hide {
                session_id: "new".into(),
                hidden: true
            }
        );
        assert_eq!(ids(&p), ["hid", "mid"]);
        p.handle_key(&key(KeyCode::Char('s')));
        assert_eq!(
            ids(&p),
            ["new", "hid2", "hid", "mid"],
            "hidden ones are shown, flagged"
        );
        p.handle_key(&key(KeyCode::Char('g')));
        assert_eq!(
            p.handle_key(&key(KeyCode::Char('h'))),
            PickerAction::Hide {
                session_id: "new".into(),
                hidden: false
            }
        );
        p.handle_key(&key(KeyCode::Char('s')));
        assert_eq!(ids(&p), ["new", "hid", "mid"]);
    }

    #[test]
    fn fuzzy_filter_over_project_and_prompt() {
        let mut p = picker(0);
        p.handle_key(&key(KeyCode::Char('/')));
        assert!(p.filtering);
        typ(&mut p, "lgn frm");
        assert_eq!(ids(&p), ["new"], "fuzzy: login form");
        // Project text matches too.
        p.handle_key(&key(KeyCode::Esc));
        assert!(!p.filtering && p.query.is_empty());
        p.handle_key(&key(KeyCode::Char('/')));
        typ(&mut p, "web");
        assert_eq!(
            ids(&p)[0],
            "new",
            "the project text is matched, best match first"
        );
        // Words of a multi-line prompt.
        p.query.set_text("docs billing");
        p.refilter();
        assert_eq!(ids(&p), ["mid"]);
        // Enter keeps the filter, Esc in list mode clears it first, then closes.
        p.handle_key(&key(KeyCode::Enter));
        assert!(!p.filtering && !p.query.is_empty());
        assert_eq!(p.handle_key(&key(KeyCode::Esc)), PickerAction::None);
        assert!(p.query.is_empty());
        assert_eq!(p.handle_key(&key(KeyCode::Esc)), PickerAction::Cancel);
    }

    #[test]
    fn a_new_query_starts_at_the_best_match() {
        let mut p = picker(0);
        p.handle_key(&key(KeyCode::Char('G')));
        assert_eq!(p.selected().unwrap().session_id, "old");
        p.handle_key(&key(KeyCode::Char('/')));
        typ(&mut p, "o");
        assert_eq!(p.cursor, 0, "not left where the previous selection was");
    }

    #[test]
    fn better_matches_rank_first() {
        let mut p = SessionPicker::new(PickerOrigin::Standalone, ProviderId::Claude, "", 0);
        p.now_ms = now_ms();
        p.set_sessions(
            ProviderId::Claude,
            Ok(vec![
                session(
                    "scattered",
                    "/x",
                    "a big refactor of the old tests",
                    "2026-03-10T10:00:00Z",
                ),
                session("exact", "/x", "deploy", "2026-03-01T10:00:00Z"),
            ]),
        );
        p.query.set_text("deploy");
        p.refilter();
        assert_eq!(ids(&p), ["exact"]);
        p.query.set_text("rtt");
        p.refilter();
        assert_eq!(ids(&p), ["scattered"]);
    }

    #[test]
    fn navigation_pick_and_cursor_follows_the_session() {
        let mut p = picker(7);
        assert_eq!(p.selected().unwrap().session_id, "new");
        p.handle_key(&key(KeyCode::Down));
        p.handle_key(&key(KeyCode::Char('j')));
        assert_eq!(p.selected().unwrap().session_id, "mid");
        p.handle_key(&key(KeyCode::Down)); // clamps
        assert_eq!(p.selected().unwrap().session_id, "mid");
        p.handle_key(&key(KeyCode::Char('g')));
        assert_eq!(p.selected().unwrap().session_id, "new");
        p.handle_key(&key(KeyCode::Char('G')));
        assert_eq!(p.cursor, 2);
        match p.handle_key(&key(KeyCode::Enter)) {
            PickerAction::Pick(s) => assert_eq!(s.session_id, "mid"),
            other => panic!("{other:?}"),
        }
        // Filtering keeps the cursor on the same session when it survives.
        p.handle_key(&key(KeyCode::Char('a')));
        assert_eq!(p.selected().unwrap().session_id, "mid");
    }

    #[test]
    fn provider_switch_reload_and_stale_answers() {
        let mut p = picker(7);
        assert_eq!(
            p.handle_key(&key(KeyCode::Tab)),
            PickerAction::Load {
                provider: ProviderId::Codex,
                force: false
            }
        );
        assert!(p.loading && p.visible_len() == 0);
        // The answer for the old provider arrives late: ignored.
        p.set_sessions(
            ProviderId::Claude,
            Ok(vec![session("x", "/a", "p", "2026-03-10T00:00:00Z")]),
        );
        assert!(p.loading);
        p.set_sessions(ProviderId::Codex, Err("boom".into()));
        assert_eq!(p.error.as_deref(), Some("boom"));
        assert_eq!(
            p.handle_key(&key(KeyCode::Char('r'))),
            PickerAction::Load {
                provider: ProviderId::Codex,
                force: true
            }
        );
        // From the dialog the provider is the dialog's.
        let mut f = SessionPicker::new(PickerOrigin::Form, ProviderId::Claude, "", 7);
        assert_eq!(f.handle_key(&key(KeyCode::Tab)), PickerAction::None);
    }

    #[test]
    fn age_labels() {
        let now = 1_000_000_000_000;
        let ago = |secs: i64| format_age(now - secs * 1000, now);
        assert_eq!(ago(5), "now");
        assert_eq!(ago(300), "5m");
        assert_eq!(ago(3 * 3600), "3h");
        assert_eq!(ago(2 * 86_400), "2d");
        assert_eq!(ago(21 * 86_400), "3w");
        assert_eq!(ago(100 * 86_400), "3mo");
        assert_eq!(ago(800 * 86_400), "2y");
        assert_eq!(format_age(now + 5000, now), "now");
    }

    #[test]
    fn normalizes_paths_and_text() {
        assert_eq!(
            normalize_path("C:\\Users\\jp\\"),
            if cfg!(windows) {
                "c:/users/jp"
            } else {
                "C:/Users/jp"
            }
        );
        assert_eq!(one_line("a\n\n  b\tc "), "a b c");
    }
}
