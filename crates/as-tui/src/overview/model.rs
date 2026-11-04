//! What the cards show, worked out from the state: pure functions over [`OverviewState`].

use std::collections::BTreeMap;

use as_core::overview::buckets::{
    BucketOptions, CountKey, HOUR_MS, LastHourData, Segment, count_series, last_hour_buckets,
    timeline_segments,
};
use as_core::overview::grouping::{
    BoardColumn, ENDED_GROUP_LIMIT, board_columns, group_by_repo, group_by_status,
};
use as_core::overview::waits::{LastHourWaits, WaitsInput, last_hour_waits, wait_totals};
use as_core::overview::{OverviewSession, OverviewStatus, WaitTotals, WaitingNow};

use super::format::Clock;
use super::state::{OverviewState, SessionsMode};

/// Cards shown per Board column before "and N more".
pub const BOARD_MAX: usize = 5;

/// One line of the List / Repos layouts.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionRow {
    Group {
        label: String,
        count: usize,
    },
    Session(Box<OverviewSession>),
    /// "Show N more" / "Show less" of a group with a limit.
    ShowMore {
        key: String,
        hidden: usize,
        expanded: bool,
    },
}

impl SessionRow {
    pub fn selectable(&self) -> bool {
        !matches!(self, SessionRow::Group { .. })
    }
}

/// The rows of the List or Repos layout, groups included.
pub fn session_rows(state: &OverviewState) -> Vec<SessionRow> {
    let sessions = state.sessions();
    let groups = match state.mode {
        SessionsMode::Repos => group_by_repo(&sessions, |s| s.repo_name.clone()),
        _ => group_by_status(&sessions),
    };
    let mut rows = Vec::new();
    for g in groups {
        rows.push(SessionRow::Group {
            label: g.label.clone(),
            count: g.sessions.len(),
        });
        let expanded = state.expanded.contains(&g.key);
        let limit = g.limit.filter(|_| !expanded).unwrap_or(usize::MAX);
        let shown = g.sessions.len().min(limit);
        rows.extend(
            g.sessions
                .iter()
                .take(shown)
                .cloned()
                .map(|s| SessionRow::Session(Box::new(s))),
        );
        if let Some(limit) = g.limit
            && g.sessions.len() > limit
        {
            rows.push(SessionRow::ShowMore {
                key: g.key.clone(),
                hidden: g.sessions.len() - limit,
                expanded,
            });
        }
    }
    rows
}

/// The Board layout's columns.
pub fn board(state: &OverviewState) -> Vec<BoardColumn> {
    board_columns(&state.sessions(), BOARD_MAX)
}

/// Agent ids in the order the sessions cursor walks them (column by column on the Board).
pub fn selectable_sessions(state: &OverviewState) -> Vec<SessionRow> {
    match state.mode {
        SessionsMode::Board => board(state)
            .into_iter()
            .flat_map(|c| c.items)
            .map(|s| SessionRow::Session(Box::new(s)))
            .collect(),
        _ => session_rows(state)
            .into_iter()
            .filter(SessionRow::selectable)
            .collect(),
    }
}

/// The session under the sessions cursor.
pub fn cursor_session(state: &OverviewState) -> Option<OverviewSession> {
    match selectable_sessions(state).get(state.sessions_cursor)? {
        SessionRow::Session(s) => Some((**s).clone()),
        _ => None,
    }
}

// ── counts ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountKind {
    Working,
    Idle,
    Stale,
    NeedsYou,
    Finished,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CountCell {
    pub kind: CountKind,
    pub label: &'static str,
    pub value: u32,
    pub detail: String,
    /// Last hour, 12 buckets of 5 minutes (`None` = not measured). Empty for stale (no series).
    pub series: Vec<Option<f64>>,
}

pub fn agent_filter(state: &OverviewState) -> Option<impl Fn(&str) -> bool + '_> {
    state.effective_project()?;
    Some(move |id: &str| state.agent_in_filter(id))
}

