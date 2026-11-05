//! Views (Chat / Grid / Overview), the grid's tile navigation, the broadcast bar, the palette
//! actions, keybinding reloads and the persisted UI state. All of it is `impl App`.

use super::*;

use crate::bindings_file;
use crate::grid::{Dir, RUNNING_TAB};

impl App {
    fn wall_clock_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    /// The focus a view starts in: the terminal in Chat, tile navigation in Grid.
    pub fn home_focus(&self) -> Focus {
        match self.view {
            ViewMode::Chat => Focus::Terminal,
            ViewMode::Grid => Focus::Grid,
            ViewMode::Overview => Focus::Overview,
        }
    }

    // ── views ───────────────────────────────────────────────────────────────

    /// Apply the current view's focus and subscriptions (after a restore or a switch).
    pub(super) fn enter_view(&mut self) -> Vec<Effect> {
        self.scroll_mode = false;
        self.focus = self.home_focus();
        let mut fx = Vec::new();
        if self.view == ViewMode::Grid {
            fx.extend(self.grid_refresh());
        }
        fx.extend(self.sync_layout());
        self.dirty = true;
        fx
    }

    pub fn set_view(&mut self, view: ViewMode) -> Vec<Effect> {
        if self.view == view {
            return Vec::new();
        }
        if self.view == ViewMode::Overview {
            self.overview.left(self.wall_ms());
        }
        self.view = view;
        if view == ViewMode::Overview {
            self.overview.entered(self.wall_ms(), self.now);
        }
        self.enter_view()
    }

    // ── grid model ──────────────────────────────────────────────────────────

    pub fn grid_tabs(&self) -> Vec<grid::Tab> {
        grid::tabs(
            &self.agents,
            self.grid.only_running,
            self.config.session_max_age_days,
            Self::wall_clock_ms(),
        )
    }

    pub fn grid_tab_index(&self) -> usize {
        self.grid_tabs()
            .iter()
            .position(|t| t.key == self.grid.tab)
            .unwrap_or(0)
    }

    /// Where the tiles go: the main area below the tab bar (and above the broadcast bar).
    pub fn grid_layout(&self) -> grid::TileLayout {
        let area = self.layout().terminal;
        let focus = self.grid_selected_index();
        let expanded = self
            .grid
            .expanded
            .as_ref()
            .and_then(|id| self.grid.tiles.iter().position(|t| t == id));
        grid::layout(
            area,
            self.grid.tiles.len(),
            self.config.grid_columns,
            focus,
            self.grid.first_row,
            MAX_LIVE_TERMS,
            expanded,
        )
    }

    pub fn grid_selected_index(&self) -> Option<usize> {
        let id = self.selected.as_ref()?;
        self.grid.tiles.iter().position(|t| t == id)
    }

    pub(super) fn grid_refresh_if_open(&mut self) -> Vec<Effect> {
        if self.view == ViewMode::Grid {
            self.grid_refresh()
        } else {
            Vec::new()
        }
    }

    fn compute_tiles(&mut self) -> Vec<String> {
        let now = Self::wall_clock_ms();
        let idx = grid::tab_agents(
            &self.agents,
            &self.grid.tab,
            self.grid.only_running,
            self.config.session_max_age_days,
            now,
        );
        let ids: Vec<String> = idx.iter().map(|i| self.agents[*i].id.clone()).collect();
        let running = self.grid.tab == RUNNING_TAB;
        self.grid.order.ordered(Some(&self.grid.tab), &ids, running)
    }

