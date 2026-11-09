//! The agent manager (Hydra `electron/agents/AgentManager.ts`).
//!
//! Owns every agent: state, PTY process, 2 MB raw output buffer, status machine,
//! submit queue and idle detection. It is `Clone` (an `Arc` inside), runs on tokio
//! and publishes [`AgentEvent`]s on a broadcast channel (see [`AgentManager::subscribe`]).
//!
//! Threading model: one blocking reader thread and one blocking waiter thread per
//! running PTY (`pty::pump_with_eof`, shared with the free terminals); delayed work (submit queue, `/rename`, initial prompt, force kill)
//! are tokio tasks spawned on the runtime that created the manager. A single
//! `parking_lot::Mutex` guards the state and is never held across an `.await`.
//! PTY writes go through a per-process writer mutex, outside the state lock.
//!
//! Sessions (P4): after a spawn the manager asks the [`SessionLocator`] for the provider session id on
//! a backoff schedule ([`DiscoverySchedule`]); [`AgentManager::import_sessions`] adds catalog sessions
//! as idle agents without a process. Claude's startup input (`/rename`, initial prompt) is held back
//! while its "trust this folder" dialog is on screen (see `agents::startup`).
//!
//! Worktree mode (F4): `create` with `WorkMode::Worktree` first makes a `git worktree` on branch
//! `studio/<slug>-<id>` under `<repo>/.worktrees/` and runs the agent there (its `project_dir` is the
//! worktree, like Hydra). `remove` deletes the worktree afterwards only when git agrees it is clean.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use chrono::SecondsFormat;
use parking_lot::Mutex;
use regex::Regex;
use tokio::runtime::Handle;
use tokio::sync::broadcast;

use super::env::{EnvOverrides, build_agent_env_from_process, claude_profile_env};
use super::model_detect::{detect_latest_model_from_terminal_output, strip_ansi};
use super::output::{AltScreenFilter, OutputBuffer};
use super::providers::{PreflightResult, get_provider};
use super::pty::{NativePtyBackend, PtyBackend, PtyControl, PtySpawnRequest};
use super::shell::{ShellConfig, SpawnSpec, wrap_with_shell_current};
use super::startup::{GateState, StartupOptions, TrustGate};
use crate::sessions::SessionSummary;
use crate::types::{
    AgentConfig, AgentState, AgentStatus, CreateAgentPayload, MAX_CONCURRENT_AGENTS_HARD_LIMIT,
    ModelId, ProviderId, WorkMode,
};
use crate::workspace::PersistedWorkspaceAgent;

// ── Timing and size constants (same values as Hydra) ────────────────────────

/// SIGTERM, then SIGKILL after this long.
pub const GRACEFUL_KILL_TIMEOUT: Duration = Duration::from_millis(5000);
/// Pause between the text and the `\r` of a submitted input (and the second `\r` for Claude).
pub const INPUT_SUBMIT_DELAY: Duration = Duration::from_millis(100);
pub const DEFAULT_PTY_COLS: u16 = 120;
pub const DEFAULT_PTY_ROWS: u16 = 30;
/// A running agent with no output for this long is "waiting".
pub const IDLE_DETECTION_MS: u64 = 5000;
/// `\r` sent after restarting an idle agent without prompt, to wake the CLI.
pub const IDLE_RESTART_WAKE_DELAY: Duration = Duration::from_millis(350);
/// Period of the idle detection poll.
pub const ACTIVITY_POLL_INTERVAL: Duration = Duration::from_millis(2000);
/// Claude `/rename <name>` is written this long after spawn (`--name` does not name sessions).
pub const CLAUDE_RENAME_DELAY: Duration = Duration::from_millis(1500);
pub const INITIAL_PROMPT_DELAY: Duration = Duration::from_millis(1500);
/// Initial prompt delay when a `/rename` is sent first.
pub const INITIAL_PROMPT_DELAY_AFTER_RENAME: Duration = Duration::from_millis(3000);
const SESSION_HINT_MAX_LENGTH: usize = 400;
/// A session of the project touched this long before the spawn still counts as the agent's own.
pub const SESSION_DISCOVERY_GRACE_MS: i64 = 5 * 60 * 1000;
/// Output window scanned for the active model.
const MODEL_DETECTION_WINDOW: usize = 12_000;
const EVENT_CHANNEL_CAPACITY: usize = 4096;

static LEADING_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<[^>]+>").expect("static regex"));

// ── Public types ────────────────────────────────────────────────────────────

/// Pushed to every subscriber. `Output` carries raw PTY bytes (already stripped of
/// Codex alt-screen sequences); a chunk may split a UTF-8 sequence.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    Output {
        agent_id: String,
        data: Bytes,
    },
    /// Status, session id or model changed (also sent on rename).
    Status {
        agent_id: String,
        status: AgentStatus,
        session_id: Option<String>,
        model: ModelId,
    },
    /// Running agent produced no output for 5 s (once, until the next input).
    Waiting {
        agent_id: String,
    },
    /// The agent list changed: created, removed, renamed or hydrated.
    Changed,
    /// A new process replaces the old one (restart, YOLO/model switch, global YOLO): the output buffer was just cleared, so a
    /// client that keeps its own copy of the screen must drop it. Sent before any output of the new process.
    Restarted {
        agent_id: String,
    },
}

/// Output/exit facts for the Overview (`getActivity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentActivity {
    /// Epoch milliseconds of the last output; `0` when none yet.
    pub last_output_at: u64,
    /// Exit code of the last PTY run; `None` while running or never run.
    pub last_exit_code: Option<i32>,
}

#[derive(Debug, thiserror::Error)]
pub enum ManagerError {
    #[error("Maximum concurrent agents ({MAX_CONCURRENT_AGENTS_HARD_LIMIT}) reached")]
    MaxAgents,
    #[error("could not create the worktree: {0}")]
    Worktree(#[from] crate::git::GitError),
}

/// `studio/<slug of the name>-<id>`; the slug keeps ASCII letters, digits, `-`, `_` and `.`.
pub fn worktree_branch_name(name: &str, id: &str) -> String {
    let mut slug = String::new();
    for c in name.trim().to_lowercase().chars() {
        let c = if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
            c
        } else {
            '-'
        };
        if c == '-' && slug.ends_with('-') {
            continue;
        }
        slug.push(c);
    }
    let slug = slug
        .trim_matches(|c| c == '-' || c == '.')
        .chars()
        .take(40)
        .collect::<String>();
    let slug = slug.trim_end_matches(['-', '.']);
    let slug = if slug.is_empty() { "agent" } else { slug };
    format!("studio/{slug}-{id}")
}

/// Remove an agent's worktree and branch if it is clean; otherwise keep everything and log.
fn cleanup_worktree(path: &str, branch: &str) {
    let git = crate::git::GitRunner::new();
    let wt = Path::new(path);
    let repo = match git.main_worktree(wt) {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(path, error = %e, "worktree already gone or not a repository");
            return;
        }
    };
    match git.worktree_remove(&repo, wt, Some(branch), false) {
        Ok(()) => tracing::info!(path, branch, "removed agent worktree"),
        Err(e) => tracing::warn!(
            path,
            branch,
            error = %e,
            "kept the agent's worktree (uncommitted work or git error)"
        ),
    }
}

/// What a [`SessionLocator`] gets to work with.
#[derive(Debug, Clone)]
pub struct SessionQuery {
    pub agent: AgentState,
    /// First words the user typed (for matching a transcript's first prompt).
    pub latest_user_prompt: Option<String>,
    /// Session ids already owned by other agents.
    pub used_session_ids: Vec<String>,
    /// Rescan the catalog instead of reusing a recent snapshot (first probe after a spawn, exit).
    pub force_refresh: bool,
}

/// Finds the provider session id of an agent from the CLI's own catalogs (Hydra
/// `probeSessionIdFromCatalog`). Blocking: the manager calls it from `spawn_blocking`.
///
/// `Some(id)` settles the agent on that session (the manager stops looking; when `id` differs from the
/// current one, e.g. Codex resuming into a new rollout, the agent switches to it); `None` means
/// "not found yet" and the manager asks again later.
pub trait SessionLocator: Send + Sync + 'static {
    fn locate(&self, query: &SessionQuery) -> Option<String>;

    /// `false` when the locator never finds anything (no point in polling).
    fn enabled(&self) -> bool {
        true
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoSessionLocator;
impl SessionLocator for NoSessionLocator {
    fn locate(&self, _query: &SessionQuery) -> Option<String> {
        None
    }
    fn enabled(&self) -> bool {
        false
    }
}

/// Backoff of the session-id discovery after a spawn: `initial`, then `initial * factor^n`, at most `max`.
#[derive(Debug, Clone, Copy)]
pub struct DiscoverySchedule {
    pub initial: Duration,
    pub max: Duration,
    pub factor: f64,
}

impl Default for DiscoverySchedule {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(2500),
            max: Duration::from_secs(30),
            factor: 1.6,
        }
    }
}

impl DiscoverySchedule {
    /// Pause before probe number `attempt + 1` (attempt 0 is the first pause).
    pub fn delay(&self, attempt: u32) -> Duration {
        let exp = i32::try_from(attempt).unwrap_or(i32::MAX);
        let secs = self.initial.as_secs_f64() * self.factor.powi(exp);
        if secs.is_finite() && secs < self.max.as_secs_f64() {
            Duration::from_secs_f64(secs)
        } else {
            self.max
        }
    }
}

/// Lowercase, whitespace-collapsed, cut to 400 chars: what prompts are compared by.
pub(crate) fn normalize_prompt_hint(input: Option<&str>) -> String {
    input
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .chars()
        .take(SESSION_HINT_MAX_LENGTH)
        .collect()
}

/// Replaces the provider CLI invocation (tests, or a custom launcher).
pub type CommandOverride = Arc<dyn Fn(&AgentState) -> Option<SpawnSpec> + Send + Sync>;
/// `(project_dir, session_id) -> CLAUDE_CONFIG_DIR`: `Some(dir)` selects a profile,
/// `Some("")` forces the default one, `None` keeps the daemon's env.
pub type ClaudeProfileResolver =
    Arc<dyn Fn(&str, Option<&str>, Option<&str>) -> Option<String> + Send + Sync>;
pub type ShellConfigProvider = Arc<dyn Fn() -> ShellConfig + Send + Sync>;

pub struct ManagerOptions {
    pub backend: Arc<dyn PtyBackend>,
    pub locator: Arc<dyn SessionLocator>,
    /// When it returns `Some`, the spec is spawned as is (no provider path, no shell wrapping).
    pub command_override: Option<CommandOverride>,
    /// Grace period between SIGTERM and SIGKILL.
    pub kill_grace: Duration,
    pub discovery: DiscoverySchedule,
    pub startup: StartupOptions,
}

impl Default for ManagerOptions {
    fn default() -> Self {
        Self {
            backend: Arc::new(NativePtyBackend),
            locator: Arc::new(NoSessionLocator),
            command_override: None,
            kill_grace: GRACEFUL_KILL_TIMEOUT,
            discovery: DiscoverySchedule::default(),
            startup: StartupOptions::default(),
        }
    }
}

// ── Internal state ──────────────────────────────────────────────────────────

struct Run {
    run_id: u64,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    control: Box<dyn PtyControl>,
    alt_filter: AltScreenFilter,
    /// Claude only: holds the startup input while the trust dialog is up.
    gate: Option<TrustGate>,
}

struct Managed {
    state: AgentState,
    /// Imported from a provider catalog rather than created here (P4); never persisted.
    imported: bool,
    run: Option<Run>,
    output: OutputBuffer,
    stop_requested: bool,
    cols: u16,
    rows: u16,
    latest_user_prompt: Option<String>,
    notified_idle: bool,
    last_output_at: u64,
    last_exit_code: Option<i32>,
    /// Serializes submitted inputs of this agent; replaced on restart so stale queued inputs die.
    input_lock: Arc<tokio::sync::Mutex<()>>,
}

impl Managed {
    fn new(state: AgentState, latest_user_prompt: Option<String>) -> Self {
        Self {
            state,
            imported: false,
            run: None,
            output: OutputBuffer::default(),
            stop_requested: false,
            cols: DEFAULT_PTY_COLS,
            rows: DEFAULT_PTY_ROWS,
            latest_user_prompt,
            notified_idle: false,
            last_output_at: 0,
            last_exit_code: None,
            input_lock: Arc::default(),
        }
    }

    fn is_active(&self) -> bool {
        self.run.is_some()
            || matches!(
                self.state.status,
                AgentStatus::Running | AgentStatus::Starting
            )
    }
}

struct Inner {
    agents: Vec<Managed>,
    provider_paths: HashMap<ProviderId, String>,
    shell_config: ShellConfigProvider,
    claude_profile: ClaudeProfileResolver,
    /// Extra env for every spawned agent (e.g. the daemon's fixed PATH).
    env_overrides: EnvOverrides,
}

impl Inner {
    fn idx(&self, id: &str) -> Option<usize> {
        self.agents.iter().position(|m| m.state.id == id)
    }

    fn count_active(&self, exclude: Option<&str>) -> usize {
        self.agents
            .iter()
            .filter(|m| Some(m.state.id.as_str()) != exclude && m.is_active())
            .count()
    }

    fn unique_id(&self, preferred: &str) -> String {
        if self.idx(preferred).is_none() {
            return preferred.to_string();
        }
        (2..)
            .map(|n| format!("{preferred}-{n}"))
            .find(|c| self.idx(c).is_none())
            .unwrap_or_default()
    }
}

/// Something written to Claude shortly after the spawn.
enum StartupStep {
    Rename(String),
    Prompt(String),
}