/// The six counts with their last-hour series.
pub fn count_cells(state: &OverviewState, clock: &Clock) -> Vec<CountCell> {
    let sessions = state.sessions();
    let counts = as_core::overview::status::compute_counts(&sessions, clock.now);
    let filter = agent_filter(state);
    let f: Option<&dyn Fn(&str) -> bool> = filter.as_ref().map(|f| f as &dyn Fn(&str) -> bool);
    let (points, starts): (&[_], &[_]) = match &state.history {
        Some(h) => (&h.points, &h.starts),
        None => (&[], &[]),
    };
    let series = |key: CountKey| {
        count_series(points, starts, key, clock.now, HOUR_MS, 12, f)
            .into_iter()
            .map(|b| b.value)
            .collect::<Vec<_>>()
    };
    let longest = |ms: Option<i64>| {
        format!(
            "longest {}",
            ms.map_or("\u{2014}".to_string(), |ms| super::format::format_duration(
                ms, true
            ))
        )
    };
    vec![
        CountCell {
            kind: CountKind::Working,
            label: "Working",
            value: counts.working,
            detail: longest(counts.longest_working_ms),
            series: series(CountKey::Working),
        },
        CountCell {
            kind: CountKind::Idle,
            label: "Idle",
            value: counts.idle,
            detail: longest(counts.longest_idle_ms),
            series: series(CountKey::Idle),
        },
        CountCell {
            kind: CountKind::Stale,
            label: "Stale",
            value: counts.stale,
            detail: "idle for 24h+".into(),
            series: Vec::new(),
        },
        CountCell {
            kind: CountKind::NeedsYou,
            label: "Needs you",
            value: counts.needs_you,
            detail: format!("{} open", counts.open),
            series: series(CountKey::NeedsYou),
        },
        CountCell {
            kind: CountKind::Finished,
            label: "Finished",
            value: counts.finished,
            detail: if counts.not_running > 0 {
                format!("{} not running", counts.not_running)
            } else {
                "ended cleanly".into()
            },
            series: series(CountKey::Finished),
        },
        CountCell {
            kind: CountKind::Failed,
            label: "Failed",
            value: counts.failed,
            detail: format!("of {} agents", counts.total),
            series: series(CountKey::Failed),
        },
    ]
}

// ── last hour, timeline, waits ──────────────────────────────────────────────

