//! Overview view: keys, jump, periodic refreshes. All of it is `impl App`.

use super::*;

use crate::overview::model::{self, SessionRow};
use crate::overview::state::Card;
use crate::overview::{SessionsMode, WaitsTab};

/// The order `Tab` walks the cards (Counts is informational).
pub const FOCUS_ORDER: [Card; 6] = [
    Card::NeedsYou,
    Card::Sessions,
    Card::Events,
    Card::LastHour,
    Card::Timeline,
    Card::Waits,
];

impl App {
    /// Time-driven work of the Overview (history/ledger refresh every 30 s, the drawer every 2 s,
    /// the one-second redraw of the timers). Call it every frame.
    pub fn poll_effects(&mut self, now: Instant) -> Vec<Effect> {
        if self.view != ViewMode::Overview {
            return Vec::new();
        }
        let ticks = self
            .overview
            .tick(now, chrono::Utc::now().timestamp_millis());
        if ticks.redraw {
            self.dirty = true;
        }
        let mut fx = Vec::new();
        if let Some(since) = ticks.history_since {
            fx.push(Effect::LoadOverviewHistory(since));
        }
        if ticks.waits {
            fx.push(Effect::LoadOverviewWaits);
        }
        if let Some(id) = ticks.details {
            fx.push(Effect::LoadAgentDetails(id));
        }
        fx
    }

    /// Jump: select the agent, show it in the Chat view, keys go to its terminal.
    pub fn overview_jump(&mut self, agent_id: &str) -> Vec<Effect> {
        if self.agent(agent_id).is_none() {
            self.toast(ToastKind::Warn, "Jump", "That agent is gone");
            return Vec::new();
        }
        let mut fx = self.select(agent_id);
        fx.extend(self.set_view(ViewMode::Chat));
        self.focus = Focus::Terminal;
        self.dirty = true;
        fx
    }

    fn open_details(&mut self, agent_id: &str) -> Vec<Effect> {
        self.overview.open_drawer(agent_id);
        self.dirty = true;
        vec![Effect::LoadAgentDetails(agent_id.to_string())]
    }

    fn step_card(&mut self, delta: isize) {
        let at = FOCUS_ORDER
            .iter()
            .position(|c| *c == self.overview.focus)
            .unwrap_or(0);
        let next = (at as isize + delta).rem_euclid(FOCUS_ORDER.len() as isize) as usize;
        self.overview.focus = FOCUS_ORDER[next];
    }

    /// Move the cursor (or the scroll) of the focused card.
    fn card_move(&mut self, delta: isize) {
        let step = |v: usize, max: usize| (v as isize + delta).clamp(0, max as isize) as usize;
        match self.overview.focus {
            Card::NeedsYou => {
                let n = self.overview.needs_you().len();
                self.overview.needs_cursor = step(self.overview.needs_cursor, n.saturating_sub(1));
            }
            Card::Sessions => {
                let n = model::selectable_sessions(&self.overview).len();
                self.overview.sessions_cursor =
                    step(self.overview.sessions_cursor, n.saturating_sub(1));
            }
            Card::Events => {
                let n = self.overview.events_newest_first().len();
                self.overview.events_cursor =
                    step(self.overview.events_cursor, n.saturating_sub(1));
            }
            Card::Timeline => {
                let n = model::timeline(&self.overview, &self.overview.clock()).len();
                self.overview.timeline_scroll =
                    step(self.overview.timeline_scroll, n.saturating_sub(1));
            }
            Card::Waits => {
                self.overview.waits_scroll = step(self.overview.waits_scroll, 5);
            }
            Card::Counts | Card::LastHour => {
                self.overview.scroll = step(self.overview.scroll, usize::MAX / 2);
            }
        }
    }

    fn page(&self) -> usize {
        usize::from(self.layout().terminal.height.saturating_sub(2).max(1))
    }

