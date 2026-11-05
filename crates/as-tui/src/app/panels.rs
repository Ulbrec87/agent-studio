//! P9 panels as `impl App`: git, files (+ search), free terminals, settings, open-in, and the
//! routing of daemon replies (`Msg::Reply`) to them.

use super::*;

use as_core::terminal_tree::SplitDir;

use crate::panels::Outcome;
use crate::panels::settings::Section;
use crate::panels::terminals::Dir;

impl App {
    /// The folder panels work on: the selected agent's project (the worktree for worktree agents,
    /// whose `project_dir` already is the worktree), else the configured default, else the cwd.
    pub fn panel_dir(&self) -> String {
        self.selected_agent()
            .map(|a| a.project_dir.clone())
            .filter(|d| !d.is_empty() && !self.selected_agent().is_some_and(|a| a.is_manager))
            .or_else(|| {
                Some(self.config.default_project_dir.clone()).filter(|d| !d.trim().is_empty())
            })
            .map(|d| {
                as_core::paths::expand_tilde(&d)
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| self.cwd.to_string_lossy().into_owned())
    }

    // ── opening ─────────────────────────────────────────────────────────────

    pub(super) fn open_git(&mut self) -> Vec<Effect> {
        let dir = self.panel_dir();
        crate::highlight::warm();
        self.modal = Some(Modal::Git);
        self.dirty = true;
        self.git.open(&dir)
    }

    pub(super) fn open_files(&mut self, search: bool) -> Vec<Effect> {
        let dir = self.panel_dir();
        crate::highlight::warm();
        self.modal = Some(Modal::Files);
        self.dirty = true;
        let mut fx = self.files.open(&dir);
        if search {
            fx.extend(self.files.start_search());
        }
        fx
    }

    pub(super) fn open_settings(&mut self, section: Option<Section>) -> Vec<Effect> {
        let bindings = self.effective_bindings();
        let (_, mcp) = self.mcp_summary();
        let data_dir = self
            .keybindings_path
            .as_deref()
            .and_then(std::path::Path::parent)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let config = self.config.clone();
        let profiles = self.profiles.clone();
        let mut fx = self
            .settings
            .open(&config, profiles, bindings, mcp, data_dir);
        if let Some(section) = section {
            self.settings.section = Section::ALL.iter().position(|s| *s == section).unwrap_or(0);
            self.settings.in_items = true;
        }
        fx.push(Effect::LoadMcpStatus);
        self.modal = Some(Modal::Settings);
        self.dirty = true;
        fx
    }

    pub(super) fn open_import_hydra(&mut self) -> Vec<Effect> {
        let mut fx = self.open_settings(Some(Section::Data));
        fx.extend(self.settings.start_import());
        fx
    }

    pub(super) fn open_open_in(&mut self, path: Option<String>) -> Vec<Effect> {
        let path = path.unwrap_or_else(|| self.panel_dir());
        self.modal = Some(Modal::OpenIn);
        self.dirty = true;
        self.open_in.open(&path)
    }

    /// Label and keys of every effective binding (Settings > Keybindings).
    fn effective_bindings(&self) -> Vec<(String, String)> {
        as_core::keybindings::merge_with_defaults(&self.user_bindings)
            .into_iter()
            .map(|r| {
                let label = as_core::keybindings::command_def(&r.command)
                    .map_or_else(|| r.command.clone(), |d| d.label.to_string());
                (label, r.keys)
            })
            .collect()
    }

    // ── free terminals ──────────────────────────────────────────────────────

    pub(super) fn term_command(&mut self, command: Command) -> Vec<Effect> {
        self.dirty = true;
        // Terminal commands open the panel first when it is closed.
        let opening = !self.free_terms.open && command != Command::TerminalToggle;
        let mut fx = Vec::new();
        if opening {
            let dir = self.panel_dir();
            fx.extend(self.free_terms.toggle(&dir, self.screen));
        }
        let more = match command {
            Command::TerminalToggle => {
                let dir = self.panel_dir();
                self.free_terms.toggle(&dir, self.screen)
            }
            // Opening already requested the first terminal.
            Command::TerminalNew | Command::TerminalSplit | Command::TerminalSplitDown
                if opening =>
            {
                Vec::new()
            }
            Command::TerminalNew => self.free_terms.new_tab(),
            Command::TerminalSplit => self.free_terms.split(SplitDir::Horizontal),
            Command::TerminalSplitDown => self.free_terms.split(SplitDir::Vertical),
            Command::TerminalFocusLeft => self.free_terms.focus_dir(Dir::Left),
            Command::TerminalFocusRight => self.free_terms.focus_dir(Dir::Right),
            Command::TerminalFocusUp => self.free_terms.focus_dir(Dir::Up),
            Command::TerminalFocusDown => self.free_terms.focus_dir(Dir::Down),
            Command::TerminalResizeLeft => self.free_terms.resize_dir(Dir::Left),
            Command::TerminalResizeRight => self.free_terms.resize_dir(Dir::Right),
            Command::TerminalResizeUp => self.free_terms.resize_dir(Dir::Up),
            Command::TerminalResizeDown => self.free_terms.resize_dir(Dir::Down),
            Command::TerminalKill => self.free_terms.kill_active(),
            Command::TerminalNextTab => self.free_terms.cycle_tab(1),
            Command::TerminalPrevTab => self.free_terms.cycle_tab(-1),
            Command::TerminalNextPane => self.free_terms.cycle_pane(),
            Command::TerminalRename => {
                self.free_terms.start_rename();
                Vec::new()
            }
            Command::TerminalFullscreen => self.free_terms.toggle_fullscreen(),
            _ => Vec::new(),
        };
        fx.extend(more);
        fx
    }

    pub(super) fn free_term_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        self.free_terms.key(key)
    }

