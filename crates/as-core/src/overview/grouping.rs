//! Grouping of sessions for the List, Repos and Board layouts (Hydra `shared/overview/grouping.ts`).

use std::collections::BTreeMap;

use super::status::is_ended;
use super::types::{OverviewSession, OverviewStatus};

pub const NO_REPOSITORY: &str = "No repository";
/// Rows shown in the Finished / Failed and Not running groups before "Show more".
pub const ENDED_GROUP_LIMIT: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionGroup {
    pub key: String,
    pub label: String,
    pub sessions: Vec<OverviewSession>,
    /// Rows shown before "Show more"; no limit when `None`.
    pub limit: Option<usize>,
}

/// Order within the list: needs-you, working, idle, stale idle, failed, finished, not running,
/// starting.
fn rank(s: &OverviewSession) -> u8 {
    match s.status {
        OverviewStatus::NeedsYou => 0,
        OverviewStatus::Working => 1,
        OverviewStatus::Idle if s.stale => 3,
        OverviewStatus::Idle => 2,
        OverviewStatus::Failed => 4,
        OverviewStatus::Finished => 5,
        OverviewStatus::NotRunning => 6,
        OverviewStatus::Starting => 7,
    }
}

/// Status group, then longest in that status first (ended sessions: most recently ended first),
/// then name.
fn sort_by_status(sessions: &[OverviewSession]) -> Vec<OverviewSession> {
    let since = |s: &OverviewSession| {
        if is_ended(s.status) {
            -s.status_since
        } else {
            s.status_since
        }
    };
    let mut sorted = sessions.to_vec();
    sorted.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then(since(a).cmp(&since(b)))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    sorted
}

/// Groups for the List layout: Working, Idle (non-stale then stale), Finished / Failed (failed
/// first), Not running, Starting. Sessions that need the user are left out (they are in the
/// Needs you card). Empty groups are dropped.
pub fn group_by_status(sessions: &[OverviewSession]) -> Vec<SessionGroup> {
    let kept: Vec<OverviewSession> = sessions
        .iter()
        .filter(|s| s.status != OverviewStatus::NeedsYou)
        .cloned()
        .collect();
    let sorted = sort_by_status(&kept);
    let defs: [(&str, &str, &[u8], Option<usize>); 5] = [
        ("working", "Working", &[1], None),
        ("idle", "Idle", &[2, 3], None),
        (
            "ended",
            "Finished / Failed",
            &[4, 5],
            Some(ENDED_GROUP_LIMIT),
        ),
        ("not-running", "Not running", &[6], Some(ENDED_GROUP_LIMIT)),
        ("starting", "Starting", &[7], None),
    ];
    defs.iter()
        .map(|(key, label, ranks, limit)| SessionGroup {
            key: (*key).to_string(),
            label: (*label).to_string(),
            sessions: sorted
                .iter()
                .filter(|s| ranks.contains(&rank(s)))
                .cloned()
                .collect(),
            limit: *limit,
        })
        .filter(|g| !g.sessions.is_empty())
        .collect()
}

