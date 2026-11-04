// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/core/sessions/diff.ts
//! Snapshot diff -> events, and the history point of a snapshot.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::types::{
    EventKind, HistoryPoint, OverviewEvent, OverviewSession, OverviewStatus, WaitingReason,
};

/// Longest time between two points that still counts as continuous watching.
pub const GAP_AFTER_MS: i64 = 15_000;

/// What changed between two snapshots, as events.
///
/// * An agent only in `next` has appeared; only in `prev`, it has ended (removed from the app).
/// * An agent in both with another status changed status. Leaving `needs-you` carries
///   `waited_ms`, measured from the previous `status_since`.
/// * Nothing else is an event: a new name, reason or branch is not reported.
///
/// `prev == None` is the first snapshot after the daemon started: agents that already exist did
/// not appear at that moment, so no event is emitted for them.
pub fn diff_snapshots(
    prev: Option<&[OverviewSession]>,
    next: &[OverviewSession],
    now: i64,
) -> Vec<OverviewEvent> {
    let Some(prev) = prev else {
        return Vec::new();
    };
    let before: HashMap<&str, &OverviewSession> =
        prev.iter().map(|s| (s.agent_id.as_str(), s)).collect();
    let after: HashSet<&str> = next.iter().map(|s| s.agent_id.as_str()).collect();
    let mut events = Vec::new();

    for s in next {
        let base = |kind: &str, k: EventKind| OverviewEvent {
            id: format!("{}@{now}:{kind}", s.agent_id),
            at: now,
            kind: k,
            agent_id: s.agent_id.clone(),
            name: s.name.clone(),
            project_dir: s.project_dir.clone(),
            from: None,
            to: None,
            waiting_reason: None,
            waited_ms: None,
            gap_ms: None,
        };
        match before.get(s.agent_id.as_str()) {
            None => events.push(OverviewEvent {
                to: Some(s.status),
                ..base("appeared", EventKind::Appeared)
            }),
            Some(old) if old.status != s.status => {
                let left_waiting = old.status == OverviewStatus::NeedsYou;
                events.push(OverviewEvent {
                    from: Some(old.status),
                    to: Some(s.status),
                    waiting_reason: if s.status == OverviewStatus::NeedsYou {
                        s.waiting_reason
                    } else {
                        None
                    },
                    waited_ms: left_waiting.then(|| (now - old.status_since).max(0)),
                    ..base("status", EventKind::Status)
                });
            }
            Some(_) => {}
        }
    }

    for old in prev {
        if after.contains(old.agent_id.as_str()) {
            continue;
        }
        events.push(OverviewEvent {
            id: format!("{}@{now}:ended", old.agent_id),
            at: now,
            kind: EventKind::Ended,
            agent_id: old.agent_id.clone(),
            name: old.name.clone(),
            project_dir: old.project_dir.clone(),
            from: Some(old.status),
            to: None,
            waiting_reason: None::<WaitingReason>,
            waited_ms: (old.status == OverviewStatus::NeedsYou)
                .then(|| (now - old.status_since).max(0)),
            gap_ms: None,
        });
    }
    events
}

/// The sample of one snapshot: counts per status and each agent's status.
pub fn point_from_sessions(sessions: &[OverviewSession], at: i64) -> HistoryPoint {
    let mut statuses = BTreeMap::new();
    let (mut needs_you, mut working, mut idle) = (0, 0, 0);
    for s in sessions {
        statuses.insert(s.agent_id.clone(), s.status);
        match s.status {
            OverviewStatus::NeedsYou => needs_you += 1,
            OverviewStatus::Working => working += 1,
            OverviewStatus::Idle => idle += 1,
            _ => {}
        }
    }
    HistoryPoint {
        at,
        needs_you,
        working,
        idle,
        total: u32::try_from(sessions.len()).unwrap_or(u32::MAX),
        statuses,
    }
}

