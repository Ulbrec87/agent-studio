// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi):
//   src/collector/adapters/claude-code/transcript/lastSaid.ts, lastUsage.ts, src/core/text.ts
//! The details drawer data: the session's last message and the token counts of its newest reply.

use serde_json::Value;

use super::types::{AgentDetails, LastMessage, TokenCounts};
use super::waiting_text::{Json, blocks_of, lines_of, non_empty};

/// The longest last message kept, in characters (the end of the text is kept).
pub const MAX_LAST_MESSAGE_LENGTH: usize = 2000;
const PART_BREAK: &str = "\n\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NothingSaid {
    NothingYet,
    TooFarBack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastSaid {
    Text { text: String, start_cut: bool },
    Nothing(NothingSaid),
}

/// The `text` blocks with anything in them of a line of the session's own assistant reply.
fn texts_of(line: &Json) -> Vec<String> {
    let flag = |k: &str| line.get(k).and_then(Value::as_bool) == Some(true);
    if line.get("type").and_then(Value::as_str) != Some("assistant")
        || flag("isSidechain")
        || flag("isMeta")
    {
        return Vec::new();
    }
    blocks_of(line)
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| non_empty(b.get("text")).map(str::to_string))
        .collect()
}

fn message_id_of(line: &Json) -> Option<String> {
    line.get("message")
        .and_then(Value::as_object)
        .and_then(|m| non_empty(m.get("id")))
        .map(str::to_string)
}

/// What the session last said: the `text` blocks of its newest assistant reply (all lines
/// sharing one `message.id`), joined with blank lines. Subagent and meta lines, thinking, tool
/// uses and results are never taken.
pub fn last_said_in_tail(tail: &str, from_start: bool) -> LastSaid {
    let lines = lines_of(tail, from_start);
    let Some(newest) = lines.iter().rposition(|l| !texts_of(l).is_empty()) else {
        return LastSaid::Nothing(if from_start {
            NothingSaid::NothingYet
        } else {
            NothingSaid::TooFarBack
        });
    };
    let mut parts: Vec<Vec<String>> = vec![texts_of(&lines[newest])];
    let Some(message_id) = message_id_of(&lines[newest]) else {
        return LastSaid::Text {
            text: parts[0].join(PART_BREAK),
            start_cut: false,
        };
    };
    let mut index = newest as isize - 1;
    while index >= 0 {
        let line = &lines[index as usize];
        if line.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            index -= 1;
            continue;
        }
        match line.get("type").and_then(Value::as_str) {
            Some("user") => break,
            Some("assistant") => {
                if message_id_of(line).as_deref() != Some(message_id.as_str()) {
                    break;
                }
                parts.insert(0, texts_of(line));
            }
            _ => {}
        }
        index -= 1;
    }
    let text = parts
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(PART_BREAK);
    // Nothing before the message stopped it, and the line dropped from the tail's start may be
    // part of it.
    LastSaid::Text {
        text,
        start_cut: index < 0 && !from_start,
    }
}

/// Control characters except tab and line feed, separators and bidi reordering marks.
fn is_unsafe(c: char) -> bool {
    matches!(c,
        '\u{0000}'..='\u{0008}' | '\u{000b}'..='\u{001f}' | '\u{007f}'..='\u{009f}' | '\u{2028}'
        | '\u{2029}' | '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}'
        | '\u{2066}'..='\u{2069}')
}

/// Makes text fit to show: line breaks kept, unsafe characters dropped, the last 2000
/// characters kept.
pub fn message_text(text: &str, start_cut: bool) -> (String, bool) {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let clean: String = normalized.chars().filter(|c| !is_unsafe(*c)).collect();
    let clean = clean.trim().to_string();
    let chars: Vec<char> = clean.chars().collect();
    if chars.len() <= MAX_LAST_MESSAGE_LENGTH {
        return (clean, start_cut);
    }
    let kept: String = chars[chars.len() - MAX_LAST_MESSAGE_LENGTH..]
        .iter()
        .collect();
    (kept.trim_start().to_string(), true)
}

const SYNTHETIC_MODEL: &str = "<synthetic>";

fn is_count(value: Option<&Value>) -> Option<Option<u64>> {
    // Outer None: present but not a whole number. Inner None: absent.
    match value {
        None => Some(None),
        Some(Value::Number(n)) => n.as_u64().map(Some),
        Some(_) => None,
    }
}

