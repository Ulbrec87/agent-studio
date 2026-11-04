//! Overview view state and the logic of Hydra's `useOverview*` hooks, as plain data: the latest
//! snapshot, the merged history (points, events, starts), the wait ledger, the "new since"
//! marker and the card/cursor state of the view.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use as_core::overview::waits::transition_of_event;
use as_core::overview::{
    AgentDetails, HistoryResponse, OverviewEvent, OverviewSession, OverviewSnapshot,
    OverviewStatus, WaitLedgerData,
};

use super::format::{Clock, project_name};

/// How much history is loaded: 6 hours (Hydra `HISTORY_WINDOW_MS`).
pub const HISTORY_WINDOW_MS: i64 = 6 * 60 * 60 * 1000;
/// How often new points and the ledger are fetched while the view is open.
pub const REFRESH_EVERY: Duration = Duration::from_secs(30);
/// How often the drawer re-reads the transcript.
pub const DETAILS_EVERY: Duration = Duration::from_secs(2);
/// How long the "new since" divider stays in view before it goes.
pub const MARKER_CLEAR_AFTER: Duration = Duration::from_secs(10);
/// Events kept.
pub const MAX_EVENTS: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Card {
    Counts,
    NeedsYou,
    LastHour,
    Sessions,
    Events,
    Timeline,
    Waits,
}

