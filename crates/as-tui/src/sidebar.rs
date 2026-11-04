//! Sidebar model: agents grouped by project, sorted, filtered. Pure functions, no rendering.

use std::collections::HashSet;
use std::path::Path;

use as_core::{AgentState, AgentStatus};
use serde::{Deserialize, Serialize};

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

pub(crate) fn timestamp_ms(text: &str) -> i64 {
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

/// How the agents inside a group (and the groups) are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    /// Creation order inside a project; projects by recent activity (the stable default).
    #[default]
    Created,
    /// Last activity first, projects by recent activity.
    Recent,
    Name,
    Status,
    Provider,
}

impl SortKey {
    pub const ALL: [SortKey; 5] = [
        SortKey::Created,
        SortKey::Recent,
        SortKey::Name,
        SortKey::Status,
        SortKey::Provider,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Created => "created",
            SortKey::Recent => "recent",
            SortKey::Name => "name",
            SortKey::Status => "status",
            SortKey::Provider => "provider",
        }
    }

    pub fn next(self) -> SortKey {
        let i = Self::ALL.iter().position(|k| *k == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// The status chips of the sidebar (a single selection, like a tab strip).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusChip {
    #[default]
    All,
    /// Running or starting (includes the waiting ones).
    Running,
    /// Running but quiet: probably waiting for you.
    Waiting,
    Idle,
    Errored,
}

impl StatusChip {
    pub const ALL: [StatusChip; 5] = [
        StatusChip::All,
        StatusChip::Running,
        StatusChip::Waiting,
        StatusChip::Idle,
        StatusChip::Errored,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StatusChip::All => "All",
            StatusChip::Running => "Running",
            StatusChip::Waiting => "Waiting",
            StatusChip::Idle => "Idle",
            StatusChip::Errored => "Errored",
        }
    }

    /// Shortest label that stays readable in a narrow sidebar.
    pub fn short(self) -> &'static str {
        match self {
            StatusChip::All => "All",
            StatusChip::Running => "Run",
            StatusChip::Waiting => "Wait",
            StatusChip::Idle => "Idle",
            StatusChip::Errored => "Err",
        }
    }

    pub fn next(self) -> StatusChip {
        let i = Self::ALL.iter().position(|k| *k == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    fn accepts(self, agent: &AgentState, waiting: bool) -> bool {
        match self {
            StatusChip::All => true,
            StatusChip::Running => {
                matches!(agent.status, AgentStatus::Running | AgentStatus::Starting)
            }
            StatusChip::Waiting => agent.status == AgentStatus::Running && waiting,
            StatusChip::Idle => agent.status == AgentStatus::Idle,
            StatusChip::Errored => agent.status == AgentStatus::Errored,
        }
    }
}

/// Everything that decides which agents the sidebar shows and in which order.
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewOpts<'a> {
    pub search: &'a str,
    pub status: StatusChip,
    pub sort: SortKey,
    /// Ids of running agents that went quiet (for the `Waiting` chip).
    pub waiting: Option<&'a HashSet<String>>,
}

impl<'a> ViewOpts<'a> {
    pub fn search(search: &'a str) -> Self {
        Self {
            search,
            ..Self::default()
        }
    }

    fn is_waiting(&self, agent: &AgentState) -> bool {
        self.waiting.is_some_and(|w| w.contains(&agent.id))
    }

    pub fn accepts(&self, agent: &AgentState) -> bool {
        self.status.accepts(agent, self.is_waiting(agent)) && matches_filter(agent, self.search)
    }
}

/// Case-insensitive match over the fields a user would search by (name, project, provider, model,
/// status); every word must match.
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

fn status_rank(status: AgentStatus) -> u8 {
    match status {
        AgentStatus::Running => 0,
        AgentStatus::Starting => 1,
        AgentStatus::Idle => 2,
        AgentStatus::Errored => 3,
    }
}

fn activity_ms(agent: &AgentState) -> i64 {
    timestamp_ms(&agent.last_activity_at).max(timestamp_ms(&agent.created_at))
}

/// Groups in display order: manager first (always pinned), then projects, each with its agents
/// ordered by `opts.sort`.
pub fn groups_with(agents: &[AgentState], opts: &ViewOpts) -> Vec<(String, Vec<usize>)> {
    let mut map: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, agent) in agents.iter().enumerate() {
        if !opts.accepts(agent) {
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
            let created = timestamp_ms(&a.created_at).cmp(&timestamp_ms(&b.created_at));
            let by_name = a.name.to_lowercase().cmp(&b.name.to_lowercase());
            match opts.sort {
                SortKey::Created => created.then_with(|| a.name.cmp(&b.name)),
                SortKey::Recent => activity_ms(b).cmp(&activity_ms(a)).then(by_name),
                SortKey::Name => by_name.then(created),
                SortKey::Status => status_rank(a.status)
                    .cmp(&status_rank(b.status))
                    .then(by_name),
                SortKey::Provider => a.provider.as_str().cmp(b.provider.as_str()).then(by_name),
            }
        });
    }
    let activity = |members: &Vec<usize>| {
        members
            .iter()
            .map(|i| activity_ms(&agents[*i]))
            .max()
            .unwrap_or(0)
    };
    let by_project_name = matches!(opts.sort, SortKey::Name | SortKey::Provider);
    map.sort_by(|(ka, a), (kb, b)| {
        let pinned = (kb == MANAGER_GROUP).cmp(&(ka == MANAGER_GROUP));
        if by_project_name {
            pinned.then_with(|| group_label(ka).cmp(&group_label(kb)))
        } else {
            pinned
                .then_with(|| activity(b).cmp(&activity(a)))
                .then_with(|| ka.cmp(kb))
        }
    });
    map
}

