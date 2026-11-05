//! Renderer log events (`window.hydra.logEvent`) in the GUI's own log file.
//!
//! The front end sends structured events (`ObservabilityLogEventPayload`: level, event name, optional message, ids and `meta`). They are
//! written through `tracing` (target `renderer`) into `<data>/logs/agent-studio-gui.<date>` as JSON lines, next to the daemon's log, and
//! so end up in the diagnostics bundle. Nothing here may fail or panic: the front end sends fire-and-forget and the input is untrusted.
//! **No secrets**: values under keys that look like credentials are replaced, obvious `token=...` text is scrubbed, everything is capped.

use serde_json::Value;

const REDACTED: &str = "<redacted>";
const MAX_FIELD_CHARS: usize = 300;
const MAX_MESSAGE_CHARS: usize = 2000;
const MAX_META_CHARS: usize = 4000;
const MAX_DEPTH: usize = 6;
/// Substrings (lowercase) of a key that make its value a secret.
const SECRET_KEYS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "api_key",
    "api-key",
    "authorization",
    "cookie",
    "credential",
    "bearer",
    "private",
    "session_key",
];
/// Words that, when followed by `=`, `:` or a space, make the next word a secret in free text.
const SECRET_WORDS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "api_key",
    "bearer",
    "authorization",
    "sk-ant-",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

/// A renderer event after validation and scrubbing.
#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    pub level: Level,
    pub event: String,
    pub message: String,
    pub trace_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub project_id: String,
    pub service: String,
    /// JSON text of the scrubbed `meta` object (empty when there is none).
    pub meta: String,
}

fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SECRET_KEYS.iter().any(|s| key.contains(s))
}

fn cap(text: &str, max: usize) -> String {
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() && c != ' ' { ' ' } else { c })
        .collect();
    if clean.chars().count() <= max {
        return clean;
    }
    let mut out: String = clean.chars().take(max).collect();
    out.push('…');
    out
}

/// Replace the word after a secret-looking word (`token abc`, `password=abc`, `Bearer abc`) and any `sk-ant-...` key.
pub fn scrub_text(text: &str) -> String {
    let mut out = Vec::new();
    let mut hide_next = false;
    for word in text.split(' ') {
        let lower = word.to_ascii_lowercase();
        if hide_next && !word.is_empty() {
            // `Authorization: Bearer abc`: the scheme word is kept, the credential after it is not.
            if SECRET_WORDS.contains(&lower.trim_end_matches([':', '=']).trim()) {
                out.push(word.to_string());
            } else {
                out.push(REDACTED.to_string());
                hide_next = false;
            }
            continue;
        }
        if let Some(at) = lower.find("sk-ant-") {
            out.push(format!("{}{REDACTED}", &word[..at]));
            continue;
        }
        let mut handled = false;
        for secret in SECRET_WORDS {
            if let Some(at) = lower.find(secret) {
                let rest = &word[at + secret.len()..];
                if let Some(sep) = rest.find(['=', ':'])
                    && sep == 0
                    && rest.len() > 1
                {
                    out.push(format!(
                        "{}{}{REDACTED}",
                        &word[..at + secret.len()],
                        &rest[..1]
                    ));
                    handled = true;
                } else if rest.is_empty() || rest == ":" || rest == "=" {
                    hide_next = true;
                    out.push(word.to_string());
                    handled = true;
                }
                break;
            }
        }
        if !handled {
            out.push(word.to_string());
        }
    }
    out.join(" ")
}

fn scrub_value(value: &Value, depth: usize) -> Value {
    match value {
        Value::String(s) => Value::String(cap(&scrub_text(s), MAX_FIELD_CHARS)),
        Value::Array(items) if depth < MAX_DEPTH => Value::Array(
            items
                .iter()
                .take(50)
                .map(|v| scrub_value(v, depth + 1))
                .collect(),
        ),
        Value::Object(map) if depth < MAX_DEPTH => Value::Object(
            map.iter()
                .take(50)
                .map(|(k, v)| {
                    let v = if is_secret_key(k) {
                        Value::String(REDACTED.into())
                    } else {
                        scrub_value(v, depth + 1)
                    };
                    (cap(k, 80), v)
                })
                .collect(),
        ),
        Value::Array(_) | Value::Object(_) => Value::String("…".into()),
        other => other.clone(),
    }
}

fn text_field(event: &Value, key: &str, max: usize) -> String {
    event
        .get(key)
        .and_then(Value::as_str)
        .map(|s| cap(&scrub_text(s), max))
        .unwrap_or_default()
}

/// Validate and scrub one event; `None` when it is not an object with an event name.
pub fn sanitize(event: &Value) -> Option<LogLine> {
    let object = event.as_object()?;
    let name = text_field(event, "event", MAX_FIELD_CHARS);
    if name.is_empty() {
        return None;
    }
    let level = match object.get("level").and_then(Value::as_str) {
        Some("debug") => Level::Debug,
        Some("warn") => Level::Warn,
        Some("error") => Level::Error,
        _ => Level::Info,
    };
    let meta = match object.get("meta") {
        Some(meta @ Value::Object(_)) => {
            let scrubbed = scrub_value(meta, 0).to_string();
            cap(&scrubbed, MAX_META_CHARS)
        }
        _ => String::new(),
    };
    Some(LogLine {
        level,
        event: name,
        message: text_field(event, "message", MAX_MESSAGE_CHARS),
        trace_id: text_field(event, "traceId", MAX_FIELD_CHARS),
        agent_id: text_field(event, "agentId", MAX_FIELD_CHARS),
        session_id: text_field(event, "sessionId", MAX_FIELD_CHARS),
        project_id: text_field(event, "projectId", MAX_FIELD_CHARS),
        service: text_field(event, "service", 40),
        meta,
    })
}

