// Adapted from Tokdash (MIT, © JingbiaoMei): src/tokdash/sources/quota/claude.py
//! Plan-limit windows of Claude's `/api/oauth/usage` response and their classification (Hydra
//! `shared/usage/limits.ts`).

use serde_json::{Map, Value};

use super::types::{LimitWindow, LimitWindowKind, UsageProvider};
use crate::usage_settings::UsageSettings;

type Obj = Map<String, Value>;

fn clamp_percent(n: f64) -> f64 {
    n.clamp(0.0, 100.0)
}

fn finite(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|n| n.is_finite())
}

/// `limits[].percent`: integers are on the 0-100 scale (1 is 1%); a float strictly inside (0,1) is
/// a fraction.
fn limit_percent(v: Option<&Value>) -> Option<f64> {
    let n = finite(v)?;
    let fraction = n.fract() != 0.0 && n > 0.0 && n < 1.0;
    Some(clamp_percent(if fraction { n * 100.0 } else { n }))
}

fn parse_resets_at(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::String(s) if !s.trim().is_empty() => super::parse_iso_ms(s),
        Value::Number(n) => {
            let x = n.as_f64().filter(|x| x.is_finite() && *x > 0.0)?;
            Some(if x < 1e11 {
                (x * 1000.0).round()
            } else {
                x.round()
            } as i64)
        }
        _ => None,
    }
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
    }
    if out.is_empty() { "model".into() } else { out }
}

fn title_case(kind: &str) -> String {
    let words: Vec<String> = kind
        .split(|c: char| c == '_' || c == '-' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut cs = w.chars();
            cs.next()
                .map(|f| f.to_uppercase().collect::<String>() + cs.as_str())
                .unwrap_or_default()
        })
        .collect();
    if words.is_empty() {
        "Other".into()
    } else {
        words.join(" ")
    }
}

fn build(
    account: &str,
    id: &str,
    kind: LimitWindowKind,
    label: &str,
    used_percent: f64,
    resets_at: Option<i64>,
) -> LimitWindow {
    LimitWindow {
        provider: UsageProvider::Claude,
        account: account.to_string(),
        id: id.to_string(),
        kind,
        label: label.to_string(),
        used_percent,
        resets_at,
    }
}