    /// Bring the grid in line with the agents: valid tab, tiles in order, a focused tile that
    /// exists, and a subscription for every tile on screen.
    pub fn grid_refresh(&mut self) -> Vec<Effect> {
        if self.grid.refreshing {
            return Vec::new();
        }
        self.grid.refreshing = true;
        let mut fx = Vec::new();

        // A valid tab: the saved one, else the selected agent's project, else the first project.
        let tabs = self.grid_tabs();
        if !tabs.iter().any(|t| t.key == self.grid.tab) {
            let own = self.selected_agent().map(|a| a.project_dir.clone());
            self.grid.tab = own
                .filter(|dir| tabs.iter().any(|t| t.key == *dir))
                .or_else(|| tabs.get(1).map(|t| t.key.clone()))
                .unwrap_or_else(|| RUNNING_TAB.to_string());
        }
        self.grid.tiles = self.compute_tiles();

        // The selected agent lives in another project: follow it. The Running tab stays put when
        // the agent merely stopped running (Hydra keeps the tab the user picked).
        let follow = self.grid.follow || self.grid.tab != RUNNING_TAB;
        self.grid.follow = false;
        if follow
            && let Some(agent) = self.selected_agent()
            && !self.grid.tiles.iter().any(|t| *t == agent.id)
        {
            let dir = agent.project_dir.clone();
            let id = agent.id.clone();
            if tabs.iter().any(|t| t.key == dir) {
                let previous = std::mem::replace(&mut self.grid.tab, dir);
                self.grid.tiles = self.compute_tiles();
                if !self.grid.tiles.contains(&id) {
                    self.grid.tab = previous;
                    self.grid.tiles = self.compute_tiles();
                }
            }
        }
        // Still nothing sensible selected: take the first tile.
        if self.grid_selected_index().is_none()
            && let Some(first) = self.grid.tiles.first().cloned()
        {
            fx.extend(self.select(&first));
        }
        if let Some(expanded) = &self.grid.expanded
            && !self.grid.tiles.contains(expanded)
        {
            self.grid.expanded = None;
        }

        let layout = self.grid_layout();
        self.grid.first_row = layout.first_row;
        self.grid.visible = layout
            .tiles
            .iter()
            .filter_map(|t| self.grid.tiles.get(t.index).cloned())
            .collect();
        for id in self.grid.visible.clone() {
            fx.extend(self.attach_term(&id));
        }
        fx.extend(self.sync_layout());
        self.grid.refreshing = false;
        self.dirty = true;
        fx
    }

    fn set_grid_tab(&mut self, key: String) -> Vec<Effect> {
        if self.grid.tab == key {
            return Vec::new();
        }
        self.grid.tab = key;
        self.grid.first_row = 0;
        self.grid.expanded = None;
        self.grid.tiles = self.compute_tiles();
        // Keep the selection when it is in the new tab, else take its first tile.
        let mut fx = Vec::new();
        if self.grid_selected_index().is_none()
            && let Some(first) = self.grid.tiles.first().cloned()
        {
            fx.extend(self.select(&first));
        }
        fx.extend(self.grid_refresh());
        fx
    }

    fn grid_step_tab(&mut self, delta: isize) -> Vec<Effect> {
        let tabs = self.grid_tabs();
        if tabs.len() < 2 {
            return Vec::new();
        }
        let cur = self.grid_tab_index();
        let next = (cur as isize + delta).rem_euclid(tabs.len() as isize) as usize;
        self.set_grid_tab(tabs[next].key.clone())
    }

    fn grid_focus_tile(&mut self, dir: Dir) -> Vec<Effect> {
        let layout = self.grid_layout();
        let count = self.grid.tiles.len();
        let Some(current) = self.grid_selected_index() else {
            return Vec::new();
        };
        let next = grid::move_focus(current, count, layout.cols, dir);
        if next == current {
            return Vec::new();
        }
        let id = self.grid.tiles[next].clone();
        self.select(&id)
    }

    fn grid_move_tile(&mut self, delta: isize) -> Vec<Effect> {
        if self.grid.tab == RUNNING_TAB {
            self.toast(
                ToastKind::Info,
                "Tiles",
                "The Running tab cannot be reordered; use a project tab",
            );
            return Vec::new();
        }
        let Some(cur) = self.grid_selected_index() else {
            return Vec::new();
        };
        let other = cur as isize + delta;
        if other < 0 || other as usize >= self.grid.tiles.len() {
            return Vec::new();
        }
        let active = self.grid.tiles[cur].clone();
        let over = self.grid.tiles[other as usize].clone();
        let ids = self.grid.tiles.clone();
        let tab = self.grid.tab.clone();
        if self
            .grid
            .order
            .reorder(Some(&tab), &ids, &active, &over, false)
        {
            return self.grid_refresh();
        }
        Vec::new()
    }

