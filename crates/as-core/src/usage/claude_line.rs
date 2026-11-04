//! Claude transcript lines -> usage records (Hydra `shared/usage/claudeLine.ts`). Tolerant: unknown
//! or missing fields never panic, anything that is not a billable assistant call is `None`.

use serde_json::Value;

use super::pricing::{is_synthetic_model, normalize_model};
use super::types::{TokenCounts, UsageProvider, UsageRecord};

/// Lines longer than this are skipped without parsing.
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedClaudeLine {
    pub record: UsageRecord,
    /// `message.id:requestId` (or `message.id`). Claude Code writes one line per content block of
    /// the same API response, and resumed/forked sessions copy earlier lines, so count a key once.
    pub dedupe_key: Option<String>,
}

/// Non-finite, negative, non-numeric -> 0; otherwise floored.
pub(crate) fn count(v: Option<&Value>) -> u64 {
    match v.and_then(Value::as_f64) {
        Some(n) if n.is_finite() && n > 0.0 => n.floor() as u64,
        _ => 0,
    }
}

fn is_num(v: Option<&Value>) -> bool {
    v.and_then(Value::as_f64).is_some_and(f64::is_finite)
}

fn non_empty(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Parses one transcript line into a usage record; `None` for anything that is not a billable
/// assistant call.
pub fn parse_claude_usage_line(line: &str) -> Option<ParsedClaudeLine> {
    if line.len() > MAX_LINE_BYTES {
        return None;
    }
    if !line.contains("\"assistant\"") || !line.contains("\"usage\"") {
        return None;
    }
    let obj: Value = serde_json::from_str(line).ok()?;
    let obj = obj.as_object()?;
    if obj.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let message = obj.get("message")?.as_object()?;
    let usage = message.get("usage")?.as_object()?;

    let raw_model = message.get("model").and_then(Value::as_str).unwrap_or("");
    if is_synthetic_model(raw_model) {
        return None;
    }
    let ts = super::parse_iso_ms(obj.get("timestamp")?.as_str()?)?;

    let split = usage.get("cache_creation").and_then(Value::as_object);
    let (cache_write5m, cache_write1h) = match split {
        Some(s)
            if is_num(s.get("ephemeral_5m_input_tokens"))
                || is_num(s.get("ephemeral_1h_input_tokens")) =>
        {
            (
                count(s.get("ephemeral_5m_input_tokens")),
                count(s.get("ephemeral_1h_input_tokens")),
            )
        }
        _ => (count(usage.get("cache_creation_input_tokens")), 0),
    };
    let tokens = TokenCounts {
        input: count(usage.get("input_tokens")),
        output: count(usage.get("output_tokens")),
        cache_write5m,
        cache_write1h,
        cache_read: count(usage.get("cache_read_input_tokens")),
    };
    if tokens.total() == 0 {
        return None;
    }

    let id = non_empty(message.get("id"));
    let dedupe_key = id.map(|id| match non_empty(obj.get("requestId")) {
        Some(req) => format!("{id}:{req}"),
        None => id.to_string(),
    });

    Some(ParsedClaudeLine {
        record: UsageRecord {
            ts,
            provider: UsageProvider::Claude,
            model: if raw_model.trim().is_empty() {
                "unknown".to_string()
            } else {
                normalize_model(raw_model)
            },
            project_key: obj
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            tokens,
        },
        dedupe_key,
    })
}

/// Display name of a project: the last path segment of the cwd.
pub fn claude_project_key_label(project_key: &str) -> String {
    project_key
        .split(['/', '\\'])
        .rfind(|s| !s.is_empty())
        .unwrap_or("Unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The sample line of Hydra's test, with overrides merged into `message.usage` / the top level.
    fn line(top: Value, usage: Value, message: Option<Value>) -> String {
        let mut obj = json!({
            "type": "assistant",
            "requestId": "req_1",
            "timestamp": "2026-10-22T17:45:01.123Z",
            "cwd": "/home/u/proj",
            "sessionId": "s1",
            "message": {
                "id": "msg_1",
                "model": "claude-opus-5",
                "role": "assistant",
                "content": [],
                "usage": {
                    "input_tokens": 2,
                    "cache_creation_input_tokens": 76124,
                    "cache_read_input_tokens": 0,
                    "output_tokens": 1312,
                    "output_tokens_details": {"thinking_tokens": 1043},
                    "cache_creation": {"ephemeral_1h_input_tokens": 76124, "ephemeral_5m_input_tokens": 0}
                }
            }
        });
        let o = obj.as_object_mut().unwrap();
        for (k, v) in top.as_object().unwrap() {
            if v.is_null() {
                o.remove(k);
            } else {
                o.insert(k.clone(), v.clone());
            }
        }
        if let Some(m) = message {
            o.insert("message".into(), m);
        }
        if let Some(u) = usage.as_object() {
            let target = o["message"]["usage"].as_object_mut().unwrap();
            for (k, v) in u {
                if v.is_null() {
                    target.remove(k);
                } else {
                    target.insert(k.clone(), v.clone());
                }
            }
        }
        obj.to_string()
    }
    fn plain() -> String {
        line(json!({}), json!({}), None)
    }
    fn parse(s: &str) -> Option<ParsedClaudeLine> {
        parse_claude_usage_line(s)
    }

    #[test]
    fn parses_the_full_sample() {
        let p = parse(&plain()).unwrap();
        assert_eq!(
            p.record,
            UsageRecord {
                ts: crate::usage::parse_iso_ms("2026-10-22T17:45:01.123Z").unwrap(),
                provider: UsageProvider::Claude,
                model: "claude-opus-5".into(),
                project_key: "/home/u/proj".into(),
                tokens: TokenCounts {
                    input: 2,
                    output: 1312,
                    cache_write5m: 0,
                    cache_write1h: 76124,
                    cache_read: 0
                },
            }
        );
        assert_eq!(p.dedupe_key.as_deref(), Some("msg_1:req_1"));
    }

    #[test]
    fn legacy_cache_creation_goes_into_the_5m_bucket() {
        let p = parse(&line(
            json!({}),
            json!({"cache_creation": null, "cache_creation_input_tokens": 500, "cache_read_input_tokens": 7}),
            None,
        ))
        .unwrap();
        assert_eq!(
            (
                p.record.tokens.cache_write5m,
                p.record.tokens.cache_write1h,
                p.record.tokens.cache_read
            ),
            (500, 0, 7)
        );
    }

    #[test]
    fn partial_split_is_used() {
        let p = parse(&line(
            json!({}),
            json!({"cache_creation": {"ephemeral_5m_input_tokens": 10}}),
            None,
        ))
        .unwrap();
        assert_eq!(
            (p.record.tokens.cache_write5m, p.record.tokens.cache_write1h),
            (10, 0)
        );
    }

    #[test]
    fn skips_synthetic_and_zero_usage() {
        let synthetic = json!({"id": "m", "model": "<synthetic>", "usage": {"input_tokens": 5}});
        assert!(parse(&line(json!({}), json!({}), Some(synthetic))).is_none());
        let zero = json!({"input_tokens": 0, "output_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "cache_creation": null});
        assert!(parse(&line(json!({}), zero, None)).is_none());
    }

    #[test]
    fn skips_non_assistant_lines() {
        assert!(parse(&line(json!({"type": "user"}), json!({}), None)).is_none());
        assert!(parse(&line(json!({"type": "system"}), json!({}), None)).is_none());
        assert!(parse(r#"{"type":"summary","summary":"an assistant usage"}"#).is_none());
    }

    #[test]
    fn skips_assistant_lines_without_object_usage() {
        assert!(
            parse(
                r#"{"type":"assistant","timestamp":"2026-10-22T17:45:01Z","message":{"usage":5}}"#
            )
            .is_none()
        );
        assert!(parse(r#"{"type":"assistant","usage":{}}"#).is_none());
    }

    #[test]
    fn malformed_json_is_none() {
        assert!(parse(r#"{"type":"assistant","usage":"#).is_none());
        assert!(parse("").is_none());
        assert!(parse(r#""assistant" "usage""#).is_none());
        assert!(parse(r#"["assistant","usage"]"#).is_none());
    }

    #[test]
    fn missing_or_invalid_timestamp_is_none() {
        assert!(parse(&line(json!({"timestamp": null}), json!({}), None)).is_none());
        assert!(parse(&line(json!({"timestamp": "not a date"}), json!({}), None)).is_none());
        assert!(parse(&line(json!({"timestamp": 12345}), json!({}), None)).is_none());
    }

    #[test]
    fn dedupe_keys() {
        assert_eq!(
            parse(&plain()).unwrap().dedupe_key.as_deref(),
            Some("msg_1:req_1")
        );
        assert_eq!(
            parse(&line(json!({"requestId": null}), json!({}), None))
                .unwrap()
                .dedupe_key
                .as_deref(),
            Some("msg_1")
        );
        assert_eq!(
            parse(&line(json!({"requestId": ""}), json!({}), None))
                .unwrap()
                .dedupe_key
                .as_deref(),
            Some("msg_1")
        );
        let no_id = json!({"model": "claude-opus-5", "usage": {"input_tokens": 3}});
        assert_eq!(
            parse(&line(json!({}), json!({}), Some(no_id)))
                .unwrap()
                .dedupe_key,
            None
        );
    }

    #[test]
    fn normalises_the_model_and_falls_back_to_unknown() {
        let m =
            json!({"id": "m", "model": "claude-sonnet-4-5-20250929", "usage": {"input_tokens": 1}});
        assert_eq!(
            parse(&line(json!({}), json!({}), Some(m)))
                .unwrap()
                .record
                .model,
            "claude-sonnet-4-5"
        );
        let m = json!({"id": "m", "usage": {"input_tokens": 1}});
        assert_eq!(
            parse(&line(json!({}), json!({}), Some(m)))
                .unwrap()
                .record
                .model,
            "unknown"
        );
    }

    #[test]
    fn bad_numbers_are_zero_and_floats_are_floored() {
        let p = parse(&line(
            json!({}),
            json!({
                "input_tokens": -5, "output_tokens": "lots", "cache_read_input_tokens": 9.9,
                "cache_creation": {"ephemeral_5m_input_tokens": null, "ephemeral_1h_input_tokens": 4}
            }),
            None,
        ))
        .unwrap();
        assert_eq!(
            p.record.tokens,
            TokenCounts {
                input: 0,
                output: 0,
                cache_write5m: 0,
                cache_write1h: 4,
                cache_read: 9
            }
        );
    }

    #[test]
    fn project_key_defaults_to_empty() {
        assert_eq!(
            parse(&line(json!({"cwd": null}), json!({}), None))
                .unwrap()
                .record
                .project_key,
            ""
        );
    }

    #[test]
    fn prefilters_and_oversize_lines() {
        assert!(parse(r#"{"type":"user","message":{"content":"hi"}}"#).is_none());
        assert!(parse(r#"{"type":"assistant","message":{"content":"hi"}}"#).is_none());
        let huge = plain().replace(
            "\"/home/u/proj\"",
            &format!("\"{}\"", "x".repeat(MAX_LINE_BYTES)),
        );
        assert!(parse(&huge).is_none());
    }

    #[test]
    fn project_key_label() {
        assert_eq!(
            claude_project_key_label("/home/u/PhpstormProjects/hydra"),
            "hydra"
        );
        assert_eq!(claude_project_key_label("/home/u/hydra/"), "hydra");
        assert_eq!(claude_project_key_label("C:\\Users\\u\\hydra"), "hydra");
        assert_eq!(claude_project_key_label(""), "Unknown");
        assert_eq!(claude_project_key_label("/"), "Unknown");
    }
}
