//! Claude Code session catalog (Hydra `electron/sessions/SessionCatalog.ts`).
//!
//! Scans `<projectsDir>/<slug>/` of every profile: `sessions-index.json` when present (stale
//! entries whose transcript is gone are ignored), then every `*.jsonl` the index did not cover,
//! reading only the head of each transcript. Parsed files are cached by (mtime, size), and the
//! whole scan result for [`SESSION_CACHE_TTL_MS`], so listing again is cheap.

use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;

use super::{
    COUNT_MAX_BYTES, HEADER_MAX_BYTES, ListSessionsOptions, SESSION_CACHE_TTL_MS, SessionSummary,
    extract_text_from_content, file_times, filter_and_sort, for_each_json_line,
    is_ignorable_prompt_text, iso,
};
use crate::types::ProviderId;

const INDEX_FILENAME: &str = "sessions-index.json";

/// What the first lines of a Claude transcript tell about the session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JsonlHeader {
    pub session_id: String,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub is_sidechain: bool,
    pub first_prompt: String,
}

/// Read the session id, cwd, git branch, sidechain flag and first real user prompt from the head
/// of a transcript (at most [`HEADER_MAX_BYTES`]). `None` without a session id and a cwd.
pub fn read_claude_header(path: &Path) -> Option<JsonlHeader> {
    let mut h = JsonlHeader::default();
    for_each_json_line(path, HEADER_MAX_BYTES, |o| {
        let text = |key: &str| o.get(key).and_then(Value::as_str);
        if h.session_id.is_empty()
            && let Some(v) = text("sessionId")
        {
            h.session_id = v.to_string();
        }
        if h.cwd.is_empty()
            && let Some(v) = text("cwd")
        {
            h.cwd = v.to_string();
        }
        if h.git_branch.is_none()
            && let Some(v) = text("gitBranch")
        {
            h.git_branch = Some(v.to_string());
        }
        if let Some(v) = o.get("isSidechain").and_then(Value::as_bool) {
            h.is_sidechain = v;
        }
        if h.first_prompt.is_empty()
            && o.get("isMeta").and_then(Value::as_bool) != Some(true)
            && let Some(message) = o.get("message").and_then(Value::as_object)
            && message.get("role").and_then(Value::as_str) == Some("user")
        {
            let prompt = extract_text_from_content(message.get("content").unwrap_or(&Value::Null));
            if !is_ignorable_prompt_text(&prompt) {
                h.first_prompt = prompt;
            }
        }
        !(!h.session_id.is_empty() && !h.cwd.is_empty() && !h.first_prompt.is_empty())
    })
    .ok()?;
    (!h.session_id.is_empty() && !h.cwd.is_empty()).then_some(h)
}

/// Number of user/assistant records in a transcript (a lower bound for transcripts bigger than
/// [`COUNT_MAX_BYTES`]).
pub fn count_claude_messages(path: &Path) -> u32 {
    let Ok(file) = std::fs::File::open(path) else {
        return 0;
    };
    let mut reader = std::io::BufReader::new(std::io::Read::take(file, COUNT_MAX_BYTES));
    let mut line = Vec::new();
    let mut count = 0u32;
    while reader.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
        let text = String::from_utf8_lossy(&line);
        if (text.contains("\"type\":\"user\"") || text.contains("\"type\":\"assistant\""))
            && !text.contains("\"isMeta\":true")
        {
            count = count.saturating_add(1);
        }
        line.clear();
    }
    count
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct IndexFile {
    entries: Vec<IndexEntry>,
    #[serde(rename = "originalPath")]
    original_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct IndexEntry {
    session_id: String,
    full_path: String,
    file_mtime: Option<f64>,
    first_prompt: Option<String>,
    message_count: Option<f64>,
    created: Option<String>,
    modified: Option<String>,
    git_branch: Option<String>,
    project_path: Option<String>,
    is_sidechain: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    mtime: SystemTime,
    size: u64,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp {
        mtime: meta.modified().ok()?,
        size: meta.len(),
    })
}

struct CachedHeader {
    stamp: Stamp,
    header: Option<JsonlHeader>,
    count: Option<u32>,
}

struct CachedIndex {
    stamp: Stamp,
    index: Option<Arc<IndexFile>>,
}

#[derive(Default)]
struct State {
    snapshot: Option<(Instant, Arc<Vec<SessionSummary>>)>,
    headers: HashMap<PathBuf, CachedHeader>,
    indexes: HashMap<PathBuf, CachedIndex>,
}

