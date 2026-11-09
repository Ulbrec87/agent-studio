//! Headless (non-interactive) runs: the provider CLI is spawned with its `-p` style arguments,
//! its stdout is kept as a JSONL log per run and a small meta file keeps the run index
//! (Hydra `electron/headless/HeadlessOrchestrator.ts`).
//!
//! Files in the runs folder: `<id>.jsonl` (stdout lines, stderr as `{"type":"stderr","text":..}`)
//! and `<id>.meta.json` (`{"schemaVersion":1,"run":{..}}`). Hydra has no retention, nor do we.

mod log;
pub mod types;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use chrono::SecondsFormat;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{Notify, broadcast};

use crate::agents::env::{EnvOverrides, build_agent_env_from_process, claude_profile_env};
use crate::agents::manager::ClaudeProfileResolver;
use crate::agents::providers::get_provider;
use crate::agents::shell::{ShellConfig, SpawnSpec, wrap_with_shell_current};
use crate::types::ProviderId;

pub use log::read_log_tail;
pub use types::*;

/// Replaces the provider CLI invocation (tests).
pub type HeadlessCommandOverride = Arc<dyn Fn(&HeadlessRun) -> Option<SpawnSpec> + Send + Sync>;

const META_EXT: &str = ".meta.json";
const LOG_EXT: &str = ".jsonl";
const META_SCHEMA_VERSION: u32 = 1;
/// Grace between the polite stop and the hard kill.
const KILL_GRACE: Duration = Duration::from_secs(3);
const STDERR_TAIL_CHARS: usize = 400;

#[derive(Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
struct MetaFile {
    schema_version: u32,
    run: HeadlessRun,
}

struct Managed {
    run: HeadlessRun,
    /// `Some` while the process lives; notified to cancel.
    cancel: Option<Arc<Notify>>,
}

struct Inner {
    dir: PathBuf,
    runs: Mutex<HashMap<String, Managed>>,
    events: broadcast::Sender<HeadlessEvent>,
    shell_config: Mutex<Arc<dyn Fn() -> ShellConfig + Send + Sync>>,
    env_overrides: Mutex<EnvOverrides>,
    claude_profile: Mutex<ClaudeProfileResolver>,
    provider_path: Mutex<Arc<dyn Fn(ProviderId) -> Option<String> + Send + Sync>>,
    command_override: Option<HeadlessCommandOverride>,
}

#[derive(Clone)]
pub struct HeadlessOrchestrator {
    inner: Arc<Inner>,
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn session_of(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    ["session_id", "sessionId"]
        .iter()
        .find_map(|k| v.get(k)?.as_str().filter(|s| !s.is_empty()))
        .map(str::to_string)
}

impl HeadlessOrchestrator {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self::with_override(dir, None)
    }