enum GateOutcome {
    Go,
    /// The process is gone (or the manager is stopping): write nothing.
    Gone,
    Expired,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum SpawnOutcome {
    Spawned,
    Capped,
    Errored,
}

struct Shared {
    inner: Mutex<Inner>,
    events: broadcast::Sender<AgentEvent>,
    handle: Handle,
    backend: Arc<dyn PtyBackend>,
    locator: Arc<dyn SessionLocator>,
    command_override: Option<CommandOverride>,
    kill_grace: Duration,
    discovery: DiscoverySchedule,
    startup: StartupOptions,
    next_run_id: AtomicU64,
    shutdown: AtomicBool,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Tell the startup gate of `m`'s process that the user sent it `data`.
fn note_user_input(m: &mut Managed, data: &[u8]) {
    if let Some(gate) = m.run.as_mut().and_then(|r| r.gate.as_mut()) {
        gate.on_user_input(data);
    }
}

fn write_pty(writer: &Arc<Mutex<Box<dyn Write + Send>>>, data: &[u8]) -> bool {
    let mut w = writer.lock();
    match w.write_all(data).and_then(|()| w.flush()) {
        Ok(()) => true,
        Err(err) => {
            tracing::debug!(%err, "PTY write failed");
            false
        }
    }
}

impl Shared {
    fn emit(&self, event: AgentEvent) {
        let _ = self.events.send(event); // no subscribers is fine
    }

    fn emit_status(&self, m: &Managed) {
        self.emit(AgentEvent::Status {
            agent_id: m.state.id.clone(),
            status: m.state.status,
            session_id: m.state.session_id.clone(),
            model: m.state.model.clone(),
        });
    }

    fn update_status(&self, m: &mut Managed, status: AgentStatus) {
        m.state.status = status;
        self.emit_status(m);
    }

    fn update_session_id(&self, m: &mut Managed, session_id: &str) {
        let normalized = session_id.trim();
        if normalized.is_empty() || m.state.session_id.as_deref() == Some(normalized) {
            return;
        }
        m.state.session_id = Some(normalized.to_string());
        self.emit_status(m);
    }

    fn capture_session_id(&self, m: &mut Managed, data: &[u8]) {
        if m.state.session_id.is_some() {
            return;
        }
        // Only the provider's own pattern: Hydra also accepted any UUID in the output, which would
        // adopt ids from the conversation text; the catalog discovery is the real source.
        let text = strip_ansi(&String::from_utf8_lossy(data));
        if let Some(caps) = get_provider(m.state.provider)
            .session_id_regex()
            .and_then(|re| re.captures(&text))
            && let Some(id) = caps.get(1)
        {
            self.update_session_id(m, id.as_str());
        }
    }

    fn capture_model(&self, m: &mut Managed) {
        let recent = m.output.recent_text(MODEL_DETECTION_WINDOW);
        let Some(detected) = detect_latest_model_from_terminal_output(m.state.provider, &recent)
        else {
            return;
        };
        if detected == m.state.model {
            return;
        }
        m.state.model = detected;
        self.emit_status(m);
    }

    /// Append already-filtered output: buffer, model capture, event, activity stamps.
    fn push_output(&self, m: &mut Managed, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        m.output.push(&data);
        self.capture_model(m);
        self.emit(AgentEvent::Output {
            agent_id: m.state.id.clone(),
            data: Bytes::from(data),
        });
        m.last_output_at = now_ms();
        m.state.last_activity_at = now_iso();
    }

    fn on_data(&self, id: &str, run_id: u64, data: &[u8]) {
        let mut inner = self.inner.lock();
        let Some(idx) = inner.idx(id) else { return };
        let m = &mut inner.agents[idx];
        let Some(run) = m.run.as_mut().filter(|r| r.run_id == run_id) else {
            return;
        };
        let codex = m.state.provider == ProviderId::Codex;
        let filtered = if codex {
            run.alt_filter.filter(data)
        } else {
            data.to_vec()
        };
        if let Some(gate) = run.gate.as_mut() {
            let was_holding = gate.holding();
            gate.on_output(data, now_ms());
            if gate.holding() != was_holding {
                tracing::info!(agent = %id, holding = gate.holding(), "Claude trust prompt state changed");
            }
        }
        // Hydra scans the raw chunk (before filtering) for the session id.
        self.capture_session_id(m, data);
        self.push_output(m, filtered);
    }

    fn on_eof(&self, id: &str, run_id: u64) {
        let mut inner = self.inner.lock();
        let Some(idx) = inner.idx(id) else { return };
        let m = &mut inner.agents[idx];
        let Some(run) = m.run.as_mut().filter(|r| r.run_id == run_id) else {
            return;
        };
        let rest = run.alt_filter.flush();
        self.push_output(m, rest);
    }

    fn on_exit(&self, id: &str, run_id: u64, exit_code: i32) {
        let query = {
            let mut inner = self.inner.lock();
            let Some(idx) = inner.idx(id) else { return };
            if !inner.agents[idx]
                .run
                .as_ref()
                .is_some_and(|r| r.run_id == run_id)
            {
                return; // stale callback from a replaced PTY
            }
            let used: Vec<String> = inner
                .agents
                .iter()
                .filter(|a| a.state.id != id)
                .filter_map(|a| a.state.session_id.clone())
                .collect();
            let m = &mut inner.agents[idx];
            m.run = None; // drops the PTY handle
            m.state.pid = None;
            m.state.ended_at = Some(now_iso());
            m.last_exit_code = Some(exit_code);
            let status = if m.stop_requested || exit_code == 0 {
                AgentStatus::Idle
            } else {
                AgentStatus::Errored
            };
            m.stop_requested = false;
            self.update_status(m, status);
            ((m.state.session_id.is_none() || m.state.provider == ProviderId::Codex)
                && !m.imported
                && get_provider(m.state.provider).supports_resume)
                .then(|| SessionQuery {
                    agent: m.state.clone(),
                    latest_user_prompt: m.latest_user_prompt.clone(),
                    used_session_ids: used,
                    force_refresh: true,
                })
        };
        // Outside the lock: a locator may read files.
        if let Some(query) = query
            && let Some(found) = self.locator.locate(&query)
        {
            let mut inner = self.inner.lock();
            if let Some(idx) = inner.idx(id) {
                let m = &mut inner.agents[idx];
                self.update_session_id(m, &found);
            }
        }
    }

    // ── spawning ────────────────────────────────────────────────────────────

    fn spawn_locked(self: &Arc<Self>, inner: &mut Inner, idx: usize) -> SpawnOutcome {
        let id = inner.agents[idx].state.id.clone();
        if inner.count_active(Some(&id)) >= MAX_CONCURRENT_AGENTS_HARD_LIMIT {
            return SpawnOutcome::Capped;
        }
        let provider_path = inner
            .provider_paths
            .get(&inner.agents[idx].state.provider)
            .cloned();
        let shell_config = (inner.shell_config)();
        let claude_profile = inner.claude_profile.clone();
        let base_overrides = inner.env_overrides.clone();
        let m = &mut inner.agents[idx];

        m.last_exit_code = None;
        let provider = get_provider(m.state.provider);

        let overridden = self.command_override.as_ref().and_then(|f| f(&m.state));
        let (cmd, args) = match overridden {
            Some(spec) => (spec.cmd, spec.args),
            None => {
                let raw = SpawnSpec {
                    cmd: provider_path.unwrap_or_else(|| provider.command.to_string()),
                    args: provider.build_args(&m.state),
                };
                let wrapped = wrap_with_shell_current(&raw, &shell_config);
                (wrapped.cmd, wrapped.args)
            }
        };

        let mut overrides: EnvOverrides = base_overrides;
        if m.state.provider == ProviderId::Claude {
            let dir = claude_profile(
                &m.state.project_dir,
                m.state.session_id.as_deref(),
                m.state.claude_config_dir.as_deref(),
            );
            overrides.extend(claude_profile_env(dir.as_deref()));
        }
        overrides.insert("TERM".into(), Some("xterm-256color".into()));
        overrides.insert("FORCE_COLOR".into(), Some("1".into()));
        let request = PtySpawnRequest {
            cmd,
            args,
            cwd: m.state.project_dir.clone(),
            env: build_agent_env_from_process(&overrides),
            cols: m.cols,
            rows: m.rows,
        };

        tracing::info!(agent = %id, provider = %m.state.provider, cmd = %request.cmd, args = ?request.args, cwd = %request.cwd, "spawning agent");
        let process = match self.backend.spawn(&request) {
            Ok(p) => Ok(p),
            Err(direct_err) if cfg!(windows) => {
                // CLI tools are often `.cmd` shims that cannot be executed directly: retry through cmd.exe.
                tracing::warn!(agent = %id, err = %direct_err, "direct spawn failed, retrying via cmd.exe");
                let mut fallback = request.clone();
                fallback.args.insert(0, request.cmd.clone());
                fallback.args.insert(0, "/c".into());
                fallback.cmd = "cmd.exe".into();
                self.backend.spawn(&fallback)
            }
            Err(err) => Err(err),
        };
        let process = match process {
            Ok(p) => p,
            Err(err) => {
                tracing::error!(agent = %id, err = %format!("{err:#}"), "spawn failed");
                m.state.ended_at = Some(now_iso());
                self.update_status(m, AgentStatus::Errored);
                return SpawnOutcome::Errored;
            }
        };

        let run_id = self.next_run_id.fetch_add(1, Ordering::Relaxed) + 1;
        let writer = Arc::new(Mutex::new(process.writer));
        m.run = Some(Run {
            run_id,
            writer: writer.clone(),
            control: process.control,
            alt_filter: AltScreenFilter::default(),
            gate: (m.state.provider == ProviderId::Claude).then(|| TrustGate::new(now_ms())),
        });
        m.stop_requested = false;
        m.state.pid = process.pid;
        m.state.started_at = Some(now_iso());
        m.state.ended_at = None;
        self.update_status(m, AgentStatus::Running);

        self.start_io_threads(&id, run_id, process.reader, process.waiter);

        // `--name` does not name Claude sessions: write /rename after startup.
        let rename = (m.state.provider == ProviderId::Claude && !m.state.name.trim().is_empty())
            .then(|| m.state.name.trim().to_string());
        let mut steps = Vec::new();
        if let Some(name) = &rename {
            steps.push((CLAUDE_RENAME_DELAY, StartupStep::Rename(name.clone())));
        }
        // Initial prompt, once, after startup (fresh and resumed sessions).
        if !m.state.initial_prompt.is_empty() {
            let prompt = std::mem::take(&mut m.state.initial_prompt);
            let delay = if rename.is_some() {
                INITIAL_PROMPT_DELAY_AFTER_RENAME
            } else {
                INITIAL_PROMPT_DELAY
            };
            steps.push((delay, StartupStep::Prompt(prompt)));
        }
        if !steps.is_empty() {
            self.spawn_startup(&id, run_id, writer, steps);
        }
        if self.locator.enabled()
            && !m.imported
            && provider.supports_resume
            && (m.state.session_id.is_none() || m.state.provider == ProviderId::Codex)
        {
            self.spawn_discovery(&id, run_id);
        }
        SpawnOutcome::Spawned
    }

    // ── startup input (held while Claude shows its trust dialog) ────────────

    fn spawn_startup(
        self: &Arc<Self>,
        id: &str,
        run_id: u64,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        steps: Vec<(Duration, StartupStep)>,
    ) {
        let (shared, id) = (self.clone(), id.to_string());
        self.handle.spawn(async move {
            let t0 = tokio::time::Instant::now();
            for (delay, step) in steps {
                tokio::time::sleep_until(t0 + delay).await;
                match shared.wait_for_gate(&id, run_id).await {
                    GateOutcome::Go => {}
                    GateOutcome::Gone => return,
                    GateOutcome::Expired => {
                        tracing::warn!(agent = %id, "trust prompt not answered in time: startup input dropped");
                        return;
                    }
                }
                match step {
                    StartupStep::Rename(name) => {
                        write_pty(&writer, format!("/rename {name}\r").as_bytes());
                    }
                    StartupStep::Prompt(text) => {
                        let mut inner = shared.inner.lock();
                        if let Some(idx) = inner.idx(&id) {
                            shared.queue_locked(&mut inner, idx, text);
                        }
                    }
                }
            }
        });
    }

    /// Wait until the startup input of this run may be written.
    async fn wait_for_gate(&self, id: &str, run_id: u64) -> GateOutcome {
        loop {
            let state = {
                let inner = self.inner.lock();
                let run = inner
                    .idx(id)
                    .and_then(|i| inner.agents[i].run.as_ref())
                    .filter(|r| r.run_id == run_id);
                match run {
                    None => return GateOutcome::Gone,
                    Some(run) => run
                        .gate
                        .as_ref()
                        .map_or(GateState::Proceed, |g| g.poll(now_ms(), &self.startup)),
                }
            };
            match state {
                GateState::Proceed => return GateOutcome::Go,
                GateState::Expired => return GateOutcome::Expired,
                GateState::Wait if self.shutdown.load(Ordering::Relaxed) => {
                    return GateOutcome::Gone;
                }
                GateState::Wait => tokio::time::sleep(self.startup.poll).await,
            }
        }
    }

    // ── session id discovery ────────────────────────────────────────────────

    /// Ask the locator right away and then on the backoff schedule until it settles on a session,
    /// the process ends or the manager shuts down.
    fn spawn_discovery(self: &Arc<Self>, id: &str, run_id: u64) {
        let (shared, id) = (self.clone(), id.to_string());
        self.handle.spawn(async move {
            let mut attempt = 0u32;
            loop {
                let Some(query) = shared.discovery_query(&id, run_id, attempt == 0) else {
                    return;
                };
                let locator = shared.locator.clone();
                let found = tokio::task::spawn_blocking(move || locator.locate(&query))
                    .await
                    .ok()
                    .flatten();
                if let Some(found) = found {
                    shared.apply_discovered(&id, run_id, &found);
                    return;
                }
                tokio::time::sleep(shared.discovery.delay(attempt)).await;
                attempt = attempt.saturating_add(1);
            }
        });
    }

    /// The query for the next probe, or `None` when discovery is over for this run.
    fn discovery_query(&self, id: &str, run_id: u64, force_refresh: bool) -> Option<SessionQuery> {
        if self.shutdown.load(Ordering::Relaxed) {
            return None;
        }
        let inner = self.inner.lock();
        let idx = inner.idx(id)?;
        let m = &inner.agents[idx];
        if m.run.as_ref().is_none_or(|r| r.run_id != run_id) || m.imported {
            return None;
        }
        // Found by other means (output, `set_session_id`): done, except Codex which may move on.
        if m.state.session_id.is_some() && m.state.provider != ProviderId::Codex {
            return None;
        }
        let used = inner
            .agents
            .iter()
            .filter(|a| a.state.id != id)
            .filter_map(|a| a.state.session_id.clone())
            .collect();
        Some(SessionQuery {
            agent: m.state.clone(),
            latest_user_prompt: m.latest_user_prompt.clone(),
            used_session_ids: used,
            force_refresh,
        })
    }