    pub(super) fn grid_command(&mut self, command: Command) -> Vec<Effect> {
        if self.view != ViewMode::Grid {
            // Grid commands from the palette or a prefix key bring the grid up first.
            let mut fx = self.set_view(ViewMode::Grid);
            if matches!(command, Command::Broadcast | Command::ToggleTileExpand) {
                fx.extend(self.grid_command(command));
            }
            return fx;
        }
        match command {
            Command::GridLeft => self.grid_focus_tile(Dir::Left),
            Command::GridRight => self.grid_focus_tile(Dir::Right),
            Command::GridUp => self.grid_focus_tile(Dir::Up),
            Command::GridDown => self.grid_focus_tile(Dir::Down),
            Command::GridAttach => {
                if self.selected.is_some() {
                    self.focus = Focus::Terminal;
                }
                Vec::new()
            }
            Command::GridOpenChat => {
                if self.selected.is_some() {
                    self.set_view(ViewMode::Chat)
                } else {
                    Vec::new()
                }
            }
            Command::GridMovePrev => self.grid_move_tile(-1),
            Command::GridMoveNext => self.grid_move_tile(1),
            Command::GridPrevTab => self.grid_step_tab(-1),
            Command::GridNextTab => self.grid_step_tab(1),
            Command::ToggleTileExpand => {
                self.grid.expanded = match (&self.grid.expanded, &self.selected) {
                    (Some(_), _) => None,
                    (None, Some(id)) => Some(id.clone()),
                    (None, None) => None,
                };
                self.grid_refresh()
            }
            Command::GridOnlyRunning => {
                if self.grid.tab == RUNNING_TAB {
                    self.toast(
                        ToastKind::Info,
                        "Running tab",
                        "It already lists running agents only",
                    );
                    return Vec::new();
                }
                self.grid.only_running = !self.grid.only_running;
                self.grid_refresh()
            }
            Command::GridCycleColumns => {
                let next = match self.config.grid_columns {
                    GridColumns::Auto => GridColumns::Two,
                    GridColumns::Two => GridColumns::Three,
                    GridColumns::Three => GridColumns::Auto,
                };
                // Applied at once; the daemon's ConfigChanged confirms it.
                self.config.grid_columns = next;
                let mut fx = vec![Effect::SetGridColumns(next)];
                fx.extend(self.grid_refresh());
                fx
            }
            Command::Broadcast => {
                if !self.grid.tiles.is_empty() {
                    self.focus = Focus::Broadcast;
                    fx_sync(self)
                } else {
                    self.toast(ToastKind::Warn, "Broadcast", "No agents in this tab");
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn grid_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match self.keymap.in_grid_focus(&key) {
            Some(cmd) => self.exec(cmd),
            None => Vec::new(),
        }
    }

    // ── broadcast ───────────────────────────────────────────────────────────

    /// Agents the broadcast reaches: the active ones of the current tab.
    pub fn broadcast_targets(&self) -> Vec<String> {
        self.grid
            .tiles
            .iter()
            .filter(|id| self.agent(id).is_some_and(|a| grid::is_active(a.status)))
            .cloned()
            .collect()
    }

    pub(super) fn broadcast_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let newline = matches!(key.code, KeyCode::Enter)
            && key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::Grid;
                fx_sync(self)
            }
            _ if newline => {
                self.broadcast.insert_newline();
                self.sync_layout()
            }
            KeyCode::Enter => {
                let text = self.broadcast.text();
                let text = text.trim_end().to_string();
                if text.trim().is_empty() {
                    return Vec::new();
                }
                let targets = self.broadcast_targets();
                if targets.is_empty() {
                    self.toast(
                        ToastKind::Warn,
                        "Broadcast",
                        "No running agent in this tab to send to",
                    );
                    return Vec::new();
                }
                self.broadcast.clear();
                let n = targets.len();
                let mut fx: Vec<Effect> = Vec::new();
                for id in targets {
                    self.waiting.remove(&id);
                    fx.push(Effect::SendInput(id, text.clone()));
                }
                let plural = if n == 1 { "" } else { "s" };
                self.toast(
                    ToastKind::Success,
                    "Broadcast",
                    &format!("Sent to {n} agent{plural}"),
                );
                fx.extend(self.sync_layout());
                fx
            }
            _ => {
                self.broadcast.handle_key(&key);
                self.sync_layout()
            }
        }
    }

    // ── mouse ───────────────────────────────────────────────────────────────

    /// A left click in the grid: a tab, a tile header (opens the agent in Chat) or a tile body
    /// (focus + type into it).
    pub(super) fn grid_click(&mut self, x: u16, y: u16) -> Vec<Effect> {
        let l = self.layout();
        let at = ratatui::layout::Position::new(x, y);
        if l.title.contains(at) {
            let tabs = self.grid_tabs();
            let active = self.grid_tab_index();
            let slots = grid::tab_slots(&tabs, active, l.title.width);
            let hit = slots
                .iter()
                .find(|(_, off, w)| x >= l.title.x + off && x < l.title.x + off + w)
                .map(|(i, _, _)| tabs[*i].key.clone());
            return match hit {
                Some(key) => self.set_grid_tab(key),
                None => Vec::new(),
            };
        }
        let layout = self.grid_layout();
        let Some(tile) = layout.tile_at(x, y).copied() else {
            return Vec::new();
        };
        let Some(id) = self.grid.tiles.get(tile.index).cloned() else {
            return Vec::new();
        };
        let mut fx = self.select(&id);
        if y == tile.rect.y {
            fx.extend(self.set_view(ViewMode::Chat));
        } else {
            self.focus = Focus::Terminal;
        }
        fx
    }