pub fn last_hour(state: &OverviewState, clock: &Clock) -> Option<LastHourData> {
    let history = state.history.as_ref()?;
    let filter = agent_filter(state);
    let f: Option<&dyn Fn(&str) -> bool> = filter.as_ref().map(|f| f as &dyn Fn(&str) -> bool);
    Some(last_hour_buckets(
        &history.points,
        &history.starts,
        clock.now,
        BucketOptions::default(),
        f,
    ))
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineRow {
    pub agent_id: String,
    pub name: String,
    pub segments: Vec<Segment>,
}

/// One row per agent seen in the last hour (current sessions first, then the ones that left),
/// leaving out agents that were never anything but not-running.
pub fn timeline(state: &OverviewState, clock: &Clock) -> Vec<TimelineRow> {
    let Some(history) = &state.history else {
        return Vec::new();
    };
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for (id, a) in &history.agents {
        if state.agent_in_filter(id) {
            names.insert(id.clone(), a.name.clone());
        }
    }
    for s in state.sessions() {
        names.insert(s.agent_id.clone(), s.name.clone());
    }
    let ids: Vec<String> = names.keys().cloned().collect();
    let mut segments =
        timeline_segments(&history.points, &history.starts, &ids, clock.now, HOUR_MS);
    let mut rows: Vec<TimelineRow> = ids
        .into_iter()
        .filter_map(|id| {
            let segs = segments.remove(&id)?;
            let visible = segs.iter().any(|s| {
                !matches!(
                    s.status,
                    as_core::overview::buckets::SegmentStatus::Unmeasured
                        | as_core::overview::buckets::SegmentStatus::Status(
                            OverviewStatus::NotRunning
                        )
                )
            });
            visible.then(|| TimelineRow {
                name: names.get(&id).cloned().unwrap_or_else(|| id.clone()),
                agent_id: id,
                segments: segs,
            })
        })
        .collect();
    let order = |id: &str| {
        state
            .all_sessions()
            .iter()
            .position(|s| s.agent_id == id)
            .unwrap_or(usize::MAX)
    };
    rows.sort_by(|a, b| {
        order(&a.agent_id)
            .cmp(&order(&b.agent_id))
            .then(a.name.cmp(&b.name))
    });
    rows
}

pub fn waiting_now(state: &OverviewState) -> Vec<WaitingNow> {
    state
        .all_sessions()
        .iter()
        .filter(|s| s.status == OverviewStatus::NeedsYou)
        .map(|s| WaitingNow {
            agent_id: s.agent_id.clone(),
            name: s.name.clone(),
            since: s.status_since,
        })
        .collect()
}

/// Which project each agent that waited belongs to, current sessions winning.
fn ledger_filter(state: &OverviewState) -> Option<impl Fn(&str) -> bool> {
    let dir = state.effective_project()?.to_string();
    let mut dirs: BTreeMap<String, String> = BTreeMap::new();
    if let Some(ledger) = &state.ledger {
        for t in &ledger.transitions {
            dirs.insert(t.agent_id.clone(), t.project_dir.clone());
        }
    }
    for s in state.all_sessions() {
        dirs.insert(s.agent_id.clone(), s.project_dir.clone());
    }
    Some(move |id: &str| dirs.get(id).is_some_and(|d| *d == dir))
}

pub fn wait_totals_for(state: &OverviewState, clock: &Clock) -> Option<WaitTotals> {
    let ledger = state.ledger.as_ref()?;
    let waiting = waiting_now(state);
    let filter = ledger_filter(state);
    let f: Option<&dyn Fn(&str) -> bool> = filter.as_ref().map(|f| f as &dyn Fn(&str) -> bool);
    Some(wait_totals(
        &WaitsInput {
            transitions: &ledger.transitions,
            spans: &ledger.spans,
            starts: &ledger.starts,
            waiting_now: &waiting,
            now: clock.now,
            agent_filter: f,
        },
        clock.zone(),
    ))
}

pub fn last_hour_waits_for(state: &OverviewState, clock: &Clock) -> Option<LastHourWaits> {
    let ledger = state.ledger.as_ref()?;
    let waiting = waiting_now(state);
    let filter = ledger_filter(state);
    let f: Option<&dyn Fn(&str) -> bool> = filter.as_ref().map(|f| f as &dyn Fn(&str) -> bool);
    Some(last_hour_waits(
        &ledger.transitions,
        &waiting,
        &ledger.spans,
        clock.now,
        &ledger.starts,
        f,
        ledger.starts.last().copied(),
    ))
}

/// Own events of one agent, newest first.
pub fn events_of_agent<'a>(
    state: &'a OverviewState,
    agent_id: &str,
) -> Vec<&'a as_core::overview::OverviewEvent> {
    let Some(h) = &state.history else {
        return Vec::new();
    };
    let mut list: Vec<_> = h.events.iter().filter(|e| e.agent_id == agent_id).collect();
    list.sort_by_key(|e| std::cmp::Reverse(e.at));
    list
}

pub fn limit_of_ended() -> usize {
    ENDED_GROUP_LIMIT
}

#[cfg(test)]
mod tests {
    use super::*;
    use OverviewStatus as S;
    use as_core::overview::{HistoryAgent, HistoryPoint, HistoryResponse, OverviewSnapshot};