    fn apply_discovered(&self, id: &str, run_id: u64, session_id: &str) {
        let mut inner = self.inner.lock();
        let Some(idx) = inner.idx(id) else { return };
        let m = &mut inner.agents[idx];
        if m.run.as_ref().is_some_and(|r| r.run_id == run_id) {
            tracing::debug!(agent = %id, session = %session_id, "session discovered");
            self.update_session_id(m, session_id);
        }
    }

    fn start_io_threads(
        self: &Arc<Self>,
        id: &str,
        run_id: u64,
        reader: Box<dyn Read + Send>,
        waiter: Box<dyn super::pty::PtyWaiter>,
    ) {
        // The shared pump runs the reader and waiter threads (and holds back the exit until the
        // reader had up to 500 ms to deliver the last output); the run id lets the callbacks of a
        // replaced PTY be ignored.
        let (on_data, on_eof, on_exit) = (self.clone(), self.clone(), self.clone());
        let (a_data, a_eof, a_exit) = (id.to_string(), id.to_string(), id.to_string());
        super::pty::pump_with_eof(
            &format!("agent-{id}"),
            reader,
            waiter,
            move |data| on_data.on_data(&a_data, run_id, data),
            move || on_eof.on_eof(&a_eof, run_id),
            move |code| on_exit.on_exit(&a_exit, run_id, code),
        );
    }

    fn run_is_current(&self, id: &str, run_id: u64) -> bool {
        let inner = self.inner.lock();
        inner
            .idx(id)
            .and_then(|i| inner.agents[i].run.as_ref())
            .is_some_and(|r| r.run_id == run_id)
    }

    fn ensure_process(self: &Arc<Self>, inner: &mut Inner, idx: usize) -> bool {
        if inner.agents[idx].run.is_some() {
            return true;
        }
        if inner.count_active(Some(&inner.agents[idx].state.id.clone()))
            >= MAX_CONCURRENT_AGENTS_HARD_LIMIT
        {
            return false;
        }
        self.spawn_locked(inner, idx) == SpawnOutcome::Spawned
    }

    // ── input ───────────────────────────────────────────────────────────────

    /// Queue `input` + Enter for the agent (see module docs). False when it has no process.
    fn queue_locked(self: &Arc<Self>, inner: &mut Inner, idx: usize, input: String) -> bool {
        let m = &mut inner.agents[idx];
        let Some(run) = &m.run else { return false };
        let (run_id, writer) = (run.run_id, run.writer.clone());
        // Reset idle detection so we notify again after this task completes.
        m.notified_idle = false;
        m.last_output_at = now_ms();
        m.state.last_activity_at = now_iso();
        let lock = m.input_lock.clone();
        let id = m.state.id.clone();
        let claude = m.state.provider == ProviderId::Claude;
        let shared = self.clone();
        self.handle.spawn(async move {
            let _turn = lock.lock().await;
            if !shared.run_is_current(&id, run_id) {
                return;
            }
            let hint = input.trim();
            if !hint.is_empty() {
                let mut inner = shared.inner.lock();
                if let Some(i) = inner.idx(&id) {
                    let m = &mut inner.agents[i];
                    if m.state.session_id.is_none() {
                        m.latest_user_prompt =
                            Some(hint.chars().take(SESSION_HINT_MAX_LENGTH).collect());
                    }
                }
            }
            write_pty(&writer, input.as_bytes());
            tokio::time::sleep(INPUT_SUBMIT_DELAY).await;
            if !shared.run_is_current(&id, run_id) {
                return;
            }
            write_pty(&writer, b"\r");
            // Claude shows long/pasted input as "[Pasted text ...]" and needs a second Enter.
            if claude {
                tokio::time::sleep(INPUT_SUBMIT_DELAY).await;
                if !shared.run_is_current(&id, run_id) {
                    return;
                }
                write_pty(&writer, b"\r");
            }
        });
        true
    }

    // ── killing ─────────────────────────────────────────────────────────────

    fn kill_locked(self: &Arc<Self>, m: &mut Managed) {
        let Some(run) = m.run.as_mut() else { return };
        m.stop_requested = true;
        run.control.terminate(false);
        let (shared, id, run_id) = (self.clone(), m.state.id.clone(), run.run_id);
        self.handle.spawn(async move {
            tokio::time::sleep(shared.kill_grace).await;
            // Only if that very process is still the current one (a restart replaces it).
            let mut inner = shared.inner.lock();
            if let Some(i) = inner.idx(&id)
                && let Some(run) = inner.agents[i].run.as_mut().filter(|r| r.run_id == run_id)
            {
                run.control.terminate(true);
            }
        });
    }

    fn poll_activity_at(&self, now: u64) {
        let mut inner = self.inner.lock();
        for m in &mut inner.agents {
            if m.state.status != AgentStatus::Running
                || m.run.is_none()
                || m.notified_idle
                || m.last_output_at == 0
            {
                continue;
            }
            if now.saturating_sub(m.last_output_at) >= IDLE_DETECTION_MS {
                m.notified_idle = true;
                self.emit(AgentEvent::Waiting {
                    agent_id: m.state.id.clone(),
                });
            }
        }
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// See the module docs. Cheap to clone; all clones share the same agents.
#[derive(Clone)]
pub struct AgentManager {
    shared: Arc<Shared>,
}

impl AgentManager {
    /// Real PTYs, default options. Must be called inside a tokio runtime.
    pub fn new() -> Self {
        Self::with_options(ManagerOptions::default())
    }

    /// Must be called inside a tokio runtime (delayed work runs on it).
    pub fn with_options(options: ManagerOptions) -> Self {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                agents: Vec::new(),
                provider_paths: HashMap::new(),
                // Default 'direct' so a bare manager spawns the CLI as is; the daemon installs the real config.
                shell_config: Arc::new(ShellConfig::direct),
                claude_profile: Arc::new(|_, _, _| None),
                env_overrides: EnvOverrides::new(),
            }),
            events,
            handle: Handle::current(),
            backend: options.backend,
            locator: options.locator,
            command_override: options.command_override,
            kill_grace: options.kill_grace,
            discovery: options.discovery,
            startup: options.startup,
            next_run_id: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
        });
        Self::start_activity_polling(Arc::downgrade(&shared), &shared.handle);
        Self { shared }
    }

    fn start_activity_polling(weak: Weak<Shared>, handle: &Handle) {
        handle.spawn(async move {
            let mut interval = tokio::time::interval(ACTIVITY_POLL_INTERVAL);
            loop {
                interval.tick().await;
                let Some(shared) = weak.upgrade() else { break };
                if shared.shutdown.load(Ordering::Relaxed) {
                    break;
                }
                shared.poll_activity_at(now_ms());
            }
        });
    }

