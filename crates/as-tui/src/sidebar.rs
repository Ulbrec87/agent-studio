//! Sidebar model: agents grouped by project, sorted, filtered. Pure functions, no rendering.

use std::collections::HashSet;
use std::path::Path;

use as_core::{AgentState, AgentStatus};

/// Key of the pinned group of manager agents.
pub const MANAGER_GROUP: &str = "\u{2605} manager";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visual {
    Running,
    Starting,
    Waiting,
    Idle,
    Errored,
}

impl Visual {
    pub fn glyph(self) -> &'static str {
        match self {
            Visual::Running => "\u{25CF}",  // ●
            Visual::Starting => "\u{25CC}", // ◌
            Visual::Waiting => "\u{25D0}",  // ◐
            Visual::Idle => "\u{25CB}",     // ○
            Visual::Errored => "\u{2716}",  // ✖
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Visual::Running => "running",
            Visual::Starting => "starting",
            Visual::Waiting => "waiting",
            Visual::Idle => "idle",
            Visual::Errored => "error",
        }
    }
}

/// Status glyph of an agent. `waiting` (silent for a while) only applies to a running agent.
pub fn visual(status: AgentStatus, waiting: bool) -> Visual {
    match status {
        AgentStatus::Running if waiting => Visual::Waiting,
        AgentStatus::Running => Visual::Running,
        AgentStatus::Starting => Visual::Starting,
        AgentStatus::Idle => Visual::Idle,
        AgentStatus::Errored => Visual::Errored,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Header {
        /// Project dir, or [`MANAGER_GROUP`].
        key: String,
        count: usize,
        collapsed: bool,
    },
    /// Index into the agent list.
    Agent(usize),
}

/// Identity of a row that survives list changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKey {
    Header(String),
    Agent(String),
}

fn timestamp_ms(text: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

fn group_key(agent: &AgentState) -> String {
    if agent.is_manager {
        MANAGER_GROUP.to_string()
    } else {
        agent.project_dir.clone()
    }
}

/// Case-insensitive match over the fields a user would search by.
pub fn matches_filter(agent: &AgentState, filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    if filter.is_empty() {
        return true;
    }
    let hay = format!(
        "{} {} {} {} {:?}",
        agent.name, agent.project_dir, agent.provider, agent.model, agent.status
    )
    .to_lowercase();
    filter.split_whitespace().all(|word| hay.contains(word))
}

/// Groups in display order: manager first, then projects by most recent activity (newest first),
/// ties by path. Each group lists agent indices by creation time.
pub fn groups(agents: &[AgentState], filter: &str) -> Vec<(String, Vec<usize>)> {
    let mut map: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, agent) in agents.iter().enumerate() {
        if !matches_filter(agent, filter) {
            continue;
        }
        let key = group_key(agent);
        match map.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(i),
            None => map.push((key, vec![i])),
        }
    }
    for (_, members) in &mut map {
        members.sort_by(|a, b| {
            let (a, b) = (&agents[*a], &agents[*b]);
            timestamp_ms(&a.created_at)
                .cmp(&timestamp_ms(&b.created_at))
                .then_with(|| a.name.cmp(&b.name))
        });
    }
    let activity = |members: &Vec<usize>| {
        members
            .iter()
            .map(|i| {
                let a = &agents[*i];
                timestamp_ms(&a.last_activity_at).max(timestamp_ms(&a.created_at))
            })
            .max()
            .unwrap_or(0)
    };
    map.sort_by(|(ka, a), (kb, b)| {
        (kb == MANAGER_GROUP)
            .cmp(&(ka == MANAGER_GROUP))
            .then_with(|| activity(b).cmp(&activity(a)))
            .then_with(|| ka.cmp(kb))
    });
    map
}

/// Visible rows (collapsed groups show only their header).
pub fn rows(agents: &[AgentState], collapsed: &HashSet<String>, filter: &str) -> Vec<Row> {
    let mut out = Vec::new();
    for (key, members) in groups(agents, filter) {
        let is_collapsed = collapsed.contains(&key);
        out.push(Row::Header {
            key,
            count: members.len(),
            collapsed: is_collapsed,
        });
        if !is_collapsed {
            out.extend(members.into_iter().map(Row::Agent));
        }
    }
    out
}