    fn state_with(sessions: Vec<OverviewSession>) -> OverviewState {
        let mut s = OverviewState::default();
        s.set_snapshot(OverviewSnapshot {
            at: 1,
            sessions,
            ..Default::default()
        });
        s
    }
    fn sess(id: &str, status: S, since: i64) -> OverviewSession {
        let mut x = OverviewSession::basic(id, status);
        x.name = id.to_string();
        x.status_since = since;
        x
    }

    #[test]
    fn list_rows_have_groups_and_a_show_more_row_past_the_ended_limit() {
        let mut sessions = vec![sess("w", S::Working, 1)];
        for i in 0..13 {
            sessions.push(sess(&format!("n{i:02}"), S::NotRunning, i));
        }
        let mut s = state_with(sessions);
        let rows = session_rows(&s);
        assert!(matches!(&rows[0], SessionRow::Group { label, count: 1 } if label == "Working"));
        assert!(
            matches!(&rows[2], SessionRow::Group { label, count: 13 } if label == "Not running")
        );
        let sessions_shown = rows
            .iter()
            .filter(|r| matches!(r, SessionRow::Session(_)))
            .count();
        assert_eq!(sessions_shown, 1 + 10);
        // Newest first: the highest `status_since` heads the group.
        assert!(matches!(&rows[3], SessionRow::Session(x) if x.agent_id == "n12"));
        assert!(matches!(
            rows.last(),
            Some(SessionRow::ShowMore {
                hidden: 3,
                expanded: false,
                ..
            })
        ));
        s.expanded.insert("not-running".into());
        let rows = session_rows(&s);
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r, SessionRow::Session(_)))
                .count(),
            14
        );
        assert!(matches!(
            rows.last(),
            Some(SessionRow::ShowMore {
                hidden: 3,
                expanded: true,
                ..
            })
        ));
    }

    #[test]
    fn repos_group_by_repository_name() {
        let mut a = sess("a", S::Working, 1);
        a.repo_name = Some("shop".into());
        let b = sess("b", S::Idle, 1);
        let mut s = state_with(vec![a, b]);
        s.mode = SessionsMode::Repos;
        let labels: Vec<String> = session_rows(&s)
            .into_iter()
            .filter_map(|r| match r {
                SessionRow::Group { label, .. } => Some(label),
                _ => None,
            })
            .collect();
        assert_eq!(labels, ["shop", "No repository"]);
    }

    #[test]
    fn the_cursor_walks_selectable_rows_only() {
        let mut s = state_with(vec![sess("a", S::Working, 1), sess("b", S::Idle, 1)]);
        assert_eq!(selectable_sessions(&s).len(), 2);
        s.sessions_cursor = 1;
        assert_eq!(cursor_session(&s).unwrap().agent_id, "b");
        s.mode = SessionsMode::Board;
        s.sessions_cursor = 0;
        assert_eq!(
            cursor_session(&s).unwrap().agent_id,
            "a",
            "working column comes first"
        );
    }

    fn history_with(points: Vec<HistoryPoint>, agents: &[(&str, &str, &str)]) -> HistoryResponse {
        HistoryResponse {
            points,
            agents: agents
                .iter()
                .map(|(id, n, d)| {
                    (
                        (*id).to_string(),
                        HistoryAgent {
                            name: (*n).into(),
                            project_dir: (*d).into(),
                        },
                    )
                })
                .collect(),
            ..Default::default()
        }
    }
    fn point(at: i64, statuses: &[(&str, S)]) -> HistoryPoint {
        let count = |st: S| statuses.iter().filter(|(_, x)| *x == st).count() as u32;
        HistoryPoint {
            at,
            needs_you: count(S::NeedsYou),
            working: count(S::Working),
            idle: count(S::Idle),
            total: statuses.len() as u32,
            statuses: statuses
                .iter()
                .map(|(k, v)| ((*k).to_string(), *v))
                .collect(),
        }
    }

    const NOW: i64 = 1_767_268_800_000 + 600_000; // 12:10 UTC, Jan 1 2026

    #[test]
    fn counts_have_series_for_the_last_hour() {
        let mut s = state_with(vec![
            sess("a", S::Working, NOW - 60_000),
            sess("b", S::NeedsYou, NOW - 5000),
        ]);
        let points: Vec<HistoryPoint> = (0..120)
            .map(|i| {
                point(
                    NOW - 600_000 + i * 5000,
                    &[("a", S::Working), ("b", S::Finished)],
                )
            })
            .collect();
        s.history = Some(history_with(points, &[]));
        let clock = Clock {
            now: NOW,
            utc: true,
        };
        let cells = count_cells(&s, &clock);
        assert_eq!(cells.len(), 6);
        assert_eq!(cells[0].label, "Working");
        assert_eq!(cells[0].value, 1);
        assert_eq!(cells[0].series.len(), 12);
        assert_eq!(cells[0].series[11], Some(1.0));
        assert_eq!(cells[0].series[0], None, "not measured an hour ago");
        assert_eq!(cells[3].value, 1);
        assert_eq!(
            cells[4].series[11],
            Some(1.0),
            "finished is counted from the statuses"
        );
        assert!(cells[2].series.is_empty(), "no series for stale");
        assert!(cells[0].detail.starts_with("longest "));
    }

    #[test]
    fn the_timeline_lists_agents_with_something_to_show() {
        let mut s = state_with(vec![sess("a", S::Working, 1)]);
        let points: Vec<HistoryPoint> = (0..60)
            .map(|i| {
                point(
                    NOW - 300_000 + i * 5000,
                    &[("a", S::Working), ("gone", S::Idle), ("nr", S::NotRunning)],
                )
            })
            .collect();
        s.history = Some(history_with(
            points,
            &[("gone", "left", "/work/gone"), ("nr", "never", "/work/nr")],
        ));
        let rows = timeline(
            &s,
            &Clock {
                now: NOW,
                utc: true,
            },
        );
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            ["a", "left"],
            "current first, not-running only agents left out"
        );
        s.project = Some("/work/basic".into()); // no session in this project: filter falls back to all
        assert_eq!(
            timeline(
                &s,
                &Clock {
                    now: NOW,
                    utc: true
                }
            )
            .len(),
            2
        );
    }

    #[test]
    fn a_project_filter_narrows_every_card() {
        let mut a = sess("a", S::NeedsYou, 5);
        a.project_dir = "/work/a".into();
        let mut b = sess("b", S::NeedsYou, 5);
        b.project_dir = "/work/b".into();
        let mut s = state_with(vec![a, b]);
        s.project = Some("/work/a".into());
        assert_eq!(s.needs_you().len(), 1);
        assert_eq!(
            count_cells(
                &s,
                &Clock {
                    now: NOW,
                    utc: true
                }
            )[3]
            .value,
            1
        );
        assert_eq!(
            selectable_sessions(&s).len(),
            0,
            "needs-you sessions are in their own card, not the list"
        );
    }

    #[test]
    fn wait_totals_use_the_ledger_and_the_sessions_waiting_now() {
        let mut s = state_with(vec![sess("a", S::NeedsYou, NOW - 120_000)]);
        s.ledger = Some(as_core::overview::WaitLedgerData {
            transitions: Vec::new(),
            spans: vec![as_core::overview::MeasuredSpan {
                start: NOW - 3_600_000,
                end: NOW,
            }],
            starts: vec![NOW - 3_600_000],
        });
        let clock = Clock {
            now: NOW,
            utc: true,
        };
        let totals = wait_totals_for(&s, &clock).unwrap();
        assert_eq!(totals.today.ms, 120_000);
        assert_eq!(totals.today.open_ms, 120_000);
        assert_eq!(totals.longest.today[0].agent_id, "a");
        let hour = last_hour_waits_for(&s, &clock).unwrap();
        assert_eq!(hour.bars[0].agent_id, "a");
        assert!(hour.bars[0].open);
        assert!(wait_totals_for(&OverviewState::default(), &clock).is_none());
    }
}
