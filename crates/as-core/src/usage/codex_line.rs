// Adapted from Tokdash (MIT, © JingbiaoMei): src/tokdash/sources/coding_tools.py
// (codex_token_event_key, CodexCompactionTracker, codex_replay_key_session_id,
// CodexParser._parse_all), src/tokdash/sources/quota/codex.py and
// src/tokdash/codex_quota_windows.py. Port of Hydra `shared/usage/codexLine.ts`.
//! Codex rollout lines -> usage records and passive plan limits. One [`CodexFileState`] per rollout
//! file, plain data (serde) so a scan can stop and resume at a byte offset.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::pricing::normalize_model;
use super::types::{LimitWindow, LimitWindowKind, TokenCounts, UsageProvider, UsageRecord};

/// Tokdash `CODEX_DEFAULT_MODEL`: used until the log names a model.
pub const CODEX_DEFAULT_MODEL: &str = "gpt-5.3-codex";

const MAX_TURN_MODELS: usize = 256;

type Obj = Map<String, Value>;

/// A remote compaction no `token_count` has carried (yet).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct PendingCompaction {
    /// Counters of the compaction request, in `COUNTER_FIELDS` order.
    usage: Vec<i64>,
    thread_usage: Option<Vec<i64>>,
    model: Option<String>,
    ts: i64,
    project_key: String,
    /// Session id the dedupe key is scoped to; `None` when unknown.
    key_scope: Option<String>,
    replay_fallback: bool,
}

/// The parts of a `token_usage_record` a compaction needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct CompactionRecord {
    response_id: Option<String>,
    turn_id: Option<String>,
    usage: Option<Vec<i64>>,
    thread_usage: Option<Vec<i64>>,
}

/// Parser state for one rollout file.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct CodexFileState {
    /// The file's own session id (first `session_meta`).
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// Turn id -> model, oldest first, at most 256.
    pub turn_models: Vec<(String, String)>,
    /// Latest cwd from `session_meta` / `turn_context`.
    pub cwd: String,
    /// Session id of the latest `session_meta` (a fork replays its parent's).
    pub current_session_id: Option<String>,
    pub subagent_parent_id: Option<String>,
    pub is_fork_file: bool,
    pub saw_turn_context: bool,
    /// Latest `token_usage_record`.
    pub last_record: Option<CompactionRecord>,
    pub last_record_carried: bool,
    pub pending_compactions: Vec<PendingCompaction>,
}

impl CodexFileState {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexParsedUsage {
    pub record: UsageRecord,
    /// Stable identity across resumed/forked files, `None` when unknown (the caller counts it).
    pub dedupe_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct CodexParsedLimits {
    /// Epoch ms of the line.
    pub at: i64,
    pub account: String,
    pub plan: Option<String>,
    pub windows: Vec<LimitWindow>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CodexLineResult {
    pub usage: Option<Vec<CodexParsedUsage>>,
    pub limits: Option<CodexParsedLimits>,
}

fn str_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => n.to_string(),
        },
        _ => String::new(),
    }
}

fn some_str(v: Option<&Value>) -> Option<String> {
    Some(str_of(v)).filter(|s| !s.is_empty())
}

const COUNTER_FIELDS: [&str; 5] = [
    "input_tokens",
    "cached_input_tokens",
    "cache_write_input_tokens",
    "output_tokens",
    "reasoning_output_tokens",
];
const IN: usize = 0;
const CACHED: usize = 1;
const OUT: usize = 3;

