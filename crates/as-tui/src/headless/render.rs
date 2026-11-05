//! Headless run logs for humans: Claude's `stream-json` lines become a few readable rows each
//! (assistant text, one line per tool call, result and cost); anything that is not JSON (Codex and
//! OpenCode print text) is shown as is.

use serde_json::Value;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Tool,
    ToolResult,
    Meta,
    Done,
    Error,
    Stderr,
    Raw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: Kind,
    pub text: String,
}

fn row(kind: Kind, text: impl Into<String>) -> Row {
    Row {
        kind,
        text: text.into(),
    }
}

/// Cut `text` to `max` characters with an ellipsis.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}\u{2026}")
}

fn first_line(text: &str) -> &str {
    text.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
}

/// The input of a tool call in a few words.
fn tool_summary(input: &Value) -> String {
    for key in [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "query",
        "description",
        "prompt",
    ] {
        if let Some(text) = input.get(key).and_then(Value::as_str) {
            return clip(first_line(text).trim(), 140);
        }
    }
    match input {
        Value::Object(map) if map.is_empty() => String::new(),
        Value::Null => String::new(),
        other => clip(&other.to_string(), 140),
    }
}

/// The text of a tool result (a string, or a list of `{type: text, text}` blocks).
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn duration(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{ms:.0}ms")
    } else if ms < 60_000.0 {
        format!("{:.1}s", ms / 1000.0)
    } else {
        format!(
            "{}m{:02}s",
            (ms / 60_000.0) as u64,
            ((ms / 1000.0) as u64) % 60
        )
    }
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Rows for one log line. `raw` shows the line untouched.
pub fn render_line(line: &str, raw: bool) -> Vec<Row> {
    if raw {
        return vec![row(Kind::Raw, line)];
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return vec![row(Kind::Raw, line)];
    };
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return vec![row(Kind::Raw, line)];
    };
    let mut rows = Vec::new();
    match kind {
        "system" => {
            let sub = value.get("subtype").and_then(Value::as_str).unwrap_or("");
            if sub == "init" {
                let mut parts = vec!["session".to_string()];
                if let Some(id) = value.get("session_id").and_then(Value::as_str) {
                    parts[0] = format!("session {}", short_id(id));
                }
                if let Some(model) = value.get("model").and_then(Value::as_str) {
                    parts.push(model.to_string());
                }
                if let Some(cwd) = value.get("cwd").and_then(Value::as_str) {
                    parts.push(cwd.to_string());
                }
                rows.push(row(
                    Kind::Meta,
                    format!("\u{25CF} {}", parts.join(" \u{00B7} ")),
                ));
            } else {
                rows.push(row(
                    Kind::Meta,
                    format!("\u{00B7} system {sub}").trim_end().to_string(),
                ));
            }
        }
        "assistant" => {
            let blocks = value
                .pointer("/message/content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                        for l in text.lines() {
                            rows.push(row(Kind::Text, l));
                        }
                    }
                    Some("tool_use") => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                        let summary = block.get("input").map(tool_summary).unwrap_or_default();
                        rows.push(row(
                            Kind::Tool,
                            format!("\u{2699} {name} {summary}").trim_end().to_string(),
                        ));
                    }
                    Some("thinking") => rows.push(row(Kind::Meta, "\u{00B7} thinking\u{2026}")),
                    _ => {}
                }
            }
        }
        "user" => {
            let blocks = value
                .pointer("/message/content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for block in blocks {
                if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                    continue;
                }
                let text = block.get("content").map(result_text).unwrap_or_default();
                let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
                let mut shown = clip(first_line(&text).trim(), 120);
                if lines > 1 {
                    shown.push_str(&format!(" (+{} lines)", lines - 1));
                }
                let is_error = block.get("is_error").and_then(Value::as_bool) == Some(true);
                rows.push(row(
                    if is_error {
                        Kind::Error
                    } else {
                        Kind::ToolResult
                    },
                    format!("  \u{21B3} {shown}").trim_end().to_string(),
                ));
            }
        }
        "result" => {
            let is_error = value.get("is_error").and_then(Value::as_bool) == Some(true)
                || value
                    .get("subtype")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.starts_with("error"));
            let mut parts = vec![if is_error {
                "\u{2716} failed".to_string()
            } else {
                "\u{2714} done".to_string()
            }];
            if let Some(turns) = value.get("num_turns").and_then(Value::as_u64) {
                parts.push(format!("{turns} turn{}", if turns == 1 { "" } else { "s" }));
            }
            if let Some(ms) = value.get("duration_ms").and_then(Value::as_f64) {
                parts.push(duration(ms));
            }
            if let Some(cost) = value.get("total_cost_usd").and_then(Value::as_f64) {
                parts.push(format!("${cost:.4}"));
            }
            rows.push(row(
                if is_error { Kind::Error } else { Kind::Done },
                parts.join(" \u{00B7} "),
            ));
            if is_error && let Some(text) = value.get("result").and_then(Value::as_str) {
                for l in text.lines().take(4) {
                    rows.push(row(Kind::Error, l));
                }
            }
        }
        "stderr" => {
            let text = value.get("text").and_then(Value::as_str).unwrap_or("");
            rows.push(row(Kind::Stderr, format!("stderr: {text}")));
        }
        // Token-by-token deltas would drown everything else.
        "stream_event" => {}
        other => rows.push(row(Kind::Meta, format!("\u{00B7} {other}"))),
    }
    rows
}

