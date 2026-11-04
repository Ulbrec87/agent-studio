//! Finds the provider session id of a spawned agent from the provider's own catalog
//! (Hydra `AgentManager.probeSessionIdFromCatalog` / `shouldDiscoverSessionId`).
//!
//! The manager calls [`SessionLocator::locate`] on a backoff schedule after a spawn (see
//! `agents::manager`); this implementation answers from the Claude and Codex catalogs.

use std::collections::HashSet;
use std::sync::Arc;

use super::{
    ClaudeSessionCatalog, CodexSessionCatalog, ListSessionsOptions, SessionSummary, parse_ms,
};
use crate::agents::manager::{
    SESSION_DISCOVERY_GRACE_MS, SessionLocator, SessionQuery, normalize_prompt_hint,
};
use crate::types::ProviderId;

/// How many of a project's newest sessions are considered.
const CANDIDATE_LIMIT: usize = 120;

pub struct CatalogSessionLocator {
    claude: Arc<ClaudeSessionCatalog>,
    codex: Arc<CodexSessionCatalog>,
}

impl CatalogSessionLocator {
    pub fn new(claude: Arc<ClaudeSessionCatalog>, codex: Arc<CodexSessionCatalog>) -> Self {
        Self { claude, codex }
    }
}

fn started_ms(query: &SessionQuery) -> Option<i64> {
    let agent = &query.agent;
    parse_ms(agent.started_at.as_deref().unwrap_or(&agent.created_at))
}

/// Hydra `shouldDiscoverSessionId`: no id yet, or (Codex only) the known session is older than this run.
fn should_discover(query: &SessionQuery, sessions: &[SessionSummary]) -> bool {
    let agent = &query.agent;
    let Some(current) = agent.session_id.as_deref() else {
        return true;
    };
    if agent.provider != ProviderId::Codex {
        return false;
    }
    let Some(started) = started_ms(query) else {
        return true;
    };
    let Some(session) = sessions.iter().find(|s| s.session_id == current) else {
        return true;
    };
    parse_ms(&session.modified_at).is_none_or(|m| m < started - SESSION_DISCOVERY_GRACE_MS)
}

/// Pure matching step on an already-listed catalog: `Some(id)` once decided.
pub fn match_session(query: &SessionQuery, sessions: &[SessionSummary]) -> Option<String> {
    if !should_discover(query, sessions) {
        return query.agent.session_id.clone();
    }
    let used: HashSet<&str> = query.used_session_ids.iter().map(String::as_str).collect();
    let candidates: Vec<&SessionSummary> = sessions
        .iter()
        .filter(|s| !used.contains(s.session_id.as_str()))
        .collect();
    let hint = normalize_prompt_hint(query.latest_user_prompt.as_deref());
    if !hint.is_empty()
        && let Some(s) = candidates
            .iter()
            .find(|s| normalize_prompt_hint(Some(&s.first_prompt)).contains(&hint))
    {
        return Some(s.session_id.clone());
    }
    let started = started_ms(query)?;
    candidates
        .iter()
        .find(|s| {
            parse_ms(&s.modified_at).is_some_and(|m| m >= started - SESSION_DISCOVERY_GRACE_MS)
        })
        .map(|s| s.session_id.clone())
}

