use serde_json::{Value, json};

use super::*;

const BASE: i64 = 1_779_192_000_000; // 2026-05-19T12:00:00Z

fn stamp(s: i64) -> String {
    chrono::DateTime::from_timestamp_millis(BASE + s * 1000)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn row(s: i64, ty: &str, payload: Value) -> String {
    json!({"timestamp": stamp(s), "type": ty, "payload": payload}).to_string()
}
fn count_with(s: i64, last: Value, total: Value, extra: Value) -> String {
    let mut p = json!({"type": "token_count", "info": {"last_token_usage": last, "total_token_usage": total}});
    if let Some(e) = extra.as_object() {
        for (k, v) in e {
            p[k] = v.clone();
        }
    }
    row(s, "event_msg", p)
}
fn count(s: i64, last: Value, total: Value) -> String {
    count_with(s, last, total, json!({}))
}
fn u(input: i64, cached: i64, output: i64, reasoning: i64) -> Value {
    json!({
        "input_tokens": input, "cached_input_tokens": cached, "output_tokens": output,
        "reasoning_output_tokens": reasoning, "total_tokens": input + output
    })
}
fn u3(input: i64, cached: i64, output: i64) -> Value {
    u(input, cached, output, 0)
}

struct Run {
    state: CodexFileState,
    results: Vec<CodexLineResult>,
    usage: Vec<CodexParsedUsage>,
}

fn feed_into(lines: &[String], mut state: CodexFileState) -> Run {
    let mut results = Vec::new();
    let mut usage = Vec::new();
    for l in lines {
        if let Some(r) = parse_codex_line(l, &mut state) {
            usage.extend(r.usage.clone().unwrap_or_default());
            results.push(r);
        }
    }
    Run {
        state,
        results,
        usage,
    }
}
fn feed(lines: &[String]) -> Run {
    feed_into(lines, CodexFileState::new())
}

fn meta(id: &str) -> String {
    row(0, "session_meta", json!({"id": id, "cwd": "/w/proj"}))
}
fn meta_with(id: &str, extra: Value) -> String {
    let mut p = json!({"id": id, "cwd": "/w/proj"});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    row(0, "session_meta", p)
}
fn ctx(s: i64, model: &str, turn: &str) -> String {
    row(s, "turn_context", json!({"turn_id": turn, "model": model}))
}

// ── usage ───────────────────────────────────────────────────────────────────

#[test]
fn does_not_double_count_cached_input_and_does_not_add_reasoning() {
    let run = feed(&[
        meta("s1"),
        ctx(1, "gpt-5-codex", "t1"),
        count(2, u(1000, 400, 50, 10), u(1000, 400, 50, 10)),
    ]);
    assert_eq!(run.usage.len(), 1);
    assert_eq!(
        run.usage[0].record,
        UsageRecord {
            ts: BASE + 2000,
            provider: UsageProvider::Codex,
            model: "gpt-5-codex".into(),
            project_key: "/w/proj".into(),
            tokens: TokenCounts {
                input: 600,
                output: 50,
                cache_write5m: 0,
                cache_write1h: 0,
                cache_read: 400
            },
        }
    );
}

#[test]
fn uses_the_latest_turn_context_model_and_the_default_before_any() {
    let run = feed(&[
        meta("s1"),
        count(1, u3(10, 0, 1), u3(10, 0, 1)),
        ctx(2, "GPT-5-Codex", "a"),
        count(3, u3(20, 0, 2), u3(30, 0, 3)),
        ctx(4, "gpt-5-mini", "b"),
        count(5, u3(30, 0, 3), u3(60, 0, 6)),
    ]);
    let models: Vec<_> = run.usage.iter().map(|x| x.record.model.as_str()).collect();
    assert_eq!(models, [CODEX_DEFAULT_MODEL, "gpt-5-codex", "gpt-5-mini"]);
    assert_eq!(
        run.state.turn_models,
        vec![
            ("a".into(), "GPT-5-Codex".into()),
            ("b".into(), "gpt-5-mini".into())
        ]
    );
}

#[test]
fn skips_absent_empty_and_all_zero_last_token_usage() {
    let run = feed(&[
        meta("s1"),
        row(1, "event_msg", json!({"type": "token_count", "info": null})),
        count(2, json!({}), u3(5, 0, 1)),
        count(3, u3(0, 0, 0), u3(5, 0, 1)),
        row(4, "event_msg", json!({"type": "agent_message"})),
    ]);
    assert!(run.usage.is_empty());
}

#[test]
fn replayed_events_in_a_resumed_file_get_the_same_dedupe_key() {
    let original = feed(&[
        meta("s1"),
        ctx(1, "m", "t1"),
        count(2, u3(100, 10, 5), u3(100, 10, 5)),
        count(3, u3(50, 0, 5), u3(150, 10, 10)),
    ]);
    let resumed = feed(&[
        meta("s1"),
        ctx(100, "m", "t1"),
        count(101, u3(100, 10, 5), u3(100, 10, 5)),
        count(102, u3(50, 0, 5), u3(150, 10, 10)),
        count(103, u3(70, 0, 5), u3(220, 10, 15)),
    ]);
    assert!(
        original.usage[0]
            .dedupe_key
            .as_ref()
            .unwrap()
            .starts_with("codex-token-v1:s1|")
    );
    assert_eq!(resumed.usage[0].dedupe_key, original.usage[0].dedupe_key);
    assert_eq!(resumed.usage[1].dedupe_key, original.usage[1].dedupe_key);
    assert_ne!(resumed.usage[2].dedupe_key, original.usage[1].dedupe_key);
    assert_ne!(resumed.usage[0].record.ts, original.usage[0].record.ts);
}

#[test]
fn missing_and_zero_counters_are_alike_in_the_key() {
    let a = feed(&[
        meta("s1"),
        count(
            1,
            json!({"input_tokens": 5, "output_tokens": 1}),
            json!({"input_tokens": 5, "output_tokens": 1}),
        ),
    ]);
    let b = feed(&[meta("s1"), count(1, u(5, 0, 1, 0), u(5, 0, 1, 0))]);
    assert_eq!(a.usage[0].dedupe_key, b.usage[0].dedupe_key);
}

#[test]
fn null_key_when_total_last_or_the_session_id_is_missing() {
    let no_total = feed(&[
        meta("s1"),
        row(
            1,
            "event_msg",
            json!({"type": "token_count", "info": {"last_token_usage": u3(5, 0, 1)}}),
        ),
    ]);
    assert_eq!(no_total.usage.len(), 1);
    assert_eq!(no_total.usage[0].dedupe_key, None);
    let no_session = feed(&[count(1, u3(5, 0, 1), u3(5, 0, 1))]);
    assert_eq!(no_session.usage[0].dedupe_key, None);
    assert_eq!(no_session.usage[0].record.project_key, "");
}

#[test]
fn drops_keyless_events_replayed_from_a_parent_in_a_fork_file() {
    let fork = feed(&[
        meta_with("child", json!({"forked_from_id": "parent"})),
        meta("parent"),
        row(
            1,
            "event_msg",
            json!({"type": "token_count", "info": {"last_token_usage": u3(5, 0, 1)}}),
        ),
        count(2, u3(7, 0, 1), u3(7, 0, 1)),
    ]);
    // Replayed parent history: the keyless row is dropped, the keyed one is scoped to the parent.
    assert_eq!(fork.usage.len(), 1);
    assert!(
        fork.usage[0]
            .dedupe_key
            .as_ref()
            .unwrap()
            .starts_with("codex-token-v1:parent|")
    );
    let parent_file = feed(&[meta("parent"), count(2, u3(7, 0, 1), u3(7, 0, 1))]);
    assert_eq!(parent_file.usage[0].dedupe_key, fork.usage[0].dedupe_key);
}

#[test]
fn tolerates_malformed_lines() {
    let mut state = CodexFileState::new();
    for l in [
        "",
        "not json",
        "[]",
        "\"x\"",
        "null",
        r#"{"type":"event_msg","payload":5}"#,
        r#"{"type":"event_msg","payload":{"type":"token_count"}}"#,
        r#"{"timestamp":"nope","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":5}}}}"#,
    ] {
        assert!(parse_codex_line(l, &mut state).is_none(), "{l}");
    }
    assert!(
        parse_codex_line(
            &count(1, json!({"input_tokens": "x"}), json!({"input_tokens": 1})),
            &mut state
        )
        .is_none()
    );
}

#[test]
fn survives_a_json_round_trip_of_the_state_mid_file() {
    let lines = [
        meta_with("s1", json!({"forked_from_id": "p"})),
        ctx(1, "gpt-5-codex", "t1"),
        count(2, u3(10, 0, 1), u3(10, 0, 1)),
        count(3, u3(20, 5, 2), u3(30, 5, 3)),
    ];
    let whole = feed(&lines);
    let first = feed(&lines[..3]);
    let restored: CodexFileState =
        serde_json::from_str(&serde_json::to_string(&first.state).unwrap()).unwrap();
    let second = feed_into(&lines[3..], restored);
    let mut both = first.usage.clone();
    both.extend(second.usage.clone());
    assert_eq!(both, whole.usage);
    assert_eq!(second.state, whole.state);
}

// ── limits ──────────────────────────────────────────────────────────────────

fn rl(extra: Value, legacy: bool) -> String {
    if legacy {
        row(
            10,
            "event_msg",
            json!({"type": "token_count", "info": {"rate_limits": extra}}),
        )
    } else {
        row(
            10,
            "event_msg",
            json!({"type": "token_count", "info": null, "rate_limits": extra}),
        )
    }
}
fn limits(line: String) -> Option<CodexParsedLimits> {
    feed(&[line])
        .results
        .into_iter()
        .next()
        .and_then(|r| r.limits)
}
fn lim(extra: Value) -> Option<CodexParsedLimits> {
    limits(rl(extra, false))
}
fn win(l: &CodexParsedLimits) -> Vec<(String, f64)> {
    l.windows
        .iter()
        .map(|w| (w.id.clone(), w.used_percent))
        .collect()
}

#[test]
fn reads_rate_limits_from_the_payload_and_from_info_legacy() {
    let body = json!({"plan_type": "plus", "account_id": "acc-1", "primary": {"used_percent": 12.5, "window_minutes": 300, "resets_at": 1779200000}});
    for legacy in [false, true] {
        let l = limits(rl(body.clone(), legacy)).unwrap();
        assert_eq!(
            (l.at, l.account.as_str(), l.plan.as_deref()),
            (BASE + 10_000, "acc-1", Some("plus"))
        );
        assert_eq!(
            l.windows,
            vec![LimitWindow {
                provider: UsageProvider::Codex,
                account: "acc-1".into(),
                id: "session".into(),
                kind: LimitWindowKind::Session,
                label: "5-hour window".into(),
                used_percent: 12.5,
                resets_at: Some(1_779_200_000_000),
            }]
        );
    }
}

#[test]
fn classifies_300_and_10080_minutes_whatever_the_position() {
    let l = lim(json!({"primary": {"used_percent": 40, "window_minutes": 10080, "resets_at": 2e9}, "secondary": {"used_percent": 10, "window_minutes": 300, "resets_at": 1.9e9}})).unwrap();
    let got: Vec<_> = l
        .windows
        .iter()
        .map(|w| (w.id.as_str(), w.kind, w.label.as_str(), w.used_percent))
        .collect();
    assert_eq!(
        got,
        [
            ("weekly", LimitWindowKind::Weekly, "Weekly", 40.0),
            ("session", LimitWindowKind::Session, "5-hour window", 10.0)
        ]
    );
    assert_eq!(l.account, "default");
    assert_eq!(l.plan, None);
}

#[test]
fn primary_session_and_secondary_weekly_for_a_pair_without_minutes() {
    let l = lim(json!({"primary": {"used_percent": 20, "resets_at": 2e9}, "secondary": {"used_percent": 60, "resets_at": 2e9}})).unwrap();
    assert_eq!(
        win(&l),
        [("session".to_string(), 20.0), ("weekly".to_string(), 60.0)]
    );
}

#[test]
fn infers_the_missing_side_from_one_recognised_duration() {
    let l = lim(json!({"primary": {"used_percent": 20, "resets_at": 2e9}, "secondary": {"used_percent": 60, "window_minutes": 300, "resets_at": 2e9}})).unwrap();
    assert_eq!(
        win(&l),
        [("weekly".to_string(), 20.0), ("session".to_string(), 60.0)]
    );
}

#[test]
fn a_single_window_is_weekly_unless_recognised_as_5_hour() {
    let ids = |v: Value| {
        lim(v)
            .unwrap()
            .windows
            .iter()
            .map(|w| w.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(json!({"primary": {"used_percent": 30, "resets_at": 2e9}})),
        ["weekly"]
    );
    assert_eq!(
        ids(json!({"secondary": {"used_percent": 30, "resets_at": 2e9}})),
        ["weekly"]
    );
    assert_eq!(
        ids(json!({"primary": {"used_percent": 30, "window_minutes": 300, "resets_at": 2e9}})),
        ["session"]
    );
}

#[test]
fn keeps_other_durations_as_their_own_window() {
    let l = lim(json!({"primary": {"used_percent": 5, "window_minutes": 1440, "resets_at": 2e9}, "secondary": {"used_percent": 6, "window_minutes": 10080, "resets_at": 2e9}})).unwrap();
    let got: Vec<_> = l
        .windows
        .iter()
        .map(|w| (w.id.as_str(), w.kind, w.label.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("weekly", LimitWindowKind::Weekly, "Weekly"),
            ("window:1440", LimitWindowKind::Other, "1-day window")
        ]
    );
}

#[test]
fn parses_resets_at_in_every_form() {
    let at = |w: Value| -> Option<Option<i64>> {
        let mut win = json!({"used_percent": 50, "window_minutes": 300});
        for (k, v) in w.as_object().unwrap() {
            win[k] = v.clone();
        }
        lim(json!({"primary": win})).map(|l| l.windows[0].resets_at)
    };
    assert_eq!(
        at(json!({"resets_at": "2026-05-19T15:00:00Z"})),
        Some(Some(1_779_202_800_000))
    );
    assert_eq!(
        at(json!({"resets_at": 1779200000})),
        Some(Some(1_779_200_000_000))
    );
    assert_eq!(
        at(json!({"resets_at": 1779200000123_i64})),
        Some(Some(1_779_200_000_123))
    );
    assert_eq!(
        at(json!({"resets_at": "1779200000"})),
        Some(Some(1_779_200_000_000))
    );
    assert_eq!(
        at(json!({"resets_in_seconds": 600})),
        Some(Some(BASE + 10_000 + 600_000))
    );
    assert_eq!(at(json!({})), Some(None));
    assert_eq!(at(json!({"resets_at": "garbage"})), Some(None));
}

#[test]
fn clamps_percentages_does_not_scale_fractions_and_nulls_the_reset_of_an_idle_window() {
    let l = lim(json!({"primary": {"used_percent": 0.5, "window_minutes": 300, "resets_at": 2e9}, "secondary": {"used_percent": 130, "window_minutes": 10080, "resets_at": 2e9}})).unwrap();
    assert_eq!(
        l.windows.iter().map(|w| w.used_percent).collect::<Vec<_>>(),
        [0.5, 100.0]
    );
    let idle =
        lim(json!({"primary": {"used_percent": 0, "window_minutes": 300, "resets_at": 2e9}}))
            .unwrap();
    assert_eq!(
        (idle.windows[0].used_percent, idle.windows[0].resets_at),
        (0.0, None)
    );
}

#[test]
fn ignores_missing_empty_or_percent_less_rate_limits_and_keeps_usage() {
    assert!(limits(count(1, u3(5, 0, 1), u3(5, 0, 1))).is_none());
    assert!(lim(json!({})).is_none());
    assert!(lim(json!({"primary": {"window_minutes": 300}})).is_none());
    assert!(lim(json!({"primary": null, "secondary": "x"})).is_none());
    let both = feed(&[
        meta("s1"),
        count_with(
            1,
            u3(5, 0, 1),
            u3(5, 0, 1),
            json!({"rate_limits": {"primary": {"used_percent": 1, "resets_at": 2e9}}}),
        ),
    ]);
    assert_eq!(both.results[0].usage.as_ref().unwrap().len(), 1);
    assert_eq!(both.results[0].limits.as_ref().unwrap().windows.len(), 1);
}

// ── compaction ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Uncarried,
    Carried,
    CarriedAfter,
    Unrecorded,
}

fn add(us: &[Value]) -> Value {
    let mut out = serde_json::Map::new();
    for k in [
        "input_tokens",
        "cached_input_tokens",
        "output_tokens",
        "reasoning_output_tokens",
        "total_tokens",
    ] {
        out.insert(
            k.into(),
            json!(us.iter().map(|u| u[k].as_i64().unwrap()).sum::<i64>()),
        );
    }
    Value::Object(out)
}

fn before() -> Value {
    u(1000, 400, 50, 10)
}
fn compaction() -> Value {
    u(5000, 3000, 700, 0)
}
fn after() -> Value {
    u(200, 100, 30, 5)
}

fn rollout(shape: Shape, id: &str, at: i64) -> Vec<String> {
    let omitted = matches!(shape, Shape::Uncarried | Shape::Unrecorded);
    let thread = add(&[before(), compaction()]);
    let record = json!({"response_id": "resp-compact", "turn_id": "turn-1", "usage": compaction(), "thread_token_usage": thread});
    let mut rows = vec![
        row(at, "session_meta", json!({"id": id, "cwd": "/w"})),
        row(
            at + 1,
            "turn_context",
            json!({"turn_id": "turn-1", "model": "gpt-5-codex"}),
        ),
        count(at + 2, before(), add(&[before()])),
    ];
    if shape != Shape::Unrecorded {
        rows.push(row(at + 3, "token_usage_record", record.clone()));
    }
    if shape == Shape::Carried {
        rows.push(count(at + 3, compaction(), thread.clone()));
    }
    rows.push(row(
        at + 3,
        "compacted",
        if shape == Shape::Unrecorded {
            json!({})
        } else {
            json!({"compaction_response_id": "resp-compact", "latest_token_usage_record": record})
        },
    ));
    rows.push(row(
        at + 4,
        "turn_context",
        json!({"turn_id": "turn-1", "model": "gpt-5-codex"}),
    ));
    if shape == Shape::CarriedAfter {
        rows.push(count(at + 3, compaction(), thread.clone()));
    } else if omitted {
        rows.push(count(at + 5, u(0, 0, 0, 0), add(&[before()])));
    }
    rows.push(count(
        at + 6,
        after(),
        if omitted {
            add(&[before(), after()])
        } else {
            add(&[before(), compaction(), after()])
        },
    ));
    rows
}

fn sum(us: &[CodexParsedUsage]) -> [u64; 3] {
    us.iter().fold([0; 3], |a, u| {
        [
            a[0] + u.record.tokens.input,
            a[1] + u.record.tokens.cache_read,
            a[2] + u.record.tokens.output,
        ]
    })
}
const SINGLE: [u64; 3] = [600 + 2000 + 100, 400 + 3000 + 100, 50 + 700 + 30];

#[test]
fn bills_an_uncarried_compaction_exactly_once() {
    let mut run = feed(&rollout(Shape::Uncarried, "s1", 0));
    assert_eq!(run.usage.len(), 2);
    let flushed = flush_codex_compactions(&mut run.state);
    assert_eq!(flushed.len(), 1);
    let r = &flushed[0].record;
    assert_eq!(
        (r.ts, r.model.as_str(), r.project_key.as_str()),
        (BASE + 3000, "gpt-5-codex", "/w")
    );
    assert_eq!(
        (r.tokens.input, r.tokens.cache_read, r.tokens.output),
        (2000, 3000, 700)
    );
    assert!(
        flushed[0]
            .dedupe_key
            .as_ref()
            .unwrap()
            .starts_with("codex-compaction-v1:s1|")
    );
    let mut all = run.usage.clone();
    all.extend(flushed);
    assert_eq!(sum(&all), SINGLE);
    assert!(flush_codex_compactions(&mut run.state).is_empty());
}

#[test]
fn does_not_bill_a_compaction_a_token_count_carries() {
    for shape in [Shape::Carried, Shape::CarriedAfter] {
        let mut run = feed(&rollout(shape, "s1", 0));
        assert_eq!(run.usage.len(), 3);
        assert!(flush_codex_compactions(&mut run.state).is_empty());
        assert_eq!(sum(&run.usage), SINGLE);
    }
}

#[test]
fn bills_a_compaction_without_a_token_usage_record_when_compacted_inlines_it() {
    let lines: Vec<String> = rollout(Shape::Uncarried, "s1", 0)
        .into_iter()
        .filter(|l| !l.contains("\"token_usage_record\""))
        .collect();
    let mut run = feed(&lines);
    assert_eq!(flush_codex_compactions(&mut run.state).len(), 1);
}

#[test]
fn ignores_a_compaction_the_log_cannot_identify() {
    let mut run = feed(&rollout(Shape::Unrecorded, "s1", 0));
    assert!(flush_codex_compactions(&mut run.state).is_empty());
}

#[test]
fn does_not_repeat_a_flushed_compaction_after_more_lines_across_a_state_round_trip() {
    let lines = rollout(Shape::Uncarried, "s1", 0);
    let cut = lines
        .iter()
        .position(|l| l.contains("\"compacted\""))
        .unwrap()
        + 1;
    let mut first = feed(&lines[..cut]);
    let flushed1 = flush_codex_compactions(&mut first.state);
    assert_eq!(flushed1.len(), 1);
    let restored: CodexFileState =
        serde_json::from_str(&serde_json::to_string(&first.state).unwrap()).unwrap();
    let mut second = feed_into(&lines[cut..], restored);
    assert!(flush_codex_compactions(&mut second.state).is_empty());
    let mut all = first.usage.clone();
    all.extend(second.usage.clone());
    all.extend(flushed1);
    assert_eq!(sum(&all), SINGLE);
}

#[test]
fn collapses_a_resumed_history_copy_against_the_original_by_key() {
    let a = flush_codex_compactions(&mut feed(&rollout(Shape::Uncarried, "s1", 0)).state);
    let b = flush_codex_compactions(&mut feed(&rollout(Shape::Uncarried, "s1", 500)).state);
    assert!(a[0].dedupe_key.is_some());
    assert_eq!(b[0].dedupe_key, a[0].dedupe_key);
    assert_ne!(b[0].record.ts, a[0].record.ts);
}

#[test]
fn a_carried_compaction_is_billed_only_through_its_token_count() {
    let mut lines = rollout(Shape::Carried, "s1", 0);
    lines.push(count(
        20,
        compaction(),
        add(&[before(), compaction(), after(), compaction()]),
    ));
    let mut run = feed(&lines);
    assert!(flush_codex_compactions(&mut run.state).is_empty());
}