    pub(super) fn on_terminal_event(&mut self, event: Event) -> Vec<Effect> {
        self.dirty = self.free_terms.open || self.dirty;
        match event {
            Event::TerminalOutput { terminal_id, data } => {
                self.free_terms.on_output(&terminal_id, &data)
            }
            Event::TerminalExit { terminal_id, .. } => {
                self.free_terms.on_exit(&terminal_id);
                Vec::new()
            }
            Event::TerminalLayoutChanged {
                project_dir,
                layout,
            } => self.free_terms.on_layout(&project_dir, layout),
            _ => Vec::new(),
        }
    }

    // ── keys of the modal panels ────────────────────────────────────────────

    pub(super) fn git_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let (outcome, fx) = self.git.key(&key);
        self.finish_panel(Modal::Git, outcome);
        fx
    }

    pub(super) fn files_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let (outcome, mut fx) = self.files.key(&key);
        if let Some(path) = self.files.want_open_in.take() {
            fx.extend(self.open_open_in(Some(path)));
            return fx;
        }
        self.finish_panel(Modal::Files, outcome);
        fx
    }

    pub(super) fn settings_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let (outcome, mut fx) = self.settings.key(&key);
        if std::mem::take(&mut self.settings.want_keybindings_file) {
            self.modal = None;
            fx.extend(self.open_keybindings_file());
            return fx;
        }
        self.finish_panel(Modal::Settings, outcome);
        fx
    }

    pub(super) fn open_in_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let (outcome, fx) = self.open_in.key(&key);
        self.finish_panel(Modal::OpenIn, outcome);
        fx
    }

    fn finish_panel(&mut self, modal: Modal, outcome: Outcome) {
        self.modal = (outcome == Outcome::Stay).then_some(modal);
    }

    /// Time-driven work of the open panels (the open file's mtime, once a second).
    pub(super) fn panel_poll(&mut self) -> Vec<Effect> {
        if matches!(self.modal, Some(Modal::Files)) {
            let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
            return self.files.poll(now);
        }
        Vec::new()
    }

    // ── replies ─────────────────────────────────────────────────────────────

    pub(super) fn on_reply(
        &mut self,
        key: &str,
        result: Result<as_proto::Response, String>,
    ) -> Vec<Effect> {
        self.dirty = true;
        let prefix = key.split(':').next().unwrap_or("");
        match prefix {
            "git" => self.git.on_reply(key, result),
            "fs" => self.files.on_reply(key, result),
            "term" => {
                if let Err(e) = &result
                    && matches!(key, "term:spawn" | "term:kill" | "term:rename")
                {
                    self.toast(ToastKind::Error, "Terminal", e);
                }
                self.free_terms.on_reply(key, result)
            }
            "set" => self.settings.on_reply(key, result),
            "open" => {
                if key == "open:do"
                    && let Err(e) = &result
                {
                    self.toast(ToastKind::Error, "Open in", e);
                } else {
                    self.open_in.on_reply(key, result);
                }
                Vec::new()
            }
            "upd" => {
                self.on_update_reply(key, result);
                Vec::new()
            }
            "diag" => {
                match result {
                    Ok(as_proto::Response::Path(p)) => {
                        self.toast(
                            ToastKind::Success,
                            "Diagnostics",
                            &format!("Written to {p}"),
                        );
                    }
                    Err(e) => self.toast(ToastKind::Error, "Diagnostics", &e),
                    Ok(_) => {}
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn export_diagnostics(&mut self) -> Vec<Effect> {
        vec![crate::panels::req(
            "diag:export",
            as_proto::Request::ExportDiagnostics,
        )]
    }
}
