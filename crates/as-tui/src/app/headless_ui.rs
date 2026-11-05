//! Headless runs panel and the manager agent / MCP status commands: all of it is `impl App`.

use super::*;

use crate::headless::{LOG_MAX_CHARS, LOG_TAIL_LINES, LogView};

/// Rows a PgUp/PgDn moves in the log viewer when the screen size is unknown.
const PAGE_FALLBACK: usize = 20;

impl App {
    // ── new agent dialog (manager toggle) ───────────────────────────────────

    /// The default project directory of a new agent or run.
    fn default_project_dir(&self) -> String {
        let dir = self
            .selected_agent()
            .map(|a| a.project_dir.clone())
            .filter(|d| !d.is_empty() && !self.selected_agent().is_some_and(|a| a.is_manager))
            .or_else(|| {
                Some(self.config.default_project_dir.clone()).filter(|d| !d.trim().is_empty())
            })
            .unwrap_or_else(|| self.cwd.to_string_lossy().into_owned());
        as_core::paths::tildify(std::path::Path::new(&dir))
    }

    /// `prefix n` / "New manager agent".
    pub(super) fn open_new_agent(&mut self, manager: bool) -> Vec<Effect> {
        let shown = self.default_project_dir();
        let mut form = NewAgentForm::new(&self.config, &shown, &self.cwd);
        form.set_profiles(self.profiles.clone());
        form.mcp_available = self.mcp.as_ref().is_none_or(|m| m.running);
        let mut fx = vec![Effect::LoadProfiles, Effect::LoadMcpStatus];
        if manager {
            form.set_manager(true);
            if form.manager {
                form.field = crate::new_agent::Field::Name;
            } else {
                let why = form.error.take().unwrap_or_default();
                self.toast(ToastKind::Error, "No manager agent", &why);
                return fx;
            }
        }
        fx.extend(self.sync_form_models(&mut form));
        self.modal = Some(Modal::NewAgent(Box::new(form)));
        fx
    }

    /// "MCP server status": a toast with the port and the workspace.
    pub(super) fn show_mcp_status(&mut self) -> Vec<Effect> {
        let (kind, body) = self.mcp_summary();
        self.toast(kind, "MCP server", &body);
        vec![Effect::LoadMcpStatus]
    }

    /// One line about the MCP server for toasts and the help overlay.
    pub fn mcp_summary(&self) -> (ToastKind, String) {
        match &self.mcp {
            None => (ToastKind::Info, "status unknown (asking the daemon)".into()),
            Some(m) if m.running => (
                ToastKind::Success,
                format!(
                    "running on 127.0.0.1:{} \u{00B7} manager workspace {}",
                    m.port.map_or_else(|| "?".into(), |p| p.to_string()),
                    m.manager_workspace
                        .as_deref()
                        .map(|w| as_core::paths::tildify(std::path::Path::new(w)))
                        .unwrap_or_default()
                ),
            ),
            Some(m) => (
                ToastKind::Error,
                format!(
                    "not running{}",
                    m.error
                        .as_deref()
                        .map(|e| format!(": {e}"))
                        .unwrap_or_default()
                ),
            ),
        }
    }

    // ── panel ───────────────────────────────────────────────────────────────

    /// `prefix H`.
    pub(super) fn open_headless(&mut self) -> Vec<Effect> {
        self.headless.view = None;
        self.headless.filtering = false;
        self.modal = Some(Modal::Headless);
        self.dirty = true;
        vec![Effect::LoadHeadlessRuns]
    }

    /// Back to the panel after the new-run dialog.
    pub(super) fn reopen_headless(&mut self) -> Vec<Effect> {
        self.modal = Some(Modal::Headless);
        self.dirty = true;
        Vec::new()
    }

    fn open_run_form(&mut self) -> Vec<Effect> {
        let shown = self.default_project_dir();
        let mut form = NewAgentForm::headless(&self.config, &shown, &self.cwd);
        let mut fx = self.sync_form_models(&mut form);
        // Resume sessions of the dialog's provider need the profiles for their labels.
        fx.push(Effect::LoadProfiles);
        self.modal = Some(Modal::NewAgent(Box::new(form)));
        fx
    }

    fn open_log(&mut self, run_id: &str) -> Vec<Effect> {
        self.headless.view = Some(LogView::new(run_id));
        vec![Effect::LoadHeadlessLog(run_id.to_string())]
    }

    fn log_page(&self) -> usize {
        usize::from(self.screen.height.saturating_sub(8)).max(PAGE_FALLBACK / 2)
    }

