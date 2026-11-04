// Modelled on agent-lookout (MIT, © 2026 Idris Olubisi): src/collector/files/transcriptTail.ts
//! Reading the end of a Claude transcript: what a waiting session asks (cached per agent), and
//! the details drawer data (on demand).

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::sessions::profiles::discover_projects_dirs_from_env;
use crate::sessions::transcript::find_claude_transcript_file;
use crate::types::ProviderId;

use super::details::details_from_tail;
use super::types::AgentDetails;
use super::waiting_text::last_ask_from_tail;

/// How much of the end of a transcript is read.
pub const MAX_TAIL_BYTES: u64 = 256 * 1024;

/// The `projects` dirs to search; the default asks every Claude profile on this machine.
pub type ProjectsDirs = Box<dyn Fn() -> Vec<PathBuf> + Send>;

pub fn default_projects_dirs() -> ProjectsDirs {
    Box::new(discover_projects_dirs_from_env)
}

/// Finds the transcript of a session: `known` if it still exists, then the agent's own profile,
/// then every profile.
pub fn locate_claude_transcript(
    session_id: &str,
    config_dir: Option<&Path>,
    projects_dirs: &dyn Fn() -> Vec<PathBuf>,
    known: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(known) = known
        && std::fs::symlink_metadata(known).is_ok_and(|m| m.is_file())
    {
        return Some(known.to_path_buf());
    }
    let mut dirs: Vec<PathBuf> = config_dir.map(|c| c.join("projects")).into_iter().collect();
    for dir in projects_dirs() {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs.iter()
        .find_map(|dir| find_claude_transcript_file(session_id, dir))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tail {
    pub text: String,
    pub whole_file: bool,
}

/// Reads the last 256 KB of a regular file (never a symlink). `None` when it is not one or
/// cannot be read.
pub fn read_transcript_tail(path: &Path) -> Option<Tail> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let length = size.min(MAX_TAIL_BYTES);
    file.seek(SeekFrom::Start(size - length)).ok()?;
    let mut buffer = Vec::with_capacity(usize::try_from(length).unwrap_or(0));
    file.take(length).read_to_end(&mut buffer).ok()?;
    Some(Tail {
        // The cut can fall inside a UTF-8 sequence of the (dropped) first line.
        text: String::from_utf8_lossy(&buffer).into_owned(),
        whole_file: size <= MAX_TAIL_BYTES,
    })
}

pub struct WaitingTextAgent<'a> {
    pub agent_id: &'a str,
    pub session_id: &'a str,
    pub project_dir: &'a str,
    /// Claude config dir of the matched profile; its `projects/` is searched first.
    pub config_dir: Option<&'a Path>,
}

struct CacheEntry {
    key: String,
    session_id: String,
    path: PathBuf,
    text: Option<String>,
}

/// Works out what a waiting Claude session asks from the last 256 KB of its transcript. Reads
/// regular files only, and caches per agent until the file changes.
pub struct WaitingTextReader {
    cache: HashMap<String, CacheEntry>,
    projects_dirs: ProjectsDirs,
}

impl Default for WaitingTextReader {
    fn default() -> Self {
        Self::new(default_projects_dirs())
    }
}

fn file_key(path: &Path, meta: &std::fs::Metadata) -> String {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(meta);
    #[cfg(not(unix))]
    let ino = 0u64;
    format!("{}|{}|{mtime}|{ino}", path.display(), meta.len())
}

impl WaitingTextReader {
    pub fn new(projects_dirs: ProjectsDirs) -> Self {
        Self {
            cache: HashMap::new(),
            projects_dirs,
        }
    }

    pub fn read(&mut self, agent: &WaitingTextAgent) -> Option<String> {
        let cached = self.cache.get(agent.agent_id);
        let known = cached
            .filter(|c| c.session_id == agent.session_id)
            .map(|c| c.path.clone());
        let Some(path) = locate_claude_transcript(
            agent.session_id,
            agent.config_dir,
            self.projects_dirs.as_ref(),
            known.as_deref(),
        ) else {
            self.cache.remove(agent.agent_id);
            return None;
        };
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let key = file_key(&path, &meta);
        if let Some(c) = self.cache.get(agent.agent_id)
            && c.key == key
        {
            return c.text.clone();
        }
        let tail = read_transcript_tail(&path)?;
        let text = last_ask_from_tail(&tail.text, tail.whole_file, agent.project_dir);
        self.cache.insert(
            agent.agent_id.to_string(),
            CacheEntry {
                key,
                session_id: agent.session_id.to_string(),
                path,
                text: text.clone(),
            },
        );
        text
    }

