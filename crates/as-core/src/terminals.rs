//! Free terminals: plain shell PTYs per project (port of Hydra's free-terminal routes in
//! `DaemonServer.ts`).
//!
//! A project has *groups* (tabs); a group has one or more *panes* (a split). An exited terminal is
//! dropped from the layout like in Hydra. Lifecycle: `explicit` keeps everything, `lru` evicts the
//! least recently active terminal when spawning past `freeTerminalMaxCount`, `idle` is applied by
//! [`TerminalManager::sweep_idle`] (the daemon calls it every minute).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::agents::env::{EnvOverrides, build_agent_env_from_process};
use crate::agents::output::OutputBuffer;
use crate::agents::pty::{PtyBackend, PtyControl, PtySpawnRequest, pump};
use crate::agents::shell::{Os, ShellConfig, split_args};
use crate::types::{AppConfig, FreeTerminalLifecyclePolicy, TerminalShellMode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FreeTerminalPane {
    pub id: String,
    pub label: String,
    pub exited: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FreeTerminalGroup {
    pub id: String,
    pub pane_ids: Vec<String>,
    pub active_pane_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FreeTerminalLayout {
    pub project_dir: String,
    pub groups: Vec<FreeTerminalGroup>,
    pub active_group_id: Option<String>,
    pub panes: Vec<FreeTerminalPane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub terminal_id: String,
    pub project_dir: String,
    pub label: String,
    pub last_activity_at: u64,
    pub last_input_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnedTerminal {
    pub terminal_id: String,
    pub group_id: String,
    pub layout: FreeTerminalLayout,
}

#[derive(Debug, Clone)]
pub enum TerminalEvent {
    Output {
        terminal_id: String,
        project_dir: String,
        data: Bytes,
    },
    Exit {
        terminal_id: String,
        project_dir: String,
        exit_code: i32,
    },
    LayoutChanged {
        project_dir: String,
        layout: FreeTerminalLayout,
    },
}

/// The slice of [`AppConfig`] the terminals care about (read on every spawn / sweep, so edits apply live).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSettings {
    pub shell: ShellConfig,
    pub policy: FreeTerminalLifecyclePolicy,
    pub max_count: u32,
    pub idle_timeout_minutes: u32,
    pub scrollback_lines: u32,
}

impl TerminalSettings {
    pub fn from_config(c: &AppConfig) -> Self {
        Self {
            shell: ShellConfig::from_config(c),
            policy: c.free_terminal_lifecycle_policy,
            max_count: c.free_terminal_max_count,
            idle_timeout_minutes: c.free_terminal_idle_timeout_minutes,
            scrollback_lines: c.free_terminal_scrollback_lines,
        }
    }
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self::from_config(&AppConfig::default())
    }
}

/// Program and arguments of the interactive shell: the configured custom shell, else `$SHELL`
/// (login shell for `auto`/`login`), else `%COMSPEC%` / `cmd.exe` on Windows and `/bin/sh` elsewhere.
pub fn resolve_shell(
    config: &ShellConfig,
    os: Os,
    env: &HashMap<String, String>,
) -> (String, Vec<String>) {
    let non_empty = |k: &str| env.get(k).filter(|v| !v.is_empty()).cloned();
    if config.mode == TerminalShellMode::Custom && !config.path.is_empty() {
        return (config.path.clone(), split_args(&config.args));
    }
    if os == Os::Windows {
        let shell = non_empty("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
        return (shell, split_args(&config.args));
    }
    let shell = non_empty("SHELL").unwrap_or_else(|| "/bin/sh".into());
    let args = if !config.args.is_empty() {
        split_args(&config.args)
    } else if matches!(
        config.mode,
        TerminalShellMode::Auto | TerminalShellMode::Login
    ) {
        vec!["-l".into()]
    } else {
        Vec::new()
    };
    (shell, args)
}

struct Entry {
    id: String,
    project_dir: String,
    label: String,
    buffer: OutputBuffer,
    control: Box<dyn PtyControl>,
    writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
    last_activity_at: u64,
    last_input_at: u64,
}

#[derive(Default)]
struct ProjectState {
    groups: Vec<FreeTerminalGroup>,
    active_group_id: Option<String>,
    next_label: u32,
}

#[derive(Default)]
struct Inner {
    terminals: HashMap<String, Entry>,
    projects: HashMap<String, ProjectState>,
}

pub struct TerminalManager {
    backend: Arc<dyn PtyBackend>,
    settings: Box<dyn Fn() -> TerminalSettings + Send + Sync>,
    env_overrides: Mutex<EnvOverrides>,
    inner: Mutex<Inner>,
    events: broadcast::Sender<TerminalEvent>,
    counter: AtomicU64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Approximate bytes per scrollback line (Hydra: 200 chars).
const BYTES_PER_LINE: usize = 200;

impl TerminalManager {
    pub fn new(
        backend: Arc<dyn PtyBackend>,
        settings: impl Fn() -> TerminalSettings + Send + Sync + 'static,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        Arc::new(Self {
            backend,
            settings: Box::new(settings),
            env_overrides: Mutex::default(),
            inner: Mutex::default(),
            events,
            counter: AtomicU64::new(0),
        })
    }

    pub fn set_env_overrides(&self, overrides: EnvOverrides) {
        *self.env_overrides.lock() = overrides;
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TerminalEvent> {
        self.events.subscribe()
    }

    fn cap_bytes(&self) -> usize {
        (self.settings)().scrollback_lines.max(100) as usize * BYTES_PER_LINE
    }

    fn layout_locked(inner: &Inner, project_dir: &str) -> FreeTerminalLayout {
        let mut layout = FreeTerminalLayout {
            project_dir: project_dir.to_string(),
            ..Default::default()
        };
        if let Some(state) = inner.projects.get(project_dir) {
            layout.groups = state.groups.clone();
            layout.active_group_id = state.active_group_id.clone();
            for g in &state.groups {
                for id in &g.pane_ids {
                    if let Some(e) = inner.terminals.get(id) {
                        layout.panes.push(FreeTerminalPane {
                            id: e.id.clone(),
                            label: e.label.clone(),
                            exited: false,
                        });
                    }
                }
            }
        }
        layout
    }

    pub fn layout(&self, project_dir: &str) -> FreeTerminalLayout {
        Self::layout_locked(&self.inner.lock(), project_dir)
    }

    fn emit_layout(&self, project_dir: &str) {
        let layout = self.layout(project_dir);
        let _ = self.events.send(TerminalEvent::LayoutChanged {
            project_dir: project_dir.to_string(),
            layout,
        });
    }

    /// Spawn a shell for `project_dir` (in `cwd` when given, else the project dir). With `group_id`
    /// the terminal joins that tab as another pane (a split), otherwise it opens a new tab.
    pub fn spawn(
        self: &Arc<Self>,
        project_dir: &str,
        cwd: Option<&str>,
        group_id: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<SpawnedTerminal> {
        let settings = (self.settings)();
        if settings.policy == FreeTerminalLifecyclePolicy::Lru {
            self.enforce_lru(settings.max_count.max(1) as usize);
        }
        let mut overrides = self.env_overrides.lock().clone();
        overrides.insert("TERM".into(), Some("xterm-256color".into()));
        overrides.insert("FORCE_COLOR".into(), Some("1".into()));
        let env = build_agent_env_from_process(&overrides);
        let (cmd, args) = resolve_shell(&settings.shell, Os::current(), &env);
        let cwd = cwd
            .filter(|c| !c.is_empty())
            .unwrap_or(project_dir)
            .to_string();
        let request = PtySpawnRequest {
            cmd,
            args,
            cwd,
            env,
            cols: cols.max(2),
            rows: rows.max(2),
        };
        let process = self.backend.spawn(&request)?;

        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!(
            "term-{n}-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..6]
        );
        let now = now_ms();
        let cap = self.cap_bytes();
        let (group, layout);
        {
            let mut inner = self.inner.lock();
            let state = inner.projects.entry(project_dir.to_string()).or_default();
            state.next_label += 1;
            let label = format!("Terminal {}", state.next_label);
            let existing = group_id.and_then(|g| state.groups.iter().position(|x| x.id == g));
            let idx = existing.unwrap_or_else(|| {
                state.groups.push(FreeTerminalGroup {
                    id: format!("grp-{n}"),
                    pane_ids: Vec::new(),
                    active_pane_id: id.clone(),
                });
                state.groups.len() - 1
            });
            state.groups[idx].pane_ids.push(id.clone());
            state.groups[idx].active_pane_id = id.clone();
            state.active_group_id = Some(state.groups[idx].id.clone());
            group = state.groups[idx].id.clone();
            inner.terminals.insert(
                id.clone(),
                Entry {
                    id: id.clone(),
                    project_dir: project_dir.to_string(),
                    label,
                    buffer: OutputBuffer::with_cap(cap),
                    control: process.control,
                    writer: Arc::new(Mutex::new(process.writer)),
                    last_activity_at: now,
                    last_input_at: now,
                },
            );
            layout = Self::layout_locked(&inner, project_dir);
        }
        let _ = self.events.send(TerminalEvent::LayoutChanged {
            project_dir: project_dir.to_string(),
            layout: layout.clone(),
        });
        let (on_data, on_exit) = (self.clone(), self.clone());
        let (data_id, exit_id) = (id.clone(), id.clone());
        pump(
            &format!("term-{n}"),
            process.reader,
            process.waiter,
            move |data| on_data.on_data(&data_id, data),
            move |code| on_exit.on_exit(&exit_id, code),
        );
        Ok(SpawnedTerminal {
            terminal_id: id,
            group_id: group,
            layout,
        })
    }

    fn on_data(&self, id: &str, data: &[u8]) {
        let project_dir = {
            let mut inner = self.inner.lock();
            let Some(e) = inner.terminals.get_mut(id) else {
                return;
            };
            e.last_activity_at = now_ms();
            e.buffer.push(data);
            e.project_dir.clone()
        };
        let _ = self.events.send(TerminalEvent::Output {
            terminal_id: id.to_string(),
            project_dir,
            data: Bytes::copy_from_slice(data),
        });
    }

    fn on_exit(&self, id: &str, exit_code: i32) {
        let Some(project_dir) = self.remove(id) else {
            return;
        };
        let _ = self.events.send(TerminalEvent::Exit {
            terminal_id: id.to_string(),
            project_dir: project_dir.clone(),
            exit_code,
        });
        self.emit_layout(&project_dir);
    }

    /// Drop the terminal from the table and the layout; returns its project dir.
    fn remove(&self, id: &str) -> Option<String> {
        let mut inner = self.inner.lock();
        let entry = inner.terminals.remove(id)?;
        let dir = entry.project_dir.clone();
        if let Some(state) = inner.projects.get_mut(&dir) {
            for g in &mut state.groups {
                g.pane_ids.retain(|p| p != id);
                if g.active_pane_id == id {
                    g.active_pane_id = g.pane_ids.first().cloned().unwrap_or_default();
                }
            }
            state.groups.retain(|g| !g.pane_ids.is_empty());
            if state
                .active_group_id
                .as_ref()
                .is_none_or(|a| !state.groups.iter().any(|g| &g.id == a))
            {
                state.active_group_id = state.groups.first().map(|g| g.id.clone());
            }
            if state.groups.is_empty() {
                inner.projects.remove(&dir);
            }
        }
        drop(inner);
        let mut entry = entry;
        entry.control.terminate(true);
        Some(dir)
    }

    /// Write raw bytes to the terminal. False when it does not exist.
    pub fn input(&self, id: &str, data: &[u8]) -> bool {
        let writer = {
            let mut inner = self.inner.lock();
            let Some(e) = inner.terminals.get_mut(id) else {
                return false;
            };
            let now = now_ms();
            e.last_input_at = now;
            e.last_activity_at = now;
            e.writer.clone()
        };
        let mut w = writer.lock();
        std::io::Write::write_all(&mut **w, data)
            .and_then(|()| std::io::Write::flush(&mut **w))
            .is_ok()
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        let mut inner = self.inner.lock();
        if let Some(e) = inner.terminals.get_mut(id)
            && let Err(err) = e.control.resize(cols.max(2), rows.max(2))
        {
            tracing::debug!(terminal = id, %err, "terminal resize failed");
        }
    }

    /// Kill the terminal and remove it from the layout. False when it does not exist.
    pub fn kill(&self, id: &str) -> bool {
        let Some(dir) = self.remove(id) else {
            return false;
        };
        self.emit_layout(&dir);
        true
    }

    pub fn kill_all(&self) {
        let ids: Vec<String> = self.inner.lock().terminals.keys().cloned().collect();
        for id in ids {
            self.remove(&id);
        }
    }

    pub fn rename(&self, id: &str, label: &str) -> bool {
        let label = label.trim();
        let dir = {
            let mut inner = self.inner.lock();
            let Some(e) = inner.terminals.get_mut(id) else {
                return false;
            };
            if !label.is_empty() {
                e.label = label.to_string();
            }
            e.project_dir.clone()
        };
        self.emit_layout(&dir);
        true
    }

    /// Raw scrollback of a terminal (also counts as activity for LRU). `None` when unknown.
    pub fn buffer(&self, id: &str) -> Option<Vec<u8>> {
        let mut inner = self.inner.lock();
        let e = inner.terminals.get_mut(id)?;
        e.last_activity_at = now_ms();
        Some(e.buffer.to_vec())
    }

    pub fn list(&self) -> Vec<TerminalInfo> {
        let inner = self.inner.lock();
        let mut v: Vec<_> = inner
            .terminals
            .values()
            .map(|e| TerminalInfo {
                terminal_id: e.id.clone(),
                project_dir: e.project_dir.clone(),
                label: e.label.clone(),
                last_activity_at: e.last_activity_at,
                last_input_at: e.last_input_at,
            })
            .collect();
        v.sort_by(|a, b| a.terminal_id.cmp(&b.terminal_id));
        v
    }

    pub fn activate(
        &self,
        project_dir: &str,
        group_id: Option<&str>,
        pane_id: Option<&str>,
    ) -> FreeTerminalLayout {
        {
            let mut inner = self.inner.lock();
            if let Some(state) = inner.projects.get_mut(project_dir)
                && let Some(g) =
                    group_id.and_then(|id| state.groups.iter_mut().find(|g| g.id == id))
            {
                state.active_group_id = Some(g.id.clone());
                if let Some(p) = pane_id.filter(|p| g.pane_ids.iter().any(|x| x == p)) {
                    g.active_pane_id = p.to_string();
                }
            }
        }
        self.emit_layout(project_dir);
        self.layout(project_dir)
    }

    fn enforce_lru(&self, cap: usize) {
        loop {
            let victim = {
                let inner = self.inner.lock();
                if inner.terminals.len() < cap {
                    return;
                }
                inner
                    .terminals
                    .values()
                    .min_by_key(|e| e.last_activity_at)
                    .map(|e| e.id.clone())
            };
            match victim {
                Some(id) => {
                    self.kill(&id);
                }
                None => return,
            }
        }
    }

    /// Kill terminals without input for `freeTerminalIdleTimeoutMinutes` (policy `idle` only).
    /// Returns the ids that were killed.
    pub fn sweep_idle(&self, now: u64) -> Vec<String> {
        let settings = (self.settings)();
        if settings.policy != FreeTerminalLifecyclePolicy::Idle {
            return Vec::new();
        }
        let timeout = u64::from(settings.idle_timeout_minutes.max(1)) * 60_000;
        let stale: Vec<String> = self
            .inner
            .lock()
            .terminals
            .values()
            .filter(|e| now.saturating_sub(e.last_input_at) > timeout)
            .map(|e| e.id.clone())
            .collect();
        for id in &stale {
            self.kill(id);
        }
        stale
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::fake_pty::FakePtyBackend;
    use std::time::{Duration, Instant};

    fn wait_for(mut f: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < end, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn manager(settings: TerminalSettings) -> (Arc<TerminalManager>, Arc<FakePtyBackend>) {
        let backend = Arc::new(FakePtyBackend::new());
        let m = TerminalManager::new(backend.clone(), move || settings.clone());
        (m, backend)
    }

    fn dir() -> String {
        std::env::temp_dir().to_string_lossy().into_owned()
    }

    #[test]
    fn spawn_input_output_buffer_and_layout() {
        let (m, backend) = manager(TerminalSettings::default());
        let mut rx = m.subscribe();
        let t = m.spawn(&dir(), None, None, 80, 24).unwrap();
        assert_eq!(t.layout.groups.len(), 1);
        assert_eq!(t.layout.panes[0].label, "Terminal 1");
        assert!(m.input(&t.terminal_id, b"echo hi\r"));
        assert_eq!(backend.ptys()[0].written(), "echo hi\r");
        backend.ptys()[0].emit_data(b"hi\r\n");
        wait_for(|| m.buffer(&t.terminal_id).is_some_and(|b| b == b"hi\r\n"));
        let mut got_output = false;
        while let Ok(ev) = rx.try_recv() {
            if let TerminalEvent::Output { data, .. } = ev {
                got_output = data.as_ref() == b"hi\r\n";
            }
        }
        assert!(got_output);
        m.resize(&t.terminal_id, 100, 30);
        assert_eq!(backend.ptys()[0].resizes(), vec![(100, 30)]);
        assert!(!m.input("nope", b"x"));
        assert_eq!(m.list().len(), 1);
    }

    #[test]
    fn split_joins_group_and_new_tab_makes_a_group() {
        let (m, _) = manager(TerminalSettings::default());
        let d = dir();
        let a = m.spawn(&d, None, None, 80, 24).unwrap();
        let b = m.spawn(&d, None, Some(&a.group_id), 80, 24).unwrap();
        assert_eq!(b.group_id, a.group_id);
        assert_eq!(b.layout.groups[0].pane_ids.len(), 2);
        assert_eq!(b.layout.groups[0].active_pane_id, b.terminal_id);
        let c = m.spawn(&d, None, None, 80, 24).unwrap();
        assert_ne!(c.group_id, a.group_id);
        assert_eq!(
            c.layout.active_group_id.as_deref(),
            Some(c.group_id.as_str())
        );
        let l = m.activate(&d, Some(&a.group_id), Some(&a.terminal_id));
        assert_eq!(l.active_group_id.as_deref(), Some(a.group_id.as_str()));
        assert_eq!(l.groups[0].active_pane_id, a.terminal_id);
        assert!(m.rename(&c.terminal_id, "build"));
        assert!(m.layout(&d).panes.iter().any(|p| p.label == "build"));
    }

    #[test]
    fn exit_and_kill_remove_from_layout() {
        let (m, backend) = manager(TerminalSettings::default());
        let d = dir();
        let a = m.spawn(&d, None, None, 80, 24).unwrap();
        let b = m.spawn(&d, None, None, 80, 24).unwrap();
        let mut rx = m.subscribe();
        backend.ptys()[0].emit_exit(3);
        wait_for(|| m.layout(&d).groups.len() == 1);
        let mut exit = None;
        wait_for(|| {
            while let Ok(ev) = rx.try_recv() {
                if let TerminalEvent::Exit {
                    terminal_id,
                    exit_code,
                    ..
                } = ev
                {
                    exit = Some((terminal_id, exit_code));
                }
            }
            exit.is_some()
        });
        assert_eq!(exit, Some((a.terminal_id.clone(), 3)));
        assert!(m.kill(&b.terminal_id));
        assert!(!m.kill(&b.terminal_id));
        assert_eq!(
            m.layout(&d),
            FreeTerminalLayout {
                project_dir: d.clone(),
                ..Default::default()
            }
        );
        assert_eq!(backend.ptys()[1].signals(), vec![true]);
    }

    #[test]
    fn lru_evicts_least_recently_active() {
        let settings = TerminalSettings {
            policy: FreeTerminalLifecyclePolicy::Lru,
            max_count: 2,
            ..TerminalSettings::default()
        };
        let (m, _) = manager(settings);
        let d = dir();
        let a = m.spawn(&d, None, None, 80, 24).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let b = m.spawn(&d, None, None, 80, 24).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        m.buffer(&a.terminal_id); // touch a: b is now the oldest
        std::thread::sleep(Duration::from_millis(5));
        let c = m.spawn(&d, None, None, 80, 24).unwrap();
        let ids: Vec<_> = m.list().into_iter().map(|t| t.terminal_id).collect();
        assert!(ids.contains(&a.terminal_id) && ids.contains(&c.terminal_id));
        assert!(!ids.contains(&b.terminal_id));
    }

    #[test]
    fn idle_sweep_only_for_idle_policy() {
        let d = dir();
        let (m, _) = manager(TerminalSettings::default());
        m.spawn(&d, None, None, 80, 24).unwrap();
        assert!(m.sweep_idle(now_ms() + 10_000_000).is_empty());
        let settings = TerminalSettings {
            policy: FreeTerminalLifecyclePolicy::Idle,
            idle_timeout_minutes: 1,
            ..TerminalSettings::default()
        };
        let (m, _) = manager(settings);
        let t = m.spawn(&d, None, None, 80, 24).unwrap();
        assert!(m.sweep_idle(now_ms()).is_empty());
        assert_eq!(m.sweep_idle(now_ms() + 61_000), vec![t.terminal_id]);
        assert!(m.list().is_empty());
    }

    #[test]
    fn scrollback_cap_follows_setting() {
        let settings = TerminalSettings {
            scrollback_lines: 100,
            ..TerminalSettings::default()
        };
        let (m, backend) = manager(settings);
        let t = m.spawn(&dir(), None, None, 80, 24).unwrap();
        backend.ptys()[0].emit_data(&vec![b'x'; 100 * BYTES_PER_LINE + 500]);
        wait_for(|| {
            m.buffer(&t.terminal_id)
                .is_some_and(|b| b.len() == 100 * BYTES_PER_LINE)
        });
    }

    #[test]
    fn shell_resolution() {
        let env = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let auto = ShellConfig::default();
        assert_eq!(
            resolve_shell(&auto, Os::Linux, &env(&[("SHELL", "/bin/zsh")])),
            ("/bin/zsh".into(), vec!["-l".into()])
        );
        assert_eq!(resolve_shell(&auto, Os::Linux, &env(&[])).0, "/bin/sh");
        assert_eq!(
            resolve_shell(&auto, Os::Windows, &env(&[("COMSPEC", "C:\\x\\cmd.exe")])).0,
            "C:\\x\\cmd.exe"
        );
        assert_eq!(resolve_shell(&auto, Os::Windows, &env(&[])).0, "cmd.exe");
        let custom = ShellConfig {
            mode: TerminalShellMode::Custom,
            path: "/usr/bin/fish".into(),
            args: "-i -l".into(),
        };
        assert_eq!(
            resolve_shell(&custom, Os::MacOs, &env(&[("SHELL", "/bin/zsh")])),
            ("/usr/bin/fish".into(), vec!["-i".into(), "-l".into()])
        );
        let direct = ShellConfig::direct();
        assert!(
            resolve_shell(&direct, Os::Linux, &env(&[("SHELL", "/bin/bash")]))
                .1
                .is_empty()
        );
    }
}