fn is_int_string(s: &str) -> bool {
    let s = s.trim();
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Counters in `COUNTER_FIELDS` order; missing = 0; `None` when a field is not a number.
fn counters(snapshot: &Obj) -> Option<Vec<i64>> {
    let mut out = Vec::with_capacity(COUNTER_FIELDS.len());
    for f in COUNTER_FIELDS {
        match snapshot.get(f) {
            None | Some(Value::Null) => out.push(0),
            Some(Value::Number(n)) => {
                let x = n.as_f64().filter(|x| x.is_finite())?;
                out.push(x.trunc() as i64);
            }
            Some(Value::String(s)) if is_int_string(s) => out.push(s.trim().parse().ok()?),
            Some(_) => return None,
        }
    }
    Some(out)
}

fn non_empty_obj(v: Option<&Value>) -> Option<&Obj> {
    v.and_then(Value::as_object).filter(|o| !o.is_empty())
}

fn join(c: &[i64]) -> String {
    c.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// Port of `codex_token_event_key`. Ignores timestamps so replays collapse.
fn token_event_key(session_id: Option<&str>, info: &Obj) -> Option<String> {
    let sid = session_id.filter(|s| !s.is_empty())?;
    let total = non_empty_obj(info.get("total_token_usage"))?;
    let last = non_empty_obj(info.get("last_token_usage"))?;
    let t = counters(total)?;
    let l = counters(last)?;
    Some(format!("codex-token-v1:{sid}|{}|{}", join(&t), join(&l)))
}

/// Port of `codex_compaction_event_key`.
fn compaction_event_key(
    session_id: Option<&str>,
    usage: &[i64],
    thread: Option<&[i64]>,
) -> Option<String> {
    let sid = session_id.filter(|s| !s.is_empty())?;
    let thread = thread?;
    Some(format!(
        "codex-compaction-v1:{sid}|{}|{}",
        join(thread),
        join(usage)
    ))
}

/// `None` for a missing or all-zero snapshot (Tokdash `_signature`).
fn signature(usage: Option<&Value>) -> Option<Vec<i64>> {
    let c = counters(usage?.as_object()?)?;
    c.iter().any(|n| *n != 0).then_some(c)
}

/// Port of `codex_fork_ancestry` (only the declared parent id is needed here).
fn fork_ancestry(p: &Obj) -> (bool, Option<String>) {
    let own_id = str_of(p.get("id"));
    let spawn = p
        .get("source")
        .and_then(Value::as_object)
        .and_then(|s| s.get("subagent"))
        .and_then(Value::as_object)
        .and_then(|s| s.get("thread_spawn"))
        .and_then(Value::as_object);
    let mut pid = spawn
        .map(|s| str_of(s.get("parent_thread_id")))
        .unwrap_or_default();
    if pid.is_empty() {
        for f in ["forked_from_id", "parent_thread_id"] {
            let c = str_of(p.get(f));
            if !c.is_empty() && c != own_id {
                pid = c;
                break;
            }
        }
    }
    (
        spawn.is_some() || !pid.is_empty(),
        Some(pid).filter(|s| !s.is_empty()),
    )
}

/// Port of `codex_replay_key_session_id`: the session a replayed event is keyed to, so a fork's
/// copied parent history collapses against the parent file.
fn replay_key_scope(state: &CodexFileState) -> (Option<String>, bool) {
    let (own, current) = (&state.session_id, &state.current_session_id);
    let (Some(own), Some(cur)) = (own, current) else {
        return (current.clone(), false);
    };
    if !state.is_fork_file {
        return (current.clone(), false);
    }
    match &state.subagent_parent_id {
        Some(parent) => {
            if cur == parent {
                return (Some(cur.clone()), true);
            }
            if cur == own && !state.saw_turn_context {
                return (Some(parent.clone()), false);
            }
        }
        None => {
            if cur != own {
                return (Some(cur.clone()), true);
            }
        }
    }
    (Some(cur.clone()), false)
}

/// ISO string or epoch (seconds, or ms when > 1e10) to epoch ms.
fn parse_time(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => {
            let x = n.as_f64().filter(|x| x.is_finite())?;
            Some(if x > 1e10 { x } else { x * 1000.0 }.round() as i64)
        }
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                return None;
            }
            if t.chars().all(|c| c.is_ascii_digit()) {
                return parse_time(Some(&Value::from(t.parse::<f64>().ok()?)));
            }
            super::parse_iso_ms(t)
        }
        _ => None,
    }
}

fn build_usage(
    usage: &[i64],
    model: Option<&str>,
    ts: i64,
    project_key: &str,
    dedupe_key: Option<String>,
) -> Option<CodexParsedUsage> {
    // Codex input_tokens INCLUDES cached tokens; reasoning is already part of output_tokens and is
    // not billed separately (as Tokdash).
    let cache_read = usage[CACHED].max(0);
    let input = (usage[IN] - cache_read).max(0);
    let output = usage[OUT].max(0);
    if input == 0 && output == 0 && cache_read == 0 {
        return None;
    }
    let model = model
        .filter(|m| !m.is_empty())
        .unwrap_or(CODEX_DEFAULT_MODEL);
    Some(CodexParsedUsage {
        record: UsageRecord {
            ts,
            provider: UsageProvider::Codex,
            model: normalize_model(model),
            project_key: project_key.to_string(),
            tokens: TokenCounts {
                input: input as u64,
                output: output as u64,
                cache_read: cache_read as u64,
                ..TokenCounts::default()
            },
        },
        dedupe_key,
    })
}