    // ── palette ─────────────────────────────────────────────────────────────

    pub fn run_palette_action(&mut self, action: PaletteAction) -> Vec<Effect> {
        match action {
            PaletteAction::Command(cmd) => self.exec(cmd),
            PaletteAction::SwitchView(view) => self.set_view(view),
            PaletteAction::SwitchAgent(id) => {
                let mut fx = Vec::new();
                if self.view == ViewMode::Overview {
                    fx.extend(self.set_view(ViewMode::Chat));
                }
                fx.extend(self.select(&id));
                self.focus = self.home_focus();
                fx
            }
        }
    }

    // ── keybindings ─────────────────────────────────────────────────────────

    pub fn set_keybindings_path(&mut self, path: PathBuf) {
        self.keybindings_path = Some(path);
    }

    /// Rebuild the keymap from the contents of `keybindings.json` (`None`: no file). Problems
    /// (unknown commands, bad keys, conflicts) are shown as toasts; `announce` adds a success toast.
    pub fn apply_keybindings(&mut self, text: Option<&str>, announce: bool) {
        let parsed = as_core::keybindings::parse_rules(text.unwrap_or(""));
        let (keymap, key_problems) =
            Keymap::from_user_rules(&self.config.prefix_key, &parsed.rules);
        let mut problems = parsed.problems;
        problems.extend(key_problems);
        self.keymap = keymap;
        self.user_bindings = parsed.rules;
        self.dirty = true;
        if problems.is_empty() {
            if announce {
                let n = self.user_bindings.len();
                self.toast(
                    ToastKind::Success,
                    "Keybindings",
                    &format!("Reloaded ({n} override{})", if n == 1 { "" } else { "s" }),
                );
            }
            return;
        }
        const SHOWN: usize = 3;
        for problem in problems.iter().take(SHOWN) {
            self.toast(ToastKind::Warn, "Keybindings", problem);
        }
        if problems.len() > SHOWN {
            self.toast(
                ToastKind::Warn,
                "Keybindings",
                &format!("and {} more problems", problems.len() - SHOWN),
            );
        }
    }

    pub(super) fn open_keybindings_file(&mut self) -> Vec<Effect> {
        let Some(path) = self.keybindings_path.clone() else {
            self.toast(ToastKind::Error, "Keybindings", "No keybindings file path");
            return Vec::new();
        };
        if let Err(err) = bindings_file::ensure_file(&path) {
            self.toast(
                ToastKind::Error,
                "Keybindings",
                &format!("Could not create the file: {err:#}"),
            );
            return Vec::new();
        }
        vec![Effect::Suspend(crate::suspend::editor_command_from_env(
            &path,
        ))]
    }

    // ── persisted state ─────────────────────────────────────────────────────

    pub fn state_snapshot(&self) -> TuiState {
        TuiState {
            view: Some(self.view),
            default_view: Some(self.config.default_view_mode),
            selected_agent: self.selected.clone(),
            sidebar_width: Some(self.sidebar_width),
            sidebar_sort: self.sidebar.sort,
            sidebar_status: self.sidebar.status,
            grid_tab: Some(self.grid.tab.clone()).filter(|t| !t.is_empty()),
            expanded_tile: self.grid.expanded.clone(),
            grid_only_running: self.grid.only_running,
            tile_order: self.grid.order.stored().clone(),
            dismissed_update: self.update_dismissed.clone(),
        }
    }

    /// Apply the saved state; call before [`App::start`]. The saved view wins unless the config's
    /// `defaultViewMode` was changed since it was saved.
    pub fn restore_state(&mut self, state: TuiState) {
        let config_default = self.config.default_view_mode;
        self.view = match (state.view, state.default_view) {
            (Some(view), Some(saved_default)) if saved_default == config_default => view,
            _ => config_default,
        };
        if let Some(width) = state.sidebar_width {
            self.sidebar_width = width.clamp(layout::MIN_SIDEBAR, layout::MAX_SIDEBAR);
        }
        self.sidebar.sort = state.sidebar_sort;
        self.sidebar.status = state.sidebar_status;
        self.grid.tab = state.grid_tab.unwrap_or_default();
        self.grid.expanded = state.expanded_tile;
        self.grid.only_running = state.grid_only_running;
        self.grid.order = TileOrder::from_stored(state.tile_order);
        self.restored_selection = state.selected_agent;
        self.update_dismissed = state.dismissed_update;
    }
}

/// Re-sync PTY sizes after the bottom bar appeared or went away.
fn fx_sync(app: &mut App) -> Vec<Effect> {
    app.dirty = true;
    app.sync_layout()
}
