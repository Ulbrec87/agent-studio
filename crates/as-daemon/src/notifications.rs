//! Agent notifications (Hydra `electron/daemon/DaemonNotificationService.ts` +
//! `electron/notifications/NotificationService.ts`).
//!
//! Rules mirrored from Hydra (no cooldown exists there, only de-duplication):
//! * `AgentWaiting` event (running agent silent for 5 s) -> "Agent Finished" / "<name> is waiting for input".
//!   Suppressed for Claude agents the Overview tracker matched to a registry entry (the silence
//!   heuristic fires falsely for busy-but-quiet agents); those get the tracker's notification
//!   instead: a Claude agent entering `needs-you` -> "Needs you" / "<name> needs permission: Run:
//!   ls" (`needs_you_body`). Codex, OpenCode and unmatched Claude agents keep the 5 s rule.
//! * Status change, de-duplicated per agent (same status twice in a row is ignored):
//!   `idle` -> "Agent Completed" / "<name> has finished", `errored` -> "Agent Error" /
//!   "<name> encountered an error", `running` -> "Agent Started" / "<name> is now running";
//!   `starting` is only recorded.
//!
//! Every notification is broadcast as `Event::Notification`; an OS notification is shown only
//! when no connected client reports its terminal as focused (and the `desktop-notifications`
//! feature is on). Failures to show it are logged, never fatal.

use std::collections::{HashMap, VecDeque};

use as_core::AgentStatus;
use as_core::overview::WaitingReason;
use as_proto::{NotificationKind, StudioNotification};
use parking_lot::Mutex;

const MAX_RECENT: usize = 50;

#[derive(Default)]
pub struct NotificationService {
    last_status: Mutex<HashMap<String, AgentStatus>>,
    recent: Mutex<VecDeque<StudioNotification>>,
}

fn make(kind: NotificationKind, title: &str, body: String, agent_id: &str) -> StudioNotification {
    StudioNotification {
        id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
        kind,
        title: title.to_string(),
        body,
        agent_id: Some(agent_id.to_string()),
        timestamp: chrono::Utc::now().to_rfc3339(),
    }
}

fn run_note(kind: NotificationKind, title: &str, body: String) -> StudioNotification {
    StudioNotification {
        id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
        kind,
        title: title.to_string(),
        body,
        agent_id: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
    }
}

/// Hydra `needsYouBody`.
pub fn needs_you_body(name: &str, reason: Option<WaitingReason>, text: Option<&str>) -> String {
    let detail = text.map(|t| format!(": {t}")).unwrap_or_default();
    match reason {
        Some(WaitingReason::Permission) => format!("{name} needs permission{detail}"),
        Some(WaitingReason::Question) => match text {
            Some(t) => format!("{name} asked a question: {t}"),
            None => format!("{name} asked a question"),
        },
        _ => format!("{name} is waiting for you{detail}"),
    }
}

impl NotificationService {
    /// The agent went quiet while running.
    pub fn waiting(&self, agent_id: &str, name: &str) -> StudioNotification {
        make(
            NotificationKind::AgentWaiting,
            "Agent Finished",
            format!("{name} is waiting for input"),
            agent_id,
        )
    }

    /// A Claude agent entered `needs-you` according to the Overview tracker.
    pub fn needs_you(
        &self,
        agent_id: &str,
        name: &str,
        reason: Option<WaitingReason>,
        text: Option<&str>,
    ) -> StudioNotification {
        make(
            NotificationKind::AgentWaiting,
            "Needs you",
            needs_you_body(name, reason, text),
            agent_id,
        )
    }

    /// A status event; `None` when it repeats the previous status or is not notifiable.
    pub fn status(
        &self,
        agent_id: &str,
        name: &str,
        status: AgentStatus,
    ) -> Option<StudioNotification> {
        let previous = self.last_status.lock().insert(agent_id.to_string(), status);
        if previous == Some(status) {
            return None;
        }
        Some(match status {
            AgentStatus::Idle => make(
                NotificationKind::AgentIdle,
                "Agent Completed",
                format!("{name} has finished"),
                agent_id,
            ),
            AgentStatus::Errored => make(
                NotificationKind::AgentErrored,
                "Agent Error",
                format!("{name} encountered an error"),
                agent_id,
            ),
            AgentStatus::Running => make(
                NotificationKind::AgentStarted,
                "Agent Started",
                format!("{name} is now running"),
                agent_id,
            ),
            AgentStatus::Starting => return None,
        })
    }

    /// A headless run finished successfully (Hydra: "Headless Run Completed").
    pub fn headless_completed(&self, run_id: &str) -> StudioNotification {
        run_note(
            NotificationKind::HeadlessCompleted,
            "Headless Run Completed",
            format!("Run {run_id} finished successfully"),
        )
    }

