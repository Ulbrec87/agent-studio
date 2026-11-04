// Modelled on agent-lookout (MIT, © 2026 Idris Olubisi): src/collector/adapters/claude-code/registry.ts
//! Claude Code's per-process registry, `<configDir>/sessions/<pid>.json` (undocumented: every
//! field is optional, parsing never fails the scan), and matching of registry entries to agents.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::types::ClaudeRegistryStatusFields;

/// Registry files are tiny; anything bigger is not one.
pub const MAX_REGISTRY_FILE_BYTES: u64 = 64 * 1024;
/// Tier 3 tolerance between the agent's start and the registry's `startedAt`.
pub const START_MATCH_WINDOW_MS: i64 = 10_000;

/// One live Claude Code process, read from `<configDir>/sessions/<pid>.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryEntry {
    pub file: PathBuf,
    pub config_dir: PathBuf,
    pub pid: u32,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    /// Epoch ms.
    pub started_at: Option<i64>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub updated_at: Option<i64>,
    pub status_updated_at: Option<i64>,
    pub fields: ClaudeRegistryStatusFields,
}

#[derive(Debug, Clone, Copy)]
pub struct FileStat {
    pub is_file: bool,
    pub is_symlink: bool,
    pub size: u64,
}

/// What the reader needs from the file system (tests inject fakes).
pub trait RegistryFs: Send {
    /// File names in a directory; `Err` when missing.
    fn read_dir(&self, dir: &Path) -> std::io::Result<Vec<String>>;
    /// Stat without following symlinks.
    fn lstat(&self, path: &Path) -> std::io::Result<FileStat>;
    fn read_file(&self, path: &Path) -> std::io::Result<String>;
}

pub struct StdRegistryFs;

impl RegistryFs for StdRegistryFs {
    fn read_dir(&self, dir: &Path) -> std::io::Result<Vec<String>> {
        Ok(std::fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect())
    }
    fn lstat(&self, path: &Path) -> std::io::Result<FileStat> {
        let m = std::fs::symlink_metadata(path)?;
        Ok(FileStat {
            is_file: m.is_file(),
            is_symlink: m.file_type().is_symlink(),
            size: m.len(),
        })
    }
    fn read_file(&self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
}

/// Whether a process exists. Default: asks the OS (`sysinfo`).
pub fn default_is_pid_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let Ok(mut system) = System::new() else {
        return true; // cannot tell: the registry file itself is the evidence
    };
    let target = [Pid::from_u32(pid)];
    system.refresh_processes(ProcessesToUpdate::Some(&target), true);
    system.process(target[0]).is_some()
}

fn str_of(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn ms_of(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f as i64)),
        _ => None,
    }
}

/// Keeps only the known fields; `None` when there is no usable pid.
fn to_entry(raw: &Value, file: &Path, config_dir: &Path, file_stem: u32) -> Option<RegistryEntry> {
    let record = raw.as_object()?;
    let pid = ms_of(record.get("pid"))
        .and_then(|p| u32::try_from(p).ok())
        .unwrap_or(file_stem);
    if pid == 0 {
        return None;
    }
    Some(RegistryEntry {
        file: file.to_path_buf(),
        config_dir: config_dir.to_path_buf(),
        pid,
        session_id: str_of(record.get("sessionId")),
        cwd: str_of(record.get("cwd")),
        started_at: ms_of(record.get("startedAt")),
        kind: str_of(record.get("kind")),
        name: str_of(record.get("name")),
        updated_at: ms_of(record.get("updatedAt")),
        status_updated_at: ms_of(record.get("statusUpdatedAt")),
        fields: ClaudeRegistryStatusFields {
            status: str_of(record.get("status")),
            state: str_of(record.get("state")),
            waiting_for: str_of(record.get("waitingFor")),
        },
    })
}

/// `<digits>.json`.
fn registry_file_stem(name: &str) -> Option<u32> {
    let stem = name.strip_suffix(".json")?;
    if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    stem.parse().ok()
}

/// Reads Claude Code's per-process registry. The format is undocumented, so every read is
/// defensive and never fails. `.key` files and symlinks are never opened.
pub struct ClaudeRegistryReader {
    fs: Box<dyn RegistryFs>,
    is_pid_alive: Box<dyn Fn(u32) -> bool + Send>,
    /// Last good parse per file, reused once when the next read fails (half-written file).
    last_good: HashMap<PathBuf, RegistryEntry>,
    reused: HashSet<PathBuf>,
}