    /// `command_override` replaces the provider CLI invocation (tests).
    pub fn with_override(
        dir: impl Into<PathBuf>,
        command_override: Option<HeadlessCommandOverride>,
    ) -> Self {
        let dir = dir.into();
        let _ = std::fs::create_dir_all(&dir);
        let (events, _) = broadcast::channel(1024);
        let this = Self {
            inner: Arc::new(Inner {
                dir,
                runs: Mutex::new(HashMap::new()),
                events,
                shell_config: Mutex::new(Arc::new(ShellConfig::default)),
                env_overrides: Mutex::new(EnvOverrides::new()),
                claude_profile: Mutex::new(Arc::new(|_, _| None)),
                provider_path: Mutex::new(Arc::new(|_| None)),
                command_override,
            }),
        };
        this.hydrate();
        this
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HeadlessEvent> {
        self.inner.events.subscribe()
    }

    pub fn set_shell_config_provider(
        &self,
        provider: impl Fn() -> ShellConfig + Send + Sync + 'static,
    ) {
        *self.inner.shell_config.lock() = Arc::new(provider);
    }

    pub fn set_env_overrides(&self, overrides: EnvOverrides) {
        *self.inner.env_overrides.lock() = overrides;
    }

    pub fn set_claude_profile_resolver(
        &self,
        resolver: impl Fn(&str, Option<&str>) -> Option<String> + Send + Sync + 'static,
    ) {
        *self.inner.claude_profile.lock() = Arc::new(resolver);
    }

    fn log_path(&self, id: &str) -> PathBuf {
        self.inner.dir.join(format!("{id}{LOG_EXT}"))
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.inner.dir.join(format!("{id}{META_EXT}"))
    }

    // ── start ───────────────────────────────────────────────────────────────

    /// Start a run. Must be called inside a tokio runtime.
    pub fn start(&self, payload: StartHeadlessRunPayload) -> HeadlessRun {
        let id = loop {
            let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
            if !self.inner.runs.lock().contains_key(&id) {
                break id;
            }
        };
        let run = HeadlessRun {
            id: id.clone(),
            prompt: payload.prompt.clone(),
            project_dir: payload.project_dir.clone(),
            provider: payload.provider,
            model: payload.model.clone(),
            reasoning_effort: payload.reasoning_effort.clone().filter(|e| !e.is_empty()),
            resume_session_id: payload.resume_session_id.clone(),
            status: HeadlessRunStatus::Running,
            started_at: now_iso(),
            ended_at: None,
            session_id: payload.resume_session_id.clone(),
            error: None,
        };
        let _ = std::fs::write(self.log_path(&id), "");
        let cancel = Arc::new(Notify::new());
        self.inner.runs.lock().insert(
            id.clone(),
            Managed {
                run: run.clone(),
                cancel: Some(cancel.clone()),
            },
        );
        self.persist(&run);
        self.emit_changed(&run);

        let command = self.build_command(&run);
        let this = self.clone();
        tokio::spawn(async move { this.drive(id, command, cancel).await });
        run
    }

    fn build_command(&self, run: &HeadlessRun) -> (SpawnSpec, HashMap<String, String>) {
        let provider = get_provider(run.provider);
        let overridden = self.inner.command_override.as_ref().and_then(|f| f(run));
        let spec = overridden.unwrap_or_else(|| {
            let raw = SpawnSpec {
                cmd: (self.inner.provider_path.lock())(run.provider)
                    .unwrap_or_else(|| provider.command.to_string()),
                args: provider.build_headless_args(
                    &run.model,
                    &run.prompt,
                    run.resume_session_id.as_deref(),
                    run.reasoning_effort.as_deref(),
                ),
            };
            let shell = (self.inner.shell_config.lock())();
            wrap_with_shell_current(&raw, &shell)
        });
        let mut overrides = self.inner.env_overrides.lock().clone();
        if run.provider == ProviderId::Claude {
            let profile = (self.inner.claude_profile.lock())(
                &run.project_dir,
                run.resume_session_id.as_deref(),
            );
            overrides.extend(claude_profile_env(profile.as_deref()));
        }
        overrides.insert("FORCE_COLOR".into(), Some("0".into()));
        overrides.insert("NO_COLOR".into(), Some("1".into()));
        (spec, build_agent_env_from_process(&overrides))
    }

    fn spawn_child(
        spec: &SpawnSpec,
        env: &HashMap<String, String>,
        cwd: &str,
    ) -> std::io::Result<tokio::process::Child> {
        let launch = |cmd: &str, args: &[String]| {
            let mut c = Command::new(cmd);
            c.args(args)
                .env_clear()
                .envs(env)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if !cwd.is_empty() {
                c.current_dir(cwd);
            }
            #[cfg(unix)]
            c.process_group(0); // own group: cancel stops the whole tree
            #[cfg(windows)]
            {
                // CREATE_NO_WINDOW
                c.creation_flags(0x0800_0000);
            }
            c.spawn()
        };
        match launch(&spec.cmd, &spec.args) {
            Err(err) if cfg!(windows) => {
                // CLI tools are often `.cmd` shims that cannot be executed directly.
                tracing::debug!(%err, "direct headless spawn failed, retrying via cmd.exe");
                let mut args = vec!["/c".to_string(), spec.cmd.clone()];
                args.extend(spec.args.iter().cloned());
                launch("cmd.exe", &args)
            }
            other => other,
        }
    }

    async fn drive(
        self,
        id: String,
        (spec, env): (SpawnSpec, HashMap<String, String>),
        cancel: Arc<Notify>,
    ) {
        let cwd = self
            .inner
            .runs
            .lock()
            .get(&id)
            .map(|m| m.run.project_dir.clone())
            .unwrap_or_default();
        tracing::info!(run = %id, cmd = %spec.cmd, cwd = %cwd, "starting headless run");
        let mut child = match Self::spawn_child(&spec, &env, &cwd) {
            Ok(c) => c,
            Err(err) => {
                self.finalize(&id, HeadlessRunStatus::Errored, Some(err.to_string()));
                return;
            }
        };
        let pid = child.id();
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let mut readers = Vec::new();
        if let Some(out) = child.stdout.take() {
            readers.push(tokio::spawn(self.clone().read_stream(
                id.clone(),
                out,
                false,
                stderr_tail.clone(),
            )));
        }
        if let Some(err) = child.stderr.take() {
            readers.push(tokio::spawn(self.clone().read_stream(
                id.clone(),
                err,
                true,
                stderr_tail.clone(),
            )));
        }
        let status = tokio::select! {
            s = child.wait() => s,
            () = cancel.notified() => {
                terminate_tree(pid, false);
                match tokio::time::timeout(KILL_GRACE, child.wait()).await {
                    Ok(s) => s,
                    Err(_) => {
                        terminate_tree(pid, true);
                        let _ = child.start_kill();
                        child.wait().await
                    }
                }
            }
        };
        // The pipes close with the process (a grandchild holding them open would block forever).
        for r in readers {
            let _ = tokio::time::timeout(Duration::from_secs(2), r).await;
        }
        let canceled = self
            .inner
            .runs
            .lock()
            .get(&id)
            .is_some_and(|m| m.run.status == HeadlessRunStatus::Canceled);
        if canceled {
            self.finalize(&id, HeadlessRunStatus::Canceled, None);
            return;
        }
        match status {
            Ok(s) if s.success() => self.finalize(&id, HeadlessRunStatus::Completed, None),
            Ok(s) => {
                let tail = stderr_tail.lock().trim().to_string();
                let mut msg = format!("CLI exited with code {}", s.code().unwrap_or(-1));
                if !tail.is_empty() {
                    msg.push_str(": ");
                    msg.push_str(&tail);
                }
                self.finalize(&id, HeadlessRunStatus::Errored, Some(msg));
            }
            Err(err) => self.finalize(&id, HeadlessRunStatus::Errored, Some(err.to_string())),
        }
    }

    async fn read_stream(
        self,
        id: String,
        stream: impl AsyncRead + Unpin,
        is_stderr: bool,
        stderr_tail: Arc<Mutex<String>>,
    ) {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&buf);
            let line = text.trim_end_matches(['\n', '\r']);
            if line.trim().is_empty() {
                continue;
            }
            if is_stderr {
                {
                    let mut tail = stderr_tail.lock();
                    tail.push_str(line);
                    tail.push('\n');
                    let len = tail.chars().count();
                    if len > STDERR_TAIL_CHARS {
                        *tail = tail.chars().skip(len - STDERR_TAIL_CHARS).collect();
                    }
                }
                let wrapped = serde_json::json!({ "type": "stderr", "text": line }).to_string();
                self.append_log(&id, &wrapped);
                self.emit(HeadlessEvent::Output {
                    run_id: id.clone(),
                    data: wrapped,
                });
            } else {
                self.append_log(&id, line);
                self.capture_session(&id, line);
                self.emit(HeadlessEvent::Output {
                    run_id: id.clone(),
                    data: line.to_string(),
                });
            }
        }
    }

    fn append_log(&self, id: &str, line: &str) {
        use std::io::Write;
        let file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(self.log_path(id));
        if let Ok(mut f) = file {
            let _ = writeln!(f, "{line}");
        }
    }

    fn capture_session(&self, id: &str, line: &str) {
        let Some(session) = session_of(line) else {
            return;
        };
        let changed = {
            let mut runs = self.inner.runs.lock();
            match runs.get_mut(id) {
                Some(m) if m.run.session_id.as_deref() != Some(session.as_str()) => {
                    m.run.session_id = Some(session);
                    Some(m.run.clone())
                }
                _ => None,
            }
        };
        if let Some(run) = changed {
            self.persist(&run);
            self.emit_changed(&run);
        }
    }

    fn finalize(&self, id: &str, status: HeadlessRunStatus, error: Option<String>) {
        let run = {
            let mut runs = self.inner.runs.lock();
            let Some(m) = runs.get_mut(id) else { return };
            m.cancel = None;
            m.run.status = status;
            m.run.error = error;
            m.run.ended_at = Some(now_iso());
            m.run.clone()
        };
        tracing::info!(run = %id, status = status.as_str(), "headless run finished");
        self.persist(&run);
        self.emit_changed(&run);
    }

    fn emit(&self, event: HeadlessEvent) {
        let _ = self.inner.events.send(event);
    }

    fn emit_changed(&self, run: &HeadlessRun) {
        self.emit(HeadlessEvent::Changed(Box::new(run.clone())));
    }

    fn persist(&self, run: &HeadlessRun) {
        let meta = MetaFile {
            schema_version: META_SCHEMA_VERSION,
            run: run.clone(),
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&meta)
            && let Err(err) = crate::util::write_atomic(&self.meta_path(&run.id), &bytes)
        {
            tracing::warn!(run = %run.id, %err, "could not save the headless run");
        }
    }

    // ── queries ─────────────────────────────────────────────────────────────

    pub fn list(&self, options: &ListHeadlessRunsOptions) -> Vec<HeadlessRun> {
        let query = options
            .query
            .as_deref()
            .map(|q| q.trim().to_lowercase())
            .filter(|q| !q.is_empty());
        let mut runs: Vec<HeadlessRun> = self
            .inner
            .runs
            .lock()
            .values()
            .map(|m| m.run.clone())
            .filter(|r| options.status.matches(r.status))
            .filter(|r| query.as_deref().is_none_or(|q| run_matches(r, q)))
            .collect();
        runs.sort_by(|a, b| {
            started_ms(b)
                .cmp(&started_ms(a))
                .then_with(|| b.id.cmp(&a.id))
        });
        if let Some(limit) = options.limit.filter(|l| *l > 0) {
            runs.truncate(limit);
        }
        runs
    }

    pub fn get(&self, id: &str) -> Option<HeadlessRun> {
        self.inner.runs.lock().get(id).map(|m| m.run.clone())
    }

    pub fn get_log(&self, id: &str, options: &HeadlessRunLogOptions) -> Option<HeadlessRunLog> {
        self.inner.runs.lock().get(id)?;
        Some(read_log_tail(&self.log_path(id), id, options))
    }

    /// Stop a running run; false when it is unknown or not running.
    pub fn cancel(&self, id: &str) -> bool {
        let (run, notify) = {
            let mut runs = self.inner.runs.lock();
            let Some(m) = runs.get_mut(id) else {
                return false;
            };
            let Some(notify) = m.cancel.clone() else {
                return false;
            };
            m.run.status = HeadlessRunStatus::Canceled;
            (m.run.clone(), notify)
        };
        self.persist(&run);
        self.emit_changed(&run);
        notify.notify_one();
        true
    }

    /// Cancel every running run (daemon shutdown); waits until they have ended or `timeout` passed.
    pub async fn cancel_all(&self, timeout: Duration) {
        let running: Vec<String> = self
            .inner
            .runs
            .lock()
            .iter()
            .filter(|(_, m)| m.cancel.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for id in &running {
            self.cancel(id);
        }
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline
            && self.inner.runs.lock().values().any(|m| m.cancel.is_some())
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub fn running_count(&self) -> usize {
        self.inner
            .runs
            .lock()
            .values()
            .filter(|m| m.cancel.is_some())
            .count()
    }

    // ── disk ────────────────────────────────────────────────────────────────

    fn hydrate(&self) {
        let Ok(entries) = std::fs::read_dir(&self.inner.dir) else {
            return;
        };
        let names: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        let mut loaded: Vec<HeadlessRun> = Vec::new();
        for name in names.iter().filter(|n| n.ends_with(META_EXT)) {
            let path = self.inner.dir.join(name);
            let Some(meta) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<MetaFile>(&t).ok())
            else {
                continue;
            };
            if meta.run.id.is_empty() || meta.run.id != name.trim_end_matches(META_EXT) {
                continue;
            }
            loaded.push(meta.run);
        }
        for name in names.iter().filter(|n| n.ends_with(LOG_EXT)) {
            let id = name.trim_end_matches(LOG_EXT);
            if loaded.iter().any(|r| r.id == id) {
                continue;
            }
            let path = self.inner.dir.join(name);
            let stamp = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .map(|t| {
                    chrono::DateTime::<chrono::Utc>::from(t)
                        .to_rfc3339_opts(SecondsFormat::Millis, true)
                })
                .unwrap_or_else(|_| now_iso());
            loaded.push(HeadlessRun {
                id: id.to_string(),
                prompt: "(legacy run)".into(),
                model: "sonnet".into(),
                status: HeadlessRunStatus::Completed,
                started_at: stamp.clone(),
                ended_at: Some(stamp),
                session_id: discover_session(&path),
                ..Default::default()
            });
        }
        for mut run in loaded {
            if run.status == HeadlessRunStatus::Running {
                // The process died with the daemon; Hydra would show "running" forever.
                run.status = HeadlessRunStatus::Errored;
                run.error =
                    Some("Interrupted: the daemon stopped while the run was in progress".into());
                run.ended_at = Some(now_iso());
            }
            self.persist(&run);
            self.inner
                .runs
                .lock()
                .insert(run.id.clone(), Managed { run, cancel: None });
        }
    }
}

