//! The Overview tracker (Hydra `electron/overview/OverviewTracker.ts`): polls the agent manager
//! and Claude Code's session registry and builds the [`OverviewSnapshot`]. A poll reports whether
//! the snapshot changed in a way worth pushing (never just because time passed) and the new
//! events, which it also hands to the history store together with a sample every 5 s.
//!
//! Synchronous and clock-injected: the daemon runs [`OverviewTracker::refresh`] from a blocking
//! task every 2 s (and shortly after an agent status event); tests drive it with a fake clock.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::DateTime;
use parking_lot::Mutex;

use super::claude_registry::{
    ClaudeRegistryReader, MatchableAgent, RegistryEntry, match_registry_entries,
};
use super::diff::{diff_snapshots, point_from_sessions, resumed_event};
use super::git_info::GitInfoCache;
use super::history_store::HistoryStore;
use super::process_tree::{ProcessTable, ProcessTableReader, descendant_pids};
use super::status::{compute_counts, derive_status, is_ended, is_stale, quiet_for_ms, to_ms};
use super::transcript_tail::{
    AgentDetailsReader, DetailsLocator, WaitingTextAgent, WaitingTextReader,
};
use super::types::{
    AgentDetails, HistoryAgent, HistoryPoint, HistoryResponse, OverviewEvent, OverviewSession,
    OverviewSnapshot, OverviewStatus,
};
use crate::agents::{AgentActivity, AgentManager};
use crate::types::{AgentState, ProviderId};

/// The slice of the agent manager the tracker needs.
pub trait OverviewAgentSource: Send {
    fn list(&self) -> Vec<AgentState>;
    fn activity(&self, agent_id: &str) -> Option<AgentActivity>;
}

impl OverviewAgentSource for AgentManager {
    fn list(&self) -> Vec<AgentState> {
        AgentManager::list(self)
    }
    fn activity(&self, agent_id: &str) -> Option<AgentActivity> {
        self.get_activity(agent_id)
    }
}

/// Where events and samples go; the tracker only feeds it.
pub trait HistorySink: Send {
    fn add_events(&self, events: &[OverviewEvent]);
    fn add_point(&self, point: HistoryPoint, agents: &BTreeMap<String, HistoryAgent>);
    fn last_point_at(&self) -> Option<i64>;
}

impl HistorySink for Arc<Mutex<HistoryStore>> {
    fn add_events(&self, events: &[OverviewEvent]) {
        self.lock().add_events(events);
    }
    fn add_point(&self, point: HistoryPoint, agents: &BTreeMap<String, HistoryAgent>) {
        self.lock().add_point(point, Some(agents));
    }
    fn last_point_at(&self) -> Option<i64> {
        self.lock().last_point_at()
    }
}

/// Reads the live Claude registry entries of the given config dirs.
pub type RegistryFn = Box<dyn FnMut(&[PathBuf]) -> Vec<RegistryEntry> + Send>;

pub struct TrackerDeps {
    pub agents: Box<dyn OverviewAgentSource>,
    pub registry: RegistryFn,
    /// The process table at `now` (throttled by the provider).
    pub process_table: Box<dyn FnMut(i64) -> ProcessTable + Send>,
    pub config_dirs: Box<dyn Fn() -> Vec<PathBuf> + Send>,
    pub waiting_text: Option<WaitingTextReader>,
    pub git: Option<GitInfoCache>,
    pub details: Option<AgentDetailsReader>,
    pub history: Option<Box<dyn HistorySink>>,
    pub now: Box<dyn Fn() -> i64 + Send>,
    /// Time between history points. Default 5 s.
    pub sample_interval_ms: i64,
}

impl TrackerDeps {
    /// The production wiring: the real registry, process table, transcripts and git files.
    pub fn production(agents: Box<dyn OverviewAgentSource>, history: Box<dyn HistorySink>) -> Self {
        let mut registry = ClaudeRegistryReader::default();
        let mut processes = ProcessTableReader::default();
        Self {
            agents,
            registry: Box::new(move |dirs| registry.read(dirs)),
            process_table: Box::new(move |now| processes.read(now).clone()),
            config_dirs: Box::new(|| {
                crate::sessions::profiles::discover_config_dirs(
                    &crate::paths::home_dir(),
                    std::env::var("CLAUDE_CONFIG_DIR").ok().as_deref(),
                )
            }),
            waiting_text: Some(WaitingTextReader::default()),
            git: Some(GitInfoCache::default()),
            details: Some(AgentDetailsReader::default()),
            history: Some(history),
            now: Box::new(super::history_store::wall_clock_ms),
            sample_interval_ms: super::history_store::SAMPLE_INTERVAL_MS,
        }
    }
}

/// What one poll found.
#[derive(Debug, Clone, PartialEq)]
pub struct PollResult {
    pub snapshot: OverviewSnapshot,
    /// The snapshot differs from the previous one in a way worth pushing.
    pub changed: bool,
    /// New events of this poll (including a `resumed` gap event), oldest first.
    pub events: Vec<OverviewEvent>,
}