fn from_limits(limits: &[Value], account: &str) -> Vec<LimitWindow> {
    let mut out: Vec<LimitWindow> = Vec::new();
    for raw in limits {
        let Some(raw) = raw.as_object() else { continue };
        let Some(kind) = raw
            .get("kind")
            .and_then(Value::as_str)
            .filter(|k| !k.is_empty())
        else {
            continue;
        };
        let Some(percent) = limit_percent(raw.get("percent")) else {
            continue;
        };
        let resets_at = parse_resets_at(raw.get("resets_at"));
        let model_name = raw
            .get("scope")
            .and_then(Value::as_object)
            .and_then(|s| s.get("model"))
            .and_then(Value::as_object)
            .and_then(|m| m.get("display_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let w = if kind == "session" {
            build(
                account,
                "session",
                LimitWindowKind::Session,
                "Current session (5h)",
                percent,
                resets_at,
            )
        } else if kind == "weekly_all" {
            build(
                account,
                "weekly",
                LimitWindowKind::Weekly,
                "Weekly \u{b7} all models",
                percent,
                resets_at,
            )
        } else if kind.starts_with("weekly") && !model_name.is_empty() {
            build(
                account,
                &format!("weekly:{}", slug(model_name)),
                LimitWindowKind::WeeklyModel,
                &format!("Weekly \u{b7} {model_name}"),
                percent,
                resets_at,
            )
        } else {
            build(
                account,
                kind,
                LimitWindowKind::Other,
                &title_case(kind),
                percent,
                resets_at,
            )
        };
        if out.iter().any(|o| o.id == w.id) {
            continue;
        }
        out.push(w);
    }
    out
}

const LEGACY: [(&str, &str, LimitWindowKind, &str); 4] = [
    (
        "five_hour",
        "session",
        LimitWindowKind::Session,
        "Current session (5h)",
    ),
    (
        "seven_day",
        "weekly",
        LimitWindowKind::Weekly,
        "Weekly \u{b7} all models",
    ),
    (
        "seven_day_opus",
        "weekly:opus",
        LimitWindowKind::WeeklyModel,
        "Weekly \u{b7} Opus",
    ),
    (
        "seven_day_sonnet",
        "weekly:sonnet",
        LimitWindowKind::WeeklyModel,
        "Weekly \u{b7} Sonnet",
    ),
];

fn from_legacy(body: &Obj, account: &str) -> Vec<LimitWindow> {
    let mut out = Vec::new();
    for (key, id, kind, label) in LEGACY {
        let Some(raw) = body.get(key).and_then(Value::as_object) else {
            continue;
        };
        let Some(percent) = finite(raw.get("utilization")).map(clamp_percent) else {
            continue;
        };
        out.push(build(
            account,
            id,
            kind,
            label,
            percent,
            parse_resets_at(raw.get("resets_at")),
        ));
    }
    out
}

/// Plan-limit windows of one `/api/oauth/usage` response. Never panics; malformed entries are
/// skipped.
pub fn parse_claude_oauth_usage(body: &Value, account: &str) -> Vec<LimitWindow> {
    let Some(body) = body.as_object() else {
        return Vec::new();
    };
    if let Some(limits) = body.get("limits").and_then(Value::as_array) {
        let windows = from_limits(limits, account);
        if !windows.is_empty() {
            return windows;
        }
    }
    from_legacy(body, account)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitSeverity {
    Ok,
    Warn,
    High,
}

pub fn limit_severity(used_percent: f64) -> LimitSeverity {
    if used_percent < 50.0 {
        LimitSeverity::Ok
    } else if used_percent < 80.0 {
        LimitSeverity::Warn
    } else {
        LimitSeverity::High
    }
}

/// The windows the header chip shows: the most-used session and the most-used all-models weekly.
pub fn pick_chip_windows<'a>(
    windows: impl IntoIterator<Item = &'a LimitWindow>,
) -> (Option<&'a LimitWindow>, Option<&'a LimitWindow>) {
    let (mut session, mut weekly): (Option<&LimitWindow>, Option<&LimitWindow>) = (None, None);
    for w in windows {
        match w.kind {
            LimitWindowKind::Session if session.is_none_or(|s| w.used_percent > s.used_percent) => {
                session = Some(w);
            }
            LimitWindowKind::Weekly if weekly.is_none_or(|s| w.used_percent > s.used_percent) => {
                weekly = Some(w);
            }
            _ => {}
        }
    }
    (session, weekly)
}

/// Percent at which a window counts as "near its limit" when no settings are given.
pub const DEFAULT_LIMIT_ALERT_OTHER: u32 = 85;

/// Alert level of a window; per-model weekly windows use the weekly level.
pub fn limit_alert_level(kind: LimitWindowKind, settings: &UsageSettings) -> u32 {
    match kind {
        LimitWindowKind::Session => settings.session_alert_percent,
        LimitWindowKind::Weekly | LimitWindowKind::WeeklyModel => settings.weekly_alert_percent,
        LimitWindowKind::Other => DEFAULT_LIMIT_ALERT_OTHER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("fixtures/claude-oauth-usage.json")).unwrap()
    }
    fn ms(s: &str) -> Option<i64> {
        super::super::parse_iso_ms(s)
    }

    #[test]
    fn reads_limits_from_the_real_fixture() {
        let w = parse_claude_oauth_usage(&fixture(), "default");
        let got: Vec<_> = w
            .iter()
            .map(|x| (x.id.as_str(), x.kind, x.label.as_str(), x.used_percent))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "session",
                    LimitWindowKind::Session,
                    "Current session (5h)",
                    4.0
                ),
                (
                    "weekly",
                    LimitWindowKind::Weekly,
                    "Weekly \u{b7} all models",
                    0.0
                )
            ]
        );
        assert_eq!(w[0].resets_at, ms("2026-10-23T10:29:59.646885+00:00"));
        assert_eq!(w[1].resets_at, ms("2026-10-30T01:59:59.646907+00:00"));
        assert_eq!(w[0].provider, UsageProvider::Claude);
        assert_eq!(w[0].account, "default");
    }

    #[test]
    fn falls_back_to_legacy_keys() {
        let w = parse_claude_oauth_usage(
            &json!({
                "five_hour": {"utilization": 12, "resets_at": "2026-10-23T10:00:00Z"},
                "seven_day": {"utilization": 30, "resets_at": null},
                "seven_day_opus": {"utilization": 55.5, "resets_at": "2026-10-30T00:00:00Z"},
                "seven_day_sonnet": null
            }),
            "work",
        );
        let got: Vec<_> = w
            .iter()
            .map(|x| (x.id.as_str(), x.kind, x.used_percent))
            .collect();
        assert_eq!(
            got,
            [
                ("session", LimitWindowKind::Session, 12.0),
                ("weekly", LimitWindowKind::Weekly, 30.0),
                ("weekly:opus", LimitWindowKind::WeeklyModel, 55.5)
            ]
        );
        assert_eq!(w[1].resets_at, None);
    }

    #[test]
    fn does_not_use_legacy_keys_when_limits_yield_windows() {
        let w = parse_claude_oauth_usage(
            &json!({"five_hour": {"utilization": 99}, "limits": [{"kind": "session", "percent": 3, "resets_at": null}]}),
            "a",
        );
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].used_percent, 3.0);
    }

    #[test]
    fn maps_scoped_weekly_model_limits_and_unknown_kinds() {
        let w = parse_claude_oauth_usage(
            &json!({"limits": [
                {"kind": "weekly_scoped", "percent": 40, "resets_at": null, "scope": {"model": {"display_name": "Claude Opus 4"}}},
                {"kind": "monthly_extra", "percent": 10, "resets_at": null, "scope": null}
            ]}),
            "a",
        );
        assert_eq!(
            (w[0].id.as_str(), w[0].kind, w[0].label.as_str()),
            (
                "weekly:claude-opus-4",
                LimitWindowKind::WeeklyModel,
                "Weekly \u{b7} Claude Opus 4"
            )
        );
        assert_eq!(
            (w[1].id.as_str(), w[1].kind, w[1].label.as_str()),
            ("monthly_extra", LimitWindowKind::Other, "Monthly Extra")
        );
    }

    #[test]
    fn scales_percent_integer_1_is_1_float_half_is_50() {
        let p = |percent: Value| {
            parse_claude_oauth_usage(
                &json!({"limits": [{"kind": "session", "percent": percent}]}),
                "a",
            )[0]
            .used_percent
        };
        assert_eq!(p(json!(1)), 1.0);
        assert_eq!(p(json!(0)), 0.0);
        assert_eq!(p(json!(0.5)), 50.0);
        assert_eq!(p(json!(250)), 100.0);
        assert_eq!(p(json!(-3)), 0.0);
    }

    #[test]
    fn skips_malformed_input_without_panicking() {
        assert!(parse_claude_oauth_usage(&Value::Null, "a").is_empty());
        assert!(parse_claude_oauth_usage(&json!("x"), "a").is_empty());
        assert!(parse_claude_oauth_usage(&json!({"limits": "nope"}), "a").is_empty());
        let w = parse_claude_oauth_usage(
            &json!({"limits": [null, 5, {"kind": 3}, {"kind": "session", "percent": "x"}, {"kind": "session", "percent": 7, "resets_at": "bad"}]}),
            "a",
        );
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].resets_at, None);
    }

    #[test]
    fn severity_thresholds_are_50_and_80() {
        assert_eq!(limit_severity(0.0), LimitSeverity::Ok);
        assert_eq!(limit_severity(49.9), LimitSeverity::Ok);
        assert_eq!(limit_severity(50.0), LimitSeverity::Warn);
        assert_eq!(limit_severity(79.9), LimitSeverity::Warn);
        assert_eq!(limit_severity(80.0), LimitSeverity::High);
        assert_eq!(limit_severity(100.0), LimitSeverity::High);
    }

    fn w(
        provider: UsageProvider,
        account: &str,
        id: &str,
        kind: LimitWindowKind,
        used: f64,
    ) -> LimitWindow {
        LimitWindow {
            provider,
            account: account.into(),
            id: id.into(),
            kind,
            label: id.into(),
            used_percent: used,
            resets_at: None,
        }
    }

    #[test]
    fn picks_the_most_used_session_and_all_models_weekly_across_accounts() {
        use LimitWindowKind::*;
        use UsageProvider::*;
        let all = [
            w(Claude, "a", "session", Session, 10.0),
            w(Codex, "b", "session", Session, 60.0),
            w(Claude, "a", "weekly", Weekly, 20.0),
            w(Claude, "a", "weekly:opus", WeeklyModel, 95.0),
            w(Codex, "b", "weekly", Weekly, 35.0),
        ];
        let (session, weekly) = pick_chip_windows(&all);
        assert_eq!(session.unwrap().account, "b");
        assert_eq!(weekly.unwrap().used_percent, 35.0);
        assert_eq!(pick_chip_windows(&[]), (None, None));
    }

    #[test]
    fn alert_levels() {
        let s = UsageSettings::default();
        assert_eq!(limit_alert_level(LimitWindowKind::Session, &s), 80);
        assert_eq!(limit_alert_level(LimitWindowKind::WeeklyModel, &s), 90);
        assert_eq!(limit_alert_level(LimitWindowKind::Other, &s), 85);
    }
}