/// Groups for the Repos layout: by repository name (sorted), "No repository" last. Rows keep the
/// status order; needs-you sessions stay.
pub fn group_by_repo<F>(sessions: &[OverviewSession], repo_of: F) -> Vec<SessionGroup>
where
    F: Fn(&OverviewSession) -> Option<String>,
{
    let mut by_repo: BTreeMap<String, Vec<OverviewSession>> = BTreeMap::new();
    for s in sort_by_status(sessions) {
        let repo = repo_of(&s)
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| NO_REPOSITORY.to_string());
        by_repo.entry(repo).or_default().push(s);
    }
    let mut groups: Vec<(String, Vec<OverviewSession>)> = by_repo.into_iter().collect();
    groups.sort_by(
        |(a, _), (b, _)| match (a == NO_REPOSITORY, b == NO_REPOSITORY) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) => a.to_lowercase().cmp(&b.to_lowercase()),
        },
    );
    groups
        .into_iter()
        .map(|(label, sessions)| SessionGroup {
            key: label.clone(),
            label,
            sessions,
            limit: None,
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardKey {
    NeedsYou,
    Working,
    Idle,
    Ended,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoardColumn {
    pub key: BoardKey,
    pub label: &'static str,
    pub items: Vec<OverviewSession>,
    /// Sessions in the column that are not shown.
    pub hidden: usize,
    pub total: usize,
    /// Idle column only: how many of `total` are stale.
    pub stale: Option<usize>,
    /// Ended column only: not-running sessions, which are left out and only in the list.
    pub not_running: Option<usize>,
}

/// The four Board columns, each capped at `max` cards.
pub fn board_columns(sessions: &[OverviewSession], max: usize) -> Vec<BoardColumn> {
    let sorted = sort_by_status(sessions);
    let column = |key: BoardKey, label: &'static str, pick: &dyn Fn(&OverviewSession) -> bool| {
        let all: Vec<&OverviewSession> = sorted.iter().filter(|s| pick(s)).collect();
        let items: Vec<OverviewSession> = all.iter().take(max).map(|s| (*s).clone()).collect();
        BoardColumn {
            key,
            label,
            hidden: all.len() - items.len(),
            total: all.len(),
            items,
            stale: None,
            not_running: None,
        }
    };
    let mut idle = column(BoardKey::Idle, "Idle", &|s| {
        s.status == OverviewStatus::Idle
    });
    idle.stale = Some(
        sorted
            .iter()
            .filter(|s| s.status == OverviewStatus::Idle && s.stale)
            .count(),
    );
    let mut ended = column(BoardKey::Ended, "Finished / Failed", &|s| {
        matches!(s.status, OverviewStatus::Finished | OverviewStatus::Failed)
    });
    ended.not_running = Some(
        sorted
            .iter()
            .filter(|s| s.status == OverviewStatus::NotRunning)
            .count(),
    );
    vec![
        column(BoardKey::NeedsYou, "Needs you", &|s| {
            s.status == OverviewStatus::NeedsYou
        }),
        column(BoardKey::Working, "Working", &|s| {
            matches!(s.status, OverviewStatus::Working | OverviewStatus::Starting)
        }),
        idle,
        ended,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use OverviewStatus as S;

    fn s(id: &str, status: S) -> OverviewSession {
        let mut x = OverviewSession::basic(id, status);
        x.name = id.to_string();
        x
    }
    fn since(id: &str, status: S, since: i64) -> OverviewSession {
        let mut x = s(id, status);
        x.status_since = since;
        x
    }
    fn ids(list: &[OverviewSession]) -> Vec<&str> {
        list.iter().map(|x| x.agent_id.as_str()).collect()
    }

    #[test]
    fn orders_groups_and_leaves_needs_you_out() {
        let mut stale = s("stale", S::Idle);
        stale.stale = true;
        let groups = group_by_status(&[
            s("fin", S::Finished),
            s("start", S::Starting),
            s("need", S::NeedsYou),
            stale,
            s("idle", S::Idle),
            s("fail", S::Failed),
            s("work", S::Working),
            s("nr", S::NotRunning),
        ]);
        let labels: Vec<&str> = groups.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Working",
                "Idle",
                "Finished / Failed",
                "Not running",
                "Starting"
            ]
        );
        assert_eq!(ids(&groups[1].sessions), ["idle", "stale"]);
        assert_eq!(ids(&groups[2].sessions), ["fail", "fin"]);
        assert!(
            groups
                .iter()
                .flat_map(|g| &g.sessions)
                .all(|x| x.agent_id != "need")
        );
    }

    #[test]
    fn drops_empty_groups_and_puts_the_longest_first() {
        let groups = group_by_status(&[since("b", S::Working, 20), since("a", S::Working, 10)]);
        assert_eq!(groups.len(), 1);
        assert_eq!(ids(&groups[0].sessions), ["a", "b"]);
    }

    #[test]
    fn ended_groups_are_newest_first() {
        let groups = group_by_status(&[
            since("f-old", S::Finished, 10),
            since("f-new", S::Finished, 30),
            since("x-fail", S::Failed, 5),
            since("nr-old", S::NotRunning, 1),
            since("nr-new", S::NotRunning, 2),
            since("w-new", S::Working, 20),
            since("w-old", S::Working, 10),
            since("i-new", S::Idle, 20),
            since("i-old", S::Idle, 10),
        ]);
        let get =
            |key: &str| ids(&groups.iter().find(|g| g.key == key).unwrap().sessions).join(",");
        assert_eq!(get("ended"), "x-fail,f-new,f-old");
        assert_eq!(get("not-running"), "nr-new,nr-old");
        assert_eq!(get("working"), "w-old,w-new");
        assert_eq!(get("idle"), "i-old,i-new");
    }

    #[test]
    fn limit_is_set_on_ended_and_not_running_only() {
        let groups = group_by_status(&[
            s("w", S::Working),
            s("i", S::Idle),
            s("f", S::Finished),
            s("nr", S::NotRunning),
            s("st", S::Starting),
        ]);
        let limits: Vec<(&str, Option<usize>)> =
            groups.iter().map(|g| (g.key.as_str(), g.limit)).collect();
        assert_eq!(
            limits,
            [
                ("working", None),
                ("idle", None),
                ("ended", Some(10)),
                ("not-running", Some(10)),
                ("starting", None)
            ]
        );
        assert_eq!(ENDED_GROUP_LIMIT, 10);
        assert_eq!(
            group_by_repo(&[s("f", S::Finished)], |_| Some("r".into()))[0].limit,
            None
        );
    }

    #[test]
    fn repos_sorted_with_no_repository_last() {
        let repo = |x: &OverviewSession| match x.agent_id.as_str() {
            "a" => Some("zeta".to_string()),
            "b" | "c" => Some("alpha".to_string()),
            _ => None,
        };
        let groups = group_by_repo(
            &[
                s("a", S::Working),
                s("c", S::Idle),
                s("b", S::Working),
                s("d", S::Finished),
            ],
            repo,
        );
        let labels: Vec<&str> = groups.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(labels, ["alpha", "zeta", NO_REPOSITORY]);
        assert_eq!(ids(&groups[0].sessions), ["b", "c"]);
    }

    #[test]
    fn board_caps_columns_and_separates_stale() {
        let mut sessions: Vec<OverviewSession> =
            (0..7).map(|i| s(&format!("w{i}"), S::Working)).collect();
        let mut stale = s("i1", S::Idle);
        stale.stale = true;
        sessions.extend([
            stale,
            s("i2", S::Idle),
            s("n", S::NeedsYou),
            s("f", S::Failed),
            s("x", S::Finished),
            s("nr", S::NotRunning),
        ]);
        let cols = board_columns(&sessions, 5);
        let by = |k: BoardKey| cols.iter().find(|c| c.key == k).unwrap();
        assert_eq!(by(BoardKey::Working).items.len(), 5);
        assert_eq!(by(BoardKey::Working).hidden, 2);
        assert_eq!(ids(&by(BoardKey::Idle).items), ["i2", "i1"]);
        assert_eq!(by(BoardKey::Idle).stale, Some(1));
        assert_eq!(by(BoardKey::NeedsYou).total, 1);
        assert_eq!(ids(&by(BoardKey::Ended).items), ["f", "x"]);
        assert!(
            cols.iter()
                .flat_map(|c| &c.items)
                .all(|x| x.agent_id != "nr")
        );
        assert_eq!(by(BoardKey::Ended).not_running, Some(1));
        assert_eq!(by(BoardKey::Working).not_running, None);
    }

    #[test]
    fn board_lists_the_newest_ended_first() {
        let cols = board_columns(&[since("o", S::Finished, 1), since("n", S::Finished, 9)], 5);
        assert_eq!(ids(&cols[3].items), ["n", "o"]);
    }
}