struct Tracked {
    status: OverviewStatus,
    since: i64,
}

pub struct OverviewTracker {
    deps: TrackerDeps,
    snapshot: Option<OverviewSnapshot>,
    last_key: Option<String>,
    /// Claude agents matched to a registry entry in the last poll.
    matched: HashSet<String>,
    tracked: HashMap<String, Tracked>,
    locators: HashMap<String, DetailsLocator>,
    last_sample_at: Option<i64>,
}

/// Fields whose change is worth a push. Timers and durations are ticked by the client.
fn session_key(s: &OverviewSession) -> String {
    format!(
        "{:?}",
        (
            (&s.agent_id, &s.name, s.provider, &s.model, &s.project_dir),
            (&s.worktree_branch, &s.repo_name, &s.branch, &s.commit),
            (s.status, s.status_since, s.ended_at, s.exit_code),
            (
                s.waiting_reason,
                &s.waiting_text,
                s.stale,
                s.quiet_for_ms.is_some()
            ),
            (s.pid, s.started_at),
        )
    )
}

fn parse_ms(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|d| d.timestamp_millis())
}

impl OverviewTracker {
    pub fn new(deps: TrackerDeps) -> Self {
        Self {
            deps,
            snapshot: None,
            last_key: None,
            matched: HashSet::new(),
            tracked: HashMap::new(),
            locators: HashMap::new(),
            last_sample_at: None,
        }
    }

    /// The latest snapshot without polling, `None` before the first poll.
    pub fn peek_snapshot(&self) -> Option<&OverviewSnapshot> {
        self.snapshot.as_ref()
    }

    /// The latest snapshot; polls once first when there is none yet.
    pub fn get_snapshot(&mut self) -> OverviewSnapshot {
        if self.snapshot.is_none() {
            self.refresh();
        }
        self.snapshot.clone().unwrap_or_default()
    }

    /// Whether the agent was matched to a Claude registry entry in the last poll.
    pub fn has_registry_match(&self, agent_id: &str) -> bool {
        self.matched.contains(agent_id)
    }

    /// Details for the drawer, read from the transcript on demand. `None` when the agent is
    /// unknown (it may have disappeared since the last poll).
    pub fn get_details(&mut self, agent_id: &str) -> Option<AgentDetails> {
        let locator = self.locators.get(agent_id)?.clone();
        Some(match &mut self.deps.details {
            Some(reader) => reader.read(&locator),
            None => AgentDetails::unavailable("not available"),
        })
    }

    /// Diffs against the previous poll and feeds the history. Returns the new events.
    fn record(
        &mut self,
        prev: Option<&[OverviewSession]>,
        sessions: &[OverviewSession],
        now: i64,
    ) -> Vec<OverviewEvent> {
        let mut events = diff_snapshots(prev, sessions, now);
        let Some(history) = &self.deps.history else {
            return events;
        };
        let due = self
            .last_sample_at
            .is_none_or(|at| now - at >= self.deps.sample_interval_ms);
        if due {
            // A long time since the last point, restored from disk or from this run: watching
            // resumed after a gap.
            if let Some(resumed) = resumed_event(history.last_point_at(), now) {
                events.insert(0, resumed);
            }
            self.last_sample_at = Some(now);
            let agents: BTreeMap<String, HistoryAgent> = sessions
                .iter()
                .map(|s| {
                    (
                        s.agent_id.clone(),
                        HistoryAgent {
                            name: s.name.clone(),
                            project_dir: s.project_dir.clone(),
                        },
                    )
                })
                .collect();
            history.add_events(&events);
            history.add_point(point_from_sessions(sessions, now), &agents);
        } else {
            history.add_events(&events);
        }
        events
    }