impl Default for ClaudeRegistryReader {
    fn default() -> Self {
        Self::new(Box::new(StdRegistryFs), Box::new(default_is_pid_alive))
    }
}

impl ClaudeRegistryReader {
    pub fn new(fs: Box<dyn RegistryFs>, is_pid_alive: Box<dyn Fn(u32) -> bool + Send>) -> Self {
        Self {
            fs,
            is_pid_alive,
            last_good: HashMap::new(),
            reused: HashSet::new(),
        }
    }

    /// Live registry entries across the given Claude config dirs.
    pub fn read(&mut self, config_dirs: &[PathBuf]) -> Vec<RegistryEntry> {
        let mut entries = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for config_dir in config_dirs {
            let sessions = config_dir.join("sessions");
            let Ok(names) = self.fs.read_dir(&sessions) else {
                continue;
            };
            for name in names {
                let Some(stem) = registry_file_stem(&name) else {
                    continue;
                };
                let file = sessions.join(&name);
                seen.insert(file.clone());
                if let Some(entry) = self.read_one(&file, config_dir, stem)
                    && (self.is_pid_alive)(entry.pid)
                {
                    entries.push(entry);
                }
            }
        }
        self.last_good.retain(|file, _| seen.contains(file));
        self.reused.retain(|file| seen.contains(file));
        entries
    }

    fn read_one(&mut self, file: &Path, config_dir: &Path, stem: u32) -> Option<RegistryEntry> {
        let stat = match self.fs.lstat(file) {
            Ok(s) => s,
            Err(_) => return self.fallback(file),
        };
        if stat.is_symlink || !stat.is_file || stat.size > MAX_REGISTRY_FILE_BYTES {
            self.forget(file);
            return None;
        }
        let parsed = self
            .fs
            .read_file(file)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|raw| to_entry(&raw, file, config_dir, stem));
        match parsed {
            Some(entry) => {
                self.last_good.insert(file.to_path_buf(), entry.clone());
                self.reused.remove(file);
                Some(entry)
            }
            None => self.fallback(file),
        }
    }

    /// Reuses the last good parse once; a second consecutive failure drops the entry.
    fn fallback(&mut self, file: &Path) -> Option<RegistryEntry> {
        if let Some(previous) = self.last_good.get(file)
            && !self.reused.contains(file)
        {
            let previous = previous.clone();
            self.reused.insert(file.to_path_buf());
            return Some(previous);
        }
        self.forget(file);
        None
    }

    fn forget(&mut self, file: &Path) {
        self.last_good.remove(file);
        self.reused.remove(file);
    }
}

/// The agent fields the matcher needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchableAgent {
    pub id: String,
    pub session_id: Option<String>,
    /// The PTY child (possibly the login shell wrapping `claude`).
    pub pid: Option<u32>,
    pub project_dir: String,
    /// Epoch ms.
    pub started_at: Option<i64>,
}

fn normalize_dir(dir: &str) -> &str {
    if dir.len() > 1 {
        dir.trim_end_matches(['/', '\\'])
    } else {
        dir
    }
}

fn starts_near(agent: &MatchableAgent, entry: &RegistryEntry) -> bool {
    match (agent.started_at, entry.started_at) {
        (Some(a), Some(e)) if a > 0 => (a - e).abs() <= START_MATCH_WINDOW_MS,
        _ => false,
    }
}

fn cwd_and_start_candidates<'a>(
    agent: &MatchableAgent,
    entries: &[&'a RegistryEntry],
) -> Vec<&'a RegistryEntry> {
    let dir = normalize_dir(&agent.project_dir);
    entries
        .iter()
        .copied()
        .filter(|e| {
            e.cwd.as_deref().is_some_and(|c| normalize_dir(c) == dir) && starts_near(agent, e)
        })
        .collect()
}

