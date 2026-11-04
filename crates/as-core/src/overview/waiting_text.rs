// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi):
//   src/collector/adapters/claude-code/transcript/lastAsk.ts, transcriptLines.ts, src/core/text.ts
//! What a waiting Claude session asks, from the tail of its transcript ("Run: npm test").

use std::collections::HashSet;

use serde_json::{Map, Value};

/// The longest text kept of what a waiting session asks, in characters.
pub const MAX_WAITING_TEXT_LENGTH: usize = 200;

pub type Json = Map<String, Value>;

/// One `tool_use` block of the session's own, in transcript order.
struct ToolUse {
    id: String,
    name: String,
    input: Json,
    /// `message.id` of the assistant message it is part of, when the line gives one.
    message_id: Option<String>,
}

/// A non-blank string value.
pub fn non_empty(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn message_of(line: &Json) -> Option<&Json> {
    line.get("message").and_then(Value::as_object)
}

/// The object blocks of `message.content`.
pub fn blocks_of(line: &Json) -> Vec<&Json> {
    message_of(line)
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

/// Parses the tail. The first line is cut short unless the tail is the whole file, so it is
/// dropped then. Lines that are not JSON objects are skipped.
pub fn lines_of(tail: &str, whole_file: bool) -> Vec<Json> {
    let mut pieces = tail.split('\n');
    if !whole_file {
        pieces.next();
    }
    pieces
        .filter(|p| !p.trim().is_empty())
        .filter_map(|p| match serde_json::from_str::<Value>(p) {
            Ok(Value::Object(o)) => Some(o),
            _ => None,
        })
        .collect()
}

/// Whether a `user` line is something the person said rather than a tool result or a meta note.
fn is_prompt(line: &Json) -> bool {
    if line.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    let Some(message) = message_of(line) else {
        return false;
    };
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        return !text.trim().is_empty();
    }
    blocks_of(line)
        .iter()
        .any(|b| b.get("type").and_then(Value::as_str) == Some("text"))
}

fn is_sidechain(line: &Json) -> bool {
    line.get("isSidechain").and_then(Value::as_bool) == Some(true)
}

/// The tool use the session waits on, if any. One message can ask for several tools; Claude Code
/// asks in order, so the one waited on is the first without a result. A line with no
/// `message.id` cannot be tied to the same message, so then only the newest tool use counts and
/// any later assistant line means it moved on.
fn pending_tool_use(lines: &[Json]) -> Option<ToolUse> {
    let mut uses: Vec<ToolUse> = Vec::new();
    let mut answered: HashSet<String> = HashSet::new();
    let mut moved_on = false;
    for line in lines {
        if is_sidechain(line) {
            continue;
        }
        let kind = line.get("type").and_then(Value::as_str);
        if kind == Some("assistant") {
            let message_id = message_of(line)
                .and_then(|m| non_empty(m.get("id")))
                .map(str::to_string);
            if let Some(newest) = uses.last()
                && (message_id.is_none() || message_id != newest.message_id)
            {
                moved_on = true;
            }
            for block in blocks_of(line) {
                if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                let (Some(id), Some(name)) =
                    (non_empty(block.get("id")), non_empty(block.get("name")))
                else {
                    continue;
                };
                uses.push(ToolUse {
                    id: id.to_string(),
                    name: name.to_string(),
                    input: block
                        .get("input")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default(),
                    message_id: message_id.clone(),
                });
                moved_on = false;
            }
        } else if kind == Some("user") {
            for block in blocks_of(line) {
                if block.get("type").and_then(Value::as_str) == Some("tool_result")
                    && let Some(id) = non_empty(block.get("tool_use_id"))
                {
                    answered.insert(id.to_string());
                }
            }
            if !uses.is_empty() && is_prompt(line) {
                moved_on = true;
            }
        }
    }
    if moved_on {
        return None;
    }
    let newest_message = uses.last()?.message_id.clone();
    match newest_message {
        None => {
            let newest = uses.pop()?;
            (!answered.contains(&newest.id)).then_some(newest)
        }
        Some(mid) => uses
            .into_iter()
            .find(|u| u.message_id.as_deref() == Some(mid.as_str()) && !answered.contains(&u.id)),
    }
}

/// A path relative to the project dir when inside it, whole otherwise.
fn short_path(file: &str, project_dir: &str) -> String {
    let folder = project_dir.trim_end_matches(['/', '\\']);
    if folder.is_empty() {
        return file.to_string();
    }
    for sep in ['/', '\\'] {
        let prefix = format!("{folder}{sep}");
        if let Some(rest) = file.strip_prefix(&prefix)
            && !rest.is_empty()
        {
            return rest.to_string();
        }
    }
    file.to_string()
}

fn file_tool(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "Edit" | "MultiEdit" => ("Edit", "file_path"),
        "NotebookEdit" => ("Edit", "notebook_path"),
        "Write" => ("Write", "file_path"),
        "Read" => ("Read", "file_path"),
        _ => return None,
    })
}

