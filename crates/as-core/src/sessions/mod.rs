//! Session catalogs: what the provider CLIs saved on disk, so it can be listed, imported and resumed.
//!
//! Ported from Hydra `electron/sessions/*`:
//! * [`claude`] scans `<configDir>/projects/<slug>/<id>.jsonl` of every Claude profile (see [`profiles`]),
//! * [`codex`] scans `~/.codex/sessions/**/rollout-*.jsonl`,
//! * [`transcript`] reads the message history of one session,
//! * [`locator`] finds the session id of a freshly spawned agent (used by the manager).
//!
//! Hydra has no OpenCode catalog; neither do we (OpenCode keeps its sessions in its own database).
//! Every parser is tolerant: unknown or missing fields and corrupt lines are skipped, never fatal.

pub mod claude;
pub mod codex;
pub mod locator;
pub mod profiles;
pub mod transcript;

use std::collections::HashSet;
use std::io::BufRead;
use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::ProviderId;

pub use claude::ClaudeSessionCatalog;
pub use codex::CodexSessionCatalog;
pub use locator::CatalogSessionLocator;
pub use profiles::ClaudeProfile;

/// How long a scan result is reused without touching the disk (Hydra: 5 s).
pub const SESSION_CACHE_TTL_MS: u64 = 5000;
/// Bytes of a transcript read to find its header (session id, cwd, branch, first prompt).
pub const HEADER_MAX_BYTES: usize = 256 * 1024;
/// Bytes of a transcript read to count its messages (huge files count as "at least").
pub const COUNT_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Hydra `ClaudeSessionSummary` (plus `provider` and `hidden`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionSummary {
    pub session_id: String,
    pub provider: ProviderId,
    pub project_path: String,
    pub first_prompt: String,
    pub message_count: u32,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub modified_at: String,
    pub git_branch: Option<String>,
    pub is_sidechain: bool,
    pub source_path: String,
    /// In the config's `hiddenSessionIds` (only listed when `include_hidden`).
    pub hidden: bool,
}

/// Hydra `ListClaudeSessionsOptions`, plus a free-text `query`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct ListSessionsOptions {
    pub provider: ProviderId,
    /// Newest first; `None` or `0` = all.
    pub limit: Option<usize>,
    /// Only sessions modified within this many days; `None` or `0` = any age.
    pub max_age_days: Option<u32>,
    /// Only sessions whose project path starts with this (slash style and trailing `/` ignored).
    pub project_path_prefix: Option<String>,
    /// Every word must appear (case-insensitive) in the session id, project path or first prompt.
    pub query: Option<String>,
    /// List hidden sessions too (flagged with `hidden`).
    pub include_hidden: bool,
    /// List Claude sub-agent transcripts (`isSidechain`) too; they cannot be resumed on their own.
    pub include_sidechains: bool,
    /// Rescan now instead of reusing the cached snapshot.
    pub force_refresh: bool,
}

pub(crate) fn normalize_path_for_comparison(input: &str) -> String {
    let normalized = input.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized.to_string()
    }
}

pub(crate) fn iso(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(crate) fn parse_ms(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|d| d.timestamp_millis())
}

/// Created time of a file, falling back to the modified time where the filesystem has no birth time.
pub(crate) fn file_times(meta: &std::fs::Metadata) -> (String, String) {
    let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let created = meta.created().unwrap_or(modified);
    (iso(created), iso(modified))
}

/// Apply the shared filters of both catalogs, newest first.
pub(crate) fn filter_and_sort(
    sessions: &[SessionSummary],
    options: &ListSessionsOptions,
    hidden: &HashSet<String>,
) -> Vec<SessionSummary> {
    let prefix = options
        .project_path_prefix
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(normalize_path_for_comparison);
    let cutoff = options
        .max_age_days
        .filter(|d| *d > 0)
        .map(|d| Utc::now().timestamp_millis() - i64::from(d) * 86_400_000);
    let words: Vec<String> = options
        .query
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let mut out: Vec<SessionSummary> = sessions
        .iter()
        .filter_map(|s| {
            let is_hidden = hidden.contains(&s.session_id);
            if is_hidden && !options.include_hidden {
                return None;
            }
            if s.is_sidechain && !options.include_sidechains {
                return None;
            }
            if let Some(prefix) = &prefix
                && !normalize_path_for_comparison(&s.project_path).starts_with(prefix.as_str())
            {
                return None;
            }
            if let Some(cutoff) = cutoff
                && parse_ms(&s.modified_at).is_none_or(|m| m < cutoff)
            {
                return None;
            }
            if !words.is_empty() {
                let hay = format!("{} {} {}", s.session_id, s.project_path, s.first_prompt)
                    .to_lowercase();
                if !words.iter().all(|w| hay.contains(w)) {
                    return None;
                }
            }
            let mut s = s.clone();
            s.hidden = is_hidden;
            Some(s)
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(parse_ms(&s.modified_at).unwrap_or(0)));
    if let Some(limit) = options.limit.filter(|l| *l > 0) {
        out.truncate(limit);
    }
    out
}

/// Text of a Claude message `content` (string, or the `text` blocks of an array).
pub(crate) fn extract_text_from_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.trim().to_string(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                let block = item.as_object()?;
                if block.get("type")?.as_str()? != "text" {
                    return None;
                }
                let text = block.get("text")?.as_str()?.trim();
                (!text.is_empty()).then(|| text.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

/// Prompts that are bookkeeping, not something the user typed.
pub(crate) fn is_ignorable_prompt_text(input: &str) -> bool {
    let n = input.trim().to_lowercase();
    n.is_empty()
        || n.starts_with("<local-command-caveat>")
        || n.starts_with("<ide_")
        || n.starts_with("<command-name>")
        || n.starts_with("<local-command-std")
        || n.contains("<command-message>")
        || n.starts_with("/clear")
}

/// Read at most `max_bytes` of `path`, line by line, and hand every non-empty line that parses as
/// a JSON object to `visit`. A line that is cut by the cap or does not parse is skipped. `visit`
/// returns `false` to stop early.
pub(crate) fn for_each_json_line(
    path: &Path,
    max_bytes: usize,
    mut visit: impl FnMut(&serde_json::Map<String, Value>) -> bool,
) -> std::io::Result<()> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(std::io::Read::take(file, max_bytes as u64));
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&line);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed)
            && !visit(&map)
        {
            return Ok(());
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::Path;

    pub fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    pub fn jsonl(lines: &[serde_json::Value]) -> String {
        lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookkeeping_prompts_are_ignorable() {
        for p in [
            "",
            "  ",
            "<local-command-caveat>x",
            "<ide_opened_file>x",
            "<command-name>/clear</command-name>",
            "x <command-message>y</command-message>",
            "/clear",
            "<local-command-stdout>Login successful</local-command-stdout>",
        ] {
            assert!(is_ignorable_prompt_text(p), "{p}");
        }
        assert!(!is_ignorable_prompt_text("fix the bug"));
    }
}
