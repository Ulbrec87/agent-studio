//! Message history of one session (Hydra `electron/sessions/TranscriptReader.ts`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::profiles::discover_projects_dirs_from_env;
use super::{codex::default_sessions_dir, extract_text_from_content};
use crate::types::ProviderId;

/// A transcript is never read past this many bytes.
const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
pub struct TranscriptMessage {
    /// `user` or `assistant`.
    pub role: String,
    pub text: String,
    pub timestamp: String,
}

/// Where to look; `None` = the real profile dirs / `~/.codex/sessions`.
#[derive(Debug, Clone, Default)]
pub struct TranscriptOptions {
    pub claude_projects_dir: Option<PathBuf>,
    pub codex_sessions_dir: Option<PathBuf>,
}

/// The last `limit` messages of a session; empty when the transcript is not found or unreadable.
/// OpenCode keeps no transcript files, so it has no history.
pub fn read_transcript_history(
    session_id: &str,
    provider: ProviderId,
    limit: usize,
    options: &TranscriptOptions,
) -> Vec<TranscriptMessage> {
    let Some(path) = find_transcript_file(session_id, provider, options) else {
        return Vec::new();
    };
    let mut messages = Vec::new();
    let read = super::for_each_json_line(&path, MAX_TRANSCRIPT_BYTES, |entry| {
        let parsed = match provider {
            ProviderId::Codex => parse_codex_entry(entry),
            _ => parse_claude_entry(entry),
        };
        messages.extend(parsed);
        true
    });
    if read.is_err() {
        return Vec::new();
    }
    if provider == ProviderId::Codex {
        messages = dedupe_adjacent(messages);
    }
    let skip = messages.len().saturating_sub(limit);
    messages.split_off(skip)
}

fn str_of<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn parse_claude_entry(entry: &Map<String, Value>) -> Option<TranscriptMessage> {
    let kind = str_of(entry, "type");
    if kind != "user" && kind != "assistant" {
        return None;
    }
    let message = entry.get("message")?.as_object()?;
    let role = str_of(message, "role");
    if role != "user" && role != "assistant" {
        return None;
    }
    let text = extract_text_from_content(message.get("content").unwrap_or(&Value::Null));
    (!text.is_empty()).then(|| TranscriptMessage {
        role: role.to_string(),
        text,
        timestamp: str_of(entry, "timestamp").to_string(),
    })
}

fn parse_codex_entry(entry: &Map<String, Value>) -> Option<TranscriptMessage> {
    let timestamp = str_of(entry, "timestamp").to_string();
    let payload = entry.get("payload")?.as_object()?;
    match str_of(entry, "type") {
        "event_msg" => {
            let role = match str_of(payload, "type") {
                "user_message" => "user",
                "agent_message" => "assistant",
                _ => return None,
            };
            let text = payload.get("message")?.as_str()?.trim().to_string();
            (!text.is_empty()).then_some(TranscriptMessage {
                role: role.into(),
                text,
                timestamp,
            })
        }
        "response_item" => {
            if str_of(payload, "type") != "message" {
                return None;
            }
            let role = str_of(payload, "role");
            let block_type = match role {
                "user" => "input_text",
                "assistant" => "output_text",
                _ => return None,
            };
            let text = payload
                .get("content")?
                .as_array()?
                .iter()
                .filter_map(|b| {
                    let b = b.as_object()?;
                    (str_of(b, "type") == block_type)
                        .then(|| str_of(b, "text").trim().to_string())
                        .filter(|t| !t.is_empty())
                })
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string();
            (!text.is_empty()).then(|| TranscriptMessage {
                role: role.into(),
                text,
                timestamp,
            })
        }
        _ => None,
    }
}

fn dedupe_adjacent(messages: Vec<TranscriptMessage>) -> Vec<TranscriptMessage> {
    let mut out: Vec<TranscriptMessage> = Vec::with_capacity(messages.len());
    for m in messages {
        if out.last() != Some(&m) {
            out.push(m);
        }
    }
    out
}

fn find_transcript_file(
    session_id: &str,
    provider: ProviderId,
    options: &TranscriptOptions,
) -> Option<PathBuf> {
    if session_id.is_empty() {
        return None;
    }
    match provider {
        ProviderId::Codex => {
            let dir = options
                .codex_sessions_dir
                .clone()
                .unwrap_or_else(default_sessions_dir);
            find_codex_transcript_file(session_id, &dir)
        }
        ProviderId::Opencode => None,
        ProviderId::Claude => {
            let dirs = match &options.claude_projects_dir {
                Some(d) => vec![d.clone()],
                None => discover_projects_dirs_from_env(),
            };
            dirs.iter()
                .find_map(|d| find_claude_transcript_file(session_id, d))
        }
    }
}