impl SessionLocator for CatalogSessionLocator {
    fn locate(&self, query: &SessionQuery) -> Option<String> {
        let provider = query.agent.provider;
        if query.agent.project_dir.is_empty() {
            return None;
        }
        let options = ListSessionsOptions {
            provider,
            limit: Some(CANDIDATE_LIMIT),
            project_path_prefix: Some(query.agent.project_dir.clone()),
            force_refresh: query.force_refresh,
            include_hidden: true,
            ..Default::default()
        };
        let none = HashSet::new();
        let sessions = match provider {
            ProviderId::Claude => self.claude.list_sessions(&options, &none),
            ProviderId::Codex => self.codex.list_sessions(&options, &none),
            ProviderId::Opencode => return None,
        };
        match_session(query, &sessions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::testutil::{jsonl, write};
    use crate::types::AgentState;
    use serde_json::json;

    fn summary(id: &str, prompt: &str, modified: &str) -> SessionSummary {
        SessionSummary {
            session_id: id.into(),
            first_prompt: prompt.into(),
            modified_at: modified.into(),
            ..Default::default()
        }
    }

    fn query(provider: ProviderId, session: Option<&str>, prompt: Option<&str>) -> SessionQuery {
        let mut agent = AgentState::default();
        agent.config.provider = provider;
        agent.config.session_id = session.map(Into::into);
        agent.config.project_dir = "/work/api".into();
        agent.config.created_at = "2026-01-01T10:00:00.000Z".into();
        agent.started_at = Some("2026-01-01T10:00:00.000Z".into());
        SessionQuery {
            agent,
            latest_user_prompt: prompt.map(Into::into),
            used_session_ids: Vec::new(),
            force_refresh: false,
        }
    }

    #[test]
    fn prefers_the_prompt_hint_over_recency() {
        let sessions = [
            summary("recent", "something else", "2026-01-01T10:04:00.000Z"),
            summary(
                "hinted",
                "Fix the   LOGIN bug in api",
                "2026-01-01T09:58:00.000Z",
            ),
        ];
        let q = query(ProviderId::Claude, None, Some("fix the login  bug"));
        assert_eq!(match_session(&q, &sessions).as_deref(), Some("hinted"));
    }

    #[test]
    fn falls_back_to_a_session_touched_since_the_start_minus_the_grace() {
        let q = query(ProviderId::Claude, None, None);
        let old = summary("old", "x", "2026-01-01T09:00:00.000Z");
        let new = summary("new", "y", "2026-01-01T09:57:00.000Z"); // within 5 min before start
        assert_eq!(match_session(&q, std::slice::from_ref(&old)), None);
        assert_eq!(match_session(&q, &[new, old]).as_deref(), Some("new"));
    }

    #[test]
    fn skips_sessions_owned_by_other_agents() {
        let mut q = query(ProviderId::Claude, None, None);
        q.used_session_ids = vec!["taken".into()];
        let sessions = [
            summary("taken", "x", "2026-01-01T10:05:00.000Z"),
            summary("free", "y", "2026-01-01T10:01:00.000Z"),
        ];
        assert_eq!(match_session(&q, &sessions).as_deref(), Some("free"));
    }

    #[test]
    fn a_known_claude_session_is_settled_but_codex_rechecks_a_stale_one() {
        let sessions = [summary("s1", "p", "2026-01-01T08:00:00.000Z")];
        let claude = query(ProviderId::Claude, Some("s1"), None);
        assert_eq!(match_session(&claude, &sessions).as_deref(), Some("s1"));

        // Codex resumed into a new rollout: the old id is older than this run.
        let codex = query(ProviderId::Codex, Some("s1"), None);
        assert_eq!(match_session(&codex, &sessions), None);
        let with_new = [
            summary("s2", "p", "2026-01-01T10:02:00.000Z"),
            summary("s1", "p", "2026-01-01T08:00:00.000Z"),
        ];
        assert_eq!(match_session(&codex, &with_new).as_deref(), Some("s2"));

        let fresh = [summary("s1", "p", "2026-01-01T10:02:00.000Z")];
        assert_eq!(match_session(&codex, &fresh).as_deref(), Some("s1"));
    }

    #[test]
    fn locates_from_the_real_catalogs() {
        let root = tempfile::tempdir().unwrap();
        let sid = "abc-session";
        write(
            &root.path().join("p").join(format!("{sid}.jsonl")),
            &jsonl(&[
                json!({"sessionId":sid,"cwd":"/work/api"}),
                json!({"type":"user","message":{"role":"user","content":"hello there"}}),
            ]),
        );
        let locator = CatalogSessionLocator::new(
            Arc::new(ClaudeSessionCatalog::with_dirs(
                vec![root.path().to_path_buf()],
                0,
            )),
            Arc::new(CodexSessionCatalog::with_dir(root.path().join("none"), 0)),
        );
        let mut q = query(ProviderId::Claude, None, Some("hello"));
        q.agent.config.project_dir = "/work/api".into();
        assert_eq!(locator.locate(&q).as_deref(), Some(sid));
        q.agent.config.project_dir = "/other".into();
        assert_eq!(locator.locate(&q), None);
        assert_eq!(
            locator.locate(&query(ProviderId::Opencode, None, None)),
            None
        );
    }
}