/// Does `run` match the (lowercase, trimmed) search `query`? Id, prompt, project, model,
/// provider, status, session id and error are searched, like Hydra.
pub fn run_matches(run: &HeadlessRun, query: &str) -> bool {
    [
        run.id.as_str(),
        run.prompt.as_str(),
        run.project_dir.as_str(),
        run.model.as_str(),
        run.provider.as_str(),
        run.status.as_str(),
        run.session_id.as_deref().unwrap_or(""),
        run.error.as_deref().unwrap_or(""),
    ]
    .iter()
    .any(|f| f.to_lowercase().contains(query))
}

fn started_ms(run: &HeadlessRun) -> i64 {
    chrono::DateTime::parse_from_rfc3339(&run.started_at).map_or(0, |d| d.timestamp_millis())
}

fn discover_session(log: &Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    text.lines().find_map(session_of)
}

/// Stop the process and its children: SIGTERM/SIGKILL to the group on Unix, `taskkill /T` on
/// Windows.
fn terminate_tree(pid: Option<u32>, force: bool) {
    let Some(pid) = pid else { return };
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;
        let Ok(raw) = i32::try_from(pid) else { return };
        let signal = if force {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        };
        let _ = killpg(Pid::from_raw(raw), signal);
    }
    #[cfg(windows)]
    {
        let _ = force;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests;