    /// Keys of the panel; it stays open until `Esc`/`q` in the list.
    pub(super) fn headless_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        self.dirty = true;
        if self.headless.view.is_some() {
            return self.log_key(key);
        }
        let h = &mut self.headless;
        if h.filtering {
            match key.code {
                KeyCode::Esc => {
                    h.query.clear();
                    h.filtering = false;
                }
                KeyCode::Enter => h.filtering = false,
                KeyCode::Down => h.move_cursor(1),
                KeyCode::Up => h.move_cursor(-1),
                _ => {
                    h.query.handle_key(&key);
                }
            }
            h.cursor = h.cursor.min(h.visible().len().saturating_sub(1));
            self.modal = Some(Modal::Headless);
            return Vec::new();
        }
        let page = self.log_page() as isize;
        let mut fx = Vec::new();
        let h = &mut self.headless;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'H') => return Vec::new(),
            KeyCode::Down | KeyCode::Char('j') => h.move_cursor(1),
            KeyCode::Up | KeyCode::Char('k') => h.move_cursor(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => h.move_cursor(page),
            KeyCode::PageUp => h.move_cursor(-page),
            KeyCode::Home | KeyCode::Char('g') => h.cursor = 0,
            KeyCode::End | KeyCode::Char('G') => h.cursor = usize::MAX / 2,
            KeyCode::Char('/') => h.filtering = true,
            KeyCode::Char('s') | KeyCode::Tab => h.cycle_status(),
            KeyCode::Char('r') => fx.push(Effect::LoadHeadlessRuns),
            KeyCode::Char('n') => return self.open_run_form(),
            KeyCode::Enter => {
                if let Some(id) = h.selected().map(|r| r.id.clone()) {
                    fx.extend(self.open_log(&id));
                }
            }
            KeyCode::Char('x') => fx.extend(self.cancel_selected()),
            _ => {}
        }
        self.headless.clamp_cursor();
        self.modal = Some(Modal::Headless);
        fx
    }

    fn cancel_selected(&mut self) -> Vec<Effect> {
        let id = match &self.headless.view {
            Some(v) => Some(v.run_id.clone()),
            None => self.headless.selected().map(|r| r.id.clone()),
        };
        let Some(id) = id else { return Vec::new() };
        match self.headless.run(&id) {
            Some(run) if run.status == as_core::headless::HeadlessRunStatus::Running => {
                vec![Effect::CancelHeadlessRun(id)]
            }
            Some(_) => {
                self.toast(ToastKind::Info, "Headless run", "The run is not running");
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    fn log_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let page = self.log_page();
        let mut fx = Vec::new();
        let Some(view) = self.headless.view.as_mut() else {
            return fx;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace | KeyCode::Left => {
                self.headless.view = None;
            }
            KeyCode::Up | KeyCode::Char('k') => view.scroll_up(1),
            KeyCode::Down | KeyCode::Char('j') => view.scroll_down(1),
            KeyCode::PageUp => view.scroll_up(page),
            KeyCode::PageDown | KeyCode::Char(' ') => view.scroll_down(page),
            KeyCode::Home | KeyCode::Char('g') => view.to_top(),
            KeyCode::End | KeyCode::Char('G' | 'f') => view.to_end(),
            KeyCode::Char('r') => view.toggle_raw(),
            KeyCode::Char('R') => {
                let id = view.run_id.clone();
                fx.push(Effect::LoadHeadlessLog(id));
            }
            KeyCode::Char('x') => fx.extend(self.cancel_selected()),
            _ => {}
        }
        self.modal = Some(Modal::Headless);
        fx
    }

    // ── daemon traffic ──────────────────────────────────────────────────────

    pub(super) fn on_headless_runs(&mut self, result: Result<Vec<HeadlessRun>, String>) {
        match result {
            Ok(runs) => self.headless.set_runs(runs),
            Err(error) => {
                tracing::debug!(%error, "listing headless runs failed");
                self.headless.error = Some(error);
                self.headless.loaded = true;
            }
        }
        self.dirty = true;
    }

    /// The run changed: keep the list current and reload the log when the run ended.
    pub(super) fn on_headless_changed(&mut self, run: HeadlessRun) -> Vec<Effect> {
        let id = run.id.clone();
        let ended = run.status != as_core::headless::HeadlessRunStatus::Running;
        self.headless.upsert(run);
        self.dirty = true;
        match &self.headless.view {
            // Lines may have been missed between the read and the first event: read once more.
            Some(v) if ended && v.run_id == id => vec![Effect::LoadHeadlessLog(id)],
            _ => Vec::new(),
        }
    }

    pub(super) fn on_headless_line(&mut self, run_id: &str, data: &str) {
        if let Some(view) = &mut self.headless.view
            && view.run_id == run_id
            && view.loaded
        {
            view.push_line(data);
            self.dirty = true;
        }
    }

    pub(super) fn on_headless_started(
        &mut self,
        result: Result<Box<HeadlessRun>, String>,
    ) -> Vec<Effect> {
        match result {
            Ok(run) => {
                let id = run.id.clone();
                self.headless.upsert(*run);
                self.headless.query.clear();
                self.headless.status = as_core::headless::StatusFilter::All;
                self.headless.cursor = 0;
                self.modal = Some(Modal::Headless);
                self.dirty = true;
                self.open_log(&id)
            }
            Err(error) => {
                if let Some(Modal::NewAgent(form)) = &mut self.modal {
                    form.submitting = false;
                    form.error = Some(error);
                } else {
                    self.toast(ToastKind::Error, "Could not start the run", &error);
                }
                self.dirty = true;
                Vec::new()
            }
        }
    }

    pub(super) fn on_headless_log(
        &mut self,
        run_id: &str,
        result: Result<Option<HeadlessRunLog>, String>,
    ) {
        if let Some(view) = &mut self.headless.view
            && view.run_id == run_id
        {
            match result {
                Ok(Some(log)) => view.set_log(log),
                Ok(None) => view.failed("The run is unknown to the daemon".into()),
                Err(error) => view.failed(error),
            }
            self.dirty = true;
        }
    }
}

/// What the daemon is asked for when a log is opened.
pub fn headless_log_options() -> as_core::headless::HeadlessRunLogOptions {
    as_core::headless::HeadlessRunLogOptions {
        tail_lines: Some(LOG_TAIL_LINES),
        max_chars: Some(LOG_MAX_CHARS),
    }
}
