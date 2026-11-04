// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/core/mapping/claudeCodeMapping.ts
//! Status model: maps a Claude registry entry (or output activity) to an [`OverviewStatus`].

use super::types::{
    ClaudeRegistryStatusFields, OverviewCounts, OverviewSession, OverviewStatus, WaitingReason,
};
use crate::agents::AgentActivity;
use crate::types::{AgentStatus, ProviderId};

/// Output within this window means a non-Claude agent is working.
pub const ACTIVE_OUTPUT_WINDOW_MS: i64 = 5000;
/// A working agent with no output for this long is "quiet".
pub const QUIET_AFTER_MS: i64 = 5 * 60 * 1000;
/// An idle agent is stale after this long.
pub const STALE_AFTER_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerivedStatus {
    pub status: OverviewStatus,
    pub waiting_reason: Option<WaitingReason>,
}

impl DerivedStatus {
    fn plain(status: OverviewStatus) -> Self {
        Self {
            status,
            waiting_reason: None,
        }
    }
}

fn live_status(value: &str) -> Option<OverviewStatus> {
    match value {
        "busy" | "shell" => Some(OverviewStatus::Working), // `shell` assumed to work like `busy`
        "waiting" => Some(OverviewStatus::NeedsYou),
        "idle" => Some(OverviewStatus::Idle),
        _ => None,
    }
}

fn background_state(value: &str) -> Option<OverviewStatus> {
    match value {
        "working" => Some(OverviewStatus::Working),
        "blocked" => Some(OverviewStatus::NeedsYou),
        "done" | "stopped" => Some(OverviewStatus::Finished),
        "failed" => Some(OverviewStatus::Failed),
        _ => None,
    }
}

fn map_waiting_for(waiting_for: Option<&str>) -> WaitingReason {
    match waiting_for.map(|w| w.trim().to_lowercase()).as_deref() {
        Some("permission prompt") => WaitingReason::Permission,
        Some("input needed") => WaitingReason::Question,
        _ => WaitingReason::Other,
    }
}

/// Maps a Claude Code registry entry to an overview status, or `None` when neither field is
/// recognised so the caller can fall back to output activity.
///
/// A live process that is busy or waiting wins over `state`; an idle one yields to a `state`
/// that says more (blocked, done, stopped, failed).
pub fn map_claude_registry_status(fields: &ClaudeRegistryStatusFields) -> Option<DerivedStatus> {
    let live = fields.status.as_deref().and_then(live_status);
    let background = fields.state.as_deref().and_then(background_state);
    let background_says_more = background.is_some_and(|b| b != OverviewStatus::Working);
    let status = if live == Some(OverviewStatus::Idle) && background_says_more {
        background
    } else {
        live.or(background)
    }?;
    if status != OverviewStatus::NeedsYou {
        return Some(DerivedStatus::plain(status));
    }
    Some(DerivedStatus {
        status,
        waiting_reason: Some(map_waiting_for(fields.waiting_for.as_deref())),
    })
}

/// Derives the overview status of an agent. The registry entry is only meaningful for Claude
/// agents; other providers use output activity.
pub fn derive_status(
    status: AgentStatus,
    provider: ProviderId,
    activity: &AgentActivity,
    registry: Option<&ClaudeRegistryStatusFields>,
    now: i64,
) -> DerivedStatus {
    match status {
        AgentStatus::Starting => DerivedStatus::plain(OverviewStatus::Starting),
        AgentStatus::Idle => DerivedStatus::plain(if activity.last_exit_code.is_none() {
            OverviewStatus::NotRunning
        } else {
            OverviewStatus::Finished
        }),
        AgentStatus::Errored => DerivedStatus::plain(OverviewStatus::Failed),
        AgentStatus::Running => {
            if provider == ProviderId::Claude
                && let Some(mapped) = registry.and_then(map_claude_registry_status)
            {
                return mapped;
            }
            let active = now - to_ms(activity.last_output_at) < ACTIVE_OUTPUT_WINDOW_MS;
            DerivedStatus::plain(if active {
                OverviewStatus::Working
            } else {
                OverviewStatus::Idle
            })
        }
    }
}

