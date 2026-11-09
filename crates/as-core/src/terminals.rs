//! Free terminals: plain shell PTYs per project (port of Hydra's free-terminal routes in
//! `DaemonServer.ts`).
//!
//! A project has *groups* (tabs); a group has one or more *panes* laid out as a split tree
//! ([`FreeTerminalNode`]: `Leaf(pane)` | `Split { dir, ratio, a, b }`, so panes can be split right
//! and down). An exited terminal is dropped from the layout like in Hydra. The layout (not the
//! shells) is saved in `terminals.json` ([`crate::terminals_store`]) and the shells are spawned again,
//! lazily, by [`TerminalManager::restore_project`] after a daemon restart. Lifecycle: `explicit` keeps everything, `lru` evicts the
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
use crate::terminal_tree::{FreeTerminalNode, SplitDir};
use crate::terminals_store::{PersistedGroup, PersistedPane, PersistedProject, PersistedTerminals};
use crate::types::{AppConfig, FreeTerminalLifecyclePolicy, TerminalShellMode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct FreeTerminalPane {
    pub id: String,
    pub label: String,
    pub exited: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct FreeTerminalGroup {
    pub id: String,
    /// Panes in reading order of `tree` (left to right, top to bottom).
    pub pane_ids: Vec<String>,
    pub active_pane_id: String,
    /// How the panes are arranged. `None` (older daemons): side by side, equal shares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<FreeTerminalNode>,
}

impl FreeTerminalGroup {
    /// The split tree, deriving the old flat arrangement when there is none.
    pub fn effective_tree(&self) -> Option<FreeTerminalNode> {
        self.tree
            .clone()
            .or_else(|| FreeTerminalNode::row(&self.pane_ids))
    }

    /// Keep `pane_ids` in the reading order of the tree.
    fn sync_panes(&mut self) {
        if let Some(t) = &self.tree {
            self.pane_ids = t.leaves().into_iter().map(String::from).collect();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct FreeTerminalLayout {
    pub project_dir: String,
    pub groups: Vec<FreeTerminalGroup>,
    pub active_group_id: Option<String>,
    pub panes: Vec<FreeTerminalPane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub terminal_id: String,
    pub project_dir: String,
    pub label: String,
    pub last_activity_at: u64,
    pub last_input_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
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
    cwd: String,
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
    /// Saved layouts whose shells have not been spawned yet (after a daemon restart).
    restored: HashMap<String, PersistedProject>,
}

/// Where [`TerminalManager::spawn_with`] puts the new shell.
#[derive(Debug, Clone, Default)]
pub struct SpawnRequest {
    pub project_dir: String,
    /// Working directory; defaults to the one of the pane being split, else the project dir.
    pub cwd: Option<String>,
    /// Join this tab (a split); `None` opens a new tab.
    pub group_id: Option<String>,
    /// The pane to split (default: the tab's active pane).
    pub split_pane: Option<String>,
    /// Where the new pane goes relative to it (default: right).
    pub split_dir: Option<SplitDir>,
    pub cols: u16,
    pub rows: u16,
}

/// A shell that was started but is not in the layout yet.
struct Started {
    id: String,
    n: u64,
    reader: Box<dyn std::io::Read + Send>,
    waiter: Box<dyn crate::agents::pty::PtyWaiter>,
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
    /// the terminal joins that tab as another pane (split right of the active one), otherwise it
    /// opens a new tab.
    pub fn spawn(
        self: &Arc<Self>,
        project_dir: &str,
        cwd: Option<&str>,
        group_id: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<SpawnedTerminal> {
        self.spawn_with(SpawnRequest {
            project_dir: project_dir.to_string(),
            cwd: cwd.map(String::from),
            group_id: group_id.map(String::from),
            cols,
            rows,
            ..Default::default()
        })
    }

    /// Start the process and register the terminal (not in any tab yet).
    fn start_shell(
        &self,
        project_dir: &str,
        cwd: &str,
        label: String,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<Started> {
        let settings = (self.settings)();
        let mut overrides = self.env_overrides.lock().clone();
        overrides.insert("TERM".into(), Some("xterm-256color".into()));
        overrides.insert("FORCE_COLOR".into(), Some("1".into()));
        let env = build_agent_env_from_process(&overrides);
        let (cmd, args) = resolve_shell(&settings.shell, Os::current(), &env);
        let request = PtySpawnRequest {
            cmd,
            args,
            cwd: cwd.to_string(),
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
        self.inner.lock().terminals.insert(
            id.clone(),
            Entry {
                id: id.clone(),
                project_dir: project_dir.to_string(),
                label,
                cwd: cwd.to_string(),
                buffer: OutputBuffer::with_cap(cap),
                control: process.control,
                writer: Arc::new(Mutex::new(process.writer)),
                last_activity_at: now,
                last_input_at: now,
            },
        );
        Ok(Started {
            id,
            n,
            reader: process.reader,
            waiter: process.waiter,
        })
    }

    /// Start the reader and exit threads of a shell that is now part of the layout.
    fn run(self: &Arc<Self>, started: Started) {
        let Started {
            id,
            n,
            reader,
            waiter,
        } = started;
        let (on_data, on_exit) = (self.clone(), self.clone());
        let (data_id, exit_id) = (id.clone(), id);
        pump(
            &format!("term-{n}"),
            reader,
            waiter,
            move |data| on_data.on_data(&data_id, data),
            move |code| on_exit.on_exit(&exit_id, code),
        );
    }

    /// [`spawn`](Self::spawn) with control over where the pane goes.
    pub fn spawn_with(self: &Arc<Self>, req: SpawnRequest) -> anyhow::Result<SpawnedTerminal> {
        // Saved shells of this project come back first; the new one joins them.
        self.restore_project(&req.project_dir);
        let settings = (self.settings)();
        if settings.policy == FreeTerminalLifecyclePolicy::Lru {
            self.enforce_lru(settings.max_count.max(1) as usize);
        }
        let (target, inherited_cwd) = {
            let inner = self.inner.lock();
            let target = inner
                .projects
                .get(&req.project_dir)
                .and_then(|s| {
                    s.groups
                        .iter()
                        .find(|g| Some(&g.id) == req.group_id.as_ref())
                })
                .and_then(|g| {
                    req.split_pane
                        .clone()
                        .filter(|p| g.pane_ids.contains(p))
                        .or_else(|| {
                            Some(g.active_pane_id.clone()).filter(|p| g.pane_ids.contains(p))
                        })
                        .or_else(|| g.pane_ids.last().cloned())
                });
            let cwd = target
                .as_ref()
                .and_then(|t| inner.terminals.get(t))
                .map(|e| e.cwd.clone());
            (target, cwd)
        };
        let cwd = req
            .cwd
            .clone()
            .filter(|c| !c.is_empty())
            .or(inherited_cwd)
            .unwrap_or_else(|| req.project_dir.clone());
        let label = {
            let mut inner = self.inner.lock();
            let state = inner.projects.entry(req.project_dir.clone()).or_default();
            state.next_label += 1;
            format!("Terminal {}", state.next_label)
        };
        let started = match self.start_shell(&req.project_dir, &cwd, label, req.cols, req.rows) {
            Ok(started) => started,
            Err(err) => {
                // Give the label back and do not leave an empty project behind.
                let mut inner = self.inner.lock();
                if let Some(state) = inner.projects.get_mut(&req.project_dir) {
                    state.next_label = state.next_label.saturating_sub(1);
                    if state.groups.is_empty() {
                        inner.projects.remove(&req.project_dir);
                    }
                }
                return Err(err);
            }
        };
        let (id, n) = (started.id.clone(), started.n);
        let (group, layout);
        {
            let mut inner = self.inner.lock();
            let state = inner.projects.entry(req.project_dir.clone()).or_default();
            let existing = req
                .group_id
                .as_deref()
                .and_then(|g| state.groups.iter().position(|x| x.id == g));
            let idx = match existing {
                Some(idx) => {
                    let g = &mut state.groups[idx];
                    let dir = req.split_dir.unwrap_or(SplitDir::Horizontal);
                    let mut tree = g
                        .effective_tree()
                        .unwrap_or_else(|| FreeTerminalNode::leaf(id.clone()));
                    let placed = target
                        .as_deref()
                        .is_some_and(|t| tree.split_leaf(t, &id, dir));
                    if !placed {
                        tree = FreeTerminalNode::Split {
                            dir,
                            ratio: crate::terminal_tree::DEFAULT_RATIO,
                            a: Box::new(tree),
                            b: Box::new(FreeTerminalNode::leaf(id.clone())),
                        };
                    }
                    g.tree = Some(tree);
                    g.sync_panes();
                    idx
                }
                None => {
                    state.groups.push(FreeTerminalGroup {
                        id: format!("grp-{n}"),
                        pane_ids: vec![id.clone()],
                        active_pane_id: id.clone(),
                        tree: Some(FreeTerminalNode::leaf(id.clone())),
                    });
                    state.groups.len() - 1
                }
            };
            state.groups[idx].active_pane_id = id.clone();
            state.active_group_id = Some(state.groups[idx].id.clone());
            group = state.groups[idx].id.clone();
            layout = Self::layout_locked(&inner, &req.project_dir);
        }
        let _ = self.events.send(TerminalEvent::LayoutChanged {
            project_dir: req.project_dir.clone(),
            layout: layout.clone(),
        });
        self.run(started);
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
                let Some(at) = g.pane_ids.iter().position(|p| p == id) else {
                    continue;
                };
                g.tree = g.effective_tree().and_then(|t| t.remove_leaf(id));
                g.pane_ids.retain(|p| p != id);
                g.sync_panes();
                if g.active_pane_id == id {
                    // The pane before the closed one (reading order), else the first.
                    g.active_pane_id = g
                        .pane_ids
                        .get(at.saturating_sub(1))
                        .or_else(|| g.pane_ids.first())
                        .cloned()
                        .unwrap_or_default();
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

    /// Move the divider nearest above `pane_id` along `axis` by `delta` percentage points
    /// (positive: the first child grows, so the divider moves right / down).
    pub fn resize_pane(
        &self,
        project_dir: &str,
        pane_id: &str,
        axis: SplitDir,
        delta: i16,
    ) -> FreeTerminalLayout {
        let changed = {
            let mut inner = self.inner.lock();
            inner
                .projects
                .get_mut(project_dir)
                .and_then(|s| {
                    s.groups
                        .iter_mut()
                        .find(|g| g.pane_ids.iter().any(|p| p == pane_id))
                })
                .is_some_and(|g| {
                    let mut tree = g.effective_tree();
                    let changed = tree
                        .as_mut()
                        .is_some_and(|t| t.resize(pane_id, axis, delta));
                    g.tree = tree;
                    changed
                })
        };
        if changed {
            self.emit_layout(project_dir);
        }
        self.layout(project_dir)
    }

    // ── persistence ─────────────────────────────────────────────────────────

    /// The layouts to save: every running project plus the saved ones whose shells have not been
    /// spawned yet (so that they are not lost by the next save).
    pub fn export(&self) -> PersistedTerminals {
        let inner = self.inner.lock();
        let mut out = PersistedTerminals {
            projects: inner
                .restored
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        for (dir, state) in &inner.projects {
            let groups: Vec<PersistedGroup> = state
                .groups
                .iter()
                .filter_map(|g| {
                    Some(PersistedGroup {
                        id: g.id.clone(),
                        active_pane_id: g.active_pane_id.clone(),
                        tree: g.effective_tree()?,
                    })
                })
                .collect();
            if groups.is_empty() {
                out.projects.remove(dir);
                continue;
            }
            let panes = state
                .groups
                .iter()
                .flat_map(|g| g.pane_ids.iter())
                .filter_map(|id| inner.terminals.get(id))
                .map(|e| PersistedPane {
                    id: e.id.clone(),
                    label: e.label.clone(),
                    cwd: e.cwd.clone(),
                })
                .collect();
            out.projects.insert(
                dir.clone(),
                PersistedProject {
                    groups,
                    active_group_id: state.active_group_id.clone(),
                    panes,
                    next_label: state.next_label,
                },
            );
        }
        out
    }

    /// Remember saved layouts. No shell is started: [`restore_project`](Self::restore_project)
    /// does that when the project's terminal panel is first used.
    pub fn restore(&self, saved: PersistedTerminals) {
        let mut inner = self.inner.lock();
        for (dir, project) in saved.projects {
            if !inner.projects.contains_key(&dir) {
                inner.restored.insert(dir, project);
            }
        }
    }

    /// Spawn fresh shells for the saved layout of `project_dir`, each in its saved working
    /// directory (the project dir when that is gone), and rebuild the tabs and trees around them.
    /// At most `freeTerminalMaxCount` terminals exist in total: panes beyond that are dropped
    /// from the layout. Does nothing when the project has no saved layout.
    pub fn restore_project(self: &Arc<Self>, project_dir: &str) -> FreeTerminalLayout {
        let Some(saved) = self.inner.lock().restored.remove(project_dir) else {
            return self.layout(project_dir);
        };
        let room = {
            let max = (self.settings)().max_count.max(1) as usize;
            max.saturating_sub(self.inner.lock().terminals.len())
        };
        if room == 0 {
            // Keep it for later rather than losing it.
            self.inner
                .lock()
                .restored
                .insert(project_dir.to_string(), saved);
            return self.layout(project_dir);
        }
        let mut next_label = saved.next_label;
        let mut spawned: HashMap<String, Started> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for group in &saved.groups {
            for old in group.tree.leaves() {
                if order.len() >= room {
                    break;
                }
                let pane = saved.panes.iter().find(|p| p.id == old);
                let cwd = pane
                    .map(|p| p.cwd.as_str())
                    .filter(|c| !c.is_empty() && std::path::Path::new(c).is_dir())
                    .unwrap_or(project_dir);
                let label = pane.map_or_else(
                    || {
                        next_label += 1;
                        format!("Terminal {next_label}")
                    },
                    |p| p.label.clone(),
                );
                match self.start_shell(project_dir, cwd, label, 80, 24) {
                    Ok(started) => {
                        spawned.insert(old.to_string(), started);
                        order.push(old.to_string());
                    }
                    Err(err) => {
                        tracing::warn!(%err, project_dir, "could not restore a terminal");
                    }
                }
            }
        }
        // Rebuild the tabs around the shells that did start.
        let mut groups: Vec<FreeTerminalGroup> = Vec::new();
        let mut active_group = None;
        for saved_group in &saved.groups {
            let mut tree = Some(saved_group.tree.clone());
            for old in saved_group.tree.leaves() {
                if !spawned.contains_key(old) {
                    tree = tree.and_then(|t| t.remove_leaf(old));
                }
            }
            let Some(mut tree) = tree else { continue };
            let active_old = saved_group.active_pane_id.clone();
            let active = spawned.get(&active_old).map(|s| s.id.clone()).or_else(|| {
                tree.leaves()
                    .last()
                    .and_then(|l| spawned.get(*l))
                    .map(|s| s.id.clone())
            });
            tree.map_panes(&mut |old| {
                spawned
                    .get(old)
                    .map_or_else(|| old.to_string(), |s| s.id.clone())
            });
            let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
            let mut g = FreeTerminalGroup {
                id: format!("grp-{n}"),
                pane_ids: Vec::new(),
                active_pane_id: active.unwrap_or_default(),
                tree: Some(tree),
            };
            g.sync_panes();
            if saved.active_group_id.as_ref() == Some(&saved_group.id) {
                active_group = Some(g.id.clone());
            }
            groups.push(g);
        }
        let layout;
        {
            let mut inner = self.inner.lock();
            let state = inner.projects.entry(project_dir.to_string()).or_default();
            state.next_label = state.next_label.max(next_label);
            state.active_group_id = active_group.or_else(|| groups.first().map(|g| g.id.clone()));
            state.groups.extend(groups);
            layout = Self::layout_locked(&inner, project_dir);
        }
        let _ = self.events.send(TerminalEvent::LayoutChanged {
            project_dir: project_dir.to_string(),
            layout: layout.clone(),
        });
        for (_, started) in spawned {
            self.run(started);
        }
        layout
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
    use crate::terminals_store::TerminalStore;
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

    fn tree_of(l: &FreeTerminalLayout, g: usize) -> FreeTerminalNode {
        l.groups[g]
            .tree
            .clone()
            .expect("the daemon always sends a tree")
    }

    #[test]
    fn split_right_and_down_build_a_tree_and_close_promotes_the_sibling() {
        let (m, _) = manager(TerminalSettings::default());
        let d = dir();
        let a = m.spawn(&d, None, None, 80, 24).unwrap();
        assert_eq!(
            tree_of(&a.layout, 0),
            FreeTerminalNode::leaf(&a.terminal_id)
        );
        let split = |group: &str, pane: &str, dir| {
            m.spawn_with(SpawnRequest {
                project_dir: d.clone(),
                group_id: Some(group.into()),
                split_pane: Some(pane.into()),
                split_dir: Some(dir),
                cols: 80,
                rows: 24,
                ..Default::default()
            })
            .unwrap()
        };
        let b = split(&a.group_id, &a.terminal_id, SplitDir::Horizontal);
        let c = split(&a.group_id, &b.terminal_id, SplitDir::Vertical);
        let l = c.layout;
        assert_eq!(
            l.groups[0].pane_ids,
            [
                a.terminal_id.clone(),
                b.terminal_id.clone(),
                c.terminal_id.clone()
            ],
            "reading order"
        );
        assert_eq!(
            l.groups[0].active_pane_id, c.terminal_id,
            "the new pane is active"
        );
        let FreeTerminalNode::Split {
            dir: d0, b: right, ..
        } = tree_of(&l, 0)
        else {
            panic!("a split")
        };
        assert_eq!(d0, SplitDir::Horizontal);
        assert!(matches!(
            *right,
            FreeTerminalNode::Split {
                dir: SplitDir::Vertical,
                ..
            }
        ));
        // Closing b hands its place to c; the active pane (c) stays.
        assert!(m.kill(&b.terminal_id));
        let l = m.layout(&d);
        assert_eq!(
            l.groups[0].pane_ids,
            [a.terminal_id.clone(), c.terminal_id.clone()]
        );
        assert_eq!(
            tree_of(&l, 0).leaves(),
            [a.terminal_id.as_str(), c.terminal_id.as_str()]
        );
        // Without a split request, joining a tab splits the active pane to the right.
        let e = m.spawn(&d, None, Some(&a.group_id), 80, 24).unwrap();
        assert_eq!(e.layout.groups[0].pane_ids.last(), Some(&e.terminal_id));
    }

    #[test]
    fn a_split_inherits_the_working_directory_of_its_pane() {
        let (m, backend) = manager(TerminalSettings::default());
        let d = dir();
        let sub = tempfile::tempdir().unwrap();
        let sub_dir = sub.path().to_string_lossy().into_owned();
        let a = m.spawn(&d, Some(&sub_dir), None, 80, 24).unwrap();
        m.spawn_with(SpawnRequest {
            project_dir: d.clone(),
            group_id: Some(a.group_id.clone()),
            split_dir: Some(SplitDir::Vertical),
            cols: 80,
            rows: 24,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(backend.request(1).unwrap().cwd, sub_dir);
    }

    #[test]
    fn resize_pane_moves_dividers_and_announces_the_layout() {
        let (m, _) = manager(TerminalSettings::default());
        let d = dir();
        let a = m.spawn(&d, None, None, 80, 24).unwrap();
        let b = m
            .spawn_with(SpawnRequest {
                project_dir: d.clone(),
                group_id: Some(a.group_id.clone()),
                split_dir: Some(SplitDir::Horizontal),
                cols: 80,
                rows: 24,
                ..Default::default()
            })
            .unwrap();
        let mut rx = m.subscribe();
        let l = m.resize_pane(&d, &a.terminal_id, SplitDir::Horizontal, 15);
        let FreeTerminalNode::Split { ratio, .. } = tree_of(&l, 0) else {
            panic!()
        };
        assert_eq!(ratio, 65);
        assert!(matches!(
            rx.try_recv(),
            Ok(TerminalEvent::LayoutChanged { .. })
        ));
        // Nothing to move on the other axis: no event, same layout.
        let same = m.resize_pane(&d, &b.terminal_id, SplitDir::Vertical, 5);
        assert_eq!(same, l);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            m.resize_pane("/nope", "x", SplitDir::Horizontal, 5)
                .groups
                .len(),
            0
        );
    }

    /// Spawn the three-pane layout used by the persistence tests: `a | (b / c)` plus a second tab.
    fn three_panes(m: &Arc<TerminalManager>, d: &str, cwd: &str) -> Vec<SpawnedTerminal> {
        let a = m.spawn(d, Some(cwd), None, 80, 24).unwrap();
        let split = |pane: &str, dir| {
            m.spawn_with(SpawnRequest {
                project_dir: d.to_string(),
                group_id: Some(a.group_id.clone()),
                split_pane: Some(pane.into()),
                split_dir: Some(dir),
                cols: 80,
                rows: 24,
                ..Default::default()
            })
            .unwrap()
        };
        let b = split(&a.terminal_id, SplitDir::Horizontal);
        let c = split(&b.terminal_id, SplitDir::Vertical);
        let t2 = m.spawn(d, Some(cwd), None, 80, 24).unwrap();
        assert!(m.rename(&b.terminal_id, "build"));
        m.resize_pane(d, &a.terminal_id, SplitDir::Horizontal, 20);
        vec![a, b, c, t2]
    }

    /// The tree with every pane id replaced by its position in reading order, to compare layouts
    /// across restarts (the shells, and so their ids, are new).
    fn shape(l: &FreeTerminalLayout) -> Vec<(serde_json::Value, usize)> {
        l.groups
            .iter()
            .map(|g| {
                let ids = g.pane_ids.clone();
                let mut t = g.tree.clone().unwrap();
                t.map_panes(&mut |id| ids.iter().position(|p| p == id).unwrap().to_string());
                let active = ids.iter().position(|p| *p == g.active_pane_id).unwrap();
                (serde_json::to_value(&t).unwrap(), active)
            })
            .collect()
    }

    #[test]
    fn the_layout_survives_a_restart_with_fresh_shells_spawned_lazily() {
        let store_dir = tempfile::tempdir().unwrap();
        let store = TerminalStore::at(store_dir.path().join("terminals.json"));
        let cwd = tempfile::tempdir().unwrap();
        let cwd = cwd.path().to_string_lossy().into_owned();
        let d = dir();

        let (m1, _) = manager(TerminalSettings::default());
        three_panes(&m1, &d, &cwd);
        let before = m1.layout(&d);
        store.save(&m1.export()).unwrap();
        m1.kill_all();

        // The next daemon: reading the file starts no shell.
        let (m2, backend2) = manager(TerminalSettings::default());
        m2.restore(store.load());
        assert_eq!(backend2.spawn_count(), 0, "nothing is spawned at start");
        assert!(m2.layout(&d).groups.is_empty(), "not materialized yet");
        assert!(m2.list().is_empty());

        // First use of the project's panel: the whole layout comes back.
        let after = m2.restore_project(&d);
        assert_eq!(backend2.spawn_count(), 4);
        assert_eq!(
            shape(&after),
            shape(&before),
            "same trees, ratios and active panes"
        );
        assert_eq!(
            after.active_group_id.as_ref(),
            Some(&after.groups[1].id),
            "the tab that was active is active again"
        );
        let labels: Vec<_> = after.panes.iter().map(|p| p.label.clone()).collect();
        assert_eq!(labels, ["Terminal 1", "build", "Terminal 3", "Terminal 4"]);
        for i in 0..4 {
            assert_eq!(
                backend2.request(i).unwrap().cwd,
                cwd,
                "same working directory"
            );
        }
        let old_ids: Vec<_> = before.panes.iter().map(|p| &p.id).collect();
        assert!(
            after.panes.iter().all(|p| !old_ids.contains(&&p.id)),
            "fresh terminals"
        );
        // Asking again changes nothing and spawns nothing.
        assert_eq!(m2.restore_project(&d), after);
        assert_eq!(backend2.spawn_count(), 4);
        // And the next save holds the new layout, so a second restart works too.
        assert_eq!(shape(&m2.layout(&d)), shape(&before));
        assert_eq!(m2.export().projects[&d].groups.len(), 2);
    }

    #[test]
    fn unmaterialized_projects_stay_in_the_next_save() {
        let (m, _) = manager(TerminalSettings::default());
        let d = dir();
        three_panes(&m, &d, &d);
        let saved = m.export();
        let (m2, _) = manager(TerminalSettings::default());
        m2.restore(saved.clone());
        // Another project gets used, `d` is not touched: its saved layout must not vanish.
        let other = tempfile::tempdir().unwrap();
        m2.spawn(&other.path().to_string_lossy(), None, None, 80, 24)
            .unwrap();
        let again = m2.export();
        assert_eq!(again.projects[&d], saved.projects[&d]);
        assert_eq!(again.projects.len(), 2);
    }

    #[test]
    fn restore_respects_the_terminal_cap_and_missing_directories() {
        let (m, _) = manager(TerminalSettings::default());
        let d = dir();
        three_panes(&m, &d, &d);
        let mut saved = m.export();
        // One pane remembers a directory that no longer exists.
        let gone = std::env::temp_dir().join("as-terminals-test-gone-dir");
        saved.projects.get_mut(&d).unwrap().panes[1].cwd = gone.to_string_lossy().into_owned();

        let capped = TerminalSettings {
            max_count: 3,
            ..TerminalSettings::default()
        };
        let (m2, backend2) = manager(capped);
        m2.restore(saved.clone());
        let l = m2.restore_project(&d);
        assert_eq!(backend2.spawn_count(), 3, "freeTerminalMaxCount");
        assert_eq!(l.panes.len(), 3);
        assert_eq!(
            l.groups.len(),
            1,
            "the tab whose only pane did not fit is dropped"
        );
        assert_eq!(tree_of(&l, 0).leaves().len(), 3);
        assert_eq!(
            backend2.request(1).unwrap().cwd,
            d,
            "a vanished directory falls back to the project folder"
        );
        // No room at all: the saved layout is kept for later.
        let none = TerminalSettings {
            max_count: 1,
            ..TerminalSettings::default()
        };
        let (m3, backend3) = manager(none);
        let other = tempfile::tempdir().unwrap();
        m3.spawn(&other.path().to_string_lossy(), None, None, 80, 24)
            .unwrap();
        m3.restore(saved.clone());
        assert!(m3.restore_project(&d).groups.is_empty());
        assert_eq!(backend3.spawn_count(), 1);
        assert_eq!(m3.export().projects[&d], saved.projects[&d]);
    }

    #[test]
    fn a_failed_restore_spawn_drops_that_pane_only() {
        let (m, _) = manager(TerminalSettings::default());
        let d = dir();
        three_panes(&m, &d, &d);
        let saved = m.export();
        let (m2, backend2) = manager(TerminalSettings::default());
        m2.restore(saved);
        backend2.set_fail(Some("no pty"));
        let l = m2.restore_project(&d);
        assert!(l.groups.is_empty() && l.panes.is_empty());
        assert!(m2.list().is_empty());
    }

    #[test]
    fn spawning_into_a_project_with_a_saved_layout_restores_it_first() {
        let (m, _) = manager(TerminalSettings::default());
        let d = dir();
        three_panes(&m, &d, &d);
        let (m2, backend2) = manager(TerminalSettings::default());
        m2.restore(m.export());
        m2.spawn(&d, None, None, 80, 24).unwrap();
        assert_eq!(backend2.spawn_count(), 5, "4 restored + 1 new tab");
        assert_eq!(m2.layout(&d).groups.len(), 3);
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