/// Rows of a whole log (the first line may be cut in half when the log was truncated: such a
/// line that does not parse is dropped unless `raw`).
pub fn render_log(lines: &[String], raw: bool, truncated: bool) -> Vec<Row> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i == 0 && truncated && !raw && serde_json::from_str::<Value>(line).is_err() {
            continue;
        }
        out.extend(render_line(line, raw));
    }
    out
}

/// `text` broken into rows of at most `width` columns (a word is only split when it does not fit
/// a row on its own). Always at least one row.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width {
            // Prefer to break after the last space of the row.
            match current.rfind(' ').filter(|i| *i > 0 && ch != ' ') {
                Some(i) => {
                    let rest = current[i + 1..].to_string();
                    current.truncate(i);
                    rows.push(std::mem::take(&mut current));
                    used = rest.chars().map(|c| c.width().unwrap_or(0)).sum();
                    current = rest;
                }
                None => {
                    rows.push(std::mem::take(&mut current));
                    used = 0;
                }
            }
        }
        if used == 0 && ch == ' ' && !rows.is_empty() {
            continue;
        }
        current.push(ch);
        used += w;
    }
    rows.push(current);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn init_assistant_tool_result_and_cost() {
        let init = r#"{"type":"system","subtype":"init","session_id":"0123456789abcdef","model":"sonnet","cwd":"/work/shop","tools":["Bash"]}"#;
        assert_eq!(
            texts(&render_line(init, false)),
            ["\u{25CF} session 01234567 \u{00B7} sonnet \u{00B7} /work/shop"]
        );
        let assistant = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking.\nDone soon."},{"type":"tool_use","name":"Bash","input":{"command":"cargo test\n--all","description":"x"}}]}}"#;
        let rows = render_line(assistant, false);
        assert_eq!(
            texts(&rows),
            ["Looking.", "Done soon.", "\u{2699} Bash cargo test"]
        );
        assert_eq!(rows[2].kind, Kind::Tool);
        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok\nline2\nline3"}]}}"#;
        assert_eq!(
            texts(&render_line(result, false)),
            ["  \u{21B3} ok (+2 lines)"]
        );
        let done = r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","num_turns":2,"duration_ms":4200,"total_cost_usd":0.01234}"#;
        let rows = render_line(done, false);
        assert_eq!(
            texts(&rows),
            ["\u{2714} done \u{00B7} 2 turns \u{00B7} 4.2s \u{00B7} $0.0123"]
        );
        assert_eq!(rows[0].kind, Kind::Done);
    }

    #[test]
    fn errors_stderr_and_unknown_lines() {
        let failed = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":"too many\nturns","num_turns":1}"#;
        let rows = render_line(failed, false);
        assert_eq!(rows[0].kind, Kind::Error);
        assert_eq!(texts(&rows)[1..], ["too many", "turns"]);
        let bad = r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":true,"content":[{"type":"text","text":"boom"}]}]}}"#;
        assert_eq!(render_line(bad, false)[0].kind, Kind::Error);
        assert_eq!(
            texts(&render_line(r#"{"type":"stderr","text":"oops"}"#, false)),
            ["stderr: oops"]
        );
        assert_eq!(texts(&render_line("plain text", false)), ["plain text"]);
        assert_eq!(texts(&render_line(r#"{"x":1}"#, false)), [r#"{"x":1}"#]);
        assert_eq!(
            texts(&render_line(r#"{"type":"rate_limit_event"}"#, false)),
            ["\u{00B7} rate_limit_event"]
        );
        assert!(render_line(r#"{"type":"stream_event"}"#, false).is_empty());
    }

    #[test]
    fn raw_mode_shows_the_line_untouched() {
        let line = r#"{"type":"result","num_turns":1}"#;
        let rows = render_line(line, true);
        assert_eq!((rows[0].kind, rows[0].text.as_str()), (Kind::Raw, line));
    }

    #[test]
    fn a_cut_first_line_is_dropped_when_the_log_was_truncated() {
        let lines = vec![
            r#"ssistant","message":{}}"#.to_string(),
            r#"{"type":"result","num_turns":1}"#.to_string(),
        ];
        assert_eq!(render_log(&lines, false, true).len(), 1);
        assert_eq!(render_log(&lines, false, false).len(), 2);
        assert_eq!(render_log(&lines, true, true).len(), 2);
    }

    #[test]
    fn wrapping_prefers_spaces_and_splits_long_words() {
        assert_eq!(
            wrap("hello brave new world", 11),
            ["hello brave", "new world"]
        );
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("", 5), [""]);
        assert_eq!(wrap("short", 80), ["short"]);
    }
}
