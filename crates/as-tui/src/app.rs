//! Application state and its update functions. No terminal and no daemon in here: input events
//! go in, [`Effect`]s (things to do outside) come out, so everything is unit-testable.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use as_core::sessions::profiles::resolve_profile;
use as_core::sessions::{ClaudeProfile, SessionSummary};
use as_core::{
    AgentState, AgentStatus, AppConfig, CreateAgentPayload, ProviderId, ProviderModelOption,
    get_default_model_for_provider,
};
use as_proto::{Event, NotificationKind, StudioNotification};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::keymap::{Command, KeyChord, Keymap};
use crate::new_agent::{FormAction, NewAgentForm};
use crate::sessions::{PickerAction, PickerOrigin, SessionPicker};
use crate::sidebar::{self, Row, RowKey};
use crate::term::{TerminalEmulator, Vt100Emulator, keys};
use crate::theme::{ColorMode, Theme};
use crate::ui::layout::{self, DEFAULT_SIDEBAR, LayoutParams, MAX_INPUT_LINES};
use crate::widgets::TextArea;

/// Terminals kept subscribed (the selected one plus recently viewed ones).
pub const MAX_LIVE_TERMS: usize = 10;
pub const TOAST_TTL: Duration = Duration::from_secs(5);
pub const MAX_TOASTS: usize = 5;
const WHEEL_LINES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// Every key goes to the selected agent's PTY (except the prefix).
    Terminal,
    /// Sidebar navigation.
    App,
    /// The input bar.
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connected,
    Reconnecting { attempt: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitChoice {
    Detach,
    StopAll,
    Cancel,
}

impl QuitChoice {
    pub const ALL: [QuitChoice; 3] = [QuitChoice::Detach, QuitChoice::StopAll, QuitChoice::Cancel];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitDecision {
    /// Nothing is running: leave right away (the daemon stays up).
    Exit,
    /// Ask: this many agents are running.
    Confirm { running: usize },
}

/// Agents with a process (or about to have one).
pub fn running_count(agents: &[AgentState]) -> usize {
    agents
        .iter()
        .filter(|a| matches!(a.status, AgentStatus::Running | AgentStatus::Starting))
        .count()
}

pub fn quit_decision(agents: &[AgentState]) -> QuitDecision {
    match running_count(agents) {
        0 => QuitDecision::Exit,
        running => QuitDecision::Confirm { running },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: String,
    pub title: String,
    pub body: String,
    pub kind: ToastKind,
    pub expires: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    NewAgent(Box<NewAgentForm>),
    Rename {
        agent_id: String,
        input: TextArea,
    },
    ConfirmRemove {
        agent_id: String,
        name: String,
    },
    Quit {
        running: usize,
        choice: QuitChoice,
        stopping: bool,
    },
    Help,
    /// `prefix o`: pick a session and resume it.
    Resume(Box<SessionPicker>),
    /// `prefix P`: Claude profile of a project.
    Profile(ProfileChooser),
}

/// One entry of the profile chooser.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileChoice {
    pub label: String,
    /// `None` = no mapping (the agent keeps the daemon's environment), `Some("")` = default profile.
    pub config_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileChooser {
    pub project_dir: String,
    pub choices: Vec<ProfileChoice>,
    pub idx: usize,
}

/// Something to do outside the state machine.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// ListAgents + GetConfig.
    Refresh,
    CreateAgent(CreateAgentPayload),
    /// Map the project to a Claude profile (`config_dir`, `""` = default), then create the agent.
    CreateAgentWithProfile {
        project_dir: String,
        config_dir: String,
        payload: CreateAgentPayload,
    },
    LoadProfiles,
    LoadModels(ProviderId),
    /// The sessions of a provider (all ages, hidden ones flagged); `force` rescans the disk.
    LoadSessions {
        provider: ProviderId,
        force: bool,
    },
    HideSession(String, bool),
    /// `None` removes the mapping.
    SetProjectProfile {
        project_dir: String,
        config_dir: Option<String>,
    },
    Kill(String),
    Restart(String),
    Remove(String),
    Rename(String, String),
    ToggleYolo(String, bool),
    SetGlobalYolo(bool),
    Subscribe(String),
    Unsubscribe(String),
    SendRaw(String, Vec<u8>),
    SendInput(String, String),
    Resize(String, u16, u16),
    SetFocus(bool),
    /// Stop the agents and the daemon, then quit.
    ShutdownAndQuit,
    /// Leave the TUI (detach).
    Quit,
    Bell,
}

/// Results of asynchronous work, fed back into [`App::handle_msg`].
#[derive(Debug)]
pub enum Msg {
    Agents(Vec<AgentState>),
    Config(Box<AppConfig>),
    Created(Result<Box<AgentState>, String>),
    Subscribed {
        agent_id: String,
        result: Result<Vec<u8>, String>,
    },
    Failed(String),
    ShutdownDone(Result<(), String>),
    Profiles(Vec<ClaudeProfile>),
    Models {
        provider: ProviderId,
        models: Vec<ProviderModelOption>,
    },
    Sessions {
        provider: ProviderId,
        result: Result<Vec<SessionSummary>, String>,
    },
}

pub enum SubState {
    /// Subscribe sent; live output that arrives before the buffer is held back here.
    Requested(Vec<u8>),
    Live,
    Failed,
}

pub struct Term {
    pub emu: Box<dyn TerminalEmulator>,
    pub sub: SubState,
    /// Last size announced to the daemon.
    pub sent_size: Option<(u16, u16)>,
}

#[derive(Debug, Clone, Default)]
pub struct SidebarState {
    pub collapsed: HashSet<String>,
    pub filter: String,
    /// Typing into the filter.
    pub filtering: bool,
    pub cursor: Option<RowKey>,
}

pub struct App {
    pub config: AppConfig,
    pub color_mode: ColorMode,
    pub theme: Theme,
    pub keymap: Keymap,
    pub agents: Vec<AgentState>,
    /// Claude profiles on this machine (for the project chips and the choosers).
    pub profiles: Vec<ClaudeProfile>,
    /// Models per provider as discovered by the daemon (the dialog falls back to the built-in ones).
    pub models: HashMap<ProviderId, Vec<ProviderModelOption>>,
    /// Running agents that went quiet (`AgentWaiting`) and have not produced output since.
    pub waiting: HashSet<String>,
    pub selected: Option<String>,
    pub focus: Focus,
    pub prefix_pending: bool,
    pub scroll_mode: bool,
    pub modal: Option<Modal>,
    pub sidebar: SidebarState,
    pub sidebar_width: u16,
    pub input_visible: bool,
    pub input: TextArea,
    pub terms: HashMap<String, Term>,
    /// Least recently viewed first.
    pub view_order: Vec<String>,
    pub toasts: Vec<Toast>,
    pub conn: ConnState,
    pub screen: Rect,
    pub terminal_focused: bool,
    pub cwd: PathBuf,
    pub now: Instant,
    pub quit: bool,
    pub dirty: bool,
    next_toast: u64,
}

impl App {
    pub fn new(
        config: AppConfig,
        agents: Vec<AgentState>,
        color_mode: ColorMode,
        cwd: PathBuf,
    ) -> Self {
        let theme = Theme::new(config.theme, color_mode);
        let keymap = Keymap::from_prefix_key(&config.prefix_key);
        let mut app = Self {
            config,
            color_mode,
            theme,
            keymap,
            agents: Vec::new(),
            profiles: Vec::new(),
            models: HashMap::new(),
            waiting: HashSet::new(),
            selected: None,
            focus: Focus::Terminal,
            prefix_pending: false,
            scroll_mode: false,
            modal: None,
            sidebar: SidebarState::default(),
            sidebar_width: DEFAULT_SIDEBAR,
            input_visible: false,
            input: TextArea::multi_line(),
            terms: HashMap::new(),
            view_order: Vec::new(),
            toasts: Vec::new(),
            conn: ConnState::Connected,
            screen: Rect::default(),
            terminal_focused: true,
            cwd,
            now: Instant::now(),
            quit: false,
            dirty: true,
            next_toast: 0,
        };
        app.agents = agents;
        app
    }

    /// Select the first agent and subscribe to it. Call once after construction.
    pub fn start(&mut self) -> Vec<Effect> {
        let mut fx = vec![
            Effect::SetFocus(self.terminal_focused),
            Effect::LoadProfiles,
        ];
        if let Some(i) = sidebar::agent_order(&self.agents, "").first() {
            let id = self.agents[*i].id.clone();
            fx.extend(self.select(&id));
        }
        fx
    }

    // ── queries ─────────────────────────────────────────────────────────────

    pub fn selected_agent(&self) -> Option<&AgentState> {
        let id = self.selected.as_ref()?;
        self.agents.iter().find(|a| a.id == *id)
    }

    pub fn agent(&self, id: &str) -> Option<&AgentState> {
        self.agents.iter().find(|a| a.id == id)
    }

    pub fn is_waiting(&self, agent: &AgentState) -> bool {
        agent.status == AgentStatus::Running && self.waiting.contains(&agent.id)
    }

    pub fn waiting_count(&self) -> usize {
        self.agents.iter().filter(|a| self.is_waiting(a)).count()
    }

    /// Terminal title: `(N) Agent Studio` while N agents wait for you.
    pub fn title(&self) -> String {
        match self.waiting_count() {
            0 => "Agent Studio".to_string(),
            n => format!("({n}) Agent Studio"),
        }
    }

    pub fn input_lines(&self) -> u16 {
        if self.input_visible {
            u16::try_from(self.input.line_count())
                .unwrap_or(MAX_INPUT_LINES)
                .clamp(1, MAX_INPUT_LINES)
        } else {
            0
        }
    }

    pub fn layout(&self) -> layout::Layout {
        layout::compute(
            self.screen,
            LayoutParams {
                sidebar_width: self.sidebar_width,
                input_lines: self.input_lines(),
            },
        )
    }

    /// Size the selected agent's terminal should have.
    pub fn pane_size(&self) -> (u16, u16) {
        if self.screen.width == 0 || self.screen.height == 0 {
            return (80, 24);
        }
        let t = self.layout().terminal;
        (t.width.max(2), t.height.max(2))
    }

    pub fn sidebar_rows(&self) -> Vec<Row> {
        sidebar::rows(&self.agents, &self.sidebar.collapsed, &self.sidebar.filter)
    }

    /// Label of the Claude profile a project is mapped to, unless it is the default one.
    pub fn profile_chip(&self, project_dir: &str) -> Option<String> {
        let mapped = resolve_profile(project_dir, &self.config.claude_profile_by_project)?;
        if mapped.is_empty() {
            return None;
        }
        let profile = self.profiles.iter().find(|p| p.config_dir == mapped);
        if profile.is_some_and(|p| p.is_default) {
            return None;
        }
        Some(as_core::sessions::profiles::profile_label(
            &mapped,
            &self.profiles,
        ))
    }

    pub fn sidebar_cursor_index(&self, rows: &[Row]) -> Option<usize> {
        let key = self.sidebar.cursor.as_ref()?;
        rows.iter()
            .position(|r| sidebar::row_key(&self.agents, r) == *key)
    }

    // ── toasts ──────────────────────────────────────────────────────────────

    pub fn toast(&mut self, kind: ToastKind, title: &str, body: &str) {
        self.next_toast += 1;
        self.toasts.push(Toast {
            id: format!("local-{}", self.next_toast),
            title: title.to_string(),
            body: body.to_string(),
            kind,
            expires: self.now + TOAST_TTL,
        });
        while self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
        self.dirty = true;
    }

    /// Advance the clock: expire toasts.
    pub fn tick(&mut self, now: Instant) {
        self.now = now;
        let before = self.toasts.len();
        self.toasts.retain(|t| t.expires > now);
        if self.toasts.len() != before {
            self.dirty = true;
        }
    }

    // ── daemon input ────────────────────────────────────────────────────────

    pub fn handle_event(&mut self, event: Event) -> Vec<Effect> {
        let mut fx = Vec::new();
        match event {
            Event::AgentOutput { agent_id, data } => fx.extend(self.on_output(&agent_id, &data)),
            Event::AgentStatus {
                agent_id,
                status,
                session_id,
                model,
            } => {
                if let Some(a) = self.agents.iter_mut().find(|a| a.id == agent_id) {
                    a.status = status;
                    a.config.session_id = session_id.or(a.config.session_id.take());
                    a.config.model = model;
                } else {
                    fx.push(Effect::Refresh);
                }
                self.waiting.remove(&agent_id);
                self.dirty = true;
            }
            Event::AgentWaiting { agent_id } => {
                if self.agent(&agent_id).is_some() {
                    self.waiting.insert(agent_id);
                    self.dirty = true;
                }
            }
            Event::AgentsChanged => fx.push(Effect::Refresh),
            Event::ConfigChanged(config) => self.apply_config(*config),
            Event::DaemonShuttingDown => {
                self.toast(ToastKind::Warn, "Daemon", "The daemon is shutting down");
            }
            Event::Notification(n) => fx.extend(self.on_notification(&n)),
        }
        fx
    }

    fn on_notification(&mut self, n: &StudioNotification) -> Vec<Effect> {
        let kind = match n.kind {
            NotificationKind::AgentWaiting => ToastKind::Warn,
            NotificationKind::AgentIdle => ToastKind::Success,
            NotificationKind::AgentErrored => ToastKind::Error,
            NotificationKind::AgentStarted => ToastKind::Info,
        };
        self.toasts.push(Toast {
            id: n.id.clone(),
            title: n.title.clone(),
            body: n.body.clone(),
            kind,
            expires: self.now + TOAST_TTL,
        });
        while self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
        self.dirty = true;
        if n.kind == NotificationKind::AgentWaiting && self.config.enable_sound_effects {
            vec![Effect::Bell]
        } else {
            Vec::new()
        }
    }

    fn apply_config(&mut self, config: AppConfig) {
        if config.prefix_key != self.config.prefix_key {
            self.keymap = Keymap::from_prefix_key(&config.prefix_key);
        }
        self.theme = Theme::new(config.theme, self.color_mode);
        self.config = config;
        self.dirty = true;
    }

    fn on_output(&mut self, agent_id: &str, data: &[u8]) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.waiting.remove(agent_id) {
            self.dirty = true;
        }
        let Some(term) = self.terms.get_mut(agent_id) else {
            return fx;
        };
        match &mut term.sub {
            SubState::Requested(pending) => pending.extend_from_slice(data),
            SubState::Live => {
                term.emu.process(data);
                let replies = term.emu.take_replies();
                if !replies.is_empty() {
                    fx.push(Effect::SendRaw(agent_id.to_string(), replies));
                }
                if term.emu.take_bell() && self.selected.as_deref() == Some(agent_id) {
                    fx.push(Effect::Bell);
                }
            }
            SubState::Failed => {}
        }
        if self.selected.as_deref() == Some(agent_id) {
            self.dirty = true;
        }
        fx
    }

    pub fn handle_msg(&mut self, msg: Msg) -> Vec<Effect> {
        let mut fx = Vec::new();
        match msg {
            Msg::Agents(agents) => fx.extend(self.set_agents(agents)),
            Msg::Config(config) => self.apply_config(*config),
            Msg::Created(Ok(agent)) => {
                let id = agent.id.clone();
                if self.agent(&id).is_none() {
                    self.agents.push(*agent);
                }
                self.modal = None;
                self.sidebar.filter.clear();
                fx.extend(self.select(&id));
                self.focus = Focus::Terminal;
                self.dirty = true;
            }
            Msg::Created(Err(error)) => {
                if let Some(Modal::NewAgent(form)) = &mut self.modal {
                    form.submitting = false;
                    form.error = Some(error);
                } else {
                    self.toast(ToastKind::Error, "Could not create the agent", &error);
                }
                self.dirty = true;
            }
            Msg::Subscribed { agent_id, result } => {
                fx.extend(self.on_subscribed(&agent_id, result))
            }
            Msg::Failed(error) => self.toast(ToastKind::Error, "Error", &error),
            Msg::Profiles(profiles) => {
                self.profiles = profiles.clone();
                if let Some(Modal::NewAgent(form)) = &mut self.modal {
                    form.set_profiles(profiles);
                }
                self.dirty = true;
            }
            Msg::Models { provider, models } => {
                if let Some(Modal::NewAgent(form)) = &mut self.modal {
                    form.set_models(provider, models.clone());
                }
                self.models.insert(provider, models);
                self.dirty = true;
            }
            Msg::Sessions { provider, result } => {
                match &mut self.modal {
                    Some(Modal::Resume(picker)) => picker.set_sessions(provider, result),
                    Some(Modal::NewAgent(form)) => {
                        if let Some(picker) = &mut form.picker {
                            picker.set_sessions(provider, result);
                        }
                    }
                    _ => {}
                }
                self.dirty = true;
            }
            Msg::ShutdownDone(result) => match result {
                Ok(()) => {
                    self.quit = true;
                    fx.push(Effect::Quit);
                }
                Err(error) => {
                    self.modal = None;
                    self.toast(ToastKind::Error, "Could not stop the agents", &error);
                }
            },
        }
        fx
    }

    fn on_subscribed(&mut self, agent_id: &str, result: Result<Vec<u8>, String>) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(term) = self.terms.get_mut(agent_id) else {
            return fx;
        };
        match result {
            Ok(buffer) => {
                let pending = match std::mem::replace(&mut term.sub, SubState::Live) {
                    SubState::Requested(p) => p,
                    _ => Vec::new(),
                };
                term.emu.process(&buffer);
                // Queries in the replayed history were answered long ago: drop those replies.
                term.emu.take_replies();
                term.emu.take_bell();
                term.emu.process(&pending);
                let replies = term.emu.take_replies();
                if !replies.is_empty() {
                    fx.push(Effect::SendRaw(agent_id.to_string(), replies));
                }
                term.emu.take_bell();
            }
            Err(error) => {
                term.sub = SubState::Failed;
                self.toast(ToastKind::Error, "Could not attach", &error);
            }
        }
        self.dirty = true;
        fx
    }

    fn set_agents(&mut self, agents: Vec<AgentState>) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.agents = agents;
        let ids: HashSet<String> = self.agents.iter().map(|a| a.id.clone()).collect();
        self.waiting.retain(|id| ids.contains(id));
        self.terms.retain(|id, _| ids.contains(id));
        self.view_order.retain(|id| ids.contains(id));
        match self.selected.clone() {
            Some(id) if ids.contains(&id) => {
                // Make sure the (possibly restored) selection has a terminal.
                if !self.terms.contains_key(&id) {
                    fx.extend(self.attach_term(&id));
                }
            }
            _ => {
                self.selected = None;
                self.scroll_mode = false;
                if let Some(i) = sidebar::agent_order(&self.agents, "").first() {
                    let id = self.agents[*i].id.clone();
                    fx.extend(self.select(&id));
                }
            }
        }
        if let Some(RowKey::Agent(id)) = &self.sidebar.cursor
            && !ids.contains(id)
        {
            self.sidebar.cursor = self.selected.clone().map(RowKey::Agent);
        }
        self.dirty = true;
        fx
    }

    // ── connection ──────────────────────────────────────────────────────────

    pub fn on_disconnected(&mut self) {
        self.conn = match self.conn {
            ConnState::Connected => ConnState::Reconnecting { attempt: 1 },
            ConnState::Reconnecting { attempt } => ConnState::Reconnecting {
                attempt: attempt + 1,
            },
        };
        self.dirty = true;
    }

    /// A new connection is up: resync everything, re-subscribe (subscriptions died with the
    /// old connection, and the daemon replays the whole buffer).
    pub fn on_reconnected(&mut self) -> Vec<Effect> {
        self.conn = ConnState::Connected;
        let (cols, rows) = self.pane_size();
        let mut fx = vec![
            Effect::Refresh,
            Effect::SetFocus(self.terminal_focused),
            Effect::LoadProfiles,
        ];
        for (id, term) in &mut self.terms {
            term.emu = Box::new(Vt100Emulator::new(cols, rows));
            term.sub = SubState::Requested(Vec::new());
            term.sent_size = None;
            fx.push(Effect::Subscribe(id.clone()));
        }
        fx.extend(self.sync_layout());
        self.toast(ToastKind::Success, "Daemon", "Reconnected");
        fx
    }

    // ── layout / resize ─────────────────────────────────────────────────────

    pub fn on_resize(&mut self, width: u16, height: u16) -> Vec<Effect> {
        self.screen = Rect::new(0, 0, width, height);
        self.dirty = true;
        self.sync_layout()
    }

    /// Keep the selected terminal and its PTY at the pane size.
    pub fn sync_layout(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(id) = self.selected.clone() else {
            return fx;
        };
        let (cols, rows) = self.pane_size();
        let known = self.screen.width > 0;
        if let Some(term) = self.terms.get_mut(&id) {
            if term.emu.size() != (cols, rows) {
                term.emu.resize(cols, rows);
                self.dirty = true;
            }
            if known && term.sent_size != Some((cols, rows)) {
                term.sent_size = Some((cols, rows));
                fx.push(Effect::Resize(id, cols, rows));
            }
        }
        fx
    }

    pub fn on_focus(&mut self, focused: bool) -> Vec<Effect> {
        self.terminal_focused = focused;
        vec![Effect::SetFocus(focused)]
    }

    // ── selection ───────────────────────────────────────────────────────────

    fn attach_term(&mut self, id: &str) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.view_order.retain(|v| v != id);
        self.view_order.push(id.to_string());
        if !self.terms.contains_key(id) {
            let (cols, rows) = self.pane_size();
            self.terms.insert(
                id.to_string(),
                Term {
                    emu: Box::new(Vt100Emulator::new(cols, rows)),
                    sub: SubState::Requested(Vec::new()),
                    sent_size: None,
                },
            );
            fx.push(Effect::Subscribe(id.to_string()));
        }
        while self.terms.len() > MAX_LIVE_TERMS {
            let Some(pos) = self
                .view_order
                .iter()
                .position(|v| self.selected.as_deref() != Some(v.as_str()) && v != id)
            else {
                break;
            };
            let old = self.view_order.remove(pos);
            if self.terms.remove(&old).is_some() {
                fx.push(Effect::Unsubscribe(old));
            }
        }
        fx
    }

    pub fn select(&mut self, id: &str) -> Vec<Effect> {
        if self.agent(id).is_none() {
            return Vec::new();
        }
        let changed = self.selected.as_deref() != Some(id);
        self.selected = Some(id.to_string());
        self.sidebar.cursor = Some(RowKey::Agent(id.to_string()));
        if changed {
            self.scroll_mode = false;
        }
        let mut fx = self.attach_term(id);
        fx.extend(self.sync_layout());
        self.dirty = true;
        fx
    }

    /// Move through agents in sidebar order (wraps around).
    fn step_agent(&mut self, delta: isize) -> Vec<Effect> {
        let order = sidebar::agent_order(&self.agents, &self.sidebar.filter);
        if order.is_empty() {
            return Vec::new();
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|id| order.iter().position(|i| self.agents[*i].id == *id));
        let next = match current {
            Some(pos) => (pos as isize + delta).rem_euclid(order.len() as isize) as usize,
            None if delta > 0 => 0,
            None => order.len() - 1,
        };
        let id = self.agents[order[next]].id.clone();
        self.select(&id)
    }

    fn jump_agent(&mut self, n: u8) -> Vec<Effect> {
        let order = sidebar::agent_order(&self.agents, &self.sidebar.filter);
        match order.get(usize::from(n).saturating_sub(1)) {
            Some(i) => {
                let id = self.agents[*i].id.clone();
                self.select(&id)
            }
            None => Vec::new(),
        }
    }

    /// Move the sidebar cursor over every row (headers too); landing on an agent selects it.
    fn move_cursor(&mut self, delta: isize) -> Vec<Effect> {
        let rows = self.sidebar_rows();
        if rows.is_empty() {
            return Vec::new();
        }
        let next = match self.sidebar_cursor_index(&rows) {
            Some(pos) => (pos as isize + delta).clamp(0, rows.len() as isize - 1) as usize,
            None => 0,
        };
        let key = sidebar::row_key(&self.agents, &rows[next]);
        self.sidebar.cursor = Some(key.clone());
        self.dirty = true;
        match key {
            RowKey::Agent(id) => self.select(&id),
            RowKey::Header(_) => Vec::new(),
        }
    }

    fn toggle_collapse(&mut self) {
        let key = match &self.sidebar.cursor {
            Some(RowKey::Header(k)) => Some(k.clone()),
            Some(RowKey::Agent(id)) => self.agent(id).map(|a| {
                if a.is_manager {
                    sidebar::MANAGER_GROUP.to_string()
                } else {
                    a.project_dir.clone()
                }
            }),
            None => None,
        };
        if let Some(key) = key {
            if !self.sidebar.collapsed.remove(&key) {
                self.sidebar.collapsed.insert(key.clone());
            }
            self.sidebar.cursor = Some(RowKey::Header(key));
            self.dirty = true;
        }
    }

    // ── keyboard ────────────────────────────────────────────────────────────

    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        self.dirty = true;
        if self.modal.is_some() {
            return self.modal_key(key);
        }
        if self.scroll_mode {
            return match self.keymap.in_scroll_mode(&key) {
                Some(cmd) => self.exec(cmd),
                None => Vec::new(),
            };
        }
        if self.prefix_pending {
            self.prefix_pending = false;
            return match self.keymap.after_prefix(&key) {
                Some(cmd) => self.exec(cmd),
                None => Vec::new(),
            };
        }
        if self.keymap.is_prefix(&key) {
            self.prefix_pending = true;
            return Vec::new();
        }
        match self.focus {
            Focus::Terminal => self.terminal_key(key),
            Focus::App => self.app_key(key),
            Focus::Input => self.input_key(key),
        }
    }

    fn terminal_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(id) = self.selected.clone() else {
            return Vec::new();
        };
        if key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
        {
            self.scroll_mode = true;
            let cmd = if key.code == KeyCode::PageUp {
                Command::ScrollPageUp
            } else {
                Command::ScrollPageDown
            };
            return self.exec(cmd);
        }
        let application = self
            .terms
            .get(&id)
            .is_some_and(|t| t.emu.application_cursor());
        let bytes = keys::encode_key(key, application);
        self.send_bytes(&id, bytes)
    }

    fn send_bytes(&mut self, id: &str, bytes: Vec<u8>) -> Vec<Effect> {
        if bytes.is_empty() {
            return Vec::new();
        }
        if let Some(term) = self.terms.get_mut(id) {
            // Typing is a reply: the agent is no longer waiting for us, and snap back to live.
            term.emu.set_scrollback(0);
        }
        self.waiting.remove(id);
        vec![Effect::SendRaw(id.to_string(), bytes)]
    }

    fn app_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if self.sidebar.filtering {
            match key.code {
                KeyCode::Esc => {
                    self.sidebar.filter.clear();
                    self.sidebar.filtering = false;
                }
                KeyCode::Enter => self.sidebar.filtering = false,
                KeyCode::Backspace => {
                    self.sidebar.filter.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.sidebar.filter.push(c);
                }
                _ => {}
            }
            return Vec::new();
        }
        match self.keymap.in_app_focus(&key) {
            Some(cmd) => self.exec(cmd),
            None => Vec::new(),
        }
    }

    fn input_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let newline = matches!(key.code, KeyCode::Enter)
            && key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::Terminal;
                Vec::new()
            }
            _ if newline => {
                self.input.insert_newline();
                Vec::new()
            }
            KeyCode::Enter => {
                let text = self.input.text();
                let text = text.trim_end().to_string();
                let Some(id) = self.selected.clone() else {
                    self.toast(ToastKind::Warn, "Input", "No agent selected");
                    return Vec::new();
                };
                if text.trim().is_empty() {
                    return Vec::new();
                }
                self.input.clear();
                self.waiting.remove(&id);
                let mut fx = vec![Effect::SendInput(id, text)];
                fx.extend(self.sync_layout());
                fx
            }
            _ => {
                self.input.handle_key(&key);
                self.sync_layout()
            }
        }
    }

    pub fn handle_paste(&mut self, text: &str) -> Vec<Effect> {
        self.dirty = true;
        match &mut self.modal {
            Some(Modal::NewAgent(form)) => {
                if !form.handle_picker_paste(text) {
                    form.handle_paste(text);
                }
                return Vec::new();
            }
            Some(Modal::Resume(picker)) => {
                picker.handle_paste(text);
                return Vec::new();
            }
            Some(Modal::Rename { input, .. }) => {
                input.insert_str(text);
                return Vec::new();
            }
            Some(_) => return Vec::new(),
            None => {}
        }
        match self.focus {
            Focus::Input => {
                self.input.insert_str(text);
                self.sync_layout()
            }
            Focus::App => {
                if self.sidebar.filtering {
                    self.sidebar.filter.push_str(text.trim());
                }
                Vec::new()
            }
            Focus::Terminal => {
                let Some(id) = self.selected.clone() else {
                    return Vec::new();
                };
                let bracketed = self.terms.get(&id).is_some_and(|t| t.emu.bracketed_paste());
                let bytes = keys::encode_paste(text, bracketed);
                self.send_bytes(&id, bytes)
            }
        }
    }

    // ── mouse ───────────────────────────────────────────────────────────────

    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Vec<Effect> {
        let layout = self.layout();
        let at = ratatui::layout::Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if layout.terminal.contains(at) =>
            {
                if self.modal.is_some() || self.selected.is_none() {
                    return Vec::new();
                }
                self.scroll_mode = true;
                let cmd = if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                    Command::ScrollUp
                } else {
                    Command::ScrollDown
                };
                for _ in 0..WHEEL_LINES {
                    self.exec(cmd);
                }
                self.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(side) = layout.sidebar.filter(|s| s.contains(at)) {
                    if self.modal.is_some() {
                        return Vec::new();
                    }
                    let rows = self.sidebar_rows();
                    let list_h = side.height.saturating_sub(1);
                    let offset =
                        crate::ui::sidebar::scroll_offset(self.sidebar_cursor_index(&rows), list_h);
                    let idx = usize::from(mouse.row.saturating_sub(side.y)) + offset;
                    if mouse.row < side.y + list_h
                        && let Some(row) = rows.get(idx)
                    {
                        {
                            let key = sidebar::row_key(&self.agents, row);
                            self.focus = Focus::App;
                            self.sidebar.cursor = Some(key.clone());
                            self.dirty = true;
                            return match key {
                                RowKey::Agent(id) => {
                                    let fx = self.select(&id);
                                    self.focus = Focus::Terminal;
                                    fx
                                }
                                RowKey::Header(_) => {
                                    self.toggle_collapse();
                                    Vec::new()
                                }
                            };
                        }
                    }
                } else if layout.terminal.contains(at) && self.modal.is_none() {
                    self.focus = Focus::Terminal;
                    self.dirty = true;
                }
            }
            _ => {}
        }
        Vec::new()
    }

    // ── commands ────────────────────────────────────────────────────────────

    pub fn exec(&mut self, command: Command) -> Vec<Effect> {
        self.dirty = true;
        let selected = self.selected.clone();
        match command {
            Command::SendPrefix => {
                let Some(id) = selected else {
                    return Vec::new();
                };
                let chord = self.keymap.prefix;
                let event = KeyEvent::new(chord.code, chord.mods);
                let bytes = keys::encode_key(event, false);
                self.send_bytes(&id, bytes)
            }
            Command::NewAgent => {
                let dir = selected
                    .as_deref()
                    .and_then(|id| self.agent(id))
                    .map(|a| a.project_dir.clone())
                    .filter(|d| !d.is_empty())
                    .or_else(|| {
                        Some(self.config.default_project_dir.clone())
                            .filter(|d| !d.trim().is_empty())
                    })
                    .unwrap_or_else(|| self.cwd.to_string_lossy().into_owned());
                let shown = as_core::paths::tildify(std::path::Path::new(&dir));
                let mut form = NewAgentForm::new(&self.config, &shown, &self.cwd);
                form.set_profiles(self.profiles.clone());
                let mut fx = vec![Effect::LoadProfiles];
                fx.extend(self.sync_form_models(&mut form));
                self.modal = Some(Modal::NewAgent(Box::new(form)));
                fx
            }
            Command::ResumeSession => {
                let agent = self.selected_agent();
                let project = agent.map(|a| a.project_dir.clone()).unwrap_or_default();
                // OpenCode keeps no session files: start from Claude.
                let provider = agent
                    .map(|a| a.provider)
                    .filter(|p| *p != ProviderId::Opencode)
                    .unwrap_or(match self.config.default_provider {
                        ProviderId::Opencode => ProviderId::Claude,
                        p => p,
                    });
                self.modal = Some(Modal::Resume(Box::new(SessionPicker::new(
                    PickerOrigin::Standalone,
                    provider,
                    &project,
                    self.config.session_max_age_days,
                ))));
                vec![Effect::LoadSessions {
                    provider,
                    force: false,
                }]
            }
            Command::SetProfile => {
                let dir = match &self.sidebar.cursor {
                    Some(RowKey::Header(key)) if key != sidebar::MANAGER_GROUP => Some(key.clone()),
                    _ => self.selected_agent().map(|a| a.project_dir.clone()),
                };
                match dir.filter(|d| !d.is_empty()) {
                    Some(project_dir) => {
                        self.open_profile_chooser(project_dir);
                        vec![Effect::LoadProfiles]
                    }
                    None => Vec::new(),
                }
            }
            Command::KillAgent => selected.map(Effect::Kill).into_iter().collect(),
            Command::RestartAgent => selected.map(Effect::Restart).into_iter().collect(),
            Command::RemoveAgent => {
                if let Some(a) = self.selected_agent() {
                    self.modal = Some(Modal::ConfirmRemove {
                        agent_id: a.id.clone(),
                        name: a.name.clone(),
                    });
                }
                Vec::new()
            }
            Command::RenameAgent => {
                if let Some(a) = self.selected_agent() {
                    self.modal = Some(Modal::Rename {
                        agent_id: a.id.clone(),
                        input: TextArea::single_line(&a.name),
                    });
                }
                Vec::new()
            }
            Command::ToggleYolo => match self.selected_agent() {
                Some(a) => vec![Effect::ToggleYolo(a.id.clone(), !a.yolo)],
                None => Vec::new(),
            },
            Command::ToggleGlobalYolo => vec![Effect::SetGlobalYolo(!self.config.global_yolo)],
            Command::NextAgent => self.step_agent(1),
            Command::PrevAgent => self.step_agent(-1),
            Command::CursorDown => self.move_cursor(1),
            Command::CursorUp => self.move_cursor(-1),
            Command::JumpAgent(n) => self.jump_agent(n),
            Command::FocusSidebar => {
                self.focus = Focus::App;
                if self.sidebar.cursor.is_none() {
                    self.sidebar.cursor = selected.map(RowKey::Agent);
                }
                Vec::new()
            }
            Command::FocusTerminal => {
                self.focus = Focus::Terminal;
                self.sidebar.filtering = false;
                Vec::new()
            }
            Command::ScrollMode => {
                if self.selected.is_some() {
                    self.scroll_mode = true;
                }
                Vec::new()
            }
            Command::ToggleInputBar => {
                self.input_visible = !self.input_visible;
                self.focus = if self.input_visible {
                    Focus::Input
                } else {
                    Focus::Terminal
                };
                self.sync_layout()
            }
            Command::Help => {
                self.modal = Some(Modal::Help);
                Vec::new()
            }
            Command::Quit => self.request_quit(),
            Command::SidebarNarrower => {
                self.sidebar_width = self
                    .sidebar_width
                    .saturating_sub(2)
                    .max(layout::MIN_SIDEBAR);
                self.sync_layout()
            }
            Command::SidebarWider => {
                self.sidebar_width = (self.sidebar_width + 2).min(layout::MAX_SIDEBAR);
                self.sync_layout()
            }
            Command::ClearToasts => {
                self.toasts.clear();
                Vec::new()
            }
            Command::Activate => {
                match self.sidebar.cursor.clone() {
                    Some(RowKey::Header(_)) => self.toggle_collapse(),
                    _ => self.focus = Focus::Terminal,
                }
                Vec::new()
            }
            Command::ToggleCollapse => {
                self.toggle_collapse();
                Vec::new()
            }
            Command::FilterSidebar => {
                self.sidebar.filtering = true;
                Vec::new()
            }
            Command::ScrollUp => self.scroll_by(1, false),
            Command::ScrollDown => self.scroll_by(-1, false),
            Command::ScrollPageUp => self.scroll_by(1, true),
            Command::ScrollPageDown => self.scroll_by(-1, true),
            Command::ScrollTop => self.scroll_to(usize::MAX),
            Command::ScrollBottom => self.scroll_to(0),
            Command::ExitScroll => {
                self.scroll_to(0);
                self.scroll_mode = false;
                Vec::new()
            }
        }
    }

    fn scroll_by(&mut self, direction: isize, page: bool) -> Vec<Effect> {
        let Some(term) = self.selected.as_ref().and_then(|id| self.terms.get_mut(id)) else {
            return Vec::new();
        };
        let (_, rows) = term.emu.size();
        let step = if page {
            usize::from(rows.saturating_sub(1).max(1))
        } else {
            1
        };
        let current = term.emu.scrollback();
        let next = if direction > 0 {
            current.saturating_add(step)
        } else {
            current.saturating_sub(step)
        };
        term.emu.set_scrollback(next);
        Vec::new()
    }

    fn scroll_to(&mut self, rows: usize) -> Vec<Effect> {
        if let Some(term) = self.selected.as_ref().and_then(|id| self.terms.get_mut(id)) {
            term.emu.set_scrollback(rows);
        }
        Vec::new()
    }

    pub fn request_quit(&mut self) -> Vec<Effect> {
        match quit_decision(&self.agents) {
            QuitDecision::Exit => {
                self.quit = true;
                vec![Effect::Quit]
            }
            QuitDecision::Confirm { running } => {
                self.modal = Some(Modal::Quit {
                    running,
                    choice: QuitChoice::Detach,
                    stopping: false,
                });
                Vec::new()
            }
        }
    }

    // ── modals ──────────────────────────────────────────────────────────────

    fn modal_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(modal) = self.modal.take() else {
            return Vec::new();
        };
        match modal {
            Modal::NewAgent(mut form) => {
                let mut fx = Vec::new();
                match form.handle_key(&key) {
                    FormAction::None => {}
                    FormAction::Cancel => return Vec::new(),
                    FormAction::Picker(action) => fx.extend(Self::picker_effects(action)),
                    FormAction::Submit => match form.build_payload() {
                        Ok(payload) => {
                            form.submitting = true;
                            let change = form.profile_change(&payload.project_dir);
                            self.modal = Some(Modal::NewAgent(form));
                            return vec![match change {
                                Some((project_dir, config_dir)) => Effect::CreateAgentWithProfile {
                                    project_dir,
                                    config_dir,
                                    payload,
                                },
                                None => Effect::CreateAgent(payload),
                            }];
                        }
                        Err(error) => form.error = Some(error),
                    },
                }
                fx.extend(self.sync_form_models(&mut form));
                self.modal = Some(Modal::NewAgent(form));
                fx
            }
            Modal::Resume(mut picker) => match picker.handle_key(&key) {
                PickerAction::Cancel => Vec::new(),
                PickerAction::Pick(session) => self.resume_session(*session, picker),
                action => {
                    let fx = Self::picker_effects(action);
                    self.modal = Some(Modal::Resume(picker));
                    fx
                }
            },
            Modal::Profile(mut chooser) => {
                let n = chooser.choices.len();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => return Vec::new(),
                    KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                        chooser.idx = (chooser.idx + 1) % n.max(1);
                    }
                    KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                        chooser.idx = (chooser.idx + n.max(1) - 1) % n.max(1);
                    }
                    KeyCode::Enter => {
                        let choice = chooser.choices.get(chooser.idx);
                        return vec![Effect::SetProjectProfile {
                            project_dir: chooser.project_dir.clone(),
                            config_dir: choice.and_then(|c| c.config_dir.clone()),
                        }];
                    }
                    _ => {}
                }
                self.modal = Some(Modal::Profile(chooser));
                Vec::new()
            }
            Modal::Rename {
                agent_id,
                mut input,
            } => match key.code {
                KeyCode::Esc => Vec::new(),
                KeyCode::Enter => {
                    let name = input.text().trim().to_string();
                    if name.is_empty() {
                        self.modal = Some(Modal::Rename { agent_id, input });
                        Vec::new()
                    } else {
                        vec![Effect::Rename(agent_id, name)]
                    }
                }
                _ => {
                    input.handle_key(&key);
                    self.modal = Some(Modal::Rename { agent_id, input });
                    Vec::new()
                }
            },
            Modal::ConfirmRemove { agent_id, name } => match key.code {
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => vec![Effect::Remove(agent_id)],
                KeyCode::Char('n' | 'N') | KeyCode::Esc => Vec::new(),
                _ => {
                    self.modal = Some(Modal::ConfirmRemove { agent_id, name });
                    Vec::new()
                }
            },
            Modal::Quit {
                running,
                mut choice,
                stopping,
            } => {
                if stopping {
                    // Waiting for the daemon; nothing to press.
                    self.modal = Some(Modal::Quit {
                        running,
                        choice,
                        stopping,
                    });
                    return Vec::new();
                }
                let pos = QuitChoice::ALL
                    .iter()
                    .position(|c| *c == choice)
                    .unwrap_or(0);
                let n = QuitChoice::ALL.len();
                match key.code {
                    KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => {
                        choice = QuitChoice::ALL[(pos + 1) % n];
                    }
                    KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => {
                        choice = QuitChoice::ALL[(pos + n - 1) % n];
                    }
                    KeyCode::Char('d' | 'D') => {
                        return self.resolve_quit(QuitChoice::Detach, running);
                    }
                    KeyCode::Char('s' | 'S') => {
                        return self.resolve_quit(QuitChoice::StopAll, running);
                    }
                    KeyCode::Char('c' | 'C') | KeyCode::Esc => return Vec::new(),
                    KeyCode::Enter => return self.resolve_quit(choice, running),
                    _ => {}
                }
                self.modal = Some(Modal::Quit {
                    running,
                    choice,
                    stopping,
                });
                Vec::new()
            }
            Modal::Help => Vec::new(),
        }
    }

    fn picker_effects(action: PickerAction) -> Vec<Effect> {
        match action {
            PickerAction::Load { provider, force } => {
                vec![Effect::LoadSessions { provider, force }]
            }
            PickerAction::Hide { session_id, hidden } => {
                vec![Effect::HideSession(session_id, hidden)]
            }
            PickerAction::None | PickerAction::Cancel | PickerAction::Pick(_) => Vec::new(),
        }
    }

    /// Use the cached model list for the dialog's provider, or ask the daemon for it.
    fn sync_form_models(&self, form: &mut NewAgentForm) -> Vec<Effect> {
        let Some(provider) = form.models_wanted.take() else {
            return Vec::new();
        };
        if let Some(models) = self.models.get(&provider) {
            form.set_models(provider, models.clone());
        }
        vec![Effect::LoadModels(provider)]
    }

    /// `prefix o`: create an agent resuming `session` with the defaults of its provider.
    fn resume_session(
        &mut self,
        session: SessionSummary,
        picker: Box<SessionPicker>,
    ) -> Vec<Effect> {
        let dir = std::path::Path::new(&session.project_path);
        if session.project_path.is_empty() || !dir.is_dir() {
            self.toast(
                ToastKind::Error,
                "Cannot resume",
                &format!(
                    "The project directory no longer exists: {}",
                    as_core::paths::tildify(dir)
                ),
            );
            self.modal = Some(Modal::Resume(picker));
            return Vec::new();
        }
        let provider = session.provider;
        let model = if self.config.default_provider == provider {
            self.config.default_model.clone()
        } else {
            get_default_model_for_provider(provider)
        };
        vec![Effect::CreateAgent(CreateAgentPayload {
            name: as_core::AgentManager::generate_name(
                &session.first_prompt,
                &session.project_path,
            ),
            project_dir: session.project_path.clone(),
            provider,
            model,
            yolo: self.config.global_yolo,
            resume_session_id: Some(session.session_id),
            ..Default::default()
        })]
    }

    fn open_profile_chooser(&mut self, project_dir: String) {
        let mut choices = vec![ProfileChoice {
            label: "inherit the daemon's environment (no mapping)".into(),
            config_dir: None,
        }];
        choices.extend(self.profiles.iter().map(|p| ProfileChoice {
            label: format!(
                "{} ({})",
                p.label,
                as_core::paths::tildify(std::path::Path::new(&p.config_dir))
            ),
            config_dir: Some(if p.is_default {
                String::new()
            } else {
                p.config_dir.clone()
            }),
        }));
        let own = self.config.claude_profile_by_project.get(&project_dir);
        let idx = own
            .and_then(|mapped| {
                choices
                    .iter()
                    .position(|c| c.config_dir.as_deref() == Some(mapped))
            })
            .unwrap_or(0);
        self.modal = Some(Modal::Profile(ProfileChooser {
            project_dir,
            choices,
            idx,
        }));
    }

    fn resolve_quit(&mut self, choice: QuitChoice, running: usize) -> Vec<Effect> {
        match choice {
            QuitChoice::Detach => {
                self.quit = true;
                vec![Effect::Quit]
            }
            QuitChoice::StopAll => {
                self.modal = Some(Modal::Quit {
                    running,
                    choice,
                    stopping: true,
                });
                vec![Effect::ShutdownAndQuit]
            }
            QuitChoice::Cancel => Vec::new(),
        }
    }

    /// Chord label of the prefix key for hints (`Ctrl+A`).
    pub fn prefix_label(&self) -> String {
        self.keymap.prefix.to_string()
    }

    pub fn prefix_chord(&self) -> KeyChord {
        self.keymap.prefix
    }
}

#[cfg(test)]
mod tests;