/// Write the event to the log (no-op for input that is not an event).
pub fn record(event: &Value) {
    let Some(line) = sanitize(event) else {
        return;
    };
    macro_rules! emit {
        ($level:expr) => {
            tracing::event!(
                target: "renderer",
                $level,
                event = %line.event,
                message = %line.message,
                trace_id = %line.trace_id,
                agent_id = %line.agent_id,
                session_id = %line.session_id,
                project_id = %line.project_id,
                service = %line.service,
                meta = %line.meta,
            )
        };
    }
    match line.level {
        Level::Debug => emit!(tracing::Level::DEBUG),
        Level::Info => emit!(tracing::Level::INFO),
        Level::Warn => emit!(tracing::Level::WARN),
        Level::Error => emit!(tracing::Level::ERROR),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[test]
    fn keeps_ordinary_events() {
        let line = sanitize(&json!({
            "level": "warn", "event": "renderer.agent.restart", "traceId": "t-1", "agentId": "a1",
            "message": "went fine", "meta": { "model": "sonnet", "yolo": false, "n": 3 }
        }))
        .unwrap();
        assert_eq!(line.level, Level::Warn);
        assert_eq!(line.event, "renderer.agent.restart");
        assert_eq!(
            (line.trace_id.as_str(), line.agent_id.as_str()),
            ("t-1", "a1")
        );
        let meta: Value = serde_json::from_str(&line.meta).unwrap();
        assert_eq!(meta, json!({ "model": "sonnet", "yolo": false, "n": 3 }));
    }

    #[test]
    fn rejects_what_is_not_an_event() {
        assert!(sanitize(&json!("hi")).is_none());
        assert!(sanitize(&json!({ "level": "info" })).is_none());
        assert!(sanitize(&json!({ "event": "" })).is_none());
        // A level we do not know is info, never a failure.
        assert_eq!(
            sanitize(&json!({ "event": "x", "level": 9 }))
                .unwrap()
                .level,
            Level::Info
        );
    }

    #[test]
    fn secrets_under_credential_keys_are_replaced_at_any_depth() {
        let line = sanitize(&json!({
            "event": "x",
            "meta": { "apiToken": "abc", "nested": { "Authorization": "Bearer abc", "ok": 1 }, "list": [{ "password": "p" }] }
        }))
        .unwrap();
        assert!(
            !line.meta.contains("abc") && !line.meta.contains("\"p\""),
            "{}",
            line.meta
        );
        assert!(line.meta.contains("<redacted>") && line.meta.contains("\"ok\":1"));
    }

    #[test]
    fn secrets_in_free_text_are_scrubbed() {
        for text in [
            "failed with token=abc123 on retry",
            "request had Authorization: Bearer abc123 attached",
            "password: abc123",
            "key sk-ant-api03-abc123 leaked",
            "using api_key=abc123",
        ] {
            let out = scrub_text(text);
            assert!(!out.contains("abc123"), "{text} -> {out}");
        }
        assert_eq!(scrub_text("nothing to hide here"), "nothing to hide here");
        let line = sanitize(&json!({ "event": "e", "message": "oops token=hunter2" })).unwrap();
        assert!(!line.message.contains("hunter2"), "{}", line.message);
    }

    #[test]
    fn values_are_capped_and_control_characters_removed() {
        let long = "x".repeat(10_000);
        let line = sanitize(
            &json!({ "event": "e\nvil", "message": long, "meta": { "big": "y".repeat(10_000) } }),
        )
        .unwrap();
        assert!(!line.event.contains('\n'));
        assert!(line.message.chars().count() <= MAX_MESSAGE_CHARS + 1);
        assert!(line.meta.chars().count() <= MAX_META_CHARS + 1);
        let deep = json!({ "event": "e", "meta": { "a": { "b": { "c": { "d": { "e": { "f": { "g": { "h": 1 } } } } } } } } });
        assert!(sanitize(&deep).unwrap().meta.contains('…'));
    }

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Sink {
        type Writer = Sink;
        fn make_writer(&'a self) -> Sink {
            self.clone()
        }
    }

    #[test]
    fn events_become_json_lines_with_their_fields() {
        let sink = Sink::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(sink.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            record(
                &json!({ "level": "error", "event": "renderer.crash", "message": "boom", "agentId": "a9", "meta": { "token": "zzz" } }),
            );
            record(&json!(42)); // ignored
        });
        let text = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        let line: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(line["level"], "ERROR");
        assert_eq!(line["target"], "renderer");
        assert_eq!(line["fields"]["event"], "renderer.crash");
        assert_eq!(line["fields"]["agent_id"], "a9");
        assert!(!text.contains("zzz"), "{text}");
    }
}