    /// Subscribe to [`AgentEvent`]s. A slow subscriber that falls 4096 events behind gets
    /// `RecvError::Lagged` and should resync from [`AgentManager::get_buffer`].
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.shared.events.subscribe()
    }

    /// Shell wrapping settings, read at every spawn (so config changes apply to the next spawn).
    pub fn set_shell_config_provider(
        &self,
        provider: impl Fn() -> ShellConfig + Send + Sync + 'static,
    ) {
        self.shared.inner.lock().shell_config = Arc::new(provider);
    }

    /// Env overrides applied to every agent spawned from now on (`Some` sets, `None` removes);
    /// the daemon uses it to hand agents `shell::fixed_path()` without touching its own env.
    pub fn set_env_overrides(&self, overrides: EnvOverrides) {
        self.shared.inner.lock().env_overrides = overrides;
    }

    /// Claude profile (`CLAUDE_CONFIG_DIR`) resolver; defaults to "keep the daemon's env".
    pub fn set_claude_profile_resolver(
        &self,
        resolver: impl Fn(&str, Option<&str>, Option<&str>) -> Option<String> + Send + Sync + 'static,
    ) {
        self.shared.inner.lock().claude_profile = Arc::new(resolver);
    }

    // ── preflight ───────────────────────────────────────────────────────────

    /// Check one provider CLI and remember its resolved path for spawning.
    pub async fn preflight(&self, provider: ProviderId) -> PreflightResult {
        let result = get_provider(provider).preflight().await;
        if let (true, Some(path)) = (result.ok, &result.path) {
            self.shared
                .inner
                .lock()
                .provider_paths
                .insert(provider, path.clone());
            tracing::info!(%provider, %path, version = ?result.version, "preflight ok");
        } else {
            tracing::warn!(%provider, error = ?result.error, "preflight failed");
        }
        result
    }

    // ── lifecycle ───────────────────────────────────────────────────────────

    /// Create an agent and start its process (up to 10 active at once).
    pub async fn create(&self, payload: CreateAgentPayload) -> Result<AgentState, ManagerError> {
        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let mut worktree = None;
        if payload.work_mode == WorkMode::Worktree && !payload.project_dir.is_empty() {
            if self.shared.inner.lock().count_active(None) >= MAX_CONCURRENT_AGENTS_HARD_LIMIT {
                return Err(ManagerError::MaxAgents);
            }
            let dir = std::path::PathBuf::from(&payload.project_dir);
            let branch = worktree_branch_name(&payload.name, &id);
            let made = tokio::task::spawn_blocking(move || {
                crate::git::GitRunner::new().worktree_create(&dir, &branch, None)
            })
            .await
            .map_err(|e| crate::git::GitError::Io(std::io::Error::other(e)))??;
            worktree = Some(made);
        }
        let mut inner = self.shared.inner.lock();
        if inner.count_active(None) >= MAX_CONCURRENT_AGENTS_HARD_LIMIT {
            drop(inner);
            Self::discard_worktree(worktree);
            return Err(ManagerError::MaxAgents);
        }
        let resumed = payload.resume_session_id.clone();
        let (project_dir, worktree_path, worktree_branch) = match &worktree {
            Some(w) => (w.path.clone(), Some(w.path.clone()), w.branch.clone()),
            None => (payload.project_dir, None, None),
        };
        let now = now_iso();
        let state = AgentState {
            config: AgentConfig {
                id,
                name: payload.name,
                project_dir,
                provider: payload.provider,
                model: payload.model,
                reasoning_effort: payload.reasoning_effort,
                claude_config_dir: payload.claude_config_dir,
                yolo: payload.yolo,
                is_manager: payload.is_manager,
                session_id: payload.resume_session_id,
                initial_prompt: payload.initial_prompt.clone(),
                created_at: now.clone(),
                work_mode: payload.work_mode,
                worktree_path,
                worktree_branch,
            },
            status: AgentStatus::Starting,
            pid: None,
            restart_count: 0,
            started_at: None,
            ended_at: None,
            last_activity_at: now,
        };
        let hint = payload.initial_prompt.trim();
        let managed = Managed::new(state, (!hint.is_empty()).then(|| hint.to_string()));
        inner.agents.push(managed);
        let idx = inner.agents.len() - 1;
        let outcome = self.shared.spawn_locked(&mut inner, idx);
        if outcome == SpawnOutcome::Capped {
            inner.agents.pop();
            drop(inner);
            Self::discard_worktree(worktree);
            return Err(ManagerError::MaxAgents);
        }
        let state = inner.agents[idx].state.clone();
        // Resuming a session that is listed as an idle imported agent: the new agent takes its place.
        if let Some(sid) = resumed {
            inner.agents.retain(|m| {
                !(m.imported
                    && m.run.is_none()
                    && m.state.id != state.id
                    && m.state.session_id.as_deref() == Some(sid.as_str())
                    && m.state.provider == state.provider)
            });
        }
        drop(inner);
        self.shared.emit(AgentEvent::Changed);
        Ok(state)
    }

    /// Drop a worktree made for an agent that was never created (cleanup runs in the background).
    fn discard_worktree(worktree: Option<crate::git::GitWorktree>) {
        if let Some(w) = worktree {
            let branch = w.branch.unwrap_or_default();
            std::thread::spawn(move || cleanup_worktree(&w.path, &branch));
        }
    }

    /// Ask the agent's process to stop (SIGTERM, SIGKILL after the grace period). An explicit
    /// stop ends as `idle` even when the exit code is not zero. False for an unknown agent.
    pub fn kill(&self, id: &str) -> bool {
        let mut inner = self.shared.inner.lock();
        let Some(idx) = inner.idx(id) else {
            return false;
        };
        self.shared.kill_locked(&mut inner.agents[idx]);
        true
    }

    /// Stop and forget an agent. The process gets SIGTERM, then SIGKILL after the grace period.
    pub async fn remove(&self, id: &str) -> bool {
        let removed = {
            let mut inner = self.shared.inner.lock();
            let Some(idx) = inner.idx(id) else {
                return false;
            };
            inner.agents.remove(idx)
        };
        let worktree = removed
            .state
            .worktree_path
            .clone()
            .zip(removed.state.worktree_branch.clone());
        if let Some(mut run) = removed.run {
            run.control.terminate(false);
            let grace = self.shared.kill_grace;
            self.shared.handle.spawn(async move {
                tokio::time::sleep(grace).await;
                run.control.terminate(true);
                if let Some((path, branch)) = worktree {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    let _ =
                        tokio::task::spawn_blocking(move || cleanup_worktree(&path, &branch)).await;
                }
            });
        } else if let Some((path, branch)) = worktree {
            self.shared.handle.spawn(async move {
                let _ = tokio::task::spawn_blocking(move || cleanup_worktree(&path, &branch)).await;
            });
        }
        self.shared.emit(AgentEvent::Changed);
        true
    }

    /// Kill the process (if any) and start a fresh one, resuming the session when known.
    /// Allowed at the cap for an agent that already counts; when blocked by the cap the agent
    /// is returned unchanged. `None` for an unknown agent.
    pub fn restart(&self, id: &str) -> Option<AgentState> {
        let shared = &self.shared;
        let mut inner = shared.inner.lock();
        let idx = inner.idx(id)?;
        let previous = inner.agents[idx].state.status;
        let wake =
            previous == AgentStatus::Idle && inner.agents[idx].state.initial_prompt.is_empty();
        if inner.count_active(Some(id)) >= MAX_CONCURRENT_AGENTS_HARD_LIMIT {
            return Some(inner.agents[idx].state.clone());
        }
        let m = &mut inner.agents[idx];
        if let Some(mut run) = m.run.take() {
            run.control.terminate(true); // the old waiter's exit callback is ignored (generation check)
        }
        m.state.restart_count += 1;
        m.state.status = AgentStatus::Starting;
        m.output.clear();
        shared.emit(AgentEvent::Restarted {
            agent_id: id.to_string(),
        });
        m.input_lock = Arc::default();
        m.stop_requested = false;
        m.notified_idle = false;
        m.last_output_at = 0;
        match shared.spawn_locked(&mut inner, idx) {
            SpawnOutcome::Capped => {
                let m = &mut inner.agents[idx];
                shared.update_status(m, previous);
            }
            SpawnOutcome::Spawned if wake => {
                let run_id = inner.agents[idx]
                    .run
                    .as_ref()
                    .map(|r| (r.run_id, r.writer.clone()));
                if let Some((run_id, writer)) = run_id {
                    let (shared, id) = (shared.clone(), id.to_string());
                    shared.handle.clone().spawn(async move {
                        tokio::time::sleep(IDLE_RESTART_WAKE_DELAY).await;
                        if shared.run_is_current(&id, run_id) {
                            write_pty(&writer, b"\r");
                        }
                    });
                }
            }
            _ => {}
        }
        Some(inner.agents[idx].state.clone())
    }

    /// Set the YOLO flag and restart so it takes effect (the session is resumed).
    pub fn toggle_yolo(&self, id: &str, yolo: bool) -> Option<AgentState> {
        {
            let mut inner = self.shared.inner.lock();
            let idx = inner.idx(id)?;
            inner.agents[idx].state.yolo = yolo;
        }
        self.restart(id)
    }

    /// Set YOLO on every agent (the global YOLO toggle); each one restarts. Returns the updated states.
    pub fn set_global_yolo(&self, yolo: bool) -> Vec<AgentState> {
        let ids: Vec<String> = self
            .shared
            .inner
            .lock()
            .agents
            .iter()
            .map(|m| m.state.id.clone())
            .collect();
        ids.iter()
            .filter_map(|id| self.toggle_yolo(id, yolo))
            .collect()
    }

    /// Rename; for a running Claude agent also writes `/rename <name>\r`. Empty names are ignored.
    pub fn rename_agent(&self, id: &str, name: &str) -> Option<AgentState> {
        let mut inner = self.shared.inner.lock();
        let idx = inner.idx(id)?;
        let trimmed = name.trim();
        let m = &mut inner.agents[idx];
        if trimmed.is_empty() {
            return Some(m.state.clone());
        }
        m.state.name = trimmed.to_string();
        if m.state.provider == ProviderId::Claude
            && let Some(run) = &m.run
        {
            if run.gate.as_ref().is_some_and(TrustGate::holding) {
                // Enter would answer the trust dialog ("No, exit" is pre-selected): write it later.
                self.shared.spawn_startup(
                    &m.state.id,
                    run.run_id,
                    run.writer.clone(),
                    vec![(Duration::ZERO, StartupStep::Rename(trimmed.to_string()))],
                );
            } else {
                write_pty(&run.writer, format!("/rename {trimmed}\r").as_bytes());
            }
        }
        self.shared.emit_status(m);
        let state = m.state.clone();
        drop(inner);
        self.shared.emit(AgentEvent::Changed);
        Some(state)
    }

    pub fn set_model(&self, id: &str, model: &str) -> Option<AgentState> {
        let mut inner = self.shared.inner.lock();
        let idx = inner.idx(id)?;
        let m = &mut inner.agents[idx];
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return Some(m.state.clone());
        }
        m.state.model = trimmed.to_string();
        self.shared.emit_status(m);
        Some(m.state.clone())
    }

    pub fn set_session_id(&self, id: &str, session_id: &str) {
        let mut inner = self.shared.inner.lock();
        if let Some(idx) = inner.idx(id) {
            self.shared
                .update_session_id(&mut inner.agents[idx], session_id);
        }
    }

    // ── input ───────────────────────────────────────────────────────────────

    /// Submit a message: text, 100 ms, Enter (and for Claude another Enter after 100 ms).
    /// Inputs to one agent are serialized. Starts the process of a stopped/restored agent
    /// first (lazy spawn, like Hydra). False when the agent is unknown or cannot start.
    pub fn send_input(&self, id: &str, input: &str) -> bool {
        let mut inner = self.shared.inner.lock();
        let Some(idx) = inner.idx(id) else {
            return false;
        };
        if !self.shared.ensure_process(&mut inner, idx) {
            return false;
        }
        note_user_input(&mut inner.agents[idx], format!("{input}\r").as_bytes());
        self.shared.queue_locked(&mut inner, idx, input.to_string())
    }

    /// Write raw bytes (keystrokes) to the PTY, starting the process when needed.
    pub fn send_raw_input(&self, id: &str, data: &[u8]) -> bool {
        let writer = {
            let mut inner = self.shared.inner.lock();
            let Some(idx) = inner.idx(id) else {
                return false;
            };
            if !self.shared.ensure_process(&mut inner, idx) {
                return false;
            }
            note_user_input(&mut inner.agents[idx], data);
            let Some(run) = &inner.agents[idx].run else {
                return false;
            };
            run.writer.clone()
        };
        write_pty(&writer, data);
        true
    }

    /// Submit `input` to every agent of `project_dir`; returns the ids it reached.
    pub fn broadcast(&self, project_dir: &str, input: &str) -> Vec<String> {
        let mut inner = self.shared.inner.lock();
        let mut sent = Vec::new();
        for idx in 0..inner.agents.len() {
            if inner.agents[idx].state.project_dir != project_dir {
                continue;
            }
            if self.shared.ensure_process(&mut inner, idx) {
                // A submitted message ends with Enter.
                note_user_input(&mut inner.agents[idx], format!("{input}\r").as_bytes());
            }
            if inner.agents[idx].run.is_some()
                && self.shared.queue_locked(&mut inner, idx, input.to_string())
            {
                sent.push(inner.agents[idx].state.id.clone());
            }
        }
        sent
    }

    /// Resize the PTY (remembered for agents that are not running yet). Sizes below 2 are ignored.
    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        if cols < 2 || rows < 2 {
            return;
        }
        let mut inner = self.shared.inner.lock();
        let Some(idx) = inner.idx(id) else { return };
        let m = &mut inner.agents[idx];
        m.cols = cols;
        m.rows = rows;
        if let Some(run) = m.run.as_mut()
            && let Err(err) = run.control.resize(cols, rows)
        {
            tracing::debug!(%err, "PTY resize failed");
        }
    }

    // ── queries ─────────────────────────────────────────────────────────────

    pub fn list(&self) -> Vec<AgentState> {
        self.shared
            .inner
            .lock()
            .agents
            .iter()
            .map(|m| m.state.clone())
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<AgentState> {
        let inner = self.shared.inner.lock();
        inner.idx(id).map(|i| inner.agents[i].state.clone())
    }

    /// The raw output buffer (at most 2 MB, oldest bytes dropped first). Replay it into a
    /// terminal emulator before applying live [`AgentEvent::Output`].
    pub fn get_buffer(&self, id: &str) -> Vec<u8> {
        let inner = self.shared.inner.lock();
        inner
            .idx(id)
            .map(|i| inner.agents[i].output.to_vec())
            .unwrap_or_default()
    }

    /// The last `max_chars` bytes of output as text (never cut inside a UTF-8 character).
    pub fn get_recent_output(&self, id: &str, max_chars: usize) -> String {
        let inner = self.shared.inner.lock();
        inner
            .idx(id)
            .map(|i| inner.agents[i].output.recent_text(max_chars))
            .unwrap_or_default()
    }

    pub fn get_activity(&self, id: &str) -> Option<AgentActivity> {
        let inner = self.shared.inner.lock();
        inner.idx(id).map(|i| AgentActivity {
            last_output_at: inner.agents[i].last_output_at,
            last_exit_code: inner.agents[i].last_exit_code,
        })
    }

    /// Agents that have a process or are starting (what counts against the cap of 10).
    pub fn active_count(&self) -> usize {
        self.shared.inner.lock().count_active(None)
    }

    pub fn project_dirs(&self) -> Vec<String> {
        let inner = self.shared.inner.lock();
        let mut dirs: Vec<String> = Vec::new();
        for m in &inner.agents {
            if !dirs.contains(&m.state.project_dir) {
                dirs.push(m.state.project_dir.clone());
            }
        }
        dirs
    }

    // ── workspace persistence ───────────────────────────────────────────────

    /// Restore persisted agents as `idle` agents without a process; each one starts lazily on
    /// its first input (or restart). Returns how many were restored.
    pub fn hydrate_workspace_agents(&self, agents: &[PersistedWorkspaceAgent]) -> usize {
        let mut restored = 0;
        let mut inner = self.shared.inner.lock();
        for p in agents {
            if p.id.is_empty() || p.project_dir.is_empty() {
                continue;
            }
            if let Some(sid) = &p.session_id
                && inner
                    .agents
                    .iter()
                    .any(|m| m.state.session_id.as_deref() == Some(sid))
            {
                continue;
            }
            let created_at = if p.created_at.is_empty() {
                now_iso()
            } else {
                p.created_at.clone()
            };
            let state = AgentState {
                config: AgentConfig {
                    id: inner.unique_id(&p.id),
                    name: p.name.clone(),
                    project_dir: p.project_dir.clone(),
                    provider: p.provider.unwrap_or(ProviderId::Claude),
                    model: p.model.clone(),
                    reasoning_effort: None,
                    yolo: p.yolo,
                    is_manager: p.is_manager.unwrap_or(false),
                    session_id: p.session_id.clone(),
                    initial_prompt: String::new(),
                    created_at: created_at.clone(),
                    work_mode: p.work_mode.unwrap_or_default(),
                    worktree_path: p.worktree_path.clone(),
                    worktree_branch: p.worktree_branch.clone(),
                    claude_config_dir: p.claude_config_dir.clone(),
                },
                status: AgentStatus::Idle,
                pid: None,
                restart_count: 0,
                started_at: None,
                ended_at: p.ended_at.clone(),
                last_activity_at: p.last_activity_at.clone().unwrap_or(created_at),
            };
            inner.agents.push(Managed::new(state, None));
            restored += 1;
        }
        drop(inner);
        if restored > 0 {
            self.shared.emit(AgentEvent::Changed);
        }
        restored
    }

    /// Add catalog sessions as idle agents without a process (Hydra `importSessions`): a session
    /// already held by an agent, or without id/project, is skipped. Each agent resumes its
    /// session (`--resume <id>`) on its first input or restart, with the provider that created it.
    /// Imported agents are not persisted in the workspace (the catalog is the source of truth).
    /// Returns how many were added.
    pub fn import_sessions(
        &self,
        sessions: &[SessionSummary],
        default_model: &str,
        provider: ProviderId,
    ) -> usize {
        let mut inner = self.shared.inner.lock();
        let mut imported = 0;
        for session in sessions {
            if session.session_id.is_empty() || session.project_path.is_empty() {
                continue;
            }
            if inner
                .agents
                .iter()
                .any(|m| m.state.session_id.as_deref() == Some(session.session_id.as_str()))
            {
                continue;
            }
            let short: String = session.session_id.chars().take(8).collect();
            let created_at = if session.created_at.is_empty() {
                now_iso()
            } else {
                session.created_at.clone()
            };
            let modified = [&session.modified_at, &session.created_at]
                .into_iter()
                .find(|t| !t.is_empty())
                .cloned();
            let state = AgentState {
                config: AgentConfig {
                    id: inner.unique_id(&format!("sess-{short}")),
                    name: Self::generate_name(&session.first_prompt, &session.project_path),
                    project_dir: session.project_path.clone(),
                    provider,
                    model: default_model.to_string(),
                    reasoning_effort: None,
                    yolo: false,
                    is_manager: false,
                    session_id: Some(session.session_id.clone()),
                    initial_prompt: String::new(),
                    created_at: created_at.clone(),
                    work_mode: WorkMode::Local,
                    worktree_path: None,
                    worktree_branch: None,
                    claude_config_dir: None,
                },
                status: AgentStatus::Idle,
                pid: None,
                restart_count: 0,
                started_at: None,
                ended_at: modified.clone(),
                last_activity_at: modified.unwrap_or(created_at),
            };
            let prompt = session.first_prompt.trim();
            let mut managed = Managed::new(state, (!prompt.is_empty()).then(|| prompt.to_string()));
            managed.imported = true;
            inner.agents.push(managed);
            imported += 1;
        }
        drop(inner);
        if imported > 0 {
            self.shared.emit(AgentEvent::Changed);
        }
        imported
    }

    /// Agents to persist. `ending_now` stamps still-running agents as ended now
    /// (daemon shutdown, before their PTYs are killed).
    pub fn export_workspace_agents(&self, ending_now: bool) -> Vec<PersistedWorkspaceAgent> {
        let mut inner = self.shared.inner.lock();
        if ending_now {
            let now = now_iso();
            for m in inner.agents.iter_mut().filter(|m| m.run.is_some()) {
                m.state.ended_at = Some(now.clone());
            }
        }
        inner
            .agents
            .iter()
            .filter(|m| !m.imported)
            .map(|m| PersistedWorkspaceAgent {
                id: m.state.id.clone(),
                name: m.state.name.clone(),
                project_dir: m.state.project_dir.clone(),
                provider: Some(m.state.provider),
                model: m.state.model.clone(),
                yolo: m.state.yolo,
                is_manager: Some(m.state.is_manager),
                session_id: m.state.session_id.clone(),
                created_at: m.state.created_at.clone(),
                last_activity_at: Some(m.state.last_activity_at.clone()),
                ended_at: m.state.ended_at.clone(),
                work_mode: Some(m.state.work_mode),
                worktree_path: m.state.worktree_path.clone(),
                worktree_branch: m.state.worktree_branch.clone(),
                claude_config_dir: m.state.claude_config_dir.clone(),
            })
            .collect()
    }

    // ── shutdown ────────────────────────────────────────────────────────────

    /// Stop idle polling and kill every agent (SIGTERM, SIGKILL for stragglers after grace + 1 s).
    /// Use [`AgentManager::wait_all_stopped`] to wait for the processes to go away.
    pub fn kill_all(&self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        {
            let mut inner = self.shared.inner.lock();
            for m in &mut inner.agents {
                self.shared.kill_locked(m);
            }
        }
        let shared = self.shared.clone();
        self.shared.handle.spawn(async move {
            tokio::time::sleep(shared.kill_grace + Duration::from_secs(1)).await;
            let mut inner = shared.inner.lock();
            for run in inner.agents.iter_mut().filter_map(|m| m.run.as_mut()) {
                run.control.terminate(true);
            }
        });
    }

    /// Wait until no agent has a process; false on timeout.
    pub async fn wait_all_stopped(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if !self
                .shared
                .inner
                .lock()
                .agents
                .iter()
                .any(|m| m.run.is_some())
            {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Run the idle ("waiting") check as of `now_ms` (epoch ms). Called by the 2 s timer; public for tests.
    pub fn poll_activity_at(&self, now_ms: u64) {
        self.shared.poll_activity_at(now_ms);
    }

    // ── helpers ─────────────────────────────────────────────────────────────

    /// A display name from a prompt (first 44 chars) or `<project dir name> <6 random hex>`.
    pub fn generate_name(prompt: &str, project_dir: &str) -> String {
        let collapsed = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        let cleaned = LEADING_TAG.replace(&collapsed, "").into_owned();
        if !cleaned.is_empty() {
            return if cleaned.chars().count() > 44 {
                format!("{}...", cleaned.chars().take(44).collect::<String>())
            } else {
                cleaned
            };
        }
        let project = Path::new(project_dir)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Agent".to_string());
        format!("{project} {}", &uuid::Uuid::new_v4().to_string()[..6])
    }
}

impl Default for AgentManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::fake_pty::FakePtyBackend;
    use pretty_assertions::assert_eq;

    type ProfileCalls = Vec<(String, Option<String>, Option<String>)>;

    /// Startup gate timing small enough for tests.
    fn fast_startup() -> StartupOptions {
        StartupOptions {
            trust_settle: Duration::from_millis(80),
            trust_timeout: Duration::from_secs(30),
            poll: Duration::from_millis(10),
            no_output_grace: Duration::from_millis(50),
        }
    }

    fn manager_with(backend: &FakePtyBackend) -> AgentManager {
        AgentManager::with_options(ManagerOptions {
            backend: Arc::new(backend.clone()),
            kill_grace: Duration::from_millis(80),
            startup: fast_startup(),
            ..Default::default()
        })
    }

    fn payload() -> CreateAgentPayload {
        CreateAgentPayload {
            name: "EP".into(),
            project_dir: "/tmp/project".into(),
            provider: ProviderId::Claude,
            model: "sonnet".into(),
            ..Default::default()
        }
    }

    async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        for _ in 0..600 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("timed out waiting for: {what}");
    }

    fn drain(rx: &mut broadcast::Receiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    fn waiting_count(rx: &mut broadcast::Receiver<AgentEvent>) -> usize {
        drain(rx)
            .iter()
            .filter(|e| matches!(e, AgentEvent::Waiting { .. }))
            .count()
    }

    fn statuses(events: &[AgentEvent]) -> Vec<AgentStatus> {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Status { status, .. } => Some(*status),
                _ => None,
            })
            .collect()
    }

    fn persisted(id: &str, session: &str, ended: Option<&str>) -> PersistedWorkspaceAgent {
        PersistedWorkspaceAgent {
            id: id.into(),
            name: format!("name-{id}"),
            project_dir: "/tmp/project".into(),
            provider: None,
            model: "sonnet".into(),
            yolo: false,
            is_manager: None,
            session_id: Some(session.into()),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            last_activity_at: None,
            ended_at: ended.map(str::to_string),
            work_mode: None,
            worktree_path: None,
            worktree_branch: None,
            claude_config_dir: None,
        }
    }

    #[tokio::test]
    async fn create_spawns_with_env_cwd_size_and_status_running() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager.create(payload()).await.unwrap();
        assert_eq!(created.id.len(), 8);
        assert_eq!(created.status, AgentStatus::Running);
        assert_eq!(created.pid, Some(1234));
        assert!(created.started_at.is_some());
        let req = backend.request(0).unwrap();
        assert_eq!((req.cols, req.rows), (120, 30));
        assert_eq!(req.cwd, "/tmp/project");
        assert_eq!(req.cmd, "claude");
        assert_eq!(req.args, ["--model", "sonnet"]);
        assert_eq!(req.env["TERM"], "xterm-256color");
        assert_eq!(req.env["FORCE_COLOR"], "1");
        assert!(!req.env.contains_key("CLAUDE_CODE_CHILD_SESSION"));
    }

    /// A repository with one commit, isolated from the user's git configuration.
    fn temp_repo() -> Option<tempfile::TempDir> {
        if which::which("git").is_err() {
            eprintln!("skipped: git is not installed");
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(tmp.path())
                .args([
                    "-c",
                    "init.defaultBranch=main",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .env(
                    "GIT_CONFIG_GLOBAL",
                    std::env::temp_dir().join("as-no-such-gitconfig"),
                )
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "T")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "T")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        std::fs::write(tmp.path().join("a.txt"), "a\n").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-q", "-m", "init"]);
        Some(tmp)
    }

    #[test]
    fn worktree_branch_names_are_slugged_and_prefixed() {
        assert_eq!(
            worktree_branch_name("My Agent", "ab12cd34"),
            "studio/my-agent-ab12cd34"
        );
        assert_eq!(
            worktree_branch_name("  Fix: the ~bug?? ", "id"),
            "studio/fix-the-bug-id"
        );
        assert_eq!(worktree_branch_name("", "id"), "studio/agent-id");
        assert_eq!(worktree_branch_name("***", "id"), "studio/agent-id");
        assert!(worktree_branch_name(&"x".repeat(100), "id").len() < 60);
    }

    #[tokio::test]
    async fn worktree_mode_runs_the_agent_in_a_new_worktree() {
        let Some(repo) = temp_repo() else { return };
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager
            .create(CreateAgentPayload {
                name: "Wt Agent".into(),
                project_dir: repo.path().to_string_lossy().into_owned(),
                work_mode: WorkMode::Worktree,
                ..payload()
            })
            .await
            .unwrap();
        let branch = created.worktree_branch.clone().unwrap();
        assert_eq!(branch, format!("studio/wt-agent-{}", created.id));
        let wt = std::path::PathBuf::from(created.worktree_path.clone().unwrap());
        assert!(wt.join("a.txt").is_file());
        assert_eq!(created.work_mode, WorkMode::Worktree);
        assert_eq!(created.project_dir, wt.to_string_lossy());
        assert_eq!(backend.request(0).unwrap().cwd, wt.to_string_lossy());

        // A clean worktree goes away with the agent, branch included.
        assert!(manager.remove(&created.id).await);
        wait_for("worktree removed", || !wt.exists()).await;
        let repo_path = repo.path().to_path_buf();
        wait_for("branch deleted", || {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo_path)
                .args(["branch", "--list", &branch])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().is_empty()
        })
        .await;
    }

    #[tokio::test]
    async fn removing_an_agent_keeps_a_worktree_with_uncommitted_work() {
        let Some(repo) = temp_repo() else { return };
        let manager = manager_with(&FakePtyBackend::new());
        let created = manager
            .create(CreateAgentPayload {
                project_dir: repo.path().to_string_lossy().into_owned(),
                work_mode: WorkMode::Worktree,
                ..payload()
            })
            .await
            .unwrap();
        let wt = std::path::PathBuf::from(created.worktree_path.clone().unwrap());
        std::fs::write(wt.join("wip.txt"), "unsaved work\n").unwrap();
        assert!(manager.remove(&created.id).await);
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(
            wt.join("wip.txt").is_file(),
            "uncommitted work must survive"
        );
        assert!(manager.list().is_empty());
    }

    #[tokio::test]
    async fn worktree_mode_outside_a_repository_is_an_error() {
        if which::which("git").is_err() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let manager = manager_with(&FakePtyBackend::new());
        let err = manager
            .create(CreateAgentPayload {
                project_dir: tmp.path().to_string_lossy().into_owned(),
                work_mode: WorkMode::Worktree,
                ..payload()
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ManagerError::Worktree(_)), "{err}");
        assert!(manager.list().is_empty());
    }

    #[tokio::test]
    async fn splits_submitted_input_into_text_and_delayed_carriage_returns() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let agent = manager.create(payload()).await.unwrap();
        assert!(manager.send_input(&agent.id, "Hi Claude"));
        let pty = backend.ptys()[0].clone();
        wait_for("text", || pty.written() == "Hi Claude").await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            pty.written(),
            "Hi Claude",
            "Enter must not come before 100 ms"
        );
        wait_for("first Enter", || pty.written() == "Hi Claude\r").await;
        wait_for("claude paste-confirm Enter", || {
            pty.written() == "Hi Claude\r\r"
        })
        .await;
    }

    #[tokio::test]
    async fn codex_gets_a_single_enter_and_inputs_are_serialized() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let agent = manager
            .create(CreateAgentPayload {
                provider: ProviderId::Codex,
                model: "gpt-5.4".into(),
                ..payload()
            })
            .await
            .unwrap();
        manager.send_input(&agent.id, "one");
        manager.send_input(&agent.id, "two");
        let pty = backend.ptys()[0].clone();
        wait_for("both inputs", || pty.written() == "one\rtwo\r").await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(pty.written(), "one\rtwo\r");
    }

    #[tokio::test]
    async fn raw_input_is_written_verbatim() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let agent = manager.create(payload()).await.unwrap();
        assert!(manager.send_raw_input(&agent.id, b"\x1b[A"));
        assert_eq!(backend.ptys()[0].written(), "\u{1b}[A");
        assert!(!manager.send_raw_input("missing", b"x"));
    }

    #[tokio::test]
    async fn claude_name_is_written_as_rename_and_initial_prompt_follows() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let agent = manager
            .create(CreateAgentPayload {
                initial_prompt: "do it".into(),
                ..payload()
            })
            .await
            .unwrap();
        let pty = backend.ptys()[0].clone();
        assert_eq!(pty.written(), "");
        wait_for("/rename", || pty.written().starts_with("/rename EP\r")).await;
        wait_for("initial prompt (3 s after a rename)", || {
            pty.written().contains("do it")
        })
        .await;
        assert!(manager.get(&agent.id).unwrap().initial_prompt.is_empty());
    }

    #[tokio::test]
    async fn rename_writes_slash_rename_for_running_claude() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let agent = manager.create(payload()).await.unwrap();
        let mut rx = manager.subscribe();
        assert_eq!(
            manager.rename_agent(&agent.id, "  New name ").unwrap().name,
            "New name"
        );
        assert_eq!(backend.ptys()[0].written(), "/rename New name\r");
        assert_eq!(
            manager.rename_agent(&agent.id, "   ").unwrap().name,
            "New name"
        );
        let events = drain(&mut rx);
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Changed)));
        assert!(manager.rename_agent("missing", "x").is_none());
    }

    #[tokio::test]
    async fn output_is_buffered_as_raw_bytes_and_broadcast() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let agent = manager.create(payload()).await.unwrap();
        let mut rx = manager.subscribe();
        let pty = backend.ptys()[0].clone();
        pty.emit_data(b"he");
        pty.emit_data(&[0xC3]); // half of "é": raw bytes are kept as they come
        pty.emit_data(&[0xA9, b'!']);
        wait_for("buffer", || manager.get_buffer(&agent.id).len() == 5).await;
        assert_eq!(manager.get_buffer(&agent.id), "heé!".as_bytes());
        assert_eq!(manager.get_recent_output(&agent.id, 100), "heé!");
        let outputs: Vec<u8> = drain(&mut rx)
            .into_iter()
            .filter_map(|e| match e {
                AgentEvent::Output { data, .. } => Some(data.to_vec()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(outputs, "heé!".as_bytes());
        assert!(manager.get_activity(&agent.id).unwrap().last_output_at > 0);
    }

    #[tokio::test]
    async fn updates_codex_model_from_terminal_output() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager
            .create(CreateAgentPayload {
                provider: ProviderId::Codex,
                model: "gpt-5.3-codex".into(),
                ..payload()
            })
            .await
            .unwrap();
        let mut rx = manager.subscribe();
        backend.ptys()[0].emit_data(b"Model changed to gpt-5.4 high");
        wait_for("model", || {
            manager.get(&created.id).unwrap().model == "gpt-5.4"
        })
        .await;
        let models: Vec<String> = drain(&mut rx)
            .into_iter()
            .filter_map(|e| match e {
                AgentEvent::Status { model, .. } => Some(model),
                _ => None,
            })
            .collect();
        assert!(models.contains(&"gpt-5.4".to_string()));
    }

    #[tokio::test]
    async fn strips_codex_alternate_screen_sequences_even_when_split() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager
            .create(CreateAgentPayload {
                provider: ProviderId::Codex,
                model: "gpt-5.4".into(),
                ..payload()
            })
            .await
            .unwrap();
        let pty = backend.ptys()[0].clone();
        pty.emit_data(b"\x1b[?1049hhello\x1b[?1049l");
        wait_for("hello", || manager.get_buffer(&created.id) == b"hello").await;
        pty.emit_data(b" wor\x1b[?10");
        pty.emit_data(b"47lld");
        wait_for("split", || {
            manager.get_buffer(&created.id) == b"hello world"
        })
        .await;
    }

    #[tokio::test]
    async fn claude_output_keeps_alt_screen_sequences() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager.create(payload()).await.unwrap();
        backend.ptys()[0].emit_data(b"\x1b[?1049hx");
        wait_for("raw", || manager.get_buffer(&created.id) == b"\x1b[?1049hx").await;
    }

    #[tokio::test]
    async fn captures_session_id_from_output_once() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager.create(payload()).await.unwrap();
        let pty = backend.ptys()[0].clone();
        pty.emit_data(b"Session ID: 5f1c9a2e-77aa-4b1c-9d3e-0123456789ab");
        wait_for("sid", || {
            manager.get(&created.id).unwrap().session_id.is_some()
        })
        .await;
        assert_eq!(
            manager.get(&created.id).unwrap().session_id.as_deref(),
            Some("5f1c9a2e-77aa-4b1c-9d3e-0123456789ab")
        );
        pty.emit_data(b"session id: ffffffff-ffff-4fff-8fff-ffffffffffff");
        pty.emit_data(b"x");
        wait_for("drain", || manager.get_buffer(&created.id).ends_with(b"x")).await;
        assert_eq!(
            manager.get(&created.id).unwrap().session_id.as_deref(),
            Some("5f1c9a2e-77aa-4b1c-9d3e-0123456789ab")
        );
    }

    #[tokio::test]
    async fn codex_uses_resume_on_restart_once_a_session_id_is_known() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager
            .create(CreateAgentPayload {
                provider: ProviderId::Codex,
                model: "gpt-5.3-codex".into(),
                ..payload()
            })
            .await
            .unwrap();
        manager.set_session_id(&created.id, "codex-session-01");
        manager.restart(&created.id).unwrap();
        assert_eq!(backend.spawn_count(), 2);
        assert_eq!(
            backend.request(1).unwrap().args,
            ["resume", "codex-session-01", "--model", "gpt-5.3-codex"]
        );
    }

    #[tokio::test]
    async fn ignores_stale_exit_callbacks_from_replaced_ptys() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager
            .create(CreateAgentPayload {
                name: "race-check".into(),
                ..payload()
            })
            .await
            .unwrap();
        let restarted = manager.restart(&created.id).unwrap();
        assert_eq!(restarted.restart_count, 1);
        assert_eq!(backend.spawn_count(), 2);
        let (first, second) = (backend.ptys()[0].clone(), backend.ptys()[1].clone());
        assert_eq!(
            first.signals(),
            [true],
            "restart force-kills the old process"
        );
        first.emit_exit(137);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            manager.get(&created.id).unwrap().status,
            AgentStatus::Running
        );
        assert!(manager.send_input(&created.id, "still attached"));
        wait_for("write to new pty", || {
            second.written().starts_with("still attached")
        })
        .await;
        assert_eq!(first.written(), "");
    }

    #[tokio::test]
    async fn force_kill_timeout_does_not_terminate_a_restarted_pty() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager.create(payload()).await.unwrap();
        assert!(manager.kill(&created.id));
        manager.restart(&created.id).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await; // > kill_grace
        let (first, second) = (backend.ptys()[0].clone(), backend.ptys()[1].clone());
        assert_eq!(first.signals(), [false, true]);
        assert_eq!(second.signals(), Vec::<bool>::new());
    }

    #[tokio::test]
    async fn kill_is_graceful_then_forced_after_the_grace_period() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager.create(payload()).await.unwrap();
        assert!(manager.kill(&created.id));
        let pty = backend.ptys()[0].clone();
        assert_eq!(pty.signals(), [false]);
        wait_for("force", || pty.signals() == [false, true]).await;
        assert!(!manager.kill("missing"));
    }

    #[tokio::test]
    async fn explicit_stop_is_idle_even_with_non_zero_exit() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager
            .create(CreateAgentPayload {
                name: "stop-check".into(),
                ..payload()
            })
            .await
            .unwrap();
        let mut rx = manager.subscribe();
        manager.kill(&created.id);
        backend.ptys()[0].emit_exit(1);
        wait_for("idle", || {
            manager.get(&created.id).unwrap().status == AgentStatus::Idle
        })
        .await;
        let seen = statuses(&drain(&mut rx));
        assert_eq!(seen.last(), Some(&AgentStatus::Idle));
        assert!(!seen.contains(&AgentStatus::Errored));
    }

    #[tokio::test]
    async fn exit_zero_is_idle_and_non_zero_is_errored() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let a = manager.create(payload()).await.unwrap();
        let b = manager.create(payload()).await.unwrap();
        backend.ptys()[0].emit_exit(0);
        backend.ptys()[1].emit_exit(3);
        wait_for("a", || {
            manager.get(&a.id).unwrap().status == AgentStatus::Idle
        })
        .await;
        wait_for("b", || {
            manager.get(&b.id).unwrap().status == AgentStatus::Errored
        })
        .await;
        assert_eq!(manager.get_activity(&b.id).unwrap().last_exit_code, Some(3));
        assert_eq!(manager.get(&b.id).unwrap().pid, None);
    }

    #[tokio::test]
    async fn tracks_exit_code_in_activity_and_resets_it_on_restart() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager.create(payload()).await.unwrap();
        assert_eq!(
            manager.get_activity(&created.id),
            Some(AgentActivity {
                last_output_at: 0,
                last_exit_code: None
            })
        );
        assert_eq!(manager.get_activity("missing"), None);
        backend.ptys()[0].emit_data(b"hello");
        wait_for("output", || {
            manager.get_activity(&created.id).unwrap().last_output_at > 0
        })
        .await;
        backend.ptys()[0].emit_exit(3);
        wait_for("exit", || {
            manager.get_activity(&created.id).unwrap().last_exit_code == Some(3)
        })
        .await;
        manager.restart(&created.id);
        assert_eq!(
            manager.get_activity(&created.id),
            Some(AgentActivity {
                last_output_at: 0,
                last_exit_code: None
            })
        );
        assert!(
            manager.get_buffer(&created.id).is_empty(),
            "restart clears the buffer"
        );
    }

    #[tokio::test]
    async fn restart_announces_itself_before_the_new_process_writes() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager.create(payload()).await.unwrap();
        backend.ptys()[0].emit_data(b"old screen");
        wait_for("old output", || {
            manager.get_buffer(&created.id) == b"old screen"
        })
        .await;
        let mut rx = manager.subscribe();
        drain(&mut rx);
        manager.restart(&created.id);
        wait_for("new pty", || backend.ptys().len() == 2).await;
        backend.ptys()[1].emit_data(b"fresh");
        wait_for("new output", || manager.get_buffer(&created.id) == b"fresh").await;
        let events = drain(&mut rx);
        let restarted = events
            .iter()
            .position(
                |e| matches!(e, AgentEvent::Restarted { agent_id } if *agent_id == created.id),
            )
            .expect("a Restarted event");
        let first_output = events
            .iter()
            .position(|e| matches!(e, AgentEvent::Output { .. }))
            .expect("the new output");
        assert!(restarted < first_output, "{events:?}");
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AgentEvent::Restarted { .. }))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn stamps_ended_at_on_exit_and_clears_it_on_respawn() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let created = manager.create(payload()).await.unwrap();
        assert_eq!(created.ended_at, None);
        backend.ptys()[0].emit_exit(0);
        wait_for("ended", || {
            manager.get(&created.id).unwrap().ended_at.is_some()
        })
        .await;
        manager.restart(&created.id);
        assert_eq!(manager.get(&created.id).unwrap().ended_at, None);
    }

    #[tokio::test]
    async fn stamps_ended_at_when_the_pty_fails_to_spawn() {
        let backend = FakePtyBackend::new();
        backend.set_fail(Some("boom"));
        let manager = manager_with(&backend);
        let created = manager
            .create(CreateAgentPayload {
                name: "spawn-fail".into(),
                ..payload()
            })
            .await
            .unwrap();
        assert_eq!(created.status, AgentStatus::Errored);
        assert!(manager.get(&created.id).unwrap().ended_at.is_some());
        assert_eq!(manager.active_count(), 0);
    }

    #[tokio::test]
    async fn hydrated_agents_are_idle_without_a_process_and_start_lazily() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let mut rx = manager.subscribe();
        let n = manager.hydrate_workspace_agents(&[
            persisted("a1", "sess-1", Some("2026-01-02T00:00:00.000Z")),
            persisted("a2", "sess-2", None),
        ]);
        assert_eq!(n, 2);
        assert!(
            drain(&mut rx)
                .iter()
                .any(|e| matches!(e, AgentEvent::Changed))
        );
        assert_eq!(backend.spawn_count(), 0);
        let by_name: HashMap<String, AgentState> = manager
            .list()
            .into_iter()
            .map(|a| (a.name.clone(), a))
            .collect();
        assert_eq!(by_name["name-a1"].status, AgentStatus::Idle);
        assert_eq!(
            by_name["name-a1"].ended_at.as_deref(),
            Some("2026-01-02T00:00:00.000Z")
        );
        assert_eq!(by_name["name-a2"].ended_at, None);

        let exported = manager.export_workspace_agents(false);
        assert_eq!(exported.len(), 2);
        assert_eq!(
            exported[0].ended_at.as_deref(),
            Some("2026-01-02T00:00:00.000Z")
        );
        assert_eq!(exported[1].ended_at, None);

        // First input spawns it, resuming the persisted session.
        assert!(manager.send_input("a1", "wake up"));
        assert_eq!(
            backend.request(0).unwrap().args,
            ["--model", "sonnet", "--resume", "sess-1"]
        );
        assert_eq!(manager.get("a1").unwrap().status, AgentStatus::Running);
    }

    #[tokio::test]
    async fn hydrate_skips_known_sessions_and_dedupes_ids() {
        let manager = manager_with(&FakePtyBackend::new());
        manager.hydrate_workspace_agents(&[persisted("a1", "s1", None)]);
        let n = manager
            .hydrate_workspace_agents(&[persisted("a1", "s1", None), persisted("a1", "s2", None)]);
        assert_eq!(n, 1);
        let ids: Vec<String> = manager.list().into_iter().map(|a| a.config.id).collect();
        assert_eq!(ids, ["a1", "a1-2"]);
    }

    #[tokio::test]
    async fn export_ending_now_stamps_running_agents_only() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        manager.hydrate_workspace_agents(&[persisted(
            "stopped",
            "s",
            Some("2026-01-02T00:00:00.000Z"),
        )]);
        let running = manager.create(payload()).await.unwrap();
        let plain = manager.export_workspace_agents(false);
        assert_eq!(
            plain.iter().find(|a| a.id == running.id).unwrap().ended_at,
            None
        );
        let ending = manager.export_workspace_agents(true);
        assert!(
            ending
                .iter()
                .find(|a| a.id == running.id)
                .unwrap()
                .ended_at
                .is_some()
        );
        assert_eq!(
            ending
                .iter()
                .find(|a| a.id == "stopped")
                .unwrap()
                .ended_at
                .as_deref(),
            Some("2026-01-02T00:00:00.000Z")
        );
    }

    #[tokio::test]
    async fn enforces_the_hard_cap_and_remove_frees_a_slot() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let mut ids = Vec::new();
        for i in 0..MAX_CONCURRENT_AGENTS_HARD_LIMIT {
            let a = manager
                .create(CreateAgentPayload {
                    name: format!("agent-{i}"),
                    ..payload()
                })
                .await
                .unwrap();
            assert_eq!(a.status, AgentStatus::Running);
            ids.push(a.id.clone());
        }
        for _ in 0..20 {
            let err = manager.create(payload()).await.unwrap_err();
            assert_eq!(err.to_string(), "Maximum concurrent agents (10) reached");
            assert_eq!(manager.active_count(), MAX_CONCURRENT_AGENTS_HARD_LIMIT);
        }
        assert_eq!(backend.spawn_count(), MAX_CONCURRENT_AGENTS_HARD_LIMIT);
        assert!(manager.remove(&ids[0]).await);
        assert!(!manager.remove(&ids[0]).await);
        assert_eq!(manager.active_count(), MAX_CONCURRENT_AGENTS_HARD_LIMIT - 1);
        assert_eq!(
            manager.create(payload()).await.unwrap().status,
            AgentStatus::Running
        );
        // the removed agent's process was asked to stop, then forced
        let removed = backend.ptys()[0].clone();
        assert_eq!(removed.signals().first(), Some(&false));
        wait_for("forced", || removed.signals() == [false, true]).await;
    }

    #[tokio::test]
    async fn idle_agents_do_not_auto_start_at_the_cap_and_restart_is_blocked() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        for _ in 0..MAX_CONCURRENT_AGENTS_HARD_LIMIT {
            manager.create(payload()).await.unwrap();
        }
        manager.hydrate_workspace_agents(&[persisted("idle1", "s1", None)]);
        assert!(!manager.send_input("idle1", "start please"));
        assert!(!manager.send_raw_input("idle1", b"x"));
        assert_eq!(backend.spawn_count(), MAX_CONCURRENT_AGENTS_HARD_LIMIT);
        let restarted = manager.restart("idle1").unwrap();
        assert_eq!(restarted.status, AgentStatus::Idle);
        assert_eq!(manager.get("idle1").unwrap().status, AgentStatus::Idle);
        assert_eq!(backend.spawn_count(), MAX_CONCURRENT_AGENTS_HARD_LIMIT);
    }

    #[tokio::test]
    async fn restart_of_a_running_agent_is_allowed_at_the_cap() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let mut first = String::new();
        for i in 0..MAX_CONCURRENT_AGENTS_HARD_LIMIT {
            let a = manager.create(payload()).await.unwrap();
            if i == 0 {
                first = a.id.clone();
            }
        }
        assert!(manager.restart(&first).is_some());
        assert_eq!(backend.spawn_count(), MAX_CONCURRENT_AGENTS_HARD_LIMIT + 1);
        assert!(manager.restart("missing").is_none());
    }

    #[tokio::test]
    async fn sends_an_initial_enter_when_restarting_an_idle_session_without_prompt() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        manager.hydrate_workspace_agents(&[persisted("idle1", "s1", None)]);
        manager.restart("idle1").unwrap();
        let pty = backend.ptys()[0].clone();
        wait_for("wake enter", || pty.written() == "\r").await;
    }

    #[tokio::test]
    async fn applies_the_claude_profile_to_claude_agents_only() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let calls: Arc<Mutex<ProfileCalls>> = Arc::default();
        let seen = calls.clone();
        manager.set_claude_profile_resolver(move |dir, sid, explicit| {
            seen.lock().push((
                dir.to_string(),
                sid.map(str::to_string),
                explicit.map(str::to_string),
            ));
            Some("/home/me/.claude-work".into())
        });
        manager.create(payload()).await.unwrap();
        assert_eq!(*calls.lock(), [("/tmp/project".to_string(), None, None)]);
        assert_eq!(
            backend.env_of(0)["CLAUDE_CONFIG_DIR"],
            "/home/me/.claude-work"
        );

        manager
            .create(CreateAgentPayload {
                provider: ProviderId::Codex,
                model: "gpt-5.3-codex".into(),
                ..payload()
            })
            .await
            .unwrap();
        assert_eq!(calls.lock().len(), 1, "resolver is not called for codex");
        assert_eq!(
            backend.env_of(1).get("CLAUDE_CONFIG_DIR"),
            std::env::var("CLAUDE_CONFIG_DIR").ok().as_ref()
        );
    }

    #[tokio::test]
    async fn explicit_claude_config_dir_is_passed_to_the_resolver_and_persisted() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let calls: Arc<Mutex<ProfileCalls>> = Arc::default();
        let seen = calls.clone();
        manager.set_claude_profile_resolver(move |dir, sid, explicit| {
            seen.lock().push((
                dir.to_string(),
                sid.map(str::to_string),
                explicit.map(str::to_string),
            ));
            explicit.map(str::to_string)
        });
        let created = manager
            .create(CreateAgentPayload {
                claude_config_dir: Some("/home/me/.claude-alternative".into()),
                ..payload()
            })
            .await
            .unwrap();
        assert_eq!(
            created.claude_config_dir.as_deref(),
            Some("/home/me/.claude-alternative")
        );
        assert_eq!(
            calls.lock()[0].2.as_deref(),
            Some("/home/me/.claude-alternative")
        );
        assert_eq!(
            backend.env_of(0)["CLAUDE_CONFIG_DIR"],
            "/home/me/.claude-alternative"
        );

        // Persisted, and kept after hydrating a fresh manager and restarting.
        let saved = manager.export_workspace_agents(false);
        assert_eq!(
            saved[0].claude_config_dir.as_deref(),
            Some("/home/me/.claude-alternative")
        );
        let backend2 = FakePtyBackend::new();
        let manager2 = manager_with(&backend2);
        manager2.set_claude_profile_resolver(|_, _, explicit| explicit.map(str::to_string));
        manager2.hydrate_workspace_agents(&saved);
        manager2.restart(&saved[0].id).unwrap();
        assert_eq!(
            backend2.env_of(0)["CLAUDE_CONFIG_DIR"],
            "/home/me/.claude-alternative"
        );
    }

    #[test]
    fn old_workspace_entries_and_payloads_load_without_the_field() {
        let a: PersistedWorkspaceAgent = serde_json::from_value(serde_json::json!({
            "id": "a", "name": "n", "projectDir": "/p", "model": "sonnet",
            "yolo": false, "createdAt": "2026-01-01T00:00:00.000Z"
        }))
        .unwrap();
        assert_eq!(a.claude_config_dir, None);
        let p: CreateAgentPayload =
            serde_json::from_value(serde_json::json!({"name": "n"})).unwrap();
        assert_eq!(p.claude_config_dir, None);
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("claudeConfigDir").is_none());
    }

    #[tokio::test]
    async fn shell_config_provider_wraps_the_command() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        manager.set_shell_config_provider(|| ShellConfig {
            mode: crate::types::TerminalShellMode::Custom,
            path: "/usr/bin/fish".into(),
            args: "-lc".into(),
        });
        manager.create(payload()).await.unwrap();
        let req = backend.request(0).unwrap();
        assert!(req.cmd == "/usr/bin/fish" || cfg!(windows));
        assert!(req.args.last().unwrap().contains("--model sonnet") || cfg!(windows));
    }

    #[tokio::test]
    async fn resize_is_remembered_for_idle_agents_and_forwarded_to_running_ones() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        manager.hydrate_workspace_agents(&[persisted("idle1", "s1", None)]);
        manager.resize("idle1", 96, 28);
        manager.resize("idle1", 1, 28); // ignored
        assert!(manager.send_input("idle1", "Start"));
        let req = backend.request(0).unwrap();
        assert_eq!(
            (req.cols, req.rows, req.cwd.as_str()),
            (96, 28, "/tmp/project")
        );
        manager.resize("idle1", 100, 40);
        assert_eq!(backend.ptys()[0].resizes(), [(100, 40)]);
    }

    #[tokio::test]
    async fn broadcast_reaches_every_agent_of_the_project() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let a = manager.create(payload()).await.unwrap();
        let b = manager.create(payload()).await.unwrap();
        let other = manager
            .create(CreateAgentPayload {
                project_dir: "/tmp/other".into(),
                ..payload()
            })
            .await
            .unwrap();
        let mut sent = manager.broadcast("/tmp/project", "hello all");
        sent.sort();
        let mut expected = vec![a.id.clone(), b.id.clone()];
        expected.sort();
        assert_eq!(sent, expected);
        let ptys = backend.ptys();
        wait_for("both", || {
            ptys[0].written().starts_with("hello all") && ptys[1].written().starts_with("hello all")
        })
        .await;
        assert_eq!(ptys[2].written(), "");
        assert_eq!(manager.project_dirs(), ["/tmp/project", "/tmp/other"]);
        let _ = other;
    }

    #[tokio::test]
    async fn toggle_yolo_restarts_with_the_flag() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let a = manager.create(payload()).await.unwrap();
        let state = manager.toggle_yolo(&a.id, true).unwrap();
        assert!(state.yolo);
        assert_eq!(
            backend.request(1).unwrap().args,
            ["--dangerously-skip-permissions", "--model", "sonnet"]
        );
        let all = manager.set_global_yolo(false);
        assert_eq!(all.len(), 1);
        assert_eq!(backend.request(2).unwrap().args, ["--model", "sonnet"]);
    }

    #[tokio::test]
    async fn detects_waiting_once_until_the_next_input() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let a = manager.create(payload()).await.unwrap();
        let mut rx = manager.subscribe();
        manager.poll_activity_at(now_ms() + 60_000);
        assert_eq!(waiting_count(&mut rx), 0, "no output yet");

        backend.ptys()[0].emit_data(b"done.");
        wait_for("output", || {
            manager.get_activity(&a.id).unwrap().last_output_at > 0
        })
        .await;
        manager.poll_activity_at(now_ms() + 1_000);
        assert_eq!(waiting_count(&mut rx), 0, "under 5 s");
        manager.poll_activity_at(now_ms() + 6_000);
        manager.poll_activity_at(now_ms() + 7_000);
        let waiting: Vec<_> = drain(&mut rx)
            .into_iter()
            .filter(|e| matches!(e, AgentEvent::Waiting { .. }))
            .collect();
        assert_eq!(waiting.len(), 1);

        manager.send_input(&a.id, "next");
        manager.poll_activity_at(now_ms() + 6_000);
        assert_eq!(
            drain(&mut rx)
                .iter()
                .filter(|e| matches!(e, AgentEvent::Waiting { .. }))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn set_model_and_kill_all() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let a = manager.create(payload()).await.unwrap();
        assert_eq!(manager.set_model(&a.id, " opus ").unwrap().model, "opus");
        assert_eq!(manager.set_model(&a.id, " ").unwrap().model, "opus");
        manager.kill_all();
        assert_eq!(backend.ptys()[0].signals(), [false]);
        backend.ptys()[0].emit_exit(0);
        assert!(manager.wait_all_stopped(Duration::from_secs(2)).await);
        assert_eq!(manager.get(&a.id).unwrap().status, AgentStatus::Idle);
    }

    #[test]
    fn generate_name_matches_hydra() {
        assert_eq!(
            AgentManager::generate_name("  fix   the\nlogin bug ", "/p/api"),
            "fix the login bug"
        );
        assert_eq!(
            AgentManager::generate_name("<ide_opened_file>x</ide_opened_file> hi", "/p"),
            "x</ide_opened_file> hi"
        );
        let long = "a".repeat(60);
        assert_eq!(
            AgentManager::generate_name(&long, "/p"),
            format!("{}...", "a".repeat(44))
        );
        let fallback = AgentManager::generate_name("   ", "/work/api");
        assert!(
            fallback.starts_with("api ") && fallback.len() == "api ".len() + 6,
            "{fallback}"
        );
        assert!(AgentManager::generate_name("", "/").starts_with("Agent "));
    }

    struct FixedLocator(&'static str);
    impl SessionLocator for FixedLocator {
        fn locate(&self, query: &SessionQuery) -> Option<String> {
            assert!(query.used_session_ids.is_empty());
            Some(self.0.to_string())
        }
    }

    #[tokio::test]
    async fn session_locator_hook_runs_when_the_process_exits() {
        let backend = FakePtyBackend::new();
        let manager = AgentManager::with_options(ManagerOptions {
            backend: Arc::new(backend.clone()),
            locator: Arc::new(FixedLocator("found-session-1")),
            ..Default::default()
        });
        let a = manager.create(payload()).await.unwrap();
        backend.ptys()[0].emit_exit(0);
        wait_for("sid", || {
            manager.get(&a.id).unwrap().session_id.as_deref() == Some("found-session-1")
        })
        .await;
    }

    // ── session discovery ───────────────────────────────────────────────────

    /// `(force_refresh, prompt hint, ids owned by other agents)` of one `locate` call.
    type LocateCall = (bool, Option<String>, Vec<String>);

    struct ScriptedLocator {
        calls: Mutex<Vec<LocateCall>>,
        answer_on_call: usize,
        id: &'static str,
    }
    impl ScriptedLocator {
        fn new(answer_on_call: usize, id: &'static str) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::default(),
                answer_on_call,
                id,
            })
        }
        fn call_count(&self) -> usize {
            self.calls.lock().len()
        }
    }
    impl SessionLocator for Arc<ScriptedLocator> {
        fn locate(&self, q: &SessionQuery) -> Option<String> {
            let mut calls = self.calls.lock();
            calls.push((
                q.force_refresh,
                q.latest_user_prompt.clone(),
                q.used_session_ids.clone(),
            ));
            (calls.len() >= self.answer_on_call).then(|| self.id.to_string())
        }
    }

    fn discovering(backend: &FakePtyBackend, locator: Arc<ScriptedLocator>) -> AgentManager {
        AgentManager::with_options(ManagerOptions {
            backend: Arc::new(backend.clone()),
            locator: Arc::new(locator),
            kill_grace: Duration::from_millis(80),
            startup: fast_startup(),
            discovery: DiscoverySchedule {
                initial: Duration::from_millis(15),
                max: Duration::from_millis(40),
                factor: 2.0,
            },
            ..Default::default()
        })
    }

    #[test]
    fn discovery_backoff_is_hydras_schedule() {
        let d = DiscoverySchedule::default();
        let secs = |n| d.delay(n).as_secs_f64();
        assert_eq!(secs(0), 2.5);
        assert_eq!(secs(1), 4.0);
        assert!((secs(2) - 6.4).abs() < 1e-9);
        assert!((secs(3) - 10.24).abs() < 1e-9);
        assert_eq!(secs(7), 30.0);
        assert_eq!(secs(500), 30.0);
    }

    #[tokio::test]
    async fn discovery_polls_after_spawn_until_the_locator_finds_the_session() {
        let backend = FakePtyBackend::new();
        let locator = ScriptedLocator::new(3, "found-by-discovery");
        let manager = discovering(&backend, locator.clone());
        let agent = manager
            .create(CreateAgentPayload {
                initial_prompt: "fix the bug".into(),
                ..payload()
            })
            .await
            .unwrap();
        wait_for("session id", || {
            manager.get(&agent.id).unwrap().session_id.as_deref() == Some("found-by-discovery")
        })
        .await;
        assert_eq!(locator.call_count(), 3);
        {
            let calls = locator.calls.lock();
            assert!(calls[0].0, "the first probe forces a rescan");
            assert!(!calls[1].0);
            assert_eq!(calls[0].1.as_deref(), Some("fix the bug"), "prompt hint");
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(locator.call_count(), 3, "stops once settled");
    }

    #[tokio::test]
    async fn discovery_hands_the_locator_the_ids_of_other_agents() {
        let backend = FakePtyBackend::new();
        let locator = ScriptedLocator::new(1_000_000, "never");
        let manager = discovering(&backend, locator.clone());
        let other = manager.create(payload()).await.unwrap();
        manager.set_session_id(&other.id, "owned-elsewhere");
        let second = manager.create(payload()).await.unwrap();
        wait_for("second probe", || {
            locator
                .calls
                .lock()
                .iter()
                .any(|c| c.2 == ["owned-elsewhere"])
        })
        .await;
        assert!(manager.get(&second.id).unwrap().session_id.is_none());
    }

    #[tokio::test]
    async fn discovery_stops_when_the_process_exits_and_probes_once_more() {
        let backend = FakePtyBackend::new();
        let locator = ScriptedLocator::new(1_000_000, "never");
        let manager = discovering(&backend, locator.clone());
        let a = manager.create(payload()).await.unwrap();
        wait_for("a few probes", || locator.call_count() >= 2).await;
        backend.ptys()[0].emit_exit(0);
        wait_status_idle(&manager, &a.id).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let after_exit = locator.call_count();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            locator.call_count(),
            after_exit,
            "no polling without a process"
        );
        assert!(
            locator.calls.lock().last().is_some_and(|c| c.0),
            "exit probe rescans"
        );
    }

    async fn wait_status_idle(manager: &AgentManager, id: &str) {
        wait_for("idle", || {
            manager.get(id).unwrap().status == AgentStatus::Idle
        })
        .await;
    }

    #[tokio::test]
    async fn no_discovery_for_a_resumed_claude_session_but_codex_keeps_checking() {
        let backend = FakePtyBackend::new();
        let locator = ScriptedLocator::new(1, "codex-new-rollout");
        let manager = discovering(&backend, locator.clone());
        manager
            .create(CreateAgentPayload {
                resume_session_id: Some("known".into()),
                ..payload()
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(locator.call_count(), 0, "a Claude session id is final");

        let codex = manager
            .create(CreateAgentPayload {
                provider: ProviderId::Codex,
                resume_session_id: Some("old-rollout".into()),
                ..payload()
            })
            .await
            .unwrap();
        wait_for("codex switches", || {
            manager.get(&codex.id).unwrap().session_id.as_deref() == Some("codex-new-rollout")
        })
        .await;
    }

    #[tokio::test]
    async fn output_regex_wins_and_ends_discovery() {
        let backend = FakePtyBackend::new();
        let locator = ScriptedLocator::new(1_000_000, "never");
        let manager = discovering(&backend, locator.clone());
        let a = manager.create(payload()).await.unwrap();
        backend.ptys()[0]
            .emit_data(b"\x1b[1mSession ID:\x1b[0m 5f1c9a2e-77aa-4b1c-9d3e-0123456789ab");
        wait_for("sid from output", || {
            manager.get(&a.id).unwrap().session_id.is_some()
        })
        .await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        let n = locator.call_count();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(locator.call_count(), n);
        // Loose words are not ids.
        let b = manager.create(payload()).await.unwrap();
        backend.ptys()[1].emit_data(b"session management is on; session: abcdefgh");
        backend.ptys()[1].emit_data(b"x");
        wait_for("drain", || manager.get_buffer(&b.id).ends_with(b"x")).await;
        assert!(manager.get(&b.id).unwrap().session_id.is_none());
    }

    // ── import ──────────────────────────────────────────────────────────────

    fn catalog_session(id: &str, project: &str, prompt: &str) -> SessionSummary {
        SessionSummary {
            session_id: id.into(),
            project_path: project.into(),
            first_prompt: prompt.into(),
            created_at: "2026-01-01T10:00:00.000Z".into(),
            modified_at: "2026-01-02T10:00:00.000Z".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn imported_sessions_are_idle_agents_that_resume_on_first_input() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let sessions = vec![
            catalog_session(
                "abcdef12-0000-0000-0000-000000000001",
                "/work/api",
                "Fix the login bug",
            ),
            catalog_session("abcdef12-0000-0000-0000-000000000002", "/work/web", ""),
            catalog_session("abcdef12-0000-0000-0000-000000000001", "/work/api", "dup"),
            catalog_session("", "/work/x", "no id"),
            catalog_session("zzz", "", "no project"),
        ];
        let mut rx = manager.subscribe();
        assert_eq!(
            manager.import_sessions(&sessions, "sonnet", ProviderId::Claude),
            2
        );
        assert!(
            drain(&mut rx)
                .iter()
                .any(|e| matches!(e, AgentEvent::Changed))
        );
        assert_eq!(backend.spawn_count(), 0, "imported agents have no process");
        let agents = manager.list();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].id, "sess-abcdef12");
        assert_eq!(agents[1].id, "sess-abcdef12-2");
        assert_eq!(agents[0].name, "Fix the login bug");
        assert!(agents[1].name.starts_with("web "), "{}", agents[1].name);
        assert!(
            agents
                .iter()
                .all(|a| a.status == AgentStatus::Idle && a.pid.is_none())
        );
        assert_eq!(agents[0].project_dir, "/work/api");
        assert_eq!(agents[0].model, "sonnet");
        assert_eq!(agents[0].created_at, "2026-01-01T10:00:00.000Z");
        assert_eq!(agents[0].last_activity_at, "2026-01-02T10:00:00.000Z");
        assert_eq!(
            agents[0].ended_at.as_deref(),
            Some("2026-01-02T10:00:00.000Z")
        );
        assert!(
            manager.export_workspace_agents(false).is_empty(),
            "not persisted"
        );
        // Importing again adds nothing.
        assert_eq!(
            manager.import_sessions(&sessions, "sonnet", ProviderId::Claude),
            0
        );

        // First input resumes the session.
        assert!(manager.send_input("sess-abcdef12", "continue"));
        let args = backend.request(0).unwrap().args;
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--resume" && w[1] == "abcdef12-0000-0000-0000-000000000001"),
            "{args:?}"
        );
    }

    #[tokio::test]
    async fn imported_codex_sessions_keep_their_provider_and_never_probe() {
        let backend = FakePtyBackend::new();
        let locator = ScriptedLocator::new(1, "x");
        let manager = discovering(&backend, locator.clone());
        manager.import_sessions(
            &[catalog_session("c1", "/w", "p")],
            "gpt-5.4",
            ProviderId::Codex,
        );
        assert!(manager.send_input("sess-c1", "go"));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(manager.get("sess-c1").unwrap().provider, ProviderId::Codex);
        assert_eq!(locator.call_count(), 0, "imported agents do not discover");
    }

    #[tokio::test]
    async fn resuming_a_session_replaces_its_idle_imported_agent() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        manager.import_sessions(
            &[
                catalog_session("s-1", "/tmp/project", "p"),
                catalog_session("s-2", "/tmp/project", "q"),
            ],
            "sonnet",
            ProviderId::Claude,
        );
        let created = manager
            .create(CreateAgentPayload {
                resume_session_id: Some("s-1".into()),
                ..payload()
            })
            .await
            .unwrap();
        let ids: Vec<String> = manager.list().into_iter().map(|a| a.id.clone()).collect();
        assert_eq!(ids, ["sess-s-2".to_string(), created.id.clone()]);
        assert_eq!(created.session_id.as_deref(), Some("s-1"));
    }

    // ── trust prompt ────────────────────────────────────────────────────────

    const TRUST: &[u8] = b"\x1b[1m Do you trust the files in this folder?\x1b[0m\r\n \x1b[36m\xe2\x9d\xaf 1. Yes, I trust this folder\x1b[0m\r\n   2. No, exit\r\n";
    const MAIN_UI: &[u8] = b"\x1b[2J\x1b[H Welcome to Claude Code!\r\n > Try \"how does this project work?\"\r\n ? for shortcuts";

    fn named_with_prompt() -> CreateAgentPayload {
        CreateAgentPayload {
            initial_prompt: "do it".into(),
            ..payload()
        }
    }

    #[tokio::test]
    async fn startup_input_is_held_while_the_trust_prompt_is_unanswered() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let agent = manager.create(named_with_prompt()).await.unwrap();
        let pty = backend.ptys()[0].clone();
        pty.emit_data(TRUST);
        // Well past /rename (1.5 s) and the initial prompt (3 s): still nothing was typed.
        tokio::time::sleep(Duration::from_millis(3600)).await;
        assert_eq!(pty.written(), "", "Enter would have pressed 'No, exit'");
        assert_eq!(manager.get(&agent.id).unwrap().status, AgentStatus::Running);

        // Navigating does not release it.
        assert!(manager.send_raw_input(&agent.id, b"\x1b[B"));
        pty.emit_data(TRUST);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(pty.written(), "\x1b[B");

        // The user answers; Claude draws its UI: the held input follows.
        assert!(manager.send_raw_input(&agent.id, b"\x1b[A"));
        pty.emit_data(TRUST);
        assert!(manager.send_raw_input(&agent.id, b"\r"));
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(pty.written(), "\x1b[B\x1b[A\r", "nothing before new output");
        pty.emit_data(MAIN_UI);
        wait_for("/rename after the answer", || {
            pty.written().contains("/rename EP\r")
        })
        .await;
        wait_for("initial prompt", || pty.written().contains("do it\r")).await;
        let written = pty.written();
        assert!(
            written.find("/rename EP").unwrap() < written.find("do it").unwrap(),
            "{written:?}"
        );
        assert!(manager.get(&agent.id).unwrap().initial_prompt.is_empty());
    }

    #[tokio::test]
    async fn startup_input_flows_normally_when_there_is_no_trust_prompt() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        manager.create(named_with_prompt()).await.unwrap();
        let pty = backend.ptys()[0].clone();
        pty.emit_data(MAIN_UI);
        wait_for("/rename", || pty.written().starts_with("/rename EP\r")).await;
        wait_for("prompt", || pty.written().contains("do it")).await;
    }

    #[tokio::test]
    async fn an_unanswered_trust_prompt_drops_the_startup_input_after_the_timeout() {
        let backend = FakePtyBackend::new();
        let manager = AgentManager::with_options(ManagerOptions {
            backend: Arc::new(backend.clone()),
            startup: StartupOptions {
                trust_timeout: Duration::from_millis(1800),
                ..fast_startup()
            },
            ..Default::default()
        });
        manager.create(named_with_prompt()).await.unwrap();
        let pty = backend.ptys()[0].clone();
        pty.emit_data(TRUST);
        tokio::time::sleep(Duration::from_millis(2500)).await;
        // Even an answer now does not bring the dropped input back.
        pty.emit_data(MAIN_UI);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(pty.written(), "");
    }

    #[tokio::test]
    async fn nothing_is_written_when_the_process_exits_at_the_trust_prompt() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let agent = manager.create(named_with_prompt()).await.unwrap();
        let pty = backend.ptys()[0].clone();
        pty.emit_data(TRUST);
        pty.emit_exit(1);
        wait_for("errored", || {
            manager.get(&agent.id).unwrap().status == AgentStatus::Errored
        })
        .await;
        tokio::time::sleep(Duration::from_millis(3300)).await;
        assert_eq!(pty.written(), "");
    }

    #[tokio::test]
    async fn rename_while_the_trust_prompt_is_up_is_deferred() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        let agent = manager
            .create(CreateAgentPayload {
                name: String::new(),
                ..payload()
            })
            .await
            .unwrap();
        let pty = backend.ptys()[0].clone();
        pty.emit_data(TRUST);
        wait_for("prompt buffered", || {
            !manager.get_buffer(&agent.id).is_empty()
        })
        .await;
        manager.rename_agent(&agent.id, "New name");
        tokio::time::sleep(Duration::from_millis(1800)).await;
        assert_eq!(pty.written(), "");
        assert_eq!(manager.get(&agent.id).unwrap().name, "New name");
        manager.send_raw_input(&agent.id, b"1");
        pty.emit_data(MAIN_UI);
        wait_for("deferred rename", || {
            pty.written().ends_with("/rename New name\r")
        })
        .await;
    }

    #[tokio::test]
    async fn other_providers_are_not_gated() {
        let backend = FakePtyBackend::new();
        let manager = manager_with(&backend);
        manager
            .create(CreateAgentPayload {
                provider: ProviderId::Codex,
                initial_prompt: "go".into(),
                ..payload()
            })
            .await
            .unwrap();
        let pty = backend.ptys()[0].clone();
        pty.emit_data(TRUST);
        wait_for("prompt written", || pty.written().contains("go")).await;
    }

    // ── real PTY integration ────────────────────────────────────────────────

    fn real_manager(script_unix: &'static str, script_windows: &'static str) -> AgentManager {
        AgentManager::with_options(ManagerOptions {
            command_override: Some(Arc::new(move |_| {
                Some(if cfg!(windows) {
                    SpawnSpec {
                        cmd: "cmd".into(),
                        args: vec!["/c".into(), script_windows.into()],
                    }
                } else {
                    SpawnSpec {
                        cmd: "sh".into(),
                        args: vec!["-c".into(), script_unix.into()],
                    }
                })
            })),
            kill_grace: Duration::from_secs(2),
            ..Default::default()
        })
    }

    /// `payload()` with a project dir that exists on every OS (`/tmp/project` does not on Windows).
    fn real_payload() -> CreateAgentPayload {
        CreateAgentPayload {
            project_dir: std::env::temp_dir().to_string_lossy().into_owned(),
            ..payload()
        }
    }

    async fn wait_status(manager: &AgentManager, id: &str, status: AgentStatus) {
        for _ in 0..600 {
            if manager.get(id).unwrap().status == status {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("agent never reached {status:?}: {:?}", manager.get(id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn real_pty_runs_a_command_and_ends_idle_on_exit_zero() {
        let manager = real_manager("echo hello; sleep 0.2", "echo hello");
        let mut rx = manager.subscribe();
        let a = manager
            .create(CreateAgentPayload {
                provider: ProviderId::Codex,
                model: "gpt-5.4".into(),
                ..real_payload()
            })
            .await
            .unwrap();
        assert_eq!(a.status, AgentStatus::Running);
        assert!(a.pid.is_some());
        wait_status(&manager, &a.id, AgentStatus::Idle).await;
        let out = String::from_utf8_lossy(&manager.get_buffer(&a.id)).into_owned();
        assert!(out.contains("hello"), "output was {out:?}");
        assert_eq!(manager.get_activity(&a.id).unwrap().last_exit_code, Some(0));
        let events = drain(&mut rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::Output { .. }))
        );
        assert_eq!(statuses(&events), [AgentStatus::Running, AgentStatus::Idle]);
        assert_eq!(manager.get(&a.id).unwrap().pid, None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn real_pty_non_zero_exit_is_errored() {
        let manager = real_manager("echo oops; exit 3", "exit 3");
        let a = manager
            .create(CreateAgentPayload {
                provider: ProviderId::Codex,
                model: "gpt-5.4".into(),
                ..real_payload()
            })
            .await
            .unwrap();
        wait_status(&manager, &a.id, AgentStatus::Errored).await;
        assert_eq!(manager.get_activity(&a.id).unwrap().last_exit_code, Some(3));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn real_pty_spawn_failure_is_errored() {
        let manager = AgentManager::with_options(ManagerOptions {
            command_override: Some(Arc::new(|_| {
                Some(SpawnSpec {
                    cmd: "definitely-not-a-command-xyz".into(),
                    args: vec![],
                })
            })),
            ..Default::default()
        });
        let a = manager
            .create(CreateAgentPayload {
                provider: ProviderId::Codex,
                model: "gpt-5.4".into(),
                ..real_payload()
            })
            .await
            .unwrap();
        // On Windows the failed direct spawn is retried through `cmd.exe`, which starts fine and
        // then exits non-zero: the agent turns Errored a moment later instead of at creation.
        wait_status(&manager, &a.id, AgentStatus::Errored).await;
        assert!(manager.get(&a.id).unwrap().ended_at.is_some());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn real_pty_accepts_input_resize_and_graceful_kill() {
        let manager = real_manager(
            "stty -echo; while read line; do echo \"got:$line\"; done",
            "",
        );
        let a = manager
            .create(CreateAgentPayload {
                provider: ProviderId::Codex,
                model: "gpt-5.4".into(),
                ..real_payload()
            })
            .await
            .unwrap();
        manager.resize(&a.id, 100, 40);
        tokio::time::sleep(Duration::from_millis(200)).await; // let `stty -echo` run
        assert!(manager.send_input(&a.id, "ping"));
        for _ in 0..300 {
            if manager.get_recent_output(&a.id, 1000).contains("got:ping") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            manager.get_recent_output(&a.id, 1000).contains("got:ping"),
            "{:?}",
            manager.get_recent_output(&a.id, 1000)
        );
        assert!(manager.kill(&a.id));
        wait_status(&manager, &a.id, AgentStatus::Idle).await; // SIGTERM exit counts as an explicit stop
    }
}
