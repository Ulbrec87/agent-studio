//! Codex session catalog (Hydra `electron/sessions/CodexSessionCatalog.ts`).
//!
//! `~/.codex/sessions/<yyyy>/<mm>/<dd>/rollout-*.jsonl`: the first `session_meta` record carries the
//! id, cwd, start time and git branch; the first user message is the prompt. Files are parsed at
//! most once per (mtime, size) and a scan is reused for [`SESSION_CACHE_TTL_MS`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;
use serde_json::{Map, Value};

use super::{
    ListSessionsOptions, SESSION_CACHE_TTL_MS, SessionSummary, file_times, filter_and_sort,
    for_each_json_line,
};
use crate::paths::home_dir;
use crate::types::ProviderId;

/// Codex reads at most this much of a rollout to find its header.
const FILE_READ_LIMIT: usize = 256 * 1024;

/// `$CODEX_HOME/sessions` or `~/.codex/sessions`.
pub fn default_sessions_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home_dir().join(".codex"), PathBuf::from)
        .join("sessions")
}

fn str_of<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The user prompt carried by a record (`event_msg`/`user_message` or a `response_item` user message).
fn extract_user_prompt(record: &Map<String, Value>) -> String {
    let Some(payload) = record.get("payload").and_then(Value::as_object) else {
        return String::new();
    };
    match str_of(record, "type") {
        "event_msg" if str_of(payload, "type") == "user_message" => {
            str_of(payload, "message").trim().to_string()
        }
        "response_item"
            if str_of(payload, "type") == "message" && str_of(payload, "role") == "user" =>
        {
            payload
                .get("content")
                .and_then(Value::as_array)
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(Value::as_object)
                        .filter(|b| str_of(b, "type") == "input_text")
                        .map(|b| str_of(b, "text").trim())
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                        .trim()
                        .to_string()
                })
                .unwrap_or_default()
        }
        _ => String::new(),
    }
}

#[derive(Debug, Clone, Default)]
struct Header {
    session_id: String,
    project_path: String,
    created_at: String,
    first_prompt: String,
    git_branch: Option<String>,
}

fn read_header(path: &Path) -> Option<Header> {
    let mut h = Header::default();
    for_each_json_line(path, FILE_READ_LIMIT, |record| {
        if str_of(record, "type") == "session_meta"
            && let Some(meta) = record.get("payload").and_then(Value::as_object)
        {
            if h.session_id.is_empty() {
                h.session_id = str_of(meta, "id").to_string();
            }
            if h.project_path.is_empty() {
                h.project_path = str_of(meta, "cwd").to_string();
            }
            if h.created_at.is_empty() {
                h.created_at = str_of(meta, "timestamp").to_string();
            }
            if h.git_branch.is_none()
                && let Some(b) = meta
                    .get("git")
                    .and_then(|g| g.get("branch"))
                    .and_then(Value::as_str)
            {
                h.git_branch = Some(b.to_string());
            }
        }
        if h.first_prompt.is_empty() {
            h.first_prompt = extract_user_prompt(record);
        }
        !(!h.session_id.is_empty() && !h.project_path.is_empty() && !h.first_prompt.is_empty())
    })
    .ok()?;
    (!h.session_id.is_empty() && !h.project_path.is_empty()).then_some(h)
}

struct Cached {
    mtime: SystemTime,
    size: u64,
    summary: Option<SessionSummary>,
}

#[derive(Default)]
struct State {
    snapshot: Option<(Instant, Arc<Vec<SessionSummary>>)>,
    files: HashMap<PathBuf, Cached>,
}

pub struct CodexSessionCatalog {
    dir: Option<PathBuf>,
    ttl: Duration,
    state: Mutex<State>,
}

impl CodexSessionCatalog {
    /// `~/.codex/sessions` (or `$CODEX_HOME/sessions`), resolved at every scan.
    pub fn discover() -> Self {
        Self {
            dir: None,
            ttl: Duration::from_millis(SESSION_CACHE_TTL_MS),
            state: Mutex::default(),
        }
    }

    /// A fixed sessions dir with a custom snapshot TTL in ms (`0` = never reuse a snapshot).
    pub fn with_dir(dir: PathBuf, ttl_ms: u64) -> Self {
        Self {
            dir: Some(dir),
            ttl: Duration::from_millis(ttl_ms),
            state: Mutex::default(),
        }
    }

    fn sessions_dir(&self) -> PathBuf {
        self.dir.clone().unwrap_or_else(default_sessions_dir)
    }

    /// Sessions matching `options` (its `provider` is ignored), newest first.
    pub fn list_sessions(
        &self,
        options: &ListSessionsOptions,
        hidden: &HashSet<String>,
    ) -> Vec<SessionSummary> {
        let snapshot = self.snapshot(options.force_refresh);
        filter_and_sort(&snapshot, options, hidden)
    }

    fn snapshot(&self, force: bool) -> Arc<Vec<SessionSummary>> {
        let mut state = self.state.lock();
        if !force
            && !self.ttl.is_zero()
            && let Some((at, sessions)) = &state.snapshot
            && at.elapsed() < self.ttl
        {
            return sessions.clone();
        }
        let sessions = Arc::new(self.scan(&mut state));
        state.snapshot = (!self.ttl.is_zero()).then(|| (Instant::now(), sessions.clone()));
        sessions
    }

