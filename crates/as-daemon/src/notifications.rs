//! Agent notifications (Hydra `electron/daemon/DaemonNotificationService.ts` +
//! `electron/notifications/NotificationService.ts`).
//!
//! Rules mirrored from Hydra (no cooldown exists there, only de-duplication):
//! * `AgentWaiting` event (running agent silent for 5 s) -> "Agent Finished" / "<name> is waiting for input".
//!   The Overview-tracker suppression for Claude agents arrives with P6.
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