fn is_own_reply(line: &Json) -> bool {
    let flag = |k: &str| line.get(k).and_then(Value::as_bool) == Some(true);
    if line.get("type").and_then(Value::as_str) != Some("assistant")
        || flag("isSidechain")
        || flag("isApiErrorMessage")
    {
        return false;
    }
    line.get("message")
        .and_then(Value::as_object)
        .and_then(|m| m.get("model"))
        .and_then(Value::as_str)
        != Some(SYNTHETIC_MODEL)
}

fn is_compaction(line: &Json) -> bool {
    line.get("type").and_then(Value::as_str) == Some("user")
        && line.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
        && line.get("isSidechain").and_then(Value::as_bool) != Some(true)
}

/// `input_tokens` is only the uncached part of the prompt; the whole prompt is that plus cache
/// creation plus cache reads. Anything not a whole number means the counts are not believed.
fn counts_of(usage: &Value) -> Option<TokenCounts> {
    let usage = usage.as_object()?;
    let uncached = is_count(usage.get("input_tokens"))??;
    let output = is_count(usage.get("output_tokens"))??;
    let written = is_count(usage.get("cache_creation_input_tokens"))?;
    let read = is_count(usage.get("cache_read_input_tokens"))?;
    let input = uncached
        .checked_add(written.unwrap_or(0))?
        .checked_add(read.unwrap_or(0))?;
    Some(TokenCounts {
        input,
        cached: read,
        output,
    })
}

/// Token counts of the session's newest reply, or `None` when there are none that could be right.
pub fn last_usage_in_tail(tail: &str, from_start: bool) -> Option<TokenCounts> {
    let pieces: Vec<&str> = tail.split('\n').collect();
    let first = usize::from(!from_start);
    for index in (first..pieces.len()).rev() {
        let piece = pieces[index];
        let named = piece.contains("\"isCompactSummary\"")
            || (piece.contains("\"assistant\"") && piece.contains("\"usage\""));
        if !named {
            continue;
        }
        let Ok(Value::Object(line)) = serde_json::from_str::<Value>(piece) else {
            continue;
        };
        // Compacted since the newest reply: its counts are of a context no longer in use.
        if is_compaction(&line) {
            return None;
        }
        if !is_own_reply(&line) {
            continue;
        }
        let Some(usage) = line
            .get("message")
            .and_then(Value::as_object)
            .and_then(|m| m.get("usage"))
        else {
            continue;
        };
        return counts_of(usage);
    }
    None
}