    pub(super) fn overview_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if self.overview.drawer.is_some() {
            return self.drawer_key(key);
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Tab => self.step_card(1),
            KeyCode::BackTab => self.step_card(-1),
            KeyCode::Down | KeyCode::Char('j') => self.card_move(1),
            KeyCode::Up | KeyCode::Char('k') => self.card_move(-1),
            KeyCode::PageDown => {
                self.overview.scroll = self.overview.scroll.saturating_add(self.page());
            }
            KeyCode::PageUp => {
                self.overview.scroll = self.overview.scroll.saturating_sub(self.page());
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.overview.scroll = 0;
                self.card_move(-100_000);
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.card_move(100_000);
            }
            KeyCode::Char('m') if !shift => {
                self.overview.mode = self.overview.mode.next();
                self.overview.sessions_cursor = 0;
                self.overview.focus = Card::Sessions;
            }
            KeyCode::Char('p') => self.overview.cycle_project(),
            KeyCode::Char('t') => {
                self.overview.waits_tab = match self.overview.waits_tab {
                    WaitsTab::Today => WaitsTab::Week,
                    WaitsTab::Week => WaitsTab::Today,
                };
            }
            KeyCode::Char('d' | ' ') => return self.card_details(),
            KeyCode::Enter => return self.card_enter(),
            KeyCode::Esc => {
                self.overview.drawer = None;
            }
            _ => {}
        }
        self.dirty = true;
        Vec::new()
    }

    /// Details of the item under the cursor of the focused card.
    fn card_details(&mut self) -> Vec<Effect> {
        let id = match self.overview.focus {
            Card::NeedsYou => self
                .overview
                .needs_you()
                .get(self.overview.needs_cursor)
                .map(|s| s.agent_id.clone()),
            Card::Sessions => model::cursor_session(&self.overview).map(|s| s.agent_id),
            Card::Events => self.event_agent(),
            _ => None,
        };
        match id {
            Some(id) => self.open_details(&id),
            None => Vec::new(),
        }
    }

    fn event_agent(&self) -> Option<String> {
        let events = self.overview.events_newest_first();
        let agent = events.get(self.overview.events_cursor)?.agent_id.clone();
        (!agent.is_empty()).then_some(agent)
    }

    /// Enter: jump from Needs you, open the drawer from Sessions and Events, expand "show more".
    fn card_enter(&mut self) -> Vec<Effect> {
        match self.overview.focus {
            Card::NeedsYou => {
                let id = self
                    .overview
                    .needs_you()
                    .get(self.overview.needs_cursor)
                    .map(|s| s.agent_id.clone());
                match id {
                    Some(id) => self.overview_jump(&id),
                    None => Vec::new(),
                }
            }
            Card::Sessions if self.overview.mode != SessionsMode::Board => {
                let rows = model::selectable_sessions(&self.overview);
                match rows.get(self.overview.sessions_cursor) {
                    Some(SessionRow::Session(s)) => {
                        let id = s.agent_id.clone();
                        self.open_details(&id)
                    }
                    Some(SessionRow::ShowMore { key, .. }) => {
                        if !self.overview.expanded.remove(key) {
                            self.overview.expanded.insert(key.clone());
                        }
                        self.dirty = true;
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            Card::Sessions | Card::Events => self.card_details(),
            _ => Vec::new(),
        }
    }

    fn drawer_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        self.dirty = true;
        let Some(drawer) = &mut self.overview.drawer else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.overview.drawer = None,
            KeyCode::Enter | KeyCode::Char('J') => {
                let id = drawer.agent_id.clone();
                return self.overview_jump(&id);
            }
            KeyCode::Down | KeyCode::Char('j') => drawer.scroll = drawer.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => drawer.scroll = drawer.scroll.saturating_sub(1),
            KeyCode::PageDown => drawer.scroll = drawer.scroll.saturating_add(10),
            KeyCode::PageUp => drawer.scroll = drawer.scroll.saturating_sub(10),
            _ => {}
        }
        Vec::new()
    }
}