pub fn to_ms(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Whether the process of a session with this status is over (or never ran).
pub fn is_ended(status: OverviewStatus) -> bool {
    matches!(
        status,
        OverviewStatus::Finished | OverviewStatus::Failed | OverviewStatus::NotRunning
    )
}

/// Idle for at least 24 h.
pub fn is_stale(status: OverviewStatus, status_since: i64, now: i64) -> bool {
    status == OverviewStatus::Idle && now - status_since >= STALE_AFTER_MS
}

/// How long a working agent has been silent, when at least 5 min.
pub fn quiet_for_ms(status: OverviewStatus, last_output_at: i64, now: i64) -> Option<i64> {
    if status != OverviewStatus::Working {
        return None;
    }
    let quiet = now - last_output_at;
    (quiet >= QUIET_AFTER_MS).then_some(quiet)
}

/// Counts sessions per status. Each session is counted once; stale idle ones go under `stale`.
pub fn compute_counts(sessions: &[OverviewSession], now: i64) -> OverviewCounts {
    let mut c = OverviewCounts {
        total: u32::try_from(sessions.len()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    for s in sessions {
        let age = (now - s.status_since).max(0);
        match s.status {
            OverviewStatus::NeedsYou => c.needs_you += 1,
            OverviewStatus::Working => {
                c.working += 1;
                c.longest_working_ms = Some(c.longest_working_ms.unwrap_or(0).max(age));
            }
            OverviewStatus::Idle if s.stale => c.stale += 1,
            OverviewStatus::Idle => {
                c.idle += 1;
                c.longest_idle_ms = Some(c.longest_idle_ms.unwrap_or(0).max(age));
            }
            OverviewStatus::Finished => c.finished += 1,
            OverviewStatus::Failed => c.failed += 1,
            OverviewStatus::NotRunning => c.not_running += 1,
            OverviewStatus::Starting => {}
        }
        if !is_ended(s.status) {
            c.open += 1;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use OverviewStatus as S;

    fn fields(
        status: Option<&str>,
        state: Option<&str>,
        waiting: Option<&str>,
    ) -> ClaudeRegistryStatusFields {
        ClaudeRegistryStatusFields {
            status: status.map(str::to_string),
            state: state.map(str::to_string),
            waiting_for: waiting.map(str::to_string),
        }
    }
    fn plain(s: S) -> Option<DerivedStatus> {
        Some(DerivedStatus::plain(s))
    }
    fn needs(reason: WaitingReason) -> Option<DerivedStatus> {
        Some(DerivedStatus {
            status: S::NeedsYou,
            waiting_reason: Some(reason),
        })
    }

    #[test]
    fn live_status_mapping() {
        for (status, expected) in [
            ("busy", S::Working),
            ("shell", S::Working),
            ("idle", S::Idle),
        ] {
            assert_eq!(
                map_claude_registry_status(&fields(Some(status), None, None)),
                plain(expected)
            );
        }
    }

    #[test]
    fn background_state_mapping() {
        for (state, expected) in [
            ("working", S::Working),
            ("done", S::Finished),
            ("stopped", S::Finished),
            ("failed", S::Failed),
        ] {
            assert_eq!(
                map_claude_registry_status(&fields(None, Some(state), None)),
                plain(expected)
            );
        }
        assert_eq!(
            map_claude_registry_status(&fields(None, Some("blocked"), None)),
            needs(WaitingReason::Other)
        );
    }

    #[test]
    fn waiting_for_maps_to_a_reason() {
        for (waiting, reason) in [
            (Some("permission prompt"), WaitingReason::Permission),
            (Some("input needed"), WaitingReason::Question),
            (Some("  Permission Prompt "), WaitingReason::Permission),
            (Some("something else"), WaitingReason::Other),
            (None, WaitingReason::Other),
        ] {
            assert_eq!(
                map_claude_registry_status(&fields(Some("waiting"), None, waiting)),
                needs(reason),
                "{waiting:?}"
            );
        }
    }

    #[test]
    fn live_busy_and_waiting_win_over_state() {
        assert_eq!(
            map_claude_registry_status(&fields(Some("busy"), Some("failed"), None)),
            plain(S::Working)
        );
        assert_eq!(
            map_claude_registry_status(&fields(Some("waiting"), Some("done"), None))
                .unwrap()
                .status,
            S::NeedsYou
        );
    }

    #[test]
    fn live_idle_yields_to_a_state_that_says_more() {
        assert_eq!(
            map_claude_registry_status(&fields(Some("idle"), Some("failed"), None)),
            plain(S::Failed)
        );
        assert_eq!(
            map_claude_registry_status(&fields(Some("idle"), Some("done"), None)),
            plain(S::Finished)
        );
        assert_eq!(
            map_claude_registry_status(&fields(Some("idle"), Some("blocked"), None))
                .unwrap()
                .status,
            S::NeedsYou
        );
        assert_eq!(
            map_claude_registry_status(&fields(Some("idle"), Some("working"), None)),
            plain(S::Idle)
        );
    }

    #[test]
    fn unknown_values_give_none_and_state_is_the_fallback() {
        assert_eq!(
            map_claude_registry_status(&ClaudeRegistryStatusFields::default()),
            None
        );
        assert_eq!(
            map_claude_registry_status(&fields(Some("zzz"), Some("yyy"), None)),
            None
        );
        assert_eq!(
            map_claude_registry_status(&fields(Some("zzz"), Some("done"), None)),
            plain(S::Finished)
        );
    }

    const NOW: i64 = 1_000_000_000;
    fn activity(ago: i64, exit: Option<i32>) -> AgentActivity {
        AgentActivity {
            last_output_at: u64::try_from(NOW - ago).unwrap(),
            last_exit_code: exit,
        }
    }
    fn derive(
        status: AgentStatus,
        provider: ProviderId,
        a: &AgentActivity,
        r: Option<&ClaudeRegistryStatusFields>,
    ) -> DerivedStatus {
        derive_status(status, provider, a, r, NOW)
    }

    #[test]
    fn derive_process_states() {
        let busy = fields(Some("busy"), None, None);
        let recent = activity(1000, None);
        assert_eq!(
            derive(
                AgentStatus::Starting,
                ProviderId::Claude,
                &recent,
                Some(&busy)
            ),
            DerivedStatus::plain(S::Starting)
        );
        assert_eq!(
            derive(
                AgentStatus::Idle,
                ProviderId::Claude,
                &activity(1000, Some(0)),
                None
            ),
            DerivedStatus::plain(S::Finished)
        );
        assert_eq!(
            derive(AgentStatus::Idle, ProviderId::Claude, &recent, None),
            DerivedStatus::plain(S::NotRunning)
        );
        assert_eq!(
            derive(
                AgentStatus::Errored,
                ProviderId::Claude,
                &recent,
                Some(&busy)
            ),
            DerivedStatus::plain(S::Failed)
        );
    }

    #[test]
    fn running_claude_uses_the_registry_then_output() {
        let silent = activity(ACTIVE_OUTPUT_WINDOW_MS, None);
        let recent = activity(1000, None);
        let busy = fields(Some("busy"), None, None);
        assert_eq!(
            derive(
                AgentStatus::Running,
                ProviderId::Claude,
                &silent,
                Some(&busy)
            )
            .status,
            S::Working
        );
        assert_eq!(
            derive(
                AgentStatus::Running,
                ProviderId::Claude,
                &recent,
                Some(&fields(Some("waiting"), None, Some("input needed")))
            ),
            DerivedStatus {
                status: S::NeedsYou,
                waiting_reason: Some(WaitingReason::Question)
            }
        );
        assert_eq!(
            derive(
                AgentStatus::Running,
                ProviderId::Claude,
                &recent,
                Some(&fields(Some("idle"), None, None))
            )
            .status,
            S::Idle
        );
        // unknown registry or none: output
        let unknown = fields(Some("zzz"), None, None);
        assert_eq!(
            derive(
                AgentStatus::Running,
                ProviderId::Claude,
                &recent,
                Some(&unknown)
            )
            .status,
            S::Working
        );
        assert_eq!(
            derive(
                AgentStatus::Running,
                ProviderId::Claude,
                &silent,
                Some(&unknown)
            )
            .status,
            S::Idle
        );
        assert_eq!(
            derive(AgentStatus::Running, ProviderId::Claude, &recent, None).status,
            S::Working
        );
        assert_eq!(
            derive(AgentStatus::Running, ProviderId::Claude, &silent, None).status,
            S::Idle
        );
    }

    #[test]
    fn other_providers_ignore_the_registry() {
        let silent = activity(ACTIVE_OUTPUT_WINDOW_MS, None);
        let recent = activity(1000, None);
        for p in [ProviderId::Codex, ProviderId::Opencode] {
            assert_eq!(
                derive(
                    AgentStatus::Running,
                    p,
                    &silent,
                    Some(&fields(Some("busy"), None, None))
                )
                .status,
                S::Idle
            );
            assert_eq!(
                derive(
                    AgentStatus::Running,
                    p,
                    &recent,
                    Some(&fields(Some("waiting"), None, None))
                )
                .status,
                S::Working
            );
        }
    }

    #[test]
    fn output_window_boundary() {
        assert_eq!(
            derive(
                AgentStatus::Running,
                ProviderId::Codex,
                &activity(4999, None),
                None
            )
            .status,
            S::Working
        );
        assert_eq!(
            derive(
                AgentStatus::Running,
                ProviderId::Codex,
                &activity(5000, None),
                None
            )
            .status,
            S::Idle
        );
    }

    #[test]
    fn ended_stale_and_quiet() {
        for s in [S::Finished, S::Failed, S::NotRunning] {
            assert!(is_ended(s));
        }
        for s in [S::NeedsYou, S::Working, S::Idle, S::Starting] {
            assert!(!is_ended(s));
        }
        assert!(is_stale(S::Idle, NOW - STALE_AFTER_MS, NOW));
        assert!(!is_stale(S::Idle, NOW - STALE_AFTER_MS + 1, NOW));
        for s in [S::NeedsYou, S::Working, S::Starting, S::Finished, S::Failed] {
            assert!(!is_stale(s, 0, NOW));
        }
        assert_eq!(
            quiet_for_ms(S::Working, NOW - QUIET_AFTER_MS, NOW),
            Some(QUIET_AFTER_MS)
        );
        assert_eq!(
            quiet_for_ms(S::Working, NOW - QUIET_AFTER_MS - 1000, NOW),
            Some(QUIET_AFTER_MS + 1000)
        );
        assert_eq!(
            quiet_for_ms(S::Working, NOW - QUIET_AFTER_MS + 1, NOW),
            None
        );
        assert_eq!(quiet_for_ms(S::Idle, 0, NOW), None);
        assert_eq!(quiet_for_ms(S::NeedsYou, 0, NOW), None);
    }

    fn session(status: S, age: i64, stale: bool) -> OverviewSession {
        let mut s = OverviewSession::basic(&format!("{status:?}-{age}"), status);
        s.status_since = NOW - age;
        s.stale = stale;
        s
    }

    #[test]
    fn counts() {
        assert_eq!(compute_counts(&[], NOW), OverviewCounts::default());
        let c = compute_counts(
            &[
                session(S::NeedsYou, 10, false),
                session(S::Working, 1000, false),
                session(S::Working, 5000, false),
                session(S::Idle, 2000, false),
                session(S::Idle, 9000, false),
                session(S::Idle, STALE_AFTER_MS + 5, true),
                session(S::Starting, 1, false),
                session(S::Finished, 1, false),
                session(S::Failed, 1, false),
                session(S::NotRunning, 1, false),
            ],
            NOW,
        );
        assert_eq!(
            c,
            OverviewCounts {
                working: 2,
                idle: 2,
                stale: 1,
                needs_you: 1,
                open: 7,
                finished: 1,
                failed: 1,
                not_running: 1,
                total: 10,
                longest_working_ms: Some(5000),
                longest_idle_ms: Some(9000),
            }
        );
    }
}