    fn scan(&self, state: &mut State) -> Vec<SessionSummary> {
        let mut out = Vec::new();
        let mut live = HashSet::new();
        for path in collect_session_files(&self.sessions_dir()) {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.len() == 0 {
                continue;
            }
            live.insert(path.clone());
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let fresh = state
                .files
                .get(&path)
                .is_some_and(|c| c.mtime == mtime && c.size == meta.len());
            if !fresh {
                let summary = read_header(&path).map(|h| {
                    let (birth, modified_at) = file_times(&meta);
                    SessionSummary {
                        session_id: h.session_id,
                        provider: ProviderId::Codex,
                        project_path: h.project_path,
                        first_prompt: h.first_prompt,
                        message_count: 0,
                        created_at: if h.created_at.is_empty() {
                            birth
                        } else {
                            h.created_at
                        },
                        modified_at,
                        git_branch: h.git_branch,
                        is_sidechain: false,
                        source_path: path.to_string_lossy().into_owned(),
                        hidden: false,
                    }
                });
                state.files.insert(
                    path.clone(),
                    Cached {
                        mtime,
                        size: meta.len(),
                        summary,
                    },
                );
            }
            if let Some(s) = state.files.get(&path).and_then(|c| c.summary.clone()) {
                out.push(s);
            }
        }
        state.files.retain(|p, _| live.contains(p));
        out
    }
}

fn collect_session_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in read.filter_map(Result::ok) {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(t) if t.is_file() && path.extension().is_some_and(|e| e == "jsonl") => {
                    files.push(path);
                }
                _ => {}
            }
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::testutil::{jsonl, write};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn all() -> ListSessionsOptions {
        ListSessionsOptions {
            provider: ProviderId::Codex,
            ..Default::default()
        }
    }

    #[test]
    fn reads_codex_session_files_and_filters_by_project_prefix() {
        let root = tempfile::tempdir().unwrap();
        let day = root.path().join("2026/03/08");
        write(
            &day.join("rollout-a.jsonl"),
            &jsonl(&[
                json!({"timestamp":"2026-03-08T10:00:00.000Z","type":"session_meta","payload":{
                    "id":"codex-session-a","timestamp":"2026-03-08T10:00:00.000Z",
                    "cwd":"/Users/jp/Documents/Personal/GitHub Projects/hydra","git":{"branch":"main"}}}),
                json!({"timestamp":"2026-03-08T10:00:02.000Z","type":"event_msg","payload":{
                    "type":"user_message","message":"Hydra Codex, session restarts is not working correctly."}}),
            ]),
        );
        write(
            &day.join("rollout-b.jsonl"),
            &jsonl(&[
                json!({"timestamp":"2026-03-08T09:00:00.000Z","type":"session_meta","payload":{
                    "id":"codex-session-b","timestamp":"2026-03-08T09:00:00.000Z",
                    "cwd":"/Users/jp/Documents/Other Project","git":{"branch":"feature/other"}}}),
                json!({"timestamp":"2026-03-08T09:00:02.000Z","type":"event_msg","payload":{
                    "type":"user_message","message":"Other project prompt"}}),
            ]),
        );
        let catalog = CodexSessionCatalog::with_dir(root.path().to_path_buf(), 0);
        let sessions = catalog.list_sessions(
            &ListSessionsOptions {
                project_path_prefix: Some(
                    "/Users/jp/Documents/Personal/GitHub Projects/hydra".into(),
                ),
                ..all()
            },
            &HashSet::new(),
        );
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.session_id, "codex-session-a");
        assert_eq!(
            s.project_path,
            "/Users/jp/Documents/Personal/GitHub Projects/hydra"
        );
        assert_eq!(
            s.first_prompt,
            "Hydra Codex, session restarts is not working correctly."
        );
        assert_eq!(s.git_branch.as_deref(), Some("main"));
        assert_eq!(s.created_at, "2026-03-08T10:00:00.000Z");
        assert_eq!(s.provider, ProviderId::Codex);
        assert_eq!(catalog.list_sessions(&all(), &HashSet::new()).len(), 2);
    }

    #[test]
    fn extracts_prompt_text_from_response_item_user_messages() {
        let root = tempfile::tempdir().unwrap();
        write(
            &root.path().join("2026/03/08/rollout-c.jsonl"),
            &jsonl(&[
                json!({"timestamp":"2026-03-08T12:00:00.000Z","type":"session_meta","payload":{
                    "id":"codex-session-c","timestamp":"2026-03-08T12:00:00.000Z","cwd":"/tmp/project"}}),
                json!({"timestamp":"2026-03-08T12:00:05.000Z","type":"response_item","payload":{
                    "type":"message","role":"user","content":[{"type":"input_text","text":"Resume the previous Codex thread."}]}}),
            ]),
        );
        let sessions = CodexSessionCatalog::with_dir(root.path().to_path_buf(), 0)
            .list_sessions(&all(), &HashSet::new());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "codex-session-c");
        assert_eq!(
            sessions[0].first_prompt,
            "Resume the previous Codex thread."
        );
    }

    #[test]
    fn skips_files_without_meta_and_survives_garbage() {
        let root = tempfile::tempdir().unwrap();
        write(
            &root.path().join("a/rollout-x.jsonl"),
            "not json\n{\"type\":\"other\"}\n",
        );
        write(&root.path().join("a/empty.jsonl"), "");
        write(&root.path().join("a/notes.txt"), "hello");
        let catalog = CodexSessionCatalog::with_dir(root.path().to_path_buf(), 0);
        assert!(catalog.list_sessions(&all(), &HashSet::new()).is_empty());
        let missing = CodexSessionCatalog::with_dir(root.path().join("nope"), 0);
        assert!(missing.list_sessions(&all(), &HashSet::new()).is_empty());
    }
}
