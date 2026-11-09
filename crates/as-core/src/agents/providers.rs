//! Provider definitions (Hydra `electron/agents/providers.ts`): CLI command,
//! argument builders, YOLO flag, session-id regex and preflight.

use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use tokio::process::Command;

use crate::types::{AgentState, ModelId, ProviderId};

const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Claude prints `Session ID: <uuid>`. Hydra's pattern (`session[ _-]?id?[:=\s]+([a-z0-9-]{8,})`) also
/// matched "session management" -> "management"; the id must now be a UUID.
static CLAUDE_SESSION_ID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)session(?:[_\s-]?id)?[:=\s]+([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\b",
    )
    .expect("static regex")
});
/// OpenCode session ids look like `ses_` + 10 or more letters/digits (e.g. `ses_3f1a9c2b7ZdEqPk`).
static OPENCODE_SESSION_ID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(ses_[A-Za-z0-9]{10,})\b").expect("static regex"));

/// Result of checking that a provider CLI is installed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
pub struct PreflightResult {
    pub ok: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
}

/// Static description of one provider CLI.
#[derive(Debug, Clone, Copy)]
pub struct Provider {
    pub id: ProviderId,
    /// CLI command name (`claude`, `codex`, `opencode`).
    pub command: &'static str,
    /// Whether the CLI supports resuming a session.
    pub supports_resume: bool,
    /// Flag for auto-approve / YOLO mode, if supported.
    pub yolo_flag: Option<&'static str>,
    missing_error: &'static str,
}

static CLAUDE: Provider = Provider {
    id: ProviderId::Claude,
    command: "claude",
    supports_resume: true,
    yolo_flag: Some("--dangerously-skip-permissions"),
    missing_error: "Claude CLI not found. Install it from https://claude.ai/download",
};
static CODEX: Provider = Provider {
    id: ProviderId::Codex,
    command: "codex",
    supports_resume: true,
    yolo_flag: Some("--full-auto"),
    missing_error: "Codex CLI not found. Install it with: npm install -g @openai/codex or bun add -g @openai/codex",
};
static OPENCODE: Provider = Provider {
    id: ProviderId::Opencode,
    command: "opencode",
    supports_resume: true,
    yolo_flag: None,
    missing_error: "OpenCode CLI not found. Install it from https://opencode.ai",
};

pub fn get_provider(id: ProviderId) -> &'static Provider {
    match id {
        ProviderId::Claude => &CLAUDE,
        ProviderId::Codex => &CODEX,
        ProviderId::Opencode => &OPENCODE,
    }
}