    pub fn forget(&mut self, agent_id: &str) {
        self.cache.remove(agent_id);
    }

    /// Drops the cache of every agent not in `agent_ids`.
    pub fn retain<'a>(&mut self, agent_ids: impl IntoIterator<Item = &'a str>) {
        let keep: std::collections::HashSet<&str> = agent_ids.into_iter().collect();
        self.cache.retain(|id, _| keep.contains(id.as_str()));
    }
}

/// What the details drawer needs to find an agent's transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailsLocator {
    pub provider: ProviderId,
    pub session_id: Option<String>,
    pub project_dir: String,
    pub config_dir: Option<PathBuf>,
}

/// Reads the details drawer data (last message, tokens of the newest reply) from the last 256 KB
/// of a Claude transcript. On demand only, never in polls.
pub struct AgentDetailsReader {
    known: HashMap<String, PathBuf>,
    projects_dirs: ProjectsDirs,
}

impl Default for AgentDetailsReader {
    fn default() -> Self {
        Self::new(default_projects_dirs())
    }
}

impl AgentDetailsReader {
    pub fn new(projects_dirs: ProjectsDirs) -> Self {
        Self {
            known: HashMap::new(),
            projects_dirs,
        }
    }

    pub fn read(&mut self, locator: &DetailsLocator) -> AgentDetails {
        if locator.provider != ProviderId::Claude {
            return AgentDetails::unavailable("not supported for this provider");
        }
        let Some(session_id) = locator.session_id.as_deref() else {
            return AgentDetails::unavailable("The session has no transcript yet");
        };
        let Some(path) = locate_claude_transcript(
            session_id,
            locator.config_dir.as_deref(),
            self.projects_dirs.as_ref(),
            self.known.get(session_id).map(PathBuf::as_path),
        ) else {
            return AgentDetails::unavailable("The transcript was not found");
        };
        self.known.insert(session_id.to_string(), path.clone());
        match read_transcript_tail(&path) {
            Some(tail) => details_from_tail(&tail.text, tail.whole_file),
            None => AgentDetails::unavailable("The transcript could not be read"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overview::types::{LastMessage, TokenCounts};
    use serde_json::json;
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn bash(cmd: &str, id: &str) -> String {
        json!({"type":"assistant","message":{"id":format!("m-{id}"),"content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":cmd}}]}}).to_string()
    }
    fn filler() -> String {
        json!({"type":"attachment","pad":"x".repeat(1000)}).to_string()
    }
    fn set_mtime(path: &Path, at: SystemTime) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(at).unwrap();
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        projects: PathBuf,
        file: PathBuf,
    }
    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let projects = root.join("projects");
        fs::create_dir_all(projects.join("p")).unwrap();
        let file = projects.join("p/s1.jsonl");
        Fixture {
            _tmp: tmp,
            root,
            projects,
            file,
        }
    }
    fn reader(f: &Fixture) -> WaitingTextReader {
        let dirs = vec![f.projects.clone()];
        WaitingTextReader::new(Box::new(move || dirs.clone()))
    }
    fn agent<'a>(session: &'a str, config: Option<&'a Path>) -> WaitingTextAgent<'a> {
        WaitingTextAgent {
            agent_id: "ag",
            session_id: session,
            project_dir: "/work/proj",
            config_dir: config,
        }
    }

    #[test]
    fn reads_the_pending_ask_and_caches_while_the_file_is_unchanged() {
        let f = fixture();
        let mut r = reader(&f);
        fs::write(&f.file, bash("npm test", "a") + "\n").unwrap();
        assert_eq!(r.read(&agent("s1", None)).as_deref(), Some("Run: npm test"));
        // Same size, other mtime: re-read.
        let stamp = SystemTime::now() - Duration::from_secs(5);
        fs::write(&f.file, bash("npm bust", "a") + "\n").unwrap();
        set_mtime(&f.file, stamp);
        assert_eq!(r.read(&agent("s1", None)).as_deref(), Some("Run: npm bust"));
        // Same size and mtime, other content: the cache answers.
        fs::write(&f.file, bash("npm tesx", "a") + "\n").unwrap();
        set_mtime(&f.file, stamp);
        assert_eq!(r.read(&agent("s1", None)).as_deref(), Some("Run: npm bust"));
    }

    #[test]
    fn rereads_when_the_file_changes_and_forgets() {
        let f = fixture();
        let mut r = reader(&f);
        fs::write(&f.file, bash("ls", "a") + "\n").unwrap();
        assert_eq!(r.read(&agent("s1", None)).as_deref(), Some("Run: ls"));
        let mut text = fs::read_to_string(&f.file).unwrap();
        text.push_str(&(bash("pwd", "b") + "\n"));
        fs::write(&f.file, text).unwrap();
        assert_eq!(r.read(&agent("s1", None)).as_deref(), Some("Run: pwd"));
        r.forget("ag");
        let stamp = SystemTime::now() - Duration::from_secs(5);
        fs::write(&f.file, bash("whoami", "a") + "\n").unwrap();
        set_mtime(&f.file, stamp);
        assert_eq!(r.read(&agent("s1", None)).as_deref(), Some("Run: whoami"));
    }

    #[test]
    fn searches_the_agent_profile_first_and_returns_none_when_missing() {
        let f = fixture();
        let mut r = reader(&f);
        let cfg = f.root.join("cfg");
        fs::create_dir_all(cfg.join("projects/q")).unwrap();
        fs::write(
            cfg.join("projects/q/s2.jsonl"),
            bash("from cfg", "a") + "\n",
        )
        .unwrap();
        assert_eq!(
            r.read(&agent("s2", Some(&cfg))).as_deref(),
            Some("Run: from cfg")
        );
        assert_eq!(r.read(&agent("nope", None)), None);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_transcript() {
        let f = fixture();
        let mut r = reader(&f);
        let real = f.root.join("real.jsonl");
        fs::write(&real, bash("ls", "a") + "\n").unwrap();
        std::os::unix::fs::symlink(&real, &f.file).unwrap();
        assert_eq!(r.read(&agent("s1", None)), None);
    }

    #[test]
    fn reads_only_the_tail_of_a_large_file() {
        let f = fixture();
        let mut r = reader(&f);
        let mut lines = vec![
            bash("old", "old"),
            json!({"type":"user","message":{"content":"go"}}).to_string(),
        ];
        let mut size = 0;
        while size < MAX_TAIL_BYTES * 2 {
            let line = filler();
            size += line.len() as u64 + 1;
            lines.push(line);
        }
        lines.push(bash("latest", "new"));
        fs::write(&f.file, lines.join("\n") + "\n").unwrap();
        assert_eq!(r.read(&agent("s1", None)).as_deref(), Some("Run: latest"));
    }

    #[test]
    fn retain_drops_other_agents_from_the_cache() {
        let f = fixture();
        let mut r = reader(&f);
        let stamp = SystemTime::now() - Duration::from_secs(5);
        fs::write(&f.file, bash("ls", "a") + "\n").unwrap();
        set_mtime(&f.file, stamp);
        r.read(&agent("s1", None));
        r.retain([]);
        fs::write(&f.file, bash("ps", "a") + "\n").unwrap();
        set_mtime(&f.file, stamp);
        assert_eq!(r.read(&agent("s1", None)).as_deref(), Some("Run: ps"));
    }

    fn details_reader(f: &Fixture) -> AgentDetailsReader {
        let dirs = vec![f.projects.clone()];
        AgentDetailsReader::new(Box::new(move || dirs.clone()))
    }
    fn locator(provider: ProviderId, session: Option<&str>) -> DetailsLocator {
        DetailsLocator {
            provider,
            session_id: session.map(str::to_string),
            project_dir: "/work/alpha".into(),
            config_dir: None,
        }
    }

    #[test]
    fn details_read_the_last_message_and_tokens_of_a_claude_session() {
        let f = fixture();
        let line = json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"All done."}],"usage":{"input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5}}});
        fs::write(&f.file, line.to_string() + "\n").unwrap();
        let d = details_reader(&f).read(&locator(ProviderId::Claude, Some("s1")));
        assert_eq!(
            d.last_message,
            LastMessage::Text {
                text: "All done.".into(),
                truncated: false
            }
        );
        assert_eq!(
            d.tokens,
            Some(TokenCounts {
                input: 10,
                cached: Some(7),
                output: 5
            })
        );
    }

    #[test]
    fn details_say_why_nothing_is_available() {
        let f = fixture();
        let mut r = details_reader(&f);
        let unavailable = |d: AgentDetails| match d.last_message {
            LastMessage::Unavailable { unavailable } => unavailable,
            LastMessage::Text { .. } => panic!("expected unavailable"),
        };
        assert_eq!(
            unavailable(r.read(&locator(ProviderId::Claude, Some("missing")))),
            "The transcript was not found"
        );
        assert_eq!(r.read(&locator(ProviderId::Claude, None)).tokens, None);
        assert_eq!(
            unavailable(r.read(&locator(ProviderId::Codex, Some("s")))),
            "not supported for this provider"
        );
    }
}