/// What one tool use asks, in a few words; `None` for unknown input or a subagent start.
fn describe_tool_use(name: &str, input: &Json, project_dir: &str) -> Option<String> {
    if name == "AskUserQuestion" {
        let questions = input.get("questions").and_then(Value::as_array)?;
        let first = questions.first().and_then(Value::as_object)?;
        let question = non_empty(first.get("question"))?;
        return Some(if questions.len() > 1 {
            format!("{question} (+{} more)", questions.len() - 1)
        } else {
            question.to_string()
        });
    }
    if name == "Bash" || name == "PowerShell" {
        let command = non_empty(input.get("command"))?;
        let line = command.lines().find(|l| !l.trim().is_empty())?;
        return Some(format!("Run: {}", line.trim()));
    }
    if let Some((word, field)) = file_tool(name) {
        let file = non_empty(input.get(field))?;
        return Some(format!("{word}: {}", short_path(file, project_dir)));
    }
    match name {
        "WebFetch" => Some(format!("Fetch: {}", non_empty(input.get("url"))?)),
        "WebSearch" => Some(format!("Search: {}", non_empty(input.get("query"))?)),
        "ExitPlanMode" => Some("Approve the plan".to_string()),
        "Task" | "Agent" => None,
        other => Some(format!("Use: {other}")),
    }
}