enum Dirs {
    Fixed(Vec<PathBuf>),
    /// Profiles are rediscovered on every scan so new ones show up.
    Discover,
}

pub struct ClaudeSessionCatalog {
    dirs: Dirs,
    ttl: Duration,
    state: Mutex<State>,
}

impl ClaudeSessionCatalog {
    /// Every profile of this machine (`~/.claude`, `$CLAUDE_CONFIG_DIR`, `~/.claude-*`).
    pub fn discover() -> Self {
        Self::build(Dirs::Discover, SESSION_CACHE_TTL_MS)
    }

    /// Only these `projects` dirs, with a custom snapshot TTL in ms (`0` = never reuse a snapshot).
    pub fn with_dirs(dirs: Vec<PathBuf>, ttl_ms: u64) -> Self {
        Self::build(Dirs::Fixed(dirs), ttl_ms)
    }

    fn build(dirs: Dirs, ttl_ms: u64) -> Self {
        Self {
            dirs,
            ttl: Duration::from_millis(ttl_ms),
            state: Mutex::default(),
        }
    }

    fn projects_dirs(&self) -> Vec<PathBuf> {
        match &self.dirs {
            Dirs::Fixed(d) => d.clone(),
            Dirs::Discover => super::profiles::discover_projects_dirs_from_env(),
        }
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

    pub fn invalidate_cache(&self) {
        self.state.lock().snapshot = None;
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
        let mut found: Vec<SessionSummary> = Vec::new();
        let mut live: HashSet<PathBuf> = HashSet::new();
        for projects_dir in self.projects_dirs() {
            let Ok(read) = std::fs::read_dir(&projects_dir) else {
                continue;
            };
            let mut project_dirs: Vec<PathBuf> = read
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect();
            project_dirs.sort();
            for project_dir in project_dirs {
                read_project(state, &project_dir, &mut found, &mut live);
            }
        }
        // Forget files that vanished.
        state.headers.retain(|p, _| live.contains(p));
        state.indexes.retain(|p, _| live.contains(p));
        dedupe_by_session_id(found)
    }

    /// Path of the transcript of `session_id` in any profile (a direct lookup, no scan).
    pub fn find_transcript(&self, session_id: &str) -> Option<PathBuf> {
        self.projects_dirs()
            .iter()
            .find_map(|d| super::transcript::find_claude_transcript_file(session_id, d))
    }
}

/// The same session can appear twice (`<id>.orphaned-<ts>.jsonl` copies): keep the newest.
fn dedupe_by_session_id(sessions: Vec<SessionSummary>) -> Vec<SessionSummary> {
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<SessionSummary> = Vec::with_capacity(sessions.len());
    for s in sessions {
        match by_id.get(&s.session_id) {
            Some(&i) => {
                if s.modified_at > out[i].modified_at {
                    out[i] = s;
                }
            }
            None => {
                by_id.insert(s.session_id.clone(), out.len());
                out.push(s);
            }
        }
    }
    out
}

fn header_of(
    state: &mut State,
    path: &Path,
    want_count: bool,
    live: &mut HashSet<PathBuf>,
) -> Option<(Option<JsonlHeader>, u32, Stamp)> {
    let stamp = stamp_of(path)?;
    live.insert(path.to_path_buf());
    let fresh = state
        .headers
        .get(path)
        .is_some_and(|c| c.stamp == stamp && (!want_count || c.count.is_some()));
    if !fresh {
        let previous = state.headers.remove(path).filter(|c| c.stamp == stamp);
        let header = match previous {
            Some(c) => c.header,
            None => read_claude_header(path),
        };
        let count = (want_count && header.is_some()).then(|| count_claude_messages(path));
        state.headers.insert(
            path.to_path_buf(),
            CachedHeader {
                stamp,
                header,
                count,
            },
        );
    }
    let cached = state.headers.get(path)?;
    Some((cached.header.clone(), cached.count.unwrap_or(0), stamp))
}

fn read_index(
    state: &mut State,
    path: &Path,
    live: &mut HashSet<PathBuf>,
) -> Option<Arc<IndexFile>> {
    let stamp = stamp_of(path)?;
    live.insert(path.to_path_buf());
    if let Some(c) = state.indexes.get(path)
        && c.stamp == stamp
    {
        return c.index.clone();
    }
    let index =
        std::fs::read(path)
            .ok()
            .and_then(|raw| match serde_json::from_slice::<IndexFile>(&raw) {
                Ok(i) => Some(Arc::new(i)),
                Err(err) => {
                    tracing::warn!(index = %path.display(), %err, "failed to read session index");
                    None
                }
            });
    state.indexes.insert(
        path.to_path_buf(),
        CachedIndex {
            stamp,
            index: index.clone(),
        },
    );
    index
}

fn ms_to_iso(ms: f64) -> Option<String> {
    (ms.is_finite() && ms >= 0.0)
        .then(|| iso(SystemTime::UNIX_EPOCH + Duration::from_millis(ms as u64)))
}

fn read_project(
    state: &mut State,
    project_dir: &Path,
    out: &mut Vec<SessionSummary>,
    live: &mut HashSet<PathBuf>,
) {
    let mut seen: HashSet<String> = HashSet::new();

    let index_path = project_dir.join(INDEX_FILENAME);
    if index_path.is_file()
        && let Some(index) = read_index(state, &index_path, live)
    {
        for entry in &index.entries {
            if entry.session_id.is_empty() || entry.full_path.is_empty() {
                continue;
            }
            let full_path = PathBuf::from(&entry.full_path);
            let Some((header, _, stamp)) = header_of(state, &full_path, false, live) else {
                continue; // stale entry: the transcript is gone
            };
            let session_id = header
                .as_ref()
                .map(|h| h.session_id.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| entry.session_id.clone());
            seen.insert(entry.session_id.clone());
            let modified_at = entry
                .modified
                .clone()
                .or_else(|| entry.file_mtime.and_then(ms_to_iso))
                .unwrap_or_else(|| iso(stamp.mtime));
            let created_at = entry.created.clone().unwrap_or_else(|| modified_at.clone());
            let stored = entry.first_prompt.as_deref().unwrap_or_default().trim();
            let first_prompt = if !stored.is_empty() && !is_ignorable_prompt_text(stored) {
                stored.to_string()
            } else {
                header
                    .as_ref()
                    .map(|h| h.first_prompt.trim().to_string())
                    .unwrap_or_default()
            };
            let project_path = entry
                .project_path
                .clone()
                .or_else(|| header.as_ref().map(|h| h.cwd.clone()))
                .or_else(|| index.original_path.clone())
                .unwrap_or_default();
            out.push(SessionSummary {
                session_id,
                provider: ProviderId::Claude,
                project_path: project_path.trim().to_string(),
                first_prompt,
                message_count: entry
                    .message_count
                    .map_or(0, |n| n.max(0.0).min(f64::from(u32::MAX)) as u32),
                created_at,
                modified_at,
                git_branch: entry
                    .git_branch
                    .clone()
                    .or_else(|| header.as_ref().and_then(|h| h.git_branch.clone())),
                is_sidechain: entry
                    .is_sidechain
                    .unwrap_or_else(|| header.as_ref().is_some_and(|h| h.is_sidechain)),
                source_path: entry.full_path.clone(),
                hidden: false,
            });
        }
    }

    // Project folders without an index (or with entries missing from it).
    let Ok(read) = std::fs::read_dir(project_dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = read
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    for file in files {
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if seen.contains(&stem) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&file) else {
            continue;
        };
        if meta.len() == 0 {
            continue;
        }
        let Some((Some(header), count, _)) = header_of(state, &file, true, live) else {
            continue;
        };
        seen.insert(header.session_id.clone());
        let (created_at, modified_at) = file_times(&meta);
        out.push(SessionSummary {
            session_id: header.session_id,
            provider: ProviderId::Claude,
            project_path: header.cwd,
            first_prompt: header.first_prompt,
            message_count: count,
            created_at,
            modified_at,
            git_branch: header.git_branch,
            is_sidechain: header.is_sidechain,
            source_path: file.to_string_lossy().into_owned(),
            hidden: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::testutil::{jsonl, write};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn ids(sessions: &[SessionSummary]) -> Vec<&str> {
        sessions.iter().map(|s| s.session_id.as_str()).collect()
    }

    fn catalog(root: &Path) -> ClaudeSessionCatalog {
        ClaudeSessionCatalog::with_dirs(vec![root.to_path_buf()], 0)
    }

    fn all() -> ListSessionsOptions {
        ListSessionsOptions::default()
    }

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn reads_index_entries_and_applies_hidden_and_prefix_filtering() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project-a");
        write(&project.join("session-new.jsonl"), "");
        write(&project.join("session-old.jsonl"), "");
        write(
            &project.join("sessions-index.json"),
            &json!({
                "originalPath": "/Users/jp/workspace",
                "entries": [
                    {"sessionId":"session-new","fullPath":project.join("session-new.jsonl"),
                     "projectPath":"/Users/jp/workspace/ep_inventory","firstPrompt":"Newest","modified":"2026-01-05T10:00:00.000Z"},
                    {"sessionId":"session-old","fullPath":project.join("session-old.jsonl"),
                     "projectPath":"/Users/jp/workspace/drug_module","firstPrompt":"Older","modified":"2026-01-02T10:00:00.000Z"},
                ]
            })
            .to_string(),
        );
        let c = catalog(root.path());
        assert_eq!(
            ids(&c.list_sessions(&all(), &none())),
            ["session-new", "session-old"]
        );
        let hidden: HashSet<String> = ["session-old".to_string()].into();
        let filtered = c.list_sessions(
            &ListSessionsOptions {
                project_path_prefix: Some("/Users/jp/workspace/ep_".into()),
                ..all()
            },
            &hidden,
        );
        assert_eq!(ids(&filtered), ["session-new"]);

        let with_hidden = c.list_sessions(
            &ListSessionsOptions {
                include_hidden: true,
                ..all()
            },
            &hidden,
        );
        assert_eq!(ids(&with_hidden), ["session-new", "session-old"]);
        assert!(with_hidden[1].hidden && !with_hidden[0].hidden);
    }

    #[test]
    fn falls_back_to_jsonl_headers_when_the_index_is_missing() {
        let root = tempfile::tempdir().unwrap();
        let sid = "fallback-session";
        write(
            &root.path().join("project-b").join(format!("{sid}.jsonl")),
            &jsonl(&[
                json!({"sessionId":sid,"cwd":"/Users/jp/workspace/ep_inventory","gitBranch":"feature/grid","isSidechain":false}),
                json!({"type":"user","message":{"role":"user","content":"Bring in a delete button"}}),
                json!({"type":"assistant","message":{"role":"assistant","content":"ok"}}),
            ]),
        );
        let sessions = catalog(root.path()).list_sessions(
            &ListSessionsOptions {
                limit: Some(1),
                ..all()
            },
            &none(),
        );
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.session_id, sid);
        assert_eq!(s.project_path, "/Users/jp/workspace/ep_inventory");
        assert_eq!(s.git_branch.as_deref(), Some("feature/grid"));
        assert_eq!(s.first_prompt, "Bring in a delete button");
        assert_eq!(s.message_count, 2);
        assert_eq!(s.provider, ProviderId::Claude);
    }

    #[test]
    fn extracts_the_first_meaningful_prompt_from_structured_content() {
        let root = tempfile::tempdir().unwrap();
        let sid = "structured-session";
        write(
            &root
                .path()
                .join("project-structured")
                .join(format!("{sid}.jsonl")),
            &jsonl(&[
                json!({"sessionId":sid,"cwd":"/Users/jp/workspace/hydra","gitBranch":"feature/session-fix","isSidechain":false}),
                json!({"type":"user","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>ignore me</local-command-caveat>"}}),
                json!({"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>\n<command-message>clear</command-message>"}}),
                json!({"type":"user","message":{"role":"user","content":[
                    {"type":"text","text":"Resume the invoice workflow and fix the restart mapping."},
                    {"type":"image","source":{"type":"base64","media_type":"image/png","data":"abc"}}]}}),
            ]),
        );
        let sessions = catalog(root.path()).list_sessions(&all(), &none());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, sid);
        assert_eq!(
            sessions[0].first_prompt,
            "Resume the invoice workflow and fix the restart mapping."
        );
    }

    #[test]
    fn ignores_stale_index_entries_and_falls_back_to_the_jsonl() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project-stale-index");
        let sid = "stale-session";
        write(
            &project.join("sessions-index.json"),
            &json!({"originalPath":"/Users/jp/workspace/stale","entries":[{
                "sessionId":sid,"fullPath":"/tmp/does-not-exist/stale-session.jsonl",
                "projectPath":"/Users/jp/workspace/stale","firstPrompt":"Wrong stale prompt",
                "modified":"2026-01-15T10:00:00.000Z"}]})
            .to_string(),
        );
        write(
            &project.join(format!("{sid}.jsonl")),
            &jsonl(&[
                json!({"sessionId":sid,"cwd":"/Users/jp/workspace/stale/live","gitBranch":"main","isSidechain":false}),
                json!({"type":"user","message":{"role":"user","content":"Live prompt from the transcript file"}}),
            ]),
        );
        let sessions = catalog(root.path()).list_sessions(&all(), &none());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].project_path, "/Users/jp/workspace/stale/live");
        assert_eq!(
            sessions[0].first_prompt,
            "Live prompt from the transcript file"
        );
    }

    #[test]
    fn refreshes_meta_index_prompts_from_the_transcript_header() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project-index-refresh");
        let sid = "indexed-session";
        let full = project.join(format!("{sid}.jsonl"));
        write(
            &full,
            &jsonl(&[
                json!({"sessionId":sid,"cwd":"/Users/jp/workspace/index-refresh","gitBranch":"feature/resume","isSidechain":false}),
                json!({"type":"user","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>ignore me</local-command-caveat>"}}),
                json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"Continue the real work from this resumed session."}]}}),
            ]),
        );
        write(
            &project.join("sessions-index.json"),
            &json!({"originalPath":"/Users/jp/workspace/index-refresh","entries":[{
                "sessionId":sid,"fullPath":full,"projectPath":"/Users/jp/workspace/index-refresh",
                "firstPrompt":"<command-name>/clear</command-name>","modified":"2026-01-16T10:00:00.000Z"}]})
            .to_string(),
        );
        let sessions = catalog(root.path()).list_sessions(&all(), &none());
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].first_prompt,
            "Continue the real work from this resumed session."
        );
        assert_eq!(sessions[0].git_branch.as_deref(), Some("feature/resume"));
    }

    fn index_with(project: &Path, names: &[(&str, &str, &str)]) -> String {
        let entries: Vec<_> = names
            .iter()
            .map(|(id, prompt, modified)| {
                json!({"sessionId":id,"fullPath":project.join(format!("{id}.jsonl")),
                       "projectPath":"/Users/jp/workspace/cache","firstPrompt":prompt,"modified":modified})
            })
            .collect();
        json!({"originalPath":"/Users/jp/workspace/cache","entries":entries}).to_string()
    }

    #[test]
    fn caches_scans_and_refreshes_via_force_or_invalidation() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project-cache");
        for id in ["cache-a", "cache-b", "cache-c"] {
            write(&project.join(format!("{id}.jsonl")), "");
        }
        let index = project.join("sessions-index.json");
        write(
            &index,
            &index_with(
                &project,
                &[("cache-a", "Alpha", "2026-01-10T10:00:00.000Z")],
            ),
        );

        let c = ClaudeSessionCatalog::with_dirs(vec![root.path().to_path_buf()], 60_000);
        assert_eq!(ids(&c.list_sessions(&all(), &none())), ["cache-a"]);

        write(
            &index,
            &index_with(
                &project,
                &[
                    ("cache-a", "Alpha", "2026-01-10T10:00:00.000Z"),
                    ("cache-b", "Beta", "2026-01-11T10:00:00.000Z"),
                ],
            ),
        );
        assert_eq!(
            ids(&c.list_sessions(&all(), &none())),
            ["cache-a"],
            "still cached"
        );
        let forced = c.list_sessions(
            &ListSessionsOptions {
                force_refresh: true,
                ..all()
            },
            &none(),
        );
        assert_eq!(ids(&forced), ["cache-b", "cache-a"]);

        write(
            &index,
            &index_with(
                &project,
                &[
                    ("cache-a", "Alpha", "2026-01-10T10:00:00.000Z"),
                    ("cache-b", "Beta", "2026-01-11T10:00:00.000Z"),
                    ("cache-c", "Gamma", "2026-01-12T10:00:00.000Z"),
                ],
            ),
        );
        c.invalidate_cache();
        assert_eq!(
            ids(&c.list_sessions(&all(), &none())),
            ["cache-c", "cache-b", "cache-a"]
        );
    }

    #[test]
    fn unchanged_files_are_not_parsed_again() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("p/s1.jsonl");
        let line = |prompt: &str| {
            jsonl(&[
                json!({"sessionId":"s1","cwd":"/w"}),
                json!({"type":"user","message":{"role":"user","content":prompt}}),
            ])
        };
        write(&file, &line("first"));
        let c = catalog(root.path());
        assert_eq!(c.list_sessions(&all(), &none())[0].first_prompt, "first");
        // Same size and mtime: the cached header is used even though the content differs.
        let stamp = std::fs::metadata(&file).unwrap().modified().unwrap();
        write(&file, &line("other"));
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(stamp)
            .unwrap();
        assert_eq!(c.list_sessions(&all(), &none())[0].first_prompt, "first");
        // A different size invalidates the entry.
        write(&file, &line("a longer prompt than before"));
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(stamp)
            .unwrap();
        assert_eq!(
            c.list_sessions(&all(), &none())[0].first_prompt,
            "a longer prompt than before"
        );
    }

    #[test]
    fn matches_project_prefixes_across_slash_styles() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project-prefix");
        write(&project.join("session-win-path.jsonl"), "");
        write(
            &project.join("sessions-index.json"),
            &json!({"originalPath":"C:\\Users\\jp\\workspace","entries":[{
                "sessionId":"session-win-path","fullPath":project.join("session-win-path.jsonl"),
                "projectPath":"C:\\Users\\jp\\workspace\\ep_inventory","firstPrompt":"Windows path",
                "modified":"2026-01-20T10:00:00.000Z"}]})
            .to_string(),
        );
        let filtered = catalog(root.path()).list_sessions(
            &ListSessionsOptions {
                project_path_prefix: Some("C:/Users/jp/workspace/ep_".into()),
                ..all()
            },
            &none(),
        );
        assert_eq!(ids(&filtered), ["session-win-path"]);
    }

    #[test]
    fn lists_sessions_from_every_projects_dir() {
        let default_root = tempfile::tempdir().unwrap();
        let work_root = tempfile::tempdir().unwrap();
        for (root, sid, cwd) in [
            (&default_root, "session-default", "/home/me/personal"),
            (&work_root, "session-work", "/var/www/test"),
        ] {
            write(
                &root.path().join("project").join(format!("{sid}.jsonl")),
                &(json!({"type":"user","sessionId":sid,"cwd":cwd,"message":{"role":"user","content":"hi"}})
                    .to_string()
                    + "\n"),
            );
        }
        let c = ClaudeSessionCatalog::with_dirs(
            vec![
                default_root.path().to_path_buf(),
                work_root.path().to_path_buf(),
            ],
            0,
        );
        let mut got: Vec<_> = c
            .list_sessions(&all(), &none())
            .into_iter()
            .map(|s| s.session_id)
            .collect();
        got.sort();
        assert_eq!(got, ["session-default", "session-work"]);
        let only_work = c.list_sessions(
            &ListSessionsOptions {
                project_path_prefix: Some("/var/www/test".into()),
                ..all()
            },
            &none(),
        );
        assert_eq!(ids(&only_work), ["session-work"]);
        assert!(c.find_transcript("session-work").is_some());
    }

    #[test]
    fn filters_by_age_and_query_and_skips_corrupt_lines_and_orphans() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("p");
        let content = |sid: &str, prompt: &str| {
            format!(
                "{{broken\n{}\n",
                jsonl(&[
                    json!({"sessionId":sid,"cwd":"/work/api"}),
                    json!({"type":"user","message":{"role":"user","content":prompt}}),
                ])
            )
        };
        write(&project.join("a.jsonl"), &content("a", "fix the login bug"));
        write(&project.join("b.jsonl"), &content("b", "write docs"));
        // A copy of session a (as Claude leaves behind): listed once.
        write(
            &project.join("a.orphaned-1.jsonl"),
            &content("a", "fix the login bug"),
        );
        // An old one.
        write(&project.join("old.jsonl"), &content("old", "ancient"));
        let old = std::time::SystemTime::now() - Duration::from_secs(40 * 86_400);
        std::fs::File::options()
            .write(true)
            .open(project.join("old.jsonl"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let c = catalog(root.path());
        assert_eq!(c.list_sessions(&all(), &none()).len(), 3);
        let recent = c.list_sessions(
            &ListSessionsOptions {
                max_age_days: Some(7),
                ..all()
            },
            &none(),
        );
        let mut got = ids(&recent);
        got.sort_unstable();
        assert_eq!(got, ["a", "b"]);
        let q = c.list_sessions(
            &ListSessionsOptions {
                query: Some("LOGIN api".into()),
                ..all()
            },
            &none(),
        );
        assert_eq!(ids(&q), ["a"]);
    }

    #[test]
    fn a_huge_first_line_does_not_break_the_scan() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("p/big.jsonl");
        let huge = "x".repeat(HEADER_MAX_BYTES * 2);
        write(
            &file,
            &format!(
                "{}\n{}\n",
                json!({"sessionId":"big","cwd":"/w","pad":huge}),
                json!({"type":"user","message":{"role":"user","content":"p"}})
            ),
        );
        // The head is cut inside the first line: no header, session skipped (not an error).
        assert!(
            catalog(root.path())
                .list_sessions(&all(), &none())
                .is_empty()
        );
    }
}