/// Matches every agent, tier by tier across all of them, so an entry is never assigned twice:
/// (1) same `session_id`, (2) the entry's pid is the agent's pid or one of its descendants
/// (the agent pid may be the login shell wrapping claude), (3) same cwd and start time, when
/// unambiguous (an entry that several agents could claim is skipped).
///
/// Returns agent id -> index into `entries`.
pub fn match_registry_entries(
    agents: &[MatchableAgent],
    entries: &[RegistryEntry],
    descendants_by_agent: &HashMap<String, HashSet<u32>>,
) -> HashMap<String, usize> {
    let mut result: HashMap<String, usize> = HashMap::new();
    let mut taken: HashSet<usize> = HashSet::new();

    for agent in agents {
        let Some(sid) = agent.session_id.as_deref() else {
            continue;
        };
        if let Some(i) = entries
            .iter()
            .enumerate()
            .position(|(i, e)| !taken.contains(&i) && e.session_id.as_deref() == Some(sid))
        {
            result.insert(agent.id.clone(), i);
            taken.insert(i);
        }
    }

    for agent in agents {
        let Some(pid) = agent.pid else { continue };
        if result.contains_key(&agent.id) {
            continue;
        }
        let descendants = descendants_by_agent.get(&agent.id);
        let found = entries
            .iter()
            .enumerate()
            .find(|(i, e)| !taken.contains(i) && e.pid == pid)
            .or_else(|| {
                entries.iter().enumerate().find(|(i, e)| {
                    !taken.contains(i) && descendants.is_some_and(|d| d.contains(&e.pid))
                })
            })
            .map(|(i, _)| i);
        if let Some(i) = found {
            result.insert(agent.id.clone(), i);
            taken.insert(i);
        }
    }

    let free: Vec<&RegistryEntry> = entries
        .iter()
        .enumerate()
        .filter(|(i, _)| !taken.contains(i))
        .map(|(_, e)| e)
        .collect();
    let mut candidates_by_agent: Vec<(&str, Vec<&RegistryEntry>)> = Vec::new();
    let mut claims: HashMap<*const RegistryEntry, usize> = HashMap::new();
    for agent in agents {
        if result.contains_key(&agent.id) {
            continue;
        }
        let candidates = cwd_and_start_candidates(agent, &free);
        for e in &candidates {
            *claims.entry(std::ptr::from_ref(*e)).or_default() += 1;
        }
        candidates_by_agent.push((agent.id.as_str(), candidates));
    }
    for (id, candidates) in candidates_by_agent {
        if let [only] = candidates.as_slice()
            && claims.get(&std::ptr::from_ref(*only)) == Some(&1)
            && let Some(i) = entries.iter().position(|e| std::ptr::eq(e, *only))
        {
            result.insert(id.to_string(), i);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    /// In-memory file system: path -> (stat, content or read error).
    type FakeFile = (FileStat, Option<String>);

    #[derive(Default, Clone)]
    struct FakeFs {
        files: Arc<Mutex<BTreeMap<PathBuf, FakeFile>>>,
        dirs: Arc<Mutex<BTreeMap<PathBuf, Vec<String>>>>,
    }
    impl FakeFs {
        fn put(&self, dir: &str, name: &str, content: &str) {
            let path = PathBuf::from(dir).join(name);
            self.dirs
                .lock()
                .unwrap()
                .entry(PathBuf::from(dir))
                .or_default()
                .push(name.to_string());
            self.files.lock().unwrap().insert(
                path,
                (
                    FileStat {
                        is_file: true,
                        is_symlink: false,
                        size: content.len() as u64,
                    },
                    Some(content.to_string()),
                ),
            );
        }
        fn put_stat(&self, dir: &str, name: &str, stat: FileStat) {
            let path = PathBuf::from(dir).join(name);
            self.dirs
                .lock()
                .unwrap()
                .entry(PathBuf::from(dir))
                .or_default()
                .push(name.to_string());
            self.files
                .lock()
                .unwrap()
                .insert(path, (stat, Some("{\"pid\":1}".into())));
        }
        fn set_content(&self, dir: &str, name: &str, content: &str) {
            let path = PathBuf::from(dir).join(name);
            if let Some(entry) = self.files.lock().unwrap().get_mut(&path) {
                entry.1 = Some(content.to_string());
            }
        }
    }
    impl RegistryFs for FakeFs {
        fn read_dir(&self, dir: &Path) -> std::io::Result<Vec<String>> {
            self.dirs
                .lock()
                .unwrap()
                .get(dir)
                .cloned()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        fn lstat(&self, path: &Path) -> std::io::Result<FileStat> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .map(|f| f.0)
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        fn read_file(&self, path: &Path) -> std::io::Result<String> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .and_then(|f| f.1.clone())
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        }
    }

    fn reader(fs: &FakeFs, dead: &'static [u32]) -> ClaudeRegistryReader {
        ClaudeRegistryReader::new(
            Box::new(fs.clone()),
            Box::new(move |pid| !dead.contains(&pid)),
        )
    }
    fn dirs(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn reads_a_good_file_and_keeps_only_known_fields() {
        let fs = FakeFs::default();
        fs.put(
            "/c/sessions",
            "100.json",
            r#"{"pid":100,"sessionId":"s1","cwd":"/work/a","startedAt":1700000000000,"kind":"interactive","status":"waiting","waitingFor":"permission prompt","extra":{"x":1}}"#,
        );
        let entries = reader(&fs, &[]).read(&dirs(&["/c"]));
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(
            (e.pid, e.session_id.as_deref(), e.cwd.as_deref()),
            (100, Some("s1"), Some("/work/a"))
        );
        assert_eq!(e.started_at, Some(1_700_000_000_000));
        assert_eq!(e.fields.status.as_deref(), Some("waiting"));
        assert_eq!(e.fields.waiting_for.as_deref(), Some("permission prompt"));
        assert_eq!(e.config_dir, PathBuf::from("/c"));
    }

    #[test]
    fn falls_back_to_the_file_name_for_the_pid_and_ignores_other_names() {
        let fs = FakeFs::default();
        fs.put("/c/sessions", "200.json", r#"{"status":"busy"}"#);
        fs.put("/c/sessions", "notes.json", r#"{"pid":5}"#);
        fs.put("/c/sessions", "300.json.key", r#"{"pid":6}"#);
        fs.put("/c/sessions", "7.txt", "{}");
        let entries = reader(&fs, &[]).read(&dirs(&["/c"]));
        assert_eq!(entries.iter().map(|e| e.pid).collect::<Vec<_>>(), [200]);
    }

    #[test]
    fn skips_symlinks_big_files_and_dead_pids() {
        let fs = FakeFs::default();
        fs.put_stat(
            "/c/sessions",
            "1.json",
            FileStat {
                is_file: true,
                is_symlink: true,
                size: 10,
            },
        );
        fs.put_stat(
            "/c/sessions",
            "2.json",
            FileStat {
                is_file: true,
                is_symlink: false,
                size: MAX_REGISTRY_FILE_BYTES + 1,
            },
        );
        fs.put_stat(
            "/c/sessions",
            "3.json",
            FileStat {
                is_file: false,
                is_symlink: false,
                size: 10,
            },
        );
        fs.put("/c/sessions", "4.json", r#"{"pid":4}"#);
        fs.put("/c/sessions", "5.json", r#"{"pid":5}"#);
        let entries = reader(&fs, &[5]).read(&dirs(&["/c"]));
        assert_eq!(entries.iter().map(|e| e.pid).collect::<Vec<_>>(), [4]);
    }

    #[test]
    fn reads_every_profile_and_skips_a_missing_one() {
        let fs = FakeFs::default();
        fs.put("/a/sessions", "10.json", r#"{"pid":10}"#);
        fs.put("/b/sessions", "20.json", r#"{"pid":20}"#);
        let entries = reader(&fs, &[]).read(&dirs(&["/a", "/missing", "/b"]));
        assert_eq!(entries.iter().map(|e| e.pid).collect::<Vec<_>>(), [10, 20]);
        assert_eq!(entries[1].config_dir, PathBuf::from("/b"));
    }

    #[test]
    fn a_half_written_file_reuses_the_last_good_parse_once() {
        let fs = FakeFs::default();
        fs.put("/c/sessions", "9.json", r#"{"pid":9,"status":"busy"}"#);
        let mut r = reader(&fs, &[]);
        assert_eq!(r.read(&dirs(&["/c"])).len(), 1);
        fs.set_content("/c/sessions", "9.json", r#"{"pid":9,"sta"#);
        let again = r.read(&dirs(&["/c"]));
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].fields.status.as_deref(), Some("busy"));
        // A second failure drops it.
        assert!(r.read(&dirs(&["/c"])).is_empty());
        // A good write brings it back.
        fs.set_content("/c/sessions", "9.json", r#"{"pid":9,"status":"idle"}"#);
        assert_eq!(
            r.read(&dirs(&["/c"]))[0].fields.status.as_deref(),
            Some("idle")
        );
    }

    #[test]
    fn malformed_json_without_history_is_dropped() {
        let fs = FakeFs::default();
        fs.put("/c/sessions", "9.json", "not json");
        fs.put("/c/sessions", "10.json", "[1,2]");
        assert!(reader(&fs, &[]).read(&dirs(&["/c"])).is_empty());
    }

    #[test]
    fn real_file_system_reader_reads_a_temp_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let pid = std::process::id();
        std::fs::write(
            sessions.join(format!("{pid}.json")),
            format!("{{\"pid\":{pid},\"status\":\"idle\",\"cwd\":\"/x\"}}"),
        )
        .unwrap();
        std::fs::write(sessions.join("999999999.json"), "{\"pid\":999999999}").unwrap();
        let entries = ClaudeRegistryReader::default().read(&[tmp.path().to_path_buf()]);
        assert_eq!(entries.len(), 1, "own pid is alive, the made-up one is not");
        assert_eq!(entries[0].pid, pid);
    }

    // ── matching ───────────────────────────────────────────────────────────────

    fn entry(
        pid: u32,
        session: Option<&str>,
        cwd: Option<&str>,
        started: Option<i64>,
    ) -> RegistryEntry {
        RegistryEntry {
            file: PathBuf::from(format!("/c/sessions/{pid}.json")),
            config_dir: PathBuf::from("/c"),
            pid,
            session_id: session.map(str::to_string),
            cwd: cwd.map(str::to_string),
            started_at: started,
            kind: None,
            name: None,
            updated_at: None,
            status_updated_at: None,
            fields: ClaudeRegistryStatusFields::default(),
        }
    }
    fn agent(
        id: &str,
        session: Option<&str>,
        pid: Option<u32>,
        dir: &str,
        started: Option<i64>,
    ) -> MatchableAgent {
        MatchableAgent {
            id: id.into(),
            session_id: session.map(str::to_string),
            pid,
            project_dir: dir.into(),
            started_at: started,
        }
    }
    fn no_desc() -> HashMap<String, HashSet<u32>> {
        HashMap::new()
    }

    #[test]
    fn tier_1_matches_by_session_id() {
        let entries = [
            entry(1, Some("other"), None, None),
            entry(2, Some("s"), None, None),
        ];
        let m = match_registry_entries(
            &[agent("a", Some("s"), Some(1), "/x", None)],
            &entries,
            &no_desc(),
        );
        assert_eq!(m["a"], 1);
    }

    #[test]
    fn tier_2_matches_the_pid_or_a_descendant_of_the_shell_wrapper() {
        let entries = [entry(50, None, None, None), entry(60, None, None, None)];
        let m = match_registry_entries(
            &[agent("a", None, Some(50), "/x", None)],
            &entries,
            &no_desc(),
        );
        assert_eq!(m["a"], 0);
        // The agent pid is the login shell (40); claude (60) is its child.
        let mut desc = HashMap::new();
        desc.insert("b".to_string(), HashSet::from([60u32]));
        let m = match_registry_entries(&[agent("b", None, Some(40), "/x", None)], &entries, &desc);
        assert_eq!(m["b"], 1);
        // Without the process tree there is no match.
        assert!(
            match_registry_entries(
                &[agent("b", None, Some(40), "/x", None)],
                &entries,
                &no_desc()
            )
            .is_empty()
        );
    }

    #[test]
    fn tier_3_matches_cwd_and_start_when_unambiguous() {
        let t = 1_700_000_000_000;
        let entries = [entry(1, None, Some("/work/a/"), Some(t + 3000))];
        let m = match_registry_entries(
            &[agent("a", None, None, "/work/a", Some(t))],
            &entries,
            &no_desc(),
        );
        assert_eq!(m["a"], 0);
        // too far apart
        let far = [entry(1, None, Some("/work/a"), Some(t + 60_000))];
        assert!(
            match_registry_entries(
                &[agent("a", None, None, "/work/a", Some(t))],
                &far,
                &no_desc()
            )
            .is_empty()
        );
    }

    #[test]
    fn tier_3_skips_an_entry_two_agents_could_claim_and_ambiguous_agents() {
        let t = 1_700_000_000_000;
        let one = [entry(1, None, Some("/work/a"), Some(t))];
        let agents = [
            agent("a", None, None, "/work/a", Some(t)),
            agent("b", None, None, "/work/a", Some(t + 1000)),
        ];
        assert!(match_registry_entries(&agents, &one, &no_desc()).is_empty());
        let two = [
            entry(1, None, Some("/work/a"), Some(t)),
            entry(2, None, Some("/work/a"), Some(t + 500)),
        ];
        assert!(
            match_registry_entries(
                &[agent("a", None, None, "/work/a", Some(t))],
                &two,
                &no_desc()
            )
            .is_empty()
        );
    }

    #[test]
    fn an_entry_is_never_assigned_twice() {
        let entries = [entry(1, Some("s"), None, None)];
        let agents = [
            agent("a", Some("s"), None, "/x", None),
            agent("b", Some("s"), None, "/x", None),
        ];
        let m = match_registry_entries(&agents, &entries, &no_desc());
        assert_eq!(m.len(), 1);
        assert_eq!(m["a"], 0);
    }
}