impl Provider {
    /// CLI arguments for an interactive session.
    pub fn build_args(&self, state: &AgentState) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        match self.id {
            ProviderId::Claude => {
                if state.yolo {
                    args.push("--dangerously-skip-permissions".into());
                }
                args.extend(["--model".into(), state.model.clone()]);
                if let Some(sid) = &state.session_id {
                    args.extend(["--resume".into(), sid.clone()]);
                }
            }
            ProviderId::Codex => {
                if let Some(sid) = &state.session_id {
                    args.extend(["resume".into(), sid.clone()]);
                }
                if state.yolo {
                    args.push("--full-auto".into());
                }
                args.extend(["--model".into(), state.model.clone()]);
                if let Some(effort) = state.reasoning_effort.as_ref().filter(|e| !e.is_empty()) {
                    args.extend(["-c".into(), format!("model_reasoning_effort=\"{effort}\"")]);
                }
            }
            ProviderId::Opencode => {
                args.extend(["--model".into(), state.model.clone()]);
                if let Some(sid) = &state.session_id {
                    args.extend(["--session".into(), sid.clone()]);
                }
            }
        }
        args
    }

    /// CLI arguments for a headless (non-interactive) run.
    pub fn build_headless_args(
        &self,
        model: &ModelId,
        prompt: &str,
        resume_session_id: Option<&str>,
        reasoning_effort: Option<&str>,
    ) -> Vec<String> {
        let s = |v: &str| v.to_string();
        match self.id {
            ProviderId::Claude => {
                let mut args = vec![
                    s("-p"),
                    s(prompt),
                    s("--output-format"),
                    s("stream-json"),
                    // Claude Code >= 2.1 refuses `--print --output-format stream-json` without it.
                    s("--verbose"),
                    s("--model"),
                    model.clone(),
                ];
                if let Some(sid) = resume_session_id {
                    args.extend([s("--resume"), s(sid)]);
                }
                args
            }
            ProviderId::Codex => {
                let mut args = vec![s("--model"), model.clone(), s("-q"), s(prompt)];
                if let Some(effort) = reasoning_effort.filter(|e| !e.is_empty()) {
                    args.extend([s("-c"), format!("model_reasoning_effort=\"{effort}\"")]);
                }
                args
            }
            ProviderId::Opencode => {
                let mut args = vec![
                    s("-p"),
                    s(prompt),
                    s("--output-format"),
                    s("text"),
                    s("--model"),
                    model.clone(),
                ];
                if let Some(sid) = resume_session_id {
                    args.extend([s("--session"), s(sid)]);
                }
                args
            }
        }
    }

    /// Regex detecting session ids in PTY output (Claude and OpenCode only).
    pub fn session_id_regex(&self) -> Option<&'static Regex> {
        match self.id {
            ProviderId::Codex => None,
            ProviderId::Claude => Some(&CLAUDE_SESSION_ID_REGEX),
            ProviderId::Opencode => Some(&OPENCODE_SESSION_ID_REGEX),
        }
    }

    /// Check that the CLI is installed: resolve the command (`which`, honours
    /// PATHEXT on Windows) and run `--version` with a 10 s timeout.
    pub async fn preflight(&self) -> PreflightResult {
        let command = self.command;
        let Ok(path) = which::which(command) else {
            return PreflightResult {
                ok: false,
                path: None,
                version: None,
                error: Some(self.missing_error.into()),
            };
        };
        let path = path.to_string_lossy().into_owned();
        let version = read_version(&path, command).await;
        PreflightResult {
            ok: true,
            path: Some(path),
            version,
            error: None,
        }
    }
}

/// Try `<command> --version` then `<path> --version`; first non-empty stdout wins.
async fn read_version(path: &str, command: &str) -> Option<String> {
    let targets: Vec<&str> = if path == command {
        vec![path]
    } else {
        vec![command, path]
    };
    for target in targets {
        if let Some(v) = run_version(target).await {
            return Some(v);
        }
    }
    None
}