    pub fn headless_errored(&self, run_id: &str, error: Option<&str>) -> StudioNotification {
        run_note(
            NotificationKind::HeadlessErrored,
            "Headless Run Error",
            format!("Run {run_id} failed: {}", error.unwrap_or("unknown error")),
        )
    }

    pub fn forget(&self, agent_id: &str) {
        self.last_status.lock().remove(agent_id);
    }

    pub fn record(&self, n: &StudioNotification) {
        let mut recent = self.recent.lock();
        recent.push_back(n.clone());
        while recent.len() > MAX_RECENT {
            recent.pop_front();
        }
    }

    pub fn recent(&self) -> Vec<StudioNotification> {
        self.recent.lock().iter().cloned().collect()
    }
}

/// Show an OS notification (best effort, off the async threads).
#[cfg(feature = "desktop-notifications")]
pub fn show_os_notification(n: &StudioNotification) {
    let (title, body) = (n.title.clone(), n.body.clone());
    std::thread::spawn(move || {
        let result = notify_rust::Notification::new()
            .appname("Agent Studio")
            .summary(&title)
            .body(&body)
            .show();
        if let Err(err) = result {
            tracing::debug!(%err, "OS notification failed");
        }
    });
}

#[cfg(not(feature = "desktop-notifications"))]
pub fn show_os_notification(_n: &StudioNotification) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_notifications_follow_hydra() {
        let s = NotificationService::default();
        assert!(s.status("a", "api", AgentStatus::Starting).is_none());
        let n = s.status("a", "api", AgentStatus::Running).unwrap();
        assert_eq!(n.kind, NotificationKind::AgentStarted);
        assert_eq!(n.body, "api is now running");
        assert!(s.status("a", "api", AgentStatus::Running).is_none());
        let n = s.status("a", "api", AgentStatus::Idle).unwrap();
        assert_eq!(
            (n.title.as_str(), n.body.as_str()),
            ("Agent Completed", "api has finished")
        );
        let n = s.status("a", "api", AgentStatus::Errored).unwrap();
        assert_eq!(n.kind, NotificationKind::AgentErrored);
        assert_eq!(n.agent_id.as_deref(), Some("a"));
        // forgotten agents notify again
        s.forget("a");
        assert!(s.status("a", "api", AgentStatus::Errored).is_some());
    }

    #[test]
    fn headless_notifications_follow_hydra() {
        let s = NotificationService::default();
        let n = s.headless_completed("ab12");
        assert_eq!(n.kind, NotificationKind::HeadlessCompleted);
        assert_eq!(n.body, "Run ab12 finished successfully");
        let n = s.headless_errored("ab12", Some("CLI exited with code 1"));
        assert_eq!(n.title, "Headless Run Error");
        assert_eq!(n.body, "Run ab12 failed: CLI exited with code 1");
        assert_eq!(
            s.headless_errored("x", None).body,
            "Run x failed: unknown error"
        );
    }

    #[test]
    fn needs_you_bodies_follow_hydra() {
        use WaitingReason::{Other, Permission, Question};
        assert_eq!(
            needs_you_body("api", Some(Permission), Some("Run: ls")),
            "api needs permission: Run: ls"
        );
        assert_eq!(
            needs_you_body("api", Some(Permission), None),
            "api needs permission"
        );
        assert_eq!(
            needs_you_body("api", Some(Question), Some("Which DB?")),
            "api asked a question: Which DB?"
        );
        assert_eq!(
            needs_you_body("api", Some(Question), None),
            "api asked a question"
        );
        assert_eq!(
            needs_you_body("api", Some(Other), None),
            "api is waiting for you"
        );
        assert_eq!(
            needs_you_body("api", None, Some("x")),
            "api is waiting for you: x"
        );
        let n =
            NotificationService::default().needs_you("a", "api", Some(Permission), Some("Run: ls"));
        assert_eq!(
            (n.kind, n.title.as_str()),
            (NotificationKind::AgentWaiting, "Needs you")
        );
    }

    #[test]
    fn waiting_and_recent_ring() {
        let s = NotificationService::default();
        let n = s.waiting("a", "api");
        assert_eq!(n.kind, NotificationKind::AgentWaiting);
        assert_eq!(n.title, "Agent Finished");
        assert_eq!(n.body, "api is waiting for input");
        assert_eq!(n.id.len(), 12);
        for _ in 0..60 {
            s.record(&s.waiting("a", "api"));
        }
        assert_eq!(s.recent().len(), MAX_RECENT);
    }
}