const FIVE_HOURS: f64 = 300.0;
const SEVEN_DAYS: f64 = 10080.0;

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64().filter(|x| x.is_finite()),
        Value::String(s) if !s.trim().is_empty() => {
            s.trim().parse::<f64>().ok().filter(|x| x.is_finite())
        }
        _ => None,
    }
}

fn window_minutes(w: &Obj) -> Option<f64> {
    if let Some(secs) = num(w.get("limit_window_seconds")).or_else(|| num(w.get("window_seconds")))
    {
        return Some(secs / 60.0);
    }
    num(w.get("window_minutes"))
}

fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        x.to_string()
    }
}

fn other_label(minutes: f64) -> String {
    if minutes % 1440.0 == 0.0 {
        format!("{}-day window", fmt_num(minutes / 1440.0))
    } else if minutes % 60.0 == 0.0 {
        format!("{}-hour window", fmt_num(minutes / 60.0))
    } else {
        format!("{}-minute window", fmt_num(minutes.round()))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Slot {
    Session,
    Weekly,
}

/// Port of `classify_codex_api_windows` for windows with no unrecognised explicit duration.
fn classify<'a>(primary: Option<&'a Obj>, secondary: Option<&'a Obj>) -> Vec<(Slot, &'a Obj)> {
    let bucket = |w: &Obj| match window_minutes(w) {
        Some(m) if m == FIVE_HOURS => Some(Slot::Session),
        Some(m) if m == SEVEN_DAYS => Some(Slot::Weekly),
        _ => None,
    };
    let other = |s: Slot| match s {
        Slot::Session => Slot::Weekly,
        Slot::Weekly => Slot::Session,
    };
    match (primary, secondary) {
        (Some(p), None) => vec![(bucket(p).unwrap_or(Slot::Weekly), p)],
        (None, Some(s)) => vec![(bucket(s).unwrap_or(Slot::Weekly), s)],
        (None, None) => Vec::new(),
        (Some(p), Some(s)) => {
            let (pb, sb) = (bucket(p), bucket(s));
            match (pb, sb) {
                (Some(a), Some(b)) if a != b => vec![(a, p), (b, s)],
                (Some(a), None) => vec![(a, p), (other(a), s)],
                (None, Some(b)) => vec![(other(b), p), (b, s)],
                _ => vec![(Slot::Session, p), (Slot::Weekly, s)],
            }
        }
    }
}

fn parse_limits(p: &Obj, info: &Obj, at: i64) -> Option<CodexParsedLimits> {
    let rl =
        non_empty_obj(p.get("rate_limits")).or_else(|| non_empty_obj(info.get("rate_limits")))?;
    let account = some_str(rl.get("account_id")).unwrap_or_else(|| "default".to_string());
    let plan = some_str(rl.get("plan_type"));

    let mut primary = rl.get("primary").and_then(Value::as_object);
    let mut secondary = rl.get("secondary").and_then(Value::as_object);
    let mut others: Vec<(&Obj, String, String)> = Vec::new();
    // A window with an explicit unrecognised duration is its own `other` window.
    for pos in [&mut primary, &mut secondary] {
        let Some(w) = *pos else { continue };
        if let Some(m) = window_minutes(w)
            && m > 0.0
            && m != FIVE_HOURS
            && m != SEVEN_DAYS
        {
            others.push((w, format!("window:{}", fmt_num(m)), other_label(m)));
            *pos = None;
        }
    }
    let classified = classify(primary, secondary);

    let mut windows: Vec<LimitWindow> = Vec::new();
    let mut push = |w: &Obj, id: &str, kind: LimitWindowKind, label: &str| {
        let raw = [
            w.get("used_percent"),
            w.get("usage_percent"),
            w.get("usedPercent"),
        ]
        .into_iter()
        .flatten()
        .find(|v| !v.is_null());
        // Codex reports 0-100 already; unlike other sources a value <= 1 is NOT a fraction.
        let Some(n) = num(raw).filter(|n| *n >= 0.0) else {
            return;
        };
        let used_percent = (n * 10000.0).round() / 10000.0;
        let used_percent = used_percent.min(100.0);
        let mut resets_at = parse_time(
            [w.get("resets_at"), w.get("reset_at"), w.get("resetAt")]
                .into_iter()
                .flatten()
                .find(|v| !v.is_null()),
        );
        if resets_at.is_none()
            && let Some(rel) = num(w.get("resets_in_seconds")).filter(|r| *r >= 0.0)
        {
            resets_at = Some(at + (rel * 1000.0).round() as i64);
        }
        // An idle window reports a phantom reset (now + window length); show none (Tokdash).
        if used_percent == 0.0 {
            resets_at = None;
        }
        windows.push(LimitWindow {
            provider: UsageProvider::Codex,
            account: account.clone(),
            id: id.to_string(),
            kind,
            label: label.to_string(),
            used_percent,
            resets_at,
        });
    };
    for (slot, w) in classified {
        match slot {
            Slot::Session => push(w, "session", LimitWindowKind::Session, "5-hour window"),
            Slot::Weekly => push(w, "weekly", LimitWindowKind::Weekly, "Weekly"),
        }
    }
    for (w, id, label) in &others {
        push(w, id, LimitWindowKind::Other, label);
    }
    if windows.is_empty() {
        return None;
    }
    Some(CodexParsedLimits {
        at,
        account,
        plan,
        windows,
    })
}

fn to_record(v: Option<&Value>) -> Option<CompactionRecord> {
    let o = v?.as_object()?;
    Some(CompactionRecord {
        response_id: some_str(o.get("response_id")),
        turn_id: some_str(o.get("turn_id")),
        usage: signature(o.get("usage")),
        thread_usage: non_empty_obj(o.get("thread_token_usage")).and_then(counters),
    })
}

/// Port of `CodexCompactionTracker.observe`; uncarried compactions queue on `state`.
fn observe_compaction(msg: &Obj, kind: &str, p: &Obj, state: &mut CodexFileState) {
    if kind == "token_usage_record" {
        state.last_record = to_record(Some(&Value::Object(p.clone())));
        state.last_record_carried = false;
    } else if kind == "compacted" {
        let rid = str_of(p.get("compaction_response_id"));
        let candidates = [
            to_record(p.get("latest_token_usage_record")),
            state.last_record.clone(),
        ];
        let record = candidates
            .into_iter()
            .flatten()
            .find(|c| !rid.is_empty() && c.response_id.as_deref() == Some(rid.as_str()));
        let Some(record) = record else { return };
        let Some(usage) = record.usage.clone() else {
            return;
        };
        let Some(ts) = msg
            .get("timestamp")
            .filter(|t| t.is_string())
            .and_then(|t| parse_time(Some(t)))
        else {
            return;
        };
        // A custom provider's token_count can land between the record and its `compacted` event.
        if state.last_record_carried
            && state
                .last_record
                .as_ref()
                .is_some_and(|r| r.response_id.as_deref() == Some(rid.as_str()))
        {
            return;
        }
        let (scope, replay_fallback) = replay_key_scope(state);
        let model = record
            .turn_id
            .as_ref()
            .and_then(|t| state.turn_models.iter().find(|(k, _)| k == t))
            .map(|(_, m)| m.clone())
            .or_else(|| state.model.clone());
        state.pending_compactions.push(PendingCompaction {
            usage,
            thread_usage: record.thread_usage,
            model,
            ts,
            project_key: state.cwd.clone(),
            key_scope: scope,
            replay_fallback,
        });
    } else if kind == "event_msg" && p.get("type").and_then(Value::as_str) == Some("token_count") {
        let empty = Obj::new();
        let info = p.get("info").and_then(Value::as_object).unwrap_or(&empty);
        let Some(sig) = signature(info.get("last_token_usage")) else {
            return;
        };
        if state
            .last_record
            .as_ref()
            .and_then(|r| r.usage.as_ref())
            .is_some_and(|u| *u == sig)
        {
            state.last_record_carried = true;
        }
        if let Some(i) = state
            .pending_compactions
            .iter()
            .position(|c| c.usage == sig)
        {
            state.pending_compactions.remove(i); // carried: the token_count bills it
        }
    }
}

fn remember_turn_model(state: &mut CodexFileState, turn_id: &str, model: &str) {
    state.turn_models.retain(|(k, _)| k != turn_id);
    state
        .turn_models
        .push((turn_id.to_string(), model.to_string()));
    while state.turn_models.len() > MAX_TURN_MODELS {
        state.turn_models.remove(0);
    }
}

/// Feed one rollout line; mutates `state`. Never panics.
pub fn parse_codex_line(line: &str, state: &mut CodexFileState) -> Option<CodexLineResult> {
    let msg: Value = serde_json::from_str(line).ok()?;
    let msg = msg.as_object()?;
    let kind = msg.get("type").and_then(Value::as_str).unwrap_or("");
    let empty = Obj::new();
    let p = msg
        .get("payload")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    if kind == "turn_context" {
        state.saw_turn_context = true;
        if let Some(model) = some_str(p.get("model")) {
            state.model = Some(model.clone());
            if let Some(turn_id) = some_str(p.get("turn_id")) {
                remember_turn_model(state, &turn_id, &model);
            }
        }
        if let Some(cwd) = p
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
        {
            state.cwd = cwd.to_string();
        }
    } else if kind == "event_msg"
        && p.get("type").and_then(Value::as_str) == Some("thread_settings_applied")
        && !state.saw_turn_context
    {
        // Newer Codex applies thread settings before the first turn_context.
        let model = p
            .get("thread_settings")
            .and_then(Value::as_object)
            .and_then(|s| some_str(s.get("model")));
        if model.is_some() {
            state.model = model;
        }
    } else if kind == "session_meta" {
        if let Some(sid) = some_str(p.get("id")) {
            if state.session_id.is_none() {
                state.session_id = Some(sid.clone());
                let (is_fork, parent) = fork_ancestry(p);
                state.is_fork_file = is_fork;
                state.subagent_parent_id = parent;
            }
            state.current_session_id = Some(sid);
        }
        if let Some(cwd) = p
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
        {
            state.cwd = cwd.to_string();
        }
        if !state.saw_turn_context {
            let model = p
                .get("base_instructions")
                .and_then(Value::as_object)
                .and_then(|b| b.get("provenance"))
                .and_then(Value::as_object)
                .and_then(|pr| some_str(pr.get("model")));
            if model.is_some() {
                state.model = model;
            }
        }
    }

    observe_compaction(msg, kind, p, state);

    if kind != "event_msg" || p.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let ts = msg
        .get("timestamp")
        .filter(|t| t.is_string())
        .and_then(|t| parse_time(Some(t)))?;

    let mut result = CodexLineResult::default();
    let info = p.get("info").and_then(Value::as_object).unwrap_or(&empty);

    if let Some(limits) = parse_limits(p, info, ts) {
        result.limits = Some(limits);
    }

    if let Some(last) = non_empty_obj(info.get("last_token_usage"))
        && let Some(last_counters) = counters(last)
    {
        let (scope, replay_fallback) = replay_key_scope(state);
        let key = token_event_key(scope.as_deref(), info);
        // A fork's replayed history without a stable key is dropped, not double counted.
        if !(replay_fallback && key.is_none())
            && let Some(parsed) =
                build_usage(&last_counters, state.model.as_deref(), ts, &state.cwd, key)
        {
            result.usage = Some(vec![parsed]);
        }
    }
    (result.usage.is_some() || result.limits.is_some()).then_some(result)
}

/// Call at the end of the available bytes: compaction requests that no `token_count` carried
/// (Tokdash issue #222). Each is returned once; it is removed from `state`, so calling again after
/// more lines never repeats it. `dedupe_key` is `None` when the log lacks the thread total.
pub fn flush_codex_compactions(state: &mut CodexFileState) -> Vec<CodexParsedUsage> {
    let pending = std::mem::take(&mut state.pending_compactions);
    let mut out = Vec::new();
    for c in pending {
        let key = compaction_event_key(c.key_scope.as_deref(), &c.usage, c.thread_usage.as_deref());
        if c.replay_fallback && key.is_none() {
            continue;
        }
        if let Some(parsed) = build_usage(&c.usage, c.model.as_deref(), c.ts, &c.project_key, key) {
            out.push(parsed);
        }
    }
    out
}

#[cfg(test)]
mod tests;