async fn run_version(target: &str) -> Option<String> {
    let mut cmd = Command::new(target);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(VERSION_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentConfig;

    fn state(provider: ProviderId, model: &str) -> AgentState {
        AgentState {
            config: AgentConfig {
                provider,
                model: model.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn claude_args() {
        let p = get_provider(ProviderId::Claude);
        let mut s = state(ProviderId::Claude, "sonnet");
        assert_eq!(p.build_args(&s), ["--model", "sonnet"]);
        s.yolo = true;
        s.session_id = Some("abc-123".into());
        assert_eq!(
            p.build_args(&s),
            [
                "--dangerously-skip-permissions",
                "--model",
                "sonnet",
                "--resume",
                "abc-123"
            ]
        );
    }

    #[test]
    fn codex_args() {
        let p = get_provider(ProviderId::Codex);
        let mut s = state(ProviderId::Codex, "gpt-5.3-codex");
        s.session_id = Some("codex-session-01".into());
        assert_eq!(
            p.build_args(&s),
            ["resume", "codex-session-01", "--model", "gpt-5.3-codex"]
        );
        s.session_id = None;
        s.yolo = true;
        s.reasoning_effort = Some("high".into());
        assert_eq!(
            p.build_args(&s),
            [
                "--full-auto",
                "--model",
                "gpt-5.3-codex",
                "-c",
                "model_reasoning_effort=\"high\""
            ]
        );
    }

    #[test]
    fn opencode_args() {
        let p = get_provider(ProviderId::Opencode);
        let mut s = state(ProviderId::Opencode, "opencode/big-pickle");
        s.yolo = true; // unsupported: ignored
        s.session_id = Some("ses_abcdef12".into());
        assert_eq!(
            p.build_args(&s),
            [
                "--model",
                "opencode/big-pickle",
                "--session",
                "ses_abcdef12"
            ]
        );
        assert_eq!(p.yolo_flag, None);
    }

    #[test]
    fn headless_args() {
        let m = "m".to_string();
        assert_eq!(
            get_provider(ProviderId::Claude).build_headless_args(
                &m,
                "do it",
                Some("sid"),
                Some("high")
            ),
            [
                "-p",
                "do it",
                "--output-format",
                "stream-json",
                "--verbose",
                "--model",
                "m",
                "--resume",
                "sid"
            ]
        );
        assert_eq!(
            get_provider(ProviderId::Codex).build_headless_args(
                &m,
                "do it",
                Some("ignored"),
                Some("low")
            ),
            [
                "--model",
                "m",
                "-q",
                "do it",
                "-c",
                "model_reasoning_effort=\"low\""
            ]
        );
        assert_eq!(
            get_provider(ProviderId::Opencode).build_headless_args(&m, "do it", Some("sid"), None),
            [
                "-p",
                "do it",
                "--output-format",
                "text",
                "--model",
                "m",
                "--session",
                "sid"
            ]
        );
    }

    #[test]
    fn session_regex() {
        assert!(get_provider(ProviderId::Codex).session_id_regex().is_none());
        let re = get_provider(ProviderId::Claude).session_id_regex().unwrap();
        let uuid = "5f1c9a2e-77aa-4b1c-9d3e-0123456789ab";
        assert_eq!(
            &re.captures(&format!("Session ID: {uuid}")).unwrap()[1],
            uuid
        );
        assert_eq!(
            &re.captures(&format!("session_id={uuid}")).unwrap()[1],
            uuid
        );
        assert_eq!(
            &re.captures(&format!("SESSION   {uuid} ok")).unwrap()[1],
            uuid
        );
        // Hydra's loose pattern matched all of these.
        for noise in [
            "session management",
            "Session ID: 1234abcd-ef",
            "session: abcdef123456",
            "the session starts now",
            "no ids here",
        ] {
            assert!(re.captures(noise).is_none(), "{noise}");
        }
        let oc = get_provider(ProviderId::Opencode)
            .session_id_regex()
            .unwrap();
        assert_eq!(
            &oc.captures("resume with: opencode --session ses_3f1a9c2b7ZdEqPk")
                .unwrap()[1],
            "ses_3f1a9c2b7ZdEqPk"
        );
        assert_eq!(
            &oc.captures("Session ses_abcdefghij12\n").unwrap()[1],
            "ses_abcdefghij12"
        );
        for noise in [
            "session management",
            "ses_short",
            "classes_abcdefghijkl",
            "session id: 12345678",
        ] {
            assert!(oc.captures(noise).is_none(), "{noise}");
        }
    }

    #[tokio::test]
    async fn preflight_reports_missing_cli() {
        let missing = Provider {
            command: "definitely-not-a-real-cli-xyz",
            ..CLAUDE
        };
        let r = missing.preflight().await;
        assert!(!r.ok);
        assert!(r.error.unwrap().contains("Claude CLI not found"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preflight_finds_a_cli_and_reads_its_version() {
        // `sh` exists everywhere on unix, but may not support --version: only check resolution.
        let sh = Provider {
            command: "sh",
            ..CLAUDE
        };
        let r = sh.preflight().await;
        assert!(r.ok);
        assert!(r.path.is_some());
    }
}
