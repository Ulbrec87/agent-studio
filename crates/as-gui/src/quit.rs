//! The quit flow's decisions, without Tauri (see `shell.rs` for the glue).
//!
//! Like Hydra (`window-all-closed`), closing the window with **nothing running** also stops the daemon, so no orphan process stays
//! behind. When agents are running the window stays open and asks the front end
//! (`confirm-quit` event with the number of running agents): **Quit All** (`quit_app { stopAgents: true }`: stop every agent, stop
//! the daemon, exit), **Keep Running** (`quit_app { stopAgents: false }`: exit the window only, the daemon and the agents live on)
//! or **Cancel** (the front end just closes its dialog). With nothing running the window closes right away.

use serde_json::{Value, json};

/// What a close request of the main window leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseDecision {
    /// Close the window (and exit the app) now; the daemon could not be asked, so it is left alone.
    Close,
    /// Nothing runs: stop the (idle) daemon, best effort and quickly, then close.
    CloseAndStopDaemon,
    /// Keep the window and ask the user what to do with the `running` agents.
    Confirm { running: usize },
}

/// The protocol request that asks for the daemon's status.
pub fn status_request() -> Value {
    json!({ "type": "status" })
}

/// The protocol request that stops the daemon (`stop_agents: true` kills every agent first).
pub fn shutdown_request(stop_agents: bool) -> Value {
    json!({ "type": "shutdown", "payload": { "stop_agents": stop_agents } })
}

/// `running_count` of a `Status` answer; `None` when the daemon did not answer or the answer has another shape.
pub fn running_agents(answer: &Result<Value, String>) -> Option<usize> {
    let response = answer.as_ref().ok()?;
    if response.get("type")?.as_str()? != "status" {
        return None;
    }
    let count = response.get("payload")?.get("running_count")?.as_u64()?;
    usize::try_from(count).ok()
}

/// A daemon that cannot be asked (not running, not connected, slow) never keeps the window open: there is nothing to protect.
pub fn decide_close(answer: &Result<Value, String>) -> CloseDecision {
    match running_agents(answer) {
        Some(running) if running > 0 => CloseDecision::Confirm { running },
        Some(_) => CloseDecision::CloseAndStopDaemon,
        None => CloseDecision::Close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use as_proto::{DaemonStatus, Request, Response};

    fn status_answer(running: usize) -> Result<Value, String> {
        let status = DaemonStatus {
            pid: 1,
            version: "0".into(),
            protocol: 1,
            uptime_secs: 1,
            agent_count: running + 1,
            running_count: running,
            clients: 1,
        };
        Ok(serde_json::to_value(Response::Status(status)).unwrap())
    }

    #[test]
    fn requests_are_valid_protocol_messages() {
        assert_eq!(
            serde_json::from_value::<Request>(status_request()).unwrap(),
            Request::Status
        );
        assert_eq!(
            serde_json::from_value::<Request>(shutdown_request(true)).unwrap(),
            Request::Shutdown { stop_agents: true }
        );
        assert_eq!(
            serde_json::from_value::<Request>(shutdown_request(false)).unwrap(),
            Request::Shutdown { stop_agents: false }
        );
    }

    #[test]
    fn running_agents_ask_for_confirmation() {
        assert_eq!(
            decide_close(&status_answer(3)),
            CloseDecision::Confirm { running: 3 }
        );
        assert_eq!(
            decide_close(&status_answer(1)),
            CloseDecision::Confirm { running: 1 }
        );
    }

    #[test]
    fn nothing_running_stops_the_daemon() {
        assert_eq!(
            decide_close(&status_answer(0)),
            CloseDecision::CloseAndStopDaemon
        );
        // Stopping without killing agents: there are none, and a race must not kill a freshly started one.
        assert_eq!(
            shutdown_request(false)["payload"]["stop_agents"],
            json!(false)
        );
    }

    #[test]
    fn no_daemon_or_garbage_just_closes() {
        assert_eq!(
            decide_close(&Err("daemon not connected".into())),
            CloseDecision::Close
        );
        // Another response variant or garbage never blocks the close either.
        assert_eq!(
            decide_close(&Ok(json!({ "type": "ok" }))),
            CloseDecision::Close
        );
        assert_eq!(decide_close(&Ok(json!(7))), CloseDecision::Close);
        assert_eq!(running_agents(&Ok(json!({"type":"status"}))), None);
    }
}