/// Agent indices in sidebar order, ignoring collapsing (what `prefix 1-9` and `prefix j/k` walk).
pub fn agent_order(agents: &[AgentState], filter: &str) -> Vec<usize> {
    groups(agents, filter)
        .into_iter()
        .flat_map(|(_, members)| members)
        .collect()
}

pub fn row_key(agents: &[AgentState], row: &Row) -> RowKey {
    match row {
        Row::Header { key, .. } => RowKey::Header(key.clone()),
        Row::Agent(i) => RowKey::Agent(agents[*i].id.clone()),
    }
}

/// Label of a group header: tildified path, or the manager name.
pub fn group_label(key: &str) -> String {
    if key == MANAGER_GROUP {
        key.to_string()
    } else {
        as_core::paths::tildify(Path::new(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, dir: &str, created: &str, activity: &str) -> AgentState {
        let mut a = AgentState::default();
        a.config.id = id.into();
        a.config.name = id.into();
        a.config.project_dir = dir.into();
        a.config.created_at = created.into();
        a.last_activity_at = activity.into();
        a
    }

    #[test]
    fn status_glyph_mapping() {
        assert_eq!(visual(AgentStatus::Running, false).glyph(), "●");
        assert_eq!(visual(AgentStatus::Starting, false).glyph(), "◌");
        assert_eq!(visual(AgentStatus::Running, true).glyph(), "◐");
        assert_eq!(
            visual(AgentStatus::Idle, true).glyph(),
            "○",
            "waiting only for running"
        );
        assert_eq!(visual(AgentStatus::Errored, false).glyph(), "✖");
        assert_eq!(visual(AgentStatus::Running, true), Visual::Waiting);
    }

    #[test]
    fn groups_sort_by_recent_activity_and_agents_by_creation() {
        let agents = vec![
            agent("a1", "/old", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
            agent("b2", "/new", "2026-01-02T00:00:00Z", "2026-03-01T00:00:00Z"),
            agent("b1", "/new", "2026-01-01T12:00:00Z", "2026-01-01T12:00:00Z"),
            agent("c1", "/mid", "2026-01-03T00:00:00Z", "2026-02-01T00:00:00Z"),
        ];
        let g = groups(&agents, "");
        let order: Vec<_> = g.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(order, ["/new", "/mid", "/old"]);
        let ids: Vec<_> = g[0].1.iter().map(|i| agents[*i].id.as_str()).collect();
        assert_eq!(ids, ["b1", "b2"]);
    }

    #[test]
    fn managers_are_pinned_on_top() {
        let mut m = agent("m", "/x", "2026-01-01T00:00:00Z", "2020-01-01T00:00:00Z");
        m.config.is_manager = true;
        let agents = vec![
            agent("a", "/x", "2026-01-01T00:00:00Z", "2026-05-01T00:00:00Z"),
            m,
        ];
        let g = groups(&agents, "");
        assert_eq!(g[0].0, MANAGER_GROUP);
        assert_eq!(g[1].0, "/x");
        assert_eq!(g[1].1.len(), 1);
    }

    #[test]
    fn rows_respect_collapse_and_filter() {
        let agents = vec![
            agent("api", "/p1", "2026-01-01T00:00:00Z", ""),
            agent("web", "/p2", "2026-01-02T00:00:00Z", ""),
            agent("api2", "/p1", "2026-01-03T00:00:00Z", ""),
        ];
        let collapsed: HashSet<String> = ["/p1".to_string()].into();
        let r = rows(&agents, &collapsed, "");
        assert_eq!(r.len(), 3, "p1 header only + p2 header + web");
        assert!(matches!(&r[0], Row::Header { key, collapsed: true, count: 2 } if key == "/p1"));
        let r = rows(&agents, &HashSet::new(), "api");
        assert_eq!(r.len(), 3);
        assert_eq!(agent_order(&agents, "api"), vec![0, 2]);
        assert_eq!(agent_order(&agents, "zzz"), Vec::<usize>::new());
        assert_eq!(agent_order(&agents, "p2 web"), vec![1]);
    }

    #[test]
    fn filter_matches_status_and_provider() {
        let mut a = agent("x", "/p", "", "");
        a.status = AgentStatus::Errored;
        assert!(matches_filter(&a, "errored"));
        assert!(matches_filter(&a, "claude"));
        assert!(!matches_filter(&a, "codex"));
    }
}