/// [`groups_with`] with only a search text.
pub fn groups(agents: &[AgentState], filter: &str) -> Vec<(String, Vec<usize>)> {
    groups_with(agents, &ViewOpts::search(filter))
}

/// Visible rows (collapsed groups show only their header).
pub fn rows_with(agents: &[AgentState], collapsed: &HashSet<String>, opts: &ViewOpts) -> Vec<Row> {
    let mut out = Vec::new();
    for (key, members) in groups_with(agents, opts) {
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

pub fn rows(agents: &[AgentState], collapsed: &HashSet<String>, filter: &str) -> Vec<Row> {
    rows_with(agents, collapsed, &ViewOpts::search(filter))
}

/// Agent indices in sidebar order, ignoring collapsing (what `prefix 1-9` and `prefix j/k` walk).
pub fn agent_order_with(agents: &[AgentState], opts: &ViewOpts) -> Vec<usize> {
    groups_with(agents, opts)
        .into_iter()
        .flat_map(|(_, members)| members)
        .collect()
}

pub fn agent_order(agents: &[AgentState], filter: &str) -> Vec<usize> {
    agent_order_with(agents, &ViewOpts::search(filter))
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

    fn with_status(id: &str, dir: &str, status: AgentStatus, activity: &str) -> AgentState {
        let mut a = agent(id, dir, "2026-01-01T00:00:00Z", activity);
        a.status = status;
        a
    }

    #[test]
    fn status_chips_filter() {
        let agents = vec![
            with_status("r", "/p", AgentStatus::Running, ""),
            with_status("w", "/p", AgentStatus::Running, ""),
            with_status("s", "/p", AgentStatus::Starting, ""),
            with_status("i", "/p", AgentStatus::Idle, ""),
            with_status("e", "/p", AgentStatus::Errored, ""),
        ];
        let waiting: HashSet<String> = ["w".to_string()].into();
        let ids = |chip| {
            let opts = ViewOpts {
                status: chip,
                waiting: Some(&waiting),
                ..ViewOpts::default()
            };
            agent_order_with(&agents, &opts)
                .into_iter()
                .map(|i| agents[i].id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(StatusChip::All).len(), 5);
        assert_eq!(ids(StatusChip::Running), ["r", "s", "w"]);
        assert_eq!(ids(StatusChip::Waiting), ["w"]);
        assert_eq!(ids(StatusChip::Idle), ["i"]);
        assert_eq!(ids(StatusChip::Errored), ["e"]);
        assert_eq!(StatusChip::Errored.next(), StatusChip::All);
    }

    #[test]
    fn sort_keys_order_the_agents_of_a_group() {
        let mut a = agent("a1", "/p", "2026-01-03T00:00:00Z", "2026-01-03T00:00:00Z");
        a.config.name = "zeta".into();
        a.status = AgentStatus::Idle;
        let mut b = agent("b1", "/p", "2026-01-01T00:00:00Z", "2026-01-09T00:00:00Z");
        b.config.name = "Alpha".into();
        b.status = AgentStatus::Running;
        b.config.provider = as_core::ProviderId::Codex;
        let mut c = agent("c1", "/p", "2026-01-02T00:00:00Z", "2026-01-05T00:00:00Z");
        c.config.name = "mid".into();
        c.status = AgentStatus::Errored;
        let agents = vec![a, b, c];
        let order = |sort| {
            let opts = ViewOpts {
                sort,
                ..ViewOpts::default()
            };
            agent_order_with(&agents, &opts)
                .into_iter()
                .map(|i| agents[i].id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(order(SortKey::Created), ["b1", "c1", "a1"]);
        assert_eq!(order(SortKey::Recent), ["b1", "c1", "a1"]);
        assert_eq!(order(SortKey::Name), ["b1", "c1", "a1"]);
        assert_eq!(order(SortKey::Status), ["b1", "a1", "c1"]);
        assert_eq!(order(SortKey::Provider), ["c1", "a1", "b1"]);
        assert_eq!(SortKey::Provider.next(), SortKey::Created);
    }

    #[test]
    fn managers_stay_pinned_for_every_sort_and_filter() {
        let mut m = agent("m", "/x", "2026-01-01T00:00:00Z", "2020-01-01T00:00:00Z");
        m.config.is_manager = true;
        let agents = vec![
            agent("a", "/a", "2026-01-01T00:00:00Z", "2026-05-01T00:00:00Z"),
            m,
        ];
        for sort in SortKey::ALL {
            let opts = ViewOpts {
                sort,
                ..ViewOpts::default()
            };
            assert_eq!(groups_with(&agents, &opts)[0].0, MANAGER_GROUP, "{sort:?}");
        }
    }

    #[test]
    fn search_matches_name_project_and_model() {
        let mut a = agent("x", "/work/billing", "", "");
        a.config.name = "refactor".into();
        a.config.model = "opus".into();
        assert!(matches_filter(&a, "refac"));
        assert!(matches_filter(&a, "billing"));
        assert!(matches_filter(&a, "opus refactor"));
        assert!(!matches_filter(&a, "sonnet"));
    }
}