/// Control characters, line/paragraph separators and bidi reordering marks.
fn is_unsafe_one_line(c: char) -> bool {
    matches!(c,
        '\u{0000}'..='\u{001f}' | '\u{007f}'..='\u{009f}' | '\u{2028}' | '\u{2029}' | '\u{061c}'
        | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Makes text stand on one line and cuts it to 200 characters with an ellipsis.
fn one_line(text: &str) -> Option<String> {
    let replaced: String = text
        .chars()
        .map(|c| if is_unsafe_one_line(c) { ' ' } else { c })
        .collect();
    let flat = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = flat.chars().count();
    let out = if count <= MAX_WAITING_TEXT_LENGTH {
        flat
    } else {
        let cut: String = flat.chars().take(MAX_WAITING_TEXT_LENGTH - 1).collect();
        format!("{}…", cut.trim_end())
    };
    (!out.is_empty()).then_some(out)
}

/// What a waiting Claude Code session is asking, from the tail of its transcript. `whole_file`
/// says the tail starts at the file start; otherwise its first (possibly partial) line is
/// dropped. Never panics; anything unexpected is `None`.
pub fn last_ask_from_tail(tail: &str, whole_file: bool, project_dir: &str) -> Option<String> {
    let pending = pending_tool_use(&lines_of(tail, whole_file))?;
    one_line(&describe_tool_use(
        &pending.name,
        &pending.input,
        project_dir,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    const DIR: &str = "/work/proj";
    thread_local!(static N: Cell<u32> = const { Cell::new(0) });
    fn next() -> u32 {
        N.with(|n| {
            n.set(n.get() + 1);
            n.get()
        })
    }
    fn asst(blocks: Value) -> String {
        json!({"type":"assistant","message":{"id":format!("m{}", next()),"content":blocks}})
            .to_string()
    }
    fn asst_extra(blocks: Value, extra: Value) -> String {
        let mut v =
            json!({"type":"assistant","message":{"id":format!("m{}", next()),"content":blocks}});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v.to_string()
    }
    fn use_(name: &str, input: Value) -> Value {
        json!({"type":"tool_use","id":format!("t{}", next()),"name":name,"input":input})
    }
    fn use_id(name: &str, input: Value, id: &str) -> Value {
        json!({"type":"tool_use","id":id,"name":name,"input":input})
    }
    fn result(id: &str) -> String {
        json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id}]}})
            .to_string()
    }
    fn prompt(text: &str) -> String {
        json!({"type":"user","message":{"content":text}}).to_string()
    }
    fn ask(lines: &[String]) -> Option<String> {
        last_ask_from_tail(&lines.join("\n"), true, DIR)
    }
    fn one(block: Value) -> Option<String> {
        ask(&[asst(json!([block]))])
    }

    #[test]
    fn describes_each_tool_kind() {
        let cases: Vec<(Value, &str)> = vec![
            (
                use_("Bash", json!({"command":"\n  npm test -- --run\nls"})),
                "Run: npm test -- --run",
            ),
            (
                use_("Edit", json!({"file_path":"/work/proj/src/a.ts"})),
                "Edit: src/a.ts",
            ),
            (
                use_("MultiEdit", json!({"file_path":"/work/proj/b.ts"})),
                "Edit: b.ts",
            ),
            (
                use_(
                    "NotebookEdit",
                    json!({"notebook_path":"/work/proj/n.ipynb"}),
                ),
                "Edit: n.ipynb",
            ),
            (
                use_("Write", json!({"file_path":"/elsewhere/x.md"})),
                "Write: /elsewhere/x.md",
            ),
            (
                use_("Read", json!({"file_path":"/work/proj/r.md"})),
                "Read: r.md",
            ),
            (
                use_("WebFetch", json!({"url":"https://x.dev"})),
                "Fetch: https://x.dev",
            ),
            (
                use_("WebSearch", json!({"query":"vitest mock"})),
                "Search: vitest mock",
            ),
            (
                use_("ExitPlanMode", json!({"plan":"p"})),
                "Approve the plan",
            ),
            (use_("mcp__x__do", json!({})), "Use: mcp__x__do"),
        ];
        for (block, expected) in cases {
            assert_eq!(one(block).as_deref(), Some(expected));
        }
    }

    #[test]
    fn uses_the_first_question_and_counts_the_rest() {
        assert_eq!(
            one(use_(
                "AskUserQuestion",
                json!({"questions":[{"question":"Which DB?"}]})
            ))
            .as_deref(),
            Some("Which DB?")
        );
        assert_eq!(
            one(use_(
                "AskUserQuestion",
                json!({"questions":[{"question":"A?"},{"question":"B?"},{"question":"C?"}]})
            ))
            .as_deref(),
            Some("A? (+2 more)")
        );
        assert_eq!(one(use_("AskUserQuestion", json!({"questions":[]}))), None);
    }

    #[test]
    fn nothing_for_subagent_tools_or_missing_input() {
        assert_eq!(one(use_("Task", json!({"prompt":"x"}))), None);
        assert_eq!(one(use_("Agent", json!({}))), None);
        assert_eq!(one(use_("Bash", json!({}))), None);
    }

    #[test]
    fn nothing_once_the_tool_has_a_result() {
        assert_eq!(
            ask(&[
                asst(json!([use_id("Bash", json!({"command":"ls"}), "x1")])),
                result("x1")
            ]),
            None
        );
    }

    #[test]
    fn a_later_prompt_or_assistant_reply_ends_the_wait() {
        let first = asst(json!([use_("Bash", json!({"command":"ls"}))]));
        assert_eq!(ask(&[first.clone(), prompt("never mind")]), None);
        assert_eq!(
            ask(&[first, asst(json!([{"type":"text","text":"ok"}]))]),
            None
        );
    }

    #[test]
    fn tool_results_and_meta_lines_are_not_prompts() {
        let meta = json!({"type":"user","isMeta":true,"message":{"content":"note"}}).to_string();
        let first = asst(json!([use_id("Bash", json!({"command":"ls"}), "x2")]));
        assert_eq!(
            ask(&[first, result("other"), meta]).as_deref(),
            Some("Run: ls")
        );
    }

    #[test]
    fn sidechain_lines_are_skipped() {
        let first = asst(json!([use_("Bash", json!({"command":"ls"}))]));
        let side_tool = asst_extra(
            json!([use_("Bash", json!({"command":"rm -rf x"}))]),
            json!({"isSidechain":true}),
        );
        let side_text = asst_extra(
            json!([{"type":"text","text":"hi"}]),
            json!({"isSidechain":true}),
        );
        assert_eq!(ask(&[first.clone(), side_tool]).as_deref(), Some("Run: ls"));
        assert_eq!(ask(&[first, side_text]).as_deref(), Some("Run: ls"));
    }

    #[test]
    fn takes_the_first_still_waiting_tool_of_the_newest_message() {
        let line = |block: Value| {
            json!({"type":"assistant","message":{"id":"same","content":[block]}}).to_string()
        };
        let a = line(use_id("Bash", json!({"command":"first"}), "a"));
        let b = line(use_id("Bash", json!({"command":"second"}), "b"));
        assert_eq!(ask(&[a.clone(), b.clone()]).as_deref(), Some("Run: first"));
        assert_eq!(
            ask(&[a.clone(), b.clone(), result("a")]).as_deref(),
            Some("Run: second")
        );
        assert_eq!(
            ask(&[a.clone(), b.clone(), result("b")]).as_deref(),
            Some("Run: first")
        );
        assert_eq!(ask(&[a, b, result("a"), result("b")]), None);
    }

    #[test]
    fn looks_only_at_the_newest_tool_use_when_lines_carry_no_message_id() {
        let no_id =
            |block: Value| json!({"type":"assistant","message":{"content":[block]}}).to_string();
        assert_eq!(
            ask(&[
                no_id(use_id("Bash", json!({"command":"one"}), "a")),
                no_id(use_id("Bash", json!({"command":"two"}), "b"))
            ])
            .as_deref(),
            Some("Run: two")
        );
        assert_eq!(
            ask(&[
                no_id(use_id("Bash", json!({"command":"one"}), "a")),
                no_id(json!({"type":"text","text":"x"}))
            ]),
            None
        );
    }

    #[test]
    fn drops_the_first_line_of_a_partial_tail_keeps_it_for_a_whole_file() {
        let line = asst(json!([use_("Bash", json!({"command":"ls"}))]));
        assert_eq!(last_ask_from_tail(&line, false, DIR), None);
        assert_eq!(
            last_ask_from_tail(&line, true, DIR).as_deref(),
            Some("Run: ls")
        );
        assert_eq!(
            last_ask_from_tail(&format!("ial\": \"cut off\"}}\n{line}"), false, DIR).as_deref(),
            Some("Run: ls")
        );
    }

    #[test]
    fn skips_garbage_lines_and_never_panics() {
        let good = asst(json!([use_("Bash", json!({"command":"ls"}))]));
        let lines: Vec<String> = ["not json", "[1,2]", "null"]
            .iter()
            .map(|s| (*s).to_string())
            .chain([good, "{\"type\":".into()])
            .collect();
        assert_eq!(ask(&lines).as_deref(), Some("Run: ls"));
        assert_eq!(ask(&[String::new(), "{}".into()]), None);
        assert_eq!(last_ask_from_tail("", true, DIR), None);
        assert_eq!(
            ask(&[json!({"type":"assistant","message":{"content":"str"}}).to_string()]),
            None
        );
    }

    #[test]
    fn one_line_and_cut_to_200_characters() {
        let question = format!("a\tb\u{202e}  c\n{}", "x".repeat(300));
        let text = one(use_(
            "AskUserQuestion",
            json!({"questions":[{"question":question}]}),
        ))
        .unwrap();
        assert_eq!(text.chars().count(), 200);
        assert!(text.ends_with('…'));
        assert!(text.starts_with("a b c x"));
    }
}