    /// Polls now.
    pub fn refresh(&mut self) -> PollResult {
        let agents = self.deps.agents.list();
        let now = (self.deps.now)();
        let running: Vec<&AgentState> = agents
            .iter()
            .filter(|a| {
                a.status == crate::types::AgentStatus::Running && a.provider == ProviderId::Claude
            })
            .collect();
        let entries = if running.is_empty() {
            Vec::new()
        } else {
            let dirs = (self.deps.config_dirs)();
            (self.deps.registry)(&dirs)
        };

        let matchable: Vec<MatchableAgent> = running
            .iter()
            .map(|a| MatchableAgent {
                id: a.id.clone(),
                session_id: a.session_id.clone(),
                pid: a.pid,
                project_dir: a.project_dir.clone(),
                started_at: a.started_at.as_deref().and_then(parse_ms),
            })
            .collect();
        let mut matches = match_registry_entries(&matchable, &entries, &HashMap::new());
        let unmatched: Vec<&MatchableAgent> = matchable
            .iter()
            .filter(|a| !matches.contains_key(&a.id) && a.pid.is_some())
            .collect();
        if !unmatched.is_empty() && !entries.is_empty() {
            // The agent pid may be the login shell wrapping `claude`: look at its descendants.
            let table = (self.deps.process_table)(now);
            let descendants: HashMap<String, HashSet<u32>> = unmatched
                .iter()
                .filter_map(|a| {
                    a.pid
                        .map(|pid| (a.id.clone(), descendant_pids(pid, &table)))
                })
                .collect();
            matches = match_registry_entries(&matchable, &entries, &descendants);
        }
        self.matched = matches.keys().cloned().collect();

        let mut alive: HashSet<String> = HashSet::new();
        let mut waiting: HashSet<String> = HashSet::new();
        let mut sessions: Vec<OverviewSession> = Vec::with_capacity(agents.len());
        for agent in &agents {
            alive.insert(agent.id.clone());
            let activity = self
                .deps
                .agents
                .activity(&agent.id)
                .unwrap_or(AgentActivity {
                    last_output_at: 0,
                    last_exit_code: None,
                });
            let entry = matches.get(&agent.id).map(|i| &entries[*i]);
            let derived = derive_status(
                agent.status,
                agent.provider,
                &activity,
                entry.map(|e| &e.fields),
                now,
            );
            let ended = is_ended(derived.status);
            let ended_at = ended
                .then(|| {
                    agent
                        .ended_at
                        .as_deref()
                        .and_then(parse_ms)
                        .or_else(|| parse_ms(&agent.last_activity_at))
                })
                .flatten();
            let since = match self.tracked.get(&agent.id) {
                Some(prev) if prev.status == derived.status => prev.since,
                _ => ended_at.map_or(now, |e| e.min(now)),
            };
            self.tracked.insert(
                agent.id.clone(),
                Tracked {
                    status: derived.status,
                    since,
                },
            );
            let session_id = entry
                .and_then(|e| e.session_id.clone())
                .or_else(|| agent.session_id.clone());
            let config_dir = entry.map(|e| e.config_dir.clone());
            let mut waiting_text = None;
            if derived.status == OverviewStatus::NeedsYou
                && agent.provider == ProviderId::Claude
                && let (Some(reader), Some(sid)) =
                    (&mut self.deps.waiting_text, session_id.as_deref())
            {
                waiting.insert(agent.id.clone());
                waiting_text = reader.read(&WaitingTextAgent {
                    agent_id: &agent.id,
                    session_id: sid,
                    project_dir: &agent.project_dir,
                    config_dir: config_dir.as_deref(),
                });
            }
            self.locators.insert(
                agent.id.clone(),
                DetailsLocator {
                    provider: agent.provider,
                    session_id: session_id.clone(),
                    project_dir: agent.project_dir.clone(),
                    config_dir: config_dir.clone(),
                },
            );
            let git = self
                .deps
                .git
                .as_mut()
                .and_then(|g| g.get(&agent.project_dir, now));
            let last_output_at = to_ms(activity.last_output_at);
            sessions.push(OverviewSession {
                agent_id: agent.id.clone(),
                name: agent.name.clone(),
                provider: agent.provider,
                model: agent.model.clone(),
                project_dir: agent.project_dir.clone(),
                worktree_branch: agent.worktree_branch.clone(),
                repo_name: git.as_ref().map(|g| g.repo_name.clone()),
                branch: git.as_ref().and_then(|g| g.branch.clone()),
                commit: git.as_ref().and_then(|g| g.commit.clone()),
                status: derived.status,
                status_since: since,
                ended_at,
                exit_code: activity.last_exit_code,
                waiting_reason: derived.waiting_reason,
                waiting_text: waiting_text.filter(|t| !t.is_empty()),
                quiet_for_ms: quiet_for_ms(derived.status, last_output_at, now),
                stale: is_stale(derived.status, since, now),
                pid: agent.pid,
                started_at: agent.started_at.as_deref().and_then(parse_ms).unwrap_or(0),
                last_output_at,
            });
        }
        if let Some(reader) = &mut self.deps.waiting_text {
            reader.retain(waiting.iter().map(String::as_str));
        }
        self.tracked.retain(|id, _| alive.contains(id));
        self.locators.retain(|id, _| alive.contains(id));
        if let Some(git) = &mut self.deps.git {
            git.retain(agents.iter().map(|a| a.project_dir.as_str()));
        }

        let snapshot = OverviewSnapshot {
            at: now,
            counts: compute_counts(&sessions, now),
            sessions,
        };
        let prev = self.snapshot.take();
        let events = self.record(
            prev.as_ref().map(|s| s.sessions.as_slice()),
            &snapshot.sessions,
            now,
        );
        let key = snapshot
            .sessions
            .iter()
            .map(session_key)
            .collect::<Vec<_>>()
            .join("\n");
        let changed = self.last_key.as_deref() != Some(key.as_str());
        self.last_key = Some(key);
        self.snapshot = Some(snapshot.clone());
        PollResult {
            snapshot,
            changed,
            events,
        }
    }
}

/// The history store and the tracker share a store; these helpers keep the daemon code short.
pub fn history_query(store: &Arc<Mutex<HistoryStore>>, since_ms: i64) -> HistoryResponse {
    store.lock().query(since_ms)
}

#[cfg(test)]
mod tests;