/// Details of a Claude session from the end of its transcript.
pub fn details_from_tail(tail: &str, from_start: bool) -> AgentDetails {
    let last_message = match last_said_in_tail(tail, from_start) {
        LastSaid::Text { text, start_cut } => {
            let (text, truncated) = message_text(&text, start_cut);
            LastMessage::Text { text, truncated }
        }
        LastSaid::Nothing(NothingSaid::NothingYet) => LastMessage::Unavailable {
            unavailable: "Nothing said yet".into(),
        },
        LastSaid::Nothing(NothingSaid::TooFarBack) => LastMessage::Unavailable {
            unavailable:
                "The last reply is further back than the part of the transcript that is read".into(),
        },
    };
    AgentDetails {
        last_message,
        tokens: last_usage_in_tail(tail, from_start),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant(id: &str, content: Value, extra: Value) -> String {
        let mut message = json!({"id": id, "content": content});
        if let Some(obj) = extra.as_object() {
            message.as_object_mut().unwrap().extend(obj.clone());
        }
        json!({"type":"assistant","message":message}).to_string()
    }
    fn user(text: &str) -> String {
        json!({"type":"user","message":{"content":text}}).to_string()
    }
    fn text(t: &str) -> Value {
        json!({"type":"text","text":t})
    }

    #[test]
    fn joins_the_text_blocks_of_the_newest_reply_and_ignores_thinking_and_tools() {
        let tail = [
            user("hi"),
            assistant("m1", json!([text("old")]), json!({})),
            user("again"),
            assistant(
                "m2",
                json!([{"type":"thinking","thinking":"x"}, text("first\nline")]),
                json!({}),
            ),
            assistant(
                "m2",
                json!([{"type":"tool_use","id":"t","name":"Bash","input":{}}]),
                json!({}),
            ),
            assistant("m2", json!([text("second")]), json!({})),
        ]
        .join("\n");
        assert_eq!(
            last_said_in_tail(&tail, true),
            LastSaid::Text {
                text: "first\nline\n\nsecond".into(),
                start_cut: false
            }
        );
    }

    #[test]
    fn skips_subagent_lines() {
        let tail = [
            assistant("m1", json!([text("mine")]), json!({})),
            json!({"type":"assistant","isSidechain":true,"message":{"id":"m9","content":[text("sub")]}}).to_string(),
        ]
        .join("\n");
        assert_eq!(
            last_said_in_tail(&tail, true),
            LastSaid::Text {
                text: "mine".into(),
                start_cut: false
            }
        );
    }

    #[test]
    fn explains_why_there_is_no_text() {
        assert_eq!(
            last_said_in_tail(&user("x"), true),
            LastSaid::Nothing(NothingSaid::NothingYet)
        );
        assert_eq!(
            last_said_in_tail(
                &format!("{{\"partial\":\n{}", json!({"type":"system"})),
                false
            ),
            LastSaid::Nothing(NothingSaid::TooFarBack)
        );
    }

    #[test]
    fn marks_a_message_that_may_start_before_the_tail() {
        let tail = format!(
            "cut off\n{}",
            assistant("m1", json!([text("hello")]), json!({}))
        );
        assert_eq!(
            last_said_in_tail(&tail, false),
            LastSaid::Text {
                text: "hello".into(),
                start_cut: true
            }
        );
    }

    #[test]
    fn skips_lines_that_do_not_parse() {
        let tail = format!(
            "not json\n{}\n{{broken",
            assistant("m1", json!([text("ok")]), json!({}))
        );
        assert_eq!(
            last_said_in_tail(&tail, true),
            LastSaid::Text {
                text: "ok".into(),
                start_cut: false
            }
        );
    }

    fn usage(extra: Value) -> Value {
        let mut u = json!({"input_tokens":10,"output_tokens":7,"cache_creation_input_tokens":20,"cache_read_input_tokens":30});
        u.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        u
    }

    #[test]
    fn usage_sums_the_prompt_and_reports_the_cached_part() {
        let tail = assistant("m1", json!([text("a")]), json!({"usage": usage(json!({}))}));
        assert_eq!(
            last_usage_in_tail(&tail, true),
            Some(TokenCounts {
                input: 60,
                cached: Some(30),
                output: 7
            })
        );
    }

    #[test]
    fn usage_leaves_cached_out_when_the_cache_read_is_not_reported() {
        let tail = assistant(
            "m1",
            json!([]),
            json!({"usage": {"input_tokens":5,"output_tokens":2}}),
        );
        assert_eq!(
            last_usage_in_tail(&tail, true),
            Some(TokenCounts {
                input: 5,
                cached: None,
                output: 2
            })
        );
    }

    #[test]
    fn usage_takes_the_newest_own_reply_and_reads_past_synthetic_ones() {
        let tail = [
            assistant(
                "m1",
                json!([]),
                json!({"usage": usage(json!({"output_tokens":1}))}),
            ),
            assistant(
                "m2",
                json!([]),
                json!({"usage": usage(json!({"output_tokens":99})), "model":"<synthetic>"}),
            ),
        ]
        .join("\n");
        assert_eq!(last_usage_in_tail(&tail, true).unwrap().output, 1);
    }

    #[test]
    fn no_usage_after_a_compaction_or_with_a_fractional_count() {
        let compact =
            json!({"type":"user","isCompactSummary":true,"message":{"content":"s"}}).to_string();
        let reply = assistant("m1", json!([]), json!({"usage": usage(json!({}))}));
        assert_eq!(last_usage_in_tail(&[reply, compact].join("\n"), true), None);
        let frac = assistant(
            "m1",
            json!([]),
            json!({"usage": usage(json!({"output_tokens":1.5}))}),
        );
        assert_eq!(last_usage_in_tail(&frac, true), None);
    }

    #[test]
    fn keeps_the_last_2000_characters_and_flags_truncation() {
        let long = format!("{}{}", "a".repeat(50), "b".repeat(MAX_LAST_MESSAGE_LENGTH));
        let d = details_from_tail(&assistant("m1", json!([text(&long)]), json!({})), true);
        let LastMessage::Text { text, truncated } = d.last_message else {
            panic!("expected text");
        };
        assert!(truncated);
        assert_eq!(text, "b".repeat(MAX_LAST_MESSAGE_LENGTH));
    }

    #[test]
    fn unavailable_when_nothing_was_said() {
        assert_eq!(
            details_from_tail("", true),
            AgentDetails::unavailable("Nothing said yet")
        );
    }
}