/// The `resumed` event for watching that picked up `gap` after the last point, `None` when there
/// was no gap.
pub fn resumed_event(last_point_at: Option<i64>, now: i64) -> Option<OverviewEvent> {
    let gap_ms = now - last_point_at?;
    if gap_ms <= GAP_AFTER_MS {
        return None;
    }
    Some(OverviewEvent {
        id: format!("resumed@{now}"),
        at: now,
        kind: EventKind::Resumed,
        agent_id: String::new(),
        name: String::new(),
        project_dir: String::new(),
        from: None,
        to: None,
        waiting_reason: None,
        waited_ms: None,
        gap_ms: Some(gap_ms),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use OverviewStatus as S;

    fn s(id: &str, status: S) -> OverviewSession {
        OverviewSession::basic(id, status)
    }

    #[test]
    fn nothing_for_the_first_snapshot() {
        assert!(diff_snapshots(None, &[s("a", S::Working), s("b", S::NeedsYou)], 1000).is_empty());
    }

    #[test]
    fn appeared_agent() {
        let events = diff_snapshots(Some(&[]), &[s("a", S::Starting)], 1000);
        assert_eq!(
            events,
            vec![OverviewEvent {
                id: "a@1000:appeared".into(),
                at: 1000,
                kind: EventKind::Appeared,
                agent_id: "a".into(),
                name: "name-a".into(),
                project_dir: "/work/a".into(),
                from: None,
                to: Some(S::Starting),
                waiting_reason: None,
                waited_ms: None,
                gap_ms: None,
            }]
        );
    }

    #[test]
    fn status_change_carries_the_reason_when_entering_needs_you() {
        let mut next = s("a", S::NeedsYou);
        next.waiting_reason = Some(WaitingReason::Permission);
        let events = diff_snapshots(Some(&[s("a", S::Working)]), &[next], 1000);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Status);
        assert_eq!(events[0].from, Some(S::Working));
        assert_eq!(events[0].to, Some(S::NeedsYou));
        assert_eq!(events[0].waiting_reason, Some(WaitingReason::Permission));
        assert_eq!(events[0].waited_ms, None);
    }

    #[test]
    fn wait_is_measured_when_leaving_needs_you() {
        let mut prev = s("a", S::NeedsYou);
        prev.status_since = 4000;
        let mut next = s("a", S::Working);
        next.status_since = 10_000;
        let events = diff_snapshots(Some(&[prev]), &[next], 10_000);
        assert_eq!(events[0].waited_ms, Some(6000));
        assert_eq!(events[0].from, Some(S::NeedsYou));
    }

    #[test]
    fn removed_agents_end_with_their_wait() {
        let mut a = s("a", S::NeedsYou);
        a.status_since = 500;
        let events = diff_snapshots(Some(&[a, s("b", S::Idle)]), &[], 2000);
        assert_eq!(events[0].id, "a@2000:ended");
        assert_eq!(events[0].waited_ms, Some(1500));
        assert_eq!(events[1].id, "b@2000:ended");
        assert_eq!(events[1].from, Some(S::Idle));
        assert_eq!(events[1].waited_ms, None);
    }

    #[test]
    fn other_changes_are_not_events() {
        let mut prev = s("a", S::Idle);
        prev.status_since = 1;
        let mut next = prev.clone();
        next.name = "renamed".into();
        next.branch = Some("x".into());
        next.waiting_text = Some("y".into());
        assert!(diff_snapshots(Some(&[prev]), &[next], 5000).is_empty());
    }

    #[test]
    fn point_counts_the_three_live_statuses() {
        let p = point_from_sessions(
            &[
                s("a", S::NeedsYou),
                s("b", S::Working),
                s("c", S::Working),
                s("d", S::Idle),
                s("e", S::Finished),
            ],
            7,
        );
        assert_eq!(
            (p.at, p.needs_you, p.working, p.idle, p.total),
            (7, 1, 2, 1, 5)
        );
        assert_eq!(p.statuses["a"], S::NeedsYou);
        assert_eq!(p.statuses["e"], S::Finished);
    }

    #[test]
    fn resumed_event_after_a_gap() {
        assert!(resumed_event(None, 100_000).is_none());
        assert!(resumed_event(Some(100_000 - GAP_AFTER_MS), 100_000).is_none());
        let e = resumed_event(Some(1000), 61_000).unwrap();
        assert_eq!(e.id, "resumed@61000");
        assert_eq!(e.kind, EventKind::Resumed);
        assert_eq!(e.gap_ms, Some(60_000));
        assert_eq!(e.agent_id, "");
    }
}