/// `<projects_dir>/<any project>/<session_id>.jsonl`.
pub fn find_claude_transcript_file(session_id: &str, projects_dir: &Path) -> Option<PathBuf> {
    if session_id.is_empty() || session_id.contains(['/', '\\']) {
        return None;
    }
    std::fs::read_dir(projects_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path().join(format!("{session_id}.jsonl")))
        .find(|candidate| candidate.is_file())
}

/// A `.jsonl` anywhere below `sessions_dir` whose file name contains the session id.
pub fn find_codex_transcript_file(session_id: &str, sessions_dir: &Path) -> Option<PathBuf> {
    let mut stack = vec![sessions_dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in read.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(".jsonl") && name.contains(session_id) {
                    return Some(path);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::testutil::{jsonl, write};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn msg(role: &str, text: &str, ts: &str) -> TranscriptMessage {
        TranscriptMessage {
            role: role.into(),
            text: text.into(),
            timestamp: ts.into(),
        }
    }

    #[test]
    fn reads_claude_history_from_project_jsonl_files() {
        let root = tempfile::tempdir().unwrap();
        let projects = root.path().join(".claude/projects");
        write(
            &projects.join("demo/session-claude.jsonl"),
            &jsonl(&[
                json!({"type":"user","timestamp":"2026-03-21T10:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"Summarize the repo."}]}}),
                json!({"type":"assistant","timestamp":"2026-03-21T10:00:10.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Here is the summary."}]}}),
            ]),
        );
        let options = TranscriptOptions {
            claude_projects_dir: Some(projects),
            ..Default::default()
        };
        assert_eq!(
            read_transcript_history("session-claude", ProviderId::Claude, 50, &options),
            vec![
                msg("user", "Summarize the repo.", "2026-03-21T10:00:00.000Z"),
                msg(
                    "assistant",
                    "Here is the summary.",
                    "2026-03-21T10:00:10.000Z"
                ),
            ]
        );
    }

    #[test]
    fn reads_codex_history_from_session_files() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join(".codex/sessions");
        write(
            &sessions.join("2026/03/21/rollout-2026-03-21T10-00-00-session-codex.jsonl"),
            &jsonl(&[
                json!({"timestamp":"2026-03-21T10:00:00.000Z","type":"event_msg","payload":{"type":"user_message","message":"Fix the remote chat history."}}),
                json!({"timestamp":"2026-03-21T10:00:05.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"I found the root cause."}]}}),
                json!({"timestamp":"2026-03-21T10:00:10.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Patch it."}]}}),
                json!({"timestamp":"2026-03-21T10:00:15.000Z","type":"event_msg","payload":{"type":"agent_message","message":"Patched and verified."}}),
            ]),
        );
        let options = TranscriptOptions {
            codex_sessions_dir: Some(sessions),
            ..Default::default()
        };
        assert_eq!(
            read_transcript_history("session-codex", ProviderId::Codex, 50, &options),
            vec![
                msg(
                    "user",
                    "Fix the remote chat history.",
                    "2026-03-21T10:00:00.000Z"
                ),
                msg(
                    "assistant",
                    "I found the root cause.",
                    "2026-03-21T10:00:05.000Z"
                ),
                msg("user", "Patch it.", "2026-03-21T10:00:10.000Z"),
                msg(
                    "assistant",
                    "Patched and verified.",
                    "2026-03-21T10:00:15.000Z"
                ),
            ]
        );
    }

    #[test]
    fn keeps_the_last_messages_skips_bad_lines_and_dedupes_codex_echoes() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("s");
        let echo = json!({"timestamp":"t1","type":"event_msg","payload":{"type":"user_message","message":"hi"}});
        let echo2 = json!({"timestamp":"t1","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}});
        write(
            &sessions.join("rollout-abc.jsonl"),
            &format!(
                "{echo}\nnot json\n{echo2}\n{}\n",
                json!({"timestamp":"t2","type":"event_msg","payload":{"type":"agent_message","message":"yo"}})
            ),
        );
        let options = TranscriptOptions {
            codex_sessions_dir: Some(sessions),
            ..Default::default()
        };
        let all = read_transcript_history("abc", ProviderId::Codex, 50, &options);
        assert_eq!(
            all,
            vec![msg("user", "hi", "t1"), msg("assistant", "yo", "t2")]
        );
        let last = read_transcript_history("abc", ProviderId::Codex, 1, &options);
        assert_eq!(last, vec![msg("assistant", "yo", "t2")]);
        assert!(read_transcript_history("zzz", ProviderId::Codex, 5, &options).is_empty());
    }
}