impl Card {
    pub fn title(self) -> &'static str {
        match self {
            Card::Counts => "Counts",
            Card::NeedsYou => "Needs you",
            Card::LastHour => "Last hour",
            Card::Sessions => "Sessions",
            Card::Events => "Events",
            Card::Timeline => "Timeline",
            Card::Waits => "Waits",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionsMode {
    #[default]
    List,
    Repos,
    Board,
}

impl SessionsMode {
    pub fn next(self) -> Self {
        match self {
            SessionsMode::List => SessionsMode::Repos,
            SessionsMode::Repos => SessionsMode::Board,
            SessionsMode::Board => SessionsMode::List,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SessionsMode::List => "List",
            SessionsMode::Repos => "Repos",
            SessionsMode::Board => "Board",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaitsTab {
    #[default]
    Today,
    Week,
}

/// The details drawer.
#[derive(Debug, Clone, PartialEq)]
pub struct Drawer {
    pub agent_id: String,
    /// `None` until the daemon answered.
    pub details: Option<AgentDetails>,
    pub scroll: usize,
}

/// Where the user left off: events newer than `at` arrived while they were elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewSinceMarker {
    pub at: i64,
    pub count: usize,
    /// Index in the newest-first list of the first event the user has already seen.
    pub index: usize,
}

/// The marker for a newest-first list of events: `None` on a first visit (`last_seen` none) or
/// when nothing is newer than `last_seen`.
pub fn new_since_marker(
    events_newest_first: &[&OverviewEvent],
    last_seen: Option<i64>,
) -> Option<NewSinceMarker> {
    let at = last_seen?;
    let count = events_newest_first.iter().take_while(|e| e.at > at).count();
    (count > 0).then_some(NewSinceMarker {
        at,
        count,
        index: count,
    })
}

/// Appends events not yet known (by id), keeping time order and at most [`MAX_EVENTS`].
pub fn merge_events(known: &mut Vec<OverviewEvent>, incoming: &[OverviewEvent]) {
    if incoming.is_empty() {
        return;
    }
    let ids: HashSet<String> = known.iter().map(|e| e.id.clone()).collect();
    let fresh: Vec<&OverviewEvent> = incoming.iter().filter(|e| !ids.contains(&e.id)).collect();
    if fresh.is_empty() {
        return;
    }
    known.extend(fresh.into_iter().cloned());
    known.sort_by_key(|e| e.at);
    if known.len() > MAX_EVENTS {
        known.drain(..known.len() - MAX_EVENTS);
    }
}

/// Folds a (possibly partial) response into what is held: points, events and starts are added by
/// their time or id, agents are merged, and the scalar fields come from the newer response.
/// Points older than `cutoff` are dropped.
pub fn merge_history(
    held: Option<HistoryResponse>,
    incoming: HistoryResponse,
    cutoff: i64,
) -> HistoryResponse {
    let Some(mut held) = held else {
        let mut first = incoming;
        first.points.retain(|p| p.at >= cutoff);
        return first;
    };
    let known: HashSet<i64> = held.points.iter().map(|p| p.at).collect();
    held.points.extend(
        incoming
            .points
            .into_iter()
            .filter(|p| !known.contains(&p.at)),
    );
    held.points.retain(|p| p.at >= cutoff);
    held.points.sort_by_key(|p| p.at);
    let mut starts: Vec<i64> = held.starts;
    starts.extend(incoming.starts);
    starts.sort_unstable();
    starts.dedup();
    starts.retain(|s| *s >= cutoff);
    merge_events(&mut held.events, &incoming.events);
    held.agents.extend(incoming.agents);
    HistoryResponse {
        since: incoming.since,
        started_at: incoming.started_at,
        starts,
        points: held.points,
        events: held.events,
        agents: held.agents,
    }
}

/// Number of agents that need the user, across all projects. 0 until the first snapshot.
pub fn needs_you_count(snapshot: Option<&OverviewSnapshot>) -> usize {
    snapshot.map_or(0, |s| {
        s.sessions
            .iter()
            .filter(|s| s.status == OverviewStatus::NeedsYou)
            .count()
    })
}

/// `(2) Agent Studio` while agents need the user.
pub fn title_with_count(title: &str, count: usize) -> String {
    if count == 0 {
        title.to_string()
    } else {
        format!("({count}) {title}")
    }
}

#[derive(Debug, Clone)]
pub struct OverviewState {
    pub snapshot: Option<OverviewSnapshot>,
    pub history: Option<HistoryResponse>,
    /// Events pushed before the first history response arrived.
    early_events: Vec<OverviewEvent>,
    pub ledger: Option<WaitLedgerData>,
    pub error: Option<String>,
    pub focus: Card,
    /// First visible line of the whole page.
    pub scroll: usize,
    pub mode: SessionsMode,
    /// Project filter: `None` = all projects.
    pub project: Option<String>,
    pub needs_cursor: usize,
    pub sessions_cursor: usize,
    pub events_cursor: usize,
    pub timeline_scroll: usize,
    pub waits_tab: WaitsTab,
    pub waits_scroll: usize,
    /// Groups whose "show more" was used.
    pub expanded: HashSet<String>,
    pub drawer: Option<Drawer>,
    /// When the user last looked at the Overview.
    pub last_seen: Option<i64>,
    pub marker: Option<NewSinceMarker>,
    marker_shown_at: Option<Instant>,
    /// Test hook: a fixed clock for the cards.
    pub clock_override: Option<Clock>,
    pub history_loading: bool,
    pub waits_loading: bool,
    last_history_load: Option<Instant>,
    last_waits_load: Option<Instant>,
    last_details_load: Option<Instant>,
    last_redraw_second: u64,
    /// The card and cursor the page was last scrolled to show (so `PgUp`/`PgDn` can leave it).
    pub ensured: Option<(Card, usize)>,
}

impl Default for OverviewState {
    fn default() -> Self {
        Self {
            snapshot: None,
            history: None,
            early_events: Vec::new(),
            ledger: None,
            error: None,
            focus: Card::NeedsYou,
            scroll: 0,
            mode: SessionsMode::List,
            project: None,
            needs_cursor: 0,
            sessions_cursor: 0,
            events_cursor: 0,
            timeline_scroll: 0,
            waits_tab: WaitsTab::Today,
            waits_scroll: 0,
            expanded: HashSet::new(),
            drawer: None,
            last_seen: None,
            marker: None,
            marker_shown_at: None,
            clock_override: None,
            history_loading: false,
            waits_loading: false,
            last_history_load: None,
            last_waits_load: None,
            last_details_load: None,
            last_redraw_second: 0,
            ensured: None,
        }
    }
}

impl OverviewState {
    pub fn needs_you_count(&self) -> usize {
        needs_you_count(self.snapshot.as_ref())
    }

    pub fn clock(&self) -> Clock {
        self.clock_override.unwrap_or(Clock {
            now: chrono::Utc::now().timestamp_millis(),
            utc: false,
        })
    }

    pub fn all_sessions(&self) -> &[OverviewSession] {
        self.snapshot
            .as_ref()
            .map_or(&[], |s| s.sessions.as_slice())
    }

    /// Project directories of every session, sorted by name.
    pub fn project_dirs(&self) -> Vec<String> {
        let mut dirs: Vec<String> = self
            .all_sessions()
            .iter()
            .map(|s| s.project_dir.clone())
            .collect();
        dirs.sort_by_key(|d| (project_name(d).to_lowercase(), d.clone()));
        dirs.dedup();
        dirs
    }

    /// The filter in effect: a stored project that no longer has agents falls back to all.
    pub fn effective_project(&self) -> Option<&str> {
        let project = self.project.as_deref()?;
        self.all_sessions()
            .iter()
            .any(|s| s.project_dir == project)
            .then_some(project)
    }

    /// `p`: all -> each project -> all.
    pub fn cycle_project(&mut self) {
        let dirs = self.project_dirs();
        self.project = match self.effective_project() {
            None => dirs.first().cloned(),
            Some(current) => dirs
                .iter()
                .position(|d| d == current)
                .and_then(|i| dirs.get(i + 1).cloned()),
        };
        self.reset_cursors();
    }

    pub fn reset_cursors(&mut self) {
        self.needs_cursor = 0;
        self.sessions_cursor = 0;
        self.events_cursor = 0;
        self.timeline_scroll = 0;
        self.waits_scroll = 0;
    }

    pub fn sessions(&self) -> Vec<OverviewSession> {
        match self.effective_project() {
            None => self.all_sessions().to_vec(),
            Some(dir) => self
                .all_sessions()
                .iter()
                .filter(|s| s.project_dir == dir)
                .cloned()
                .collect(),
        }
    }

    /// Sessions that need the user, longest wait first.
    pub fn needs_you(&self) -> Vec<OverviewSession> {
        let mut list: Vec<OverviewSession> = self
            .sessions()
            .into_iter()
            .filter(|s| s.status == OverviewStatus::NeedsYou)
            .collect();
        list.sort_by_key(|s| s.status_since);
        list
    }

    /// Does this agent belong to the project in effect? Names of agents that are gone come from
    /// the history.
    pub fn agent_in_filter(&self, agent_id: &str) -> bool {
        let Some(dir) = self.effective_project() else {
            return true;
        };
        if let Some(s) = self.all_sessions().iter().find(|s| s.agent_id == agent_id) {
            return s.project_dir == dir;
        }
        self.history
            .as_ref()
            .and_then(|h| h.agents.get(agent_id))
            .is_some_and(|a| a.project_dir == dir)
    }

    /// Events of the project in effect, newest first.
    pub fn events_newest_first(&self) -> Vec<&OverviewEvent> {
        let Some(history) = &self.history else {
            return Vec::new();
        };
        let mut list: Vec<&OverviewEvent> = history
            .events
            .iter()
            .filter(|e| e.agent_id.is_empty() || self.agent_in_filter(&e.agent_id))
            .collect();
        list.reverse();
        list
    }

    pub fn set_snapshot(&mut self, snapshot: OverviewSnapshot) {
        // A pushed snapshot may already be newer than the initial response.
        if self.snapshot.as_ref().is_some_and(|s| s.at > snapshot.at) {
            return;
        }
        self.snapshot = Some(snapshot);
        self.error = None;
        self.clamp_cursors();
    }

    /// Keeps cursors inside their lists after the data changed.
    pub fn clamp_cursors(&mut self) {
        let needs = self.needs_you().len();
        self.needs_cursor = self.needs_cursor.min(needs.saturating_sub(1));
        if let Some(drawer) = &self.drawer
            && !self
                .all_sessions()
                .iter()
                .any(|s| s.agent_id == drawer.agent_id)
            && self
                .history
                .as_ref()
                .is_none_or(|h| !h.agents.contains_key(&drawer.agent_id))
        {
            self.drawer = None;
        }
    }

    pub fn set_error(&mut self, error: String) {
        if self.snapshot.is_none() {
            self.error = Some(error);
        }
    }

    /// The `since` to ask the daemon for: after the newest point held, else the window start.
    pub fn history_since(&self, now: i64) -> i64 {
        self.history
            .as_ref()
            .and_then(|h| h.points.last())
            .map_or(now - HISTORY_WINDOW_MS, |p| p.at + 1)
    }

    pub fn apply_history(&mut self, response: HistoryResponse, now: i64) {
        let cutoff = now - HISTORY_WINDOW_MS;
        let mut merged = merge_history(self.history.take(), response, cutoff);
        if !self.early_events.is_empty() {
            let early = std::mem::take(&mut self.early_events);
            merge_events(&mut merged.events, &early);
        }
        self.history = Some(merged);
        self.history_loading = false;
        self.last_history_load = Some(Instant::now());
    }

    /// Events pushed by the daemon. Returns true when one moves an agent into or out of
    /// needs-you (the ledger should be reloaded).
    pub fn add_events(&mut self, events: &[OverviewEvent]) -> bool {
        match &mut self.history {
            Some(history) => {
                merge_events(&mut history.events, events);
                for e in events {
                    if !e.agent_id.is_empty() {
                        history.agents.insert(
                            e.agent_id.clone(),
                            as_core::overview::HistoryAgent {
                                name: e.name.clone(),
                                project_dir: e.project_dir.clone(),
                            },
                        );
                    }
                }
            }
            None => self.early_events.extend_from_slice(events),
        }
        events.iter().any(|e| transition_of_event(e).is_some())
    }

    pub fn apply_ledger(&mut self, ledger: WaitLedgerData) {
        self.ledger = Some(ledger);
        self.waits_loading = false;
        self.last_waits_load = Some(Instant::now());
    }

    // ── "new since" marker ──────────────────────────────────────────────────

    /// The user opened the Overview: mark what happened since they last looked.
    pub fn entered(&mut self, now_ms: i64, at: Instant) {
        let events = self.events_newest_first();
        self.marker = new_since_marker(&events, self.last_seen);
        self.marker_shown_at = self.marker.map(|_| at);
        self.last_seen.get_or_insert(now_ms);
        // What the view shows is refreshed as soon as it opens.
        self.last_history_load = None;
        self.last_waits_load = None;
    }

    /// The user left the Overview.
    pub fn left(&mut self, now_ms: i64) {
        self.last_seen = Some(now_ms);
        self.marker = None;
        self.marker_shown_at = None;
        self.drawer = None;
    }

    // ── periodic work ───────────────────────────────────────────────────────

    /// Time-driven chores while the view is open. Returns what to refresh and whether the screen
    /// needs a redraw (timers tick once a second).
    pub fn tick(&mut self, now: Instant, now_ms: i64) -> Ticks {
        let mut out = Ticks::default();
        let due = |last: Option<Instant>, every: Duration| {
            last.is_none_or(|l| now.duration_since(l) >= every)
        };
        if !self.history_loading && due(self.last_history_load, REFRESH_EVERY) {
            self.history_loading = true;
            out.history_since = Some(self.history_since(now_ms));
        }
        if !self.waits_loading && due(self.last_waits_load, REFRESH_EVERY) {
            self.waits_loading = true;
            out.waits = true;
        }
        if let Some(drawer) = &self.drawer
            && due(self.last_details_load, DETAILS_EVERY)
        {
            self.last_details_load = Some(now);
            out.details = Some(drawer.agent_id.clone());
        }
        if let (Some(_), Some(shown)) = (self.marker, self.marker_shown_at)
            && now.duration_since(shown) >= MARKER_CLEAR_AFTER
        {
            self.marker = None;
            self.marker_shown_at = None;
            self.last_seen = Some(now_ms);
            out.redraw = true;
        }
        let second = u64::try_from(now_ms.max(0) / 1000).unwrap_or(0);
        if second != self.last_redraw_second {
            self.last_redraw_second = second;
            out.redraw = true;
        }
        out
    }

    pub fn open_drawer(&mut self, agent_id: &str) {
        self.drawer = Some(Drawer {
            agent_id: agent_id.to_string(),
            details: None,
            scroll: 0,
        });
        self.last_details_load = None;
    }

    pub fn set_details(&mut self, agent_id: &str, details: AgentDetails) {
        if let Some(d) = &mut self.drawer
            && d.agent_id == agent_id
        {
            d.details = Some(details);
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Ticks {
    pub history_since: Option<i64>,
    pub waits: bool,
    pub details: Option<String>,
    pub redraw: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use as_core::overview::{EventKind, HistoryAgent, HistoryPoint};

    const NOW: i64 = 1_800_000_000_000;

    fn ev(at: i64) -> OverviewEvent {
        OverviewEvent {
            id: format!("a@{at}"),
            at,
            kind: EventKind::Status,
            agent_id: "a".into(),
            name: "alpha".into(),
            project_dir: "/w".into(),
            from: None,
            to: None,
            waiting_reason: None,
            waited_ms: None,
            gap_ms: None,
        }
    }
    fn response() -> HistoryResponse {
        HistoryResponse {
            since: NOW - 1000,
            started_at: NOW - 1000,
            starts: vec![NOW - 1000],
            ..Default::default()
        }
    }
    fn pt(at: i64) -> HistoryPoint {
        HistoryPoint {
            at,
            working: 1,
            total: 1,
            ..Default::default()
        }
    }
    fn agent_info(name: &str, dir: &str) -> HistoryAgent {
        HistoryAgent {
            name: name.into(),
            project_dir: dir.into(),
        }
    }

    #[test]
    fn merge_adds_new_points_events_and_agents_once_and_drops_points_past_the_cutoff() {
        let mut held = response();
        held.points = vec![pt(10), pt(20)];
        held.events = vec![ev(15)];
        held.agents.insert("a".into(), agent_info("alpha", "/w"));
        let mut incoming = response();
        incoming.points = vec![pt(20), pt(30)];
        incoming.events = vec![ev(15), ev(25)];
        incoming.starts = vec![5];
        incoming.agents.insert("b".into(), agent_info("b", "/b"));
        let merged = merge_history(Some(held), incoming, 15);
        assert_eq!(
            merged.points.iter().map(|p| p.at).collect::<Vec<_>>(),
            [20, 30]
        );
        assert_eq!(
            merged.events.iter().map(|e| e.at).collect::<Vec<_>>(),
            [15, 25]
        );
        assert_eq!(
            merged.agents.keys().cloned().collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(merged.starts, [NOW - 1000]); // the start at 5 is before the cutoff
    }

    #[test]
    fn the_first_response_is_taken_as_is_minus_old_points() {
        let mut first = response();
        first.points = vec![pt(1), pt(NOW - 5)];
        let merged = merge_history(None, first, NOW - 100);
        assert_eq!(merged.points.len(), 1);
    }

    #[test]
    fn events_pushed_before_the_first_response_are_kept_and_not_duplicated() {
        let mut s = OverviewState::default();
        s.add_events(&[ev(NOW - 10)]);
        let mut first = response();
        first.events = vec![ev(NOW - 100)];
        s.apply_history(first, NOW);
        let ats = |s: &OverviewState| {
            s.history
                .as_ref()
                .unwrap()
                .events
                .iter()
                .map(|e| e.at)
                .collect::<Vec<_>>()
        };
        assert_eq!(ats(&s), [NOW - 100, NOW - 10]);
        s.add_events(&[ev(NOW - 10), ev(NOW - 5)]);
        assert_eq!(ats(&s), [NOW - 100, NOW - 10, NOW - 5]);
    }

    #[test]
    fn asks_only_for_points_newer_than_the_newest_held() {
        let mut s = OverviewState::default();
        assert_eq!(s.history_since(NOW), NOW - HISTORY_WINDOW_MS);
        let mut first = response();
        first.points = vec![pt(NOW - 5000)];
        s.apply_history(first, NOW);
        assert_eq!(s.history_since(NOW), NOW - 5000 + 1);
        let mut second = response();
        second.points = vec![pt(NOW + 25_000)];
        s.apply_history(second, NOW + 30_000);
        assert_eq!(s.history.as_ref().unwrap().points.len(), 2);
    }

    #[test]
    fn a_stale_snapshot_never_replaces_a_newer_one() {
        let mut s = OverviewState::default();
        s.set_snapshot(OverviewSnapshot {
            at: 200,
            ..Default::default()
        });
        s.set_snapshot(OverviewSnapshot {
            at: 100,
            ..Default::default()
        });
        assert_eq!(s.snapshot.as_ref().unwrap().at, 200);
        s.set_error("late".into());
        assert_eq!(
            s.error, None,
            "an error is only kept while there is no snapshot"
        );
    }

    #[test]
    fn events_that_move_an_agent_in_or_out_of_needs_you_ask_for_the_ledger() {
        let mut s = OverviewState::default();
        let mut e = ev(5);
        e.from = Some(OverviewStatus::Working);
        e.to = Some(OverviewStatus::NeedsYou);
        assert!(s.add_events(&[e]));
        let mut other = ev(6);
        other.from = Some(OverviewStatus::Working);
        other.to = Some(OverviewStatus::Idle);
        assert!(!s.add_events(&[other]));
    }

    #[test]
    fn needs_you_count_and_title() {
        assert_eq!(needs_you_count(None), 0);
        let snap = OverviewSnapshot {
            sessions: vec![
                OverviewSession::basic("a", OverviewStatus::NeedsYou),
                OverviewSession::basic("b", OverviewStatus::Working),
                OverviewSession::basic("c", OverviewStatus::NeedsYou),
            ],
            ..Default::default()
        };
        assert_eq!(needs_you_count(Some(&snap)), 2);
        assert_eq!(title_with_count("Agent Studio", 0), "Agent Studio");
        assert_eq!(title_with_count("Agent Studio", 3), "(3) Agent Studio");
    }

    #[test]
    fn the_new_since_marker() {
        let events = [ev(50), ev(40), ev(30), ev(20)];
        let list: Vec<&OverviewEvent> = events.iter().collect();
        assert_eq!(new_since_marker(&list, None), None);
        assert_eq!(new_since_marker(&list, Some(50)), None);
        assert_eq!(new_since_marker(&[], Some(10)), None);
        assert_eq!(
            new_since_marker(&list, Some(35)),
            Some(NewSinceMarker {
                at: 35,
                count: 2,
                index: 2
            })
        );
        assert_eq!(
            new_since_marker(&list, Some(5)),
            Some(NewSinceMarker {
                at: 5,
                count: 4,
                index: 4
            })
        );
    }

    #[test]
    fn the_marker_is_set_on_entering_and_clears_after_10_seconds_in_view() {
        let mut s = OverviewState::default();
        let mut h = response();
        h.events = vec![ev(100), ev(200), ev(300)];
        s.apply_history(h, NOW);
        s.last_seen = Some(150);
        let t0 = Instant::now();
        s.entered(NOW, t0);
        assert_eq!(
            s.marker,
            Some(NewSinceMarker {
                at: 150,
                count: 2,
                index: 2
            })
        );
        let early = s.tick(t0 + Duration::from_secs(5), NOW);
        assert!(s.marker.is_some());
        assert!(early.redraw || !early.redraw);
        let later = s.tick(t0 + Duration::from_secs(11), NOW + 11_000);
        assert!(s.marker.is_none());
        assert!(later.redraw);
        assert_eq!(s.last_seen, Some(NOW + 11_000));
        s.left(NOW + 20_000);
        assert_eq!(s.last_seen, Some(NOW + 20_000));
    }

    #[test]
    fn the_first_visit_has_no_marker() {
        let mut s = OverviewState::default();
        let mut h = response();
        h.events = vec![ev(100)];
        s.apply_history(h, NOW);
        s.entered(NOW, Instant::now());
        assert_eq!(s.marker, None);
        assert_eq!(s.last_seen, Some(NOW));
    }

    #[test]
    fn tick_asks_for_refreshes_when_due_and_once() {
        let mut s = OverviewState::default();
        let t0 = Instant::now();
        let first = s.tick(t0, NOW);
        assert_eq!(first.history_since, Some(NOW - HISTORY_WINDOW_MS));
        assert!(first.waits);
        // While a load is in flight nothing is asked twice.
        let again = s.tick(t0 + Duration::from_secs(1), NOW + 1000);
        assert_eq!((again.history_since, again.waits), (None, false));
        s.apply_history(response(), NOW);
        s.apply_ledger(WaitLedgerData::default());
        assert_eq!(
            s.tick(t0 + Duration::from_secs(2), NOW + 2000)
                .history_since,
            None
        );
        let due = s.tick(
            Instant::now() + REFRESH_EVERY + Duration::from_secs(1),
            NOW + 40_000,
        );
        assert!(due.history_since.is_some() && due.waits);
    }

    #[test]
    fn the_drawer_polls_every_two_seconds_and_follows_its_agent() {
        let mut s = OverviewState::default();
        s.set_snapshot(OverviewSnapshot {
            sessions: vec![OverviewSession::basic("a", OverviewStatus::Working)],
            ..Default::default()
        });
        s.open_drawer("a");
        let t0 = Instant::now();
        assert_eq!(s.tick(t0, NOW).details.as_deref(), Some("a"));
        assert_eq!(
            s.tick(t0 + Duration::from_secs(1), NOW + 1000).details,
            None
        );
        assert_eq!(
            s.tick(t0 + Duration::from_secs(3), NOW + 3000)
                .details
                .as_deref(),
            Some("a")
        );
        s.set_details("other", AgentDetails::unavailable("x"));
        assert!(s.drawer.as_ref().unwrap().details.is_none());
        s.set_details("a", AgentDetails::unavailable("x"));
        assert!(s.drawer.as_ref().unwrap().details.is_some());
        // The agent goes away: the drawer closes.
        s.set_snapshot(OverviewSnapshot {
            at: 5,
            ..Default::default()
        });
        assert!(s.drawer.is_none());
    }

    #[test]
    fn the_project_filter_cycles_and_falls_back_to_all() {
        let mut s = OverviewState::default();
        let mut a = OverviewSession::basic("a", OverviewStatus::Working);
        a.project_dir = "/w/zeta".into();
        let mut b = OverviewSession::basic("b", OverviewStatus::NeedsYou);
        b.project_dir = "/w/alpha".into();
        s.set_snapshot(OverviewSnapshot {
            sessions: vec![a, b],
            ..Default::default()
        });
        assert_eq!(s.project_dirs(), ["/w/alpha", "/w/zeta"]);
        assert_eq!(s.effective_project(), None);
        s.cycle_project();
        assert_eq!(s.effective_project(), Some("/w/alpha"));
        assert_eq!(s.sessions().len(), 1);
        assert_eq!(s.needs_you().len(), 1);
        s.cycle_project();
        assert_eq!(s.effective_project(), Some("/w/zeta"));
        assert_eq!(s.needs_you().len(), 0);
        s.cycle_project();
        assert_eq!(s.effective_project(), None);
        // A stored project that has no agents any more behaves like "All" without forgetting.
        s.project = Some("/gone".into());
        assert_eq!(s.effective_project(), None);
        assert_eq!(s.sessions().len(), 2);
    }

    #[test]
    fn needs_you_is_longest_wait_first() {
        let mut s = OverviewState::default();
        let mk = |id: &str, since: i64| {
            let mut x = OverviewSession::basic(id, OverviewStatus::NeedsYou);
            x.status_since = since;
            x
        };
        s.set_snapshot(OverviewSnapshot {
            sessions: vec![mk("new", 900), mk("old", 100)],
            ..Default::default()
        });
        let ids: Vec<String> = s.needs_you().into_iter().map(|x| x.agent_id).collect();
        assert_eq!(ids, ["old", "new"]);
    }
}
