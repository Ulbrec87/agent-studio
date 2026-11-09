//! Low-limit and soft-budget alerts (Hydra `shared/usage/alerts.ts` + `electron/usage/UsageAlerts.ts`).
//! Pure crossing detection plus a small stateful notifier that remembers what it already said in
//! `usage-alerts.json`, so a restart never repeats an alert and a new `resetsAt` re-arms a window.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};

use super::files;
use super::limits::limit_alert_level;
use super::types::{LimitWindow, LimitWindowKind, UsageProvider, UsageState};
use crate::usage_settings::UsageSettings;

/// Codex limits come from session logs; older than this they are not trusted for an alert.
pub const CODEX_ALERT_MAX_AGE_MS: i64 = 30 * 60_000;
const MAX_ALERTS_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct DueAlert {
    pub window: LimitWindow,
    pub threshold: u32,
    pub key: String,
}

/// Percent used at which `window` alerts: session and weekly levels from the settings, anything
/// else the default.
pub fn alert_threshold(kind: LimitWindowKind, settings: &UsageSettings) -> u32 {
    limit_alert_level(kind, settings)
}

fn provider_key(p: UsageProvider) -> &'static str {
    match p {
        UsageProvider::Claude => "claude",
        UsageProvider::Codex => "codex",
    }
}

/// Identity of one alert: it repeats only when the window resets (new `resets_at`) or the
/// threshold changes.
pub fn alert_key(w: &LimitWindow, threshold: u32) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        provider_key(w.provider),
        w.account,
        w.id,
        w.resets_at.map(|r| r.to_string()).unwrap_or_default(),
        threshold
    )
}

/// Windows at or over their threshold that have not been notified. A window without a reset time
/// never alerts (it cannot be deduplicated or re-armed), nor does one whose reset time has passed.
pub fn due_alerts(
    windows: &[LimitWindow],
    settings: &UsageSettings,
    notified: &HashSet<String>,
    now: Option<i64>,
) -> Vec<DueAlert> {
    let mut out = Vec::new();
    for w in windows {
        let Some(resets_at) = w.resets_at else {
            continue;
        };
        if now.is_some_and(|n| resets_at <= n) {
            continue;
        }
        let threshold = alert_threshold(w.kind, settings);
        if w.used_percent < f64::from(threshold) {
            continue;
        }
        let key = alert_key(w, threshold);
        if notified.contains(&key) {
            continue;
        }
        out.push(DueAlert {
            window: w.clone(),
            threshold,
            key,
        });
    }
    out
}

/// The windows that may alert: Claude accounts with a fresh sign-in (`ok`), Codex limits read in
/// the last 30 minutes.
pub fn alertable_windows(state: &UsageState, now: i64) -> Vec<LimitWindow> {
    let mut out = Vec::new();
    for profile in &state.claude {
        if profile.status == super::types::ProfileStatus::Ok {
            out.extend(profile.windows.iter().cloned());
        }
    }
    if let Some(codex) = &state.codex
        && now - codex.at <= CODEX_ALERT_MAX_AGE_MS
    {
        out.extend(codex.windows.iter().cloned());
    }
    out
}

fn describe_duration(ms: i64) -> String {
    let minutes = ((ms as f64) / 60_000.0).round().max(1.0) as i64;
    if minutes < 60 {
        return format!("{minutes} min");
    }
    let hours = minutes / 60;
    if hours < 48 {
        let rest = minutes % 60;
        return if rest == 0 {
            format!("{hours} h")
        } else {
            format!("{hours} h {rest} min")
        };
    }
    format!("{} days", ((hours as f64) / 24.0).round() as i64)
}

fn window_name(w: &LimitWindow) -> String {
    match w.kind {
        LimitWindowKind::Session => "session".into(),
        LimitWindowKind::Weekly => "weekly".into(),
        LimitWindowKind::WeeklyModel => {
            let rest = w
                .label
                .trim_start_matches("Weekly")
                .trim_start()
                .trim_start_matches('\u{b7}');
            format!("weekly {}", rest.trim()).trim().to_string()
        }
        LimitWindowKind::Other => w.label.to_lowercase(),
    }
}

/// Title and body of the notification for one alert (`running_agents` is optional context).
pub fn describe_alert(
    w: &LimitWindow,
    now: i64,
    running_agents: Option<usize>,
) -> (String, String) {
    let provider = match w.provider {
        UsageProvider::Codex => "Codex",
        UsageProvider::Claude => "Claude",
    };
    let account = if w.provider == UsageProvider::Claude && w.account != "default" {
        format!(" ({})", w.account)
    } else {
        String::new()
    };
    let title = format!(
        "{provider} {} limit{account}: {}% used",
        window_name(w),
        w.used_percent.round() as i64
    );
    let mut body = String::new();
    if let Some(resets_at) = w.resets_at {
        let at = Local
            .timestamp_millis_opt(resets_at)
            .single()
            .map(|d| d.format("%H:%M").to_string())
            .unwrap_or_default();
        body = format!("Resets in {} ({at}).", describe_duration(resets_at - now));
    }
    if let Some(n) = running_agents.filter(|n| *n > 0) {
        if !body.is_empty() {
            body.push(' ');
        }
        body.push_str(&format!(
            "{n} {provider} agent{} running.",
            if n == 1 { " is" } else { "s are" }
        ));
    }
    (title, body)
}

// ── budgets ─────────────────────────────────────────────────────────────────

/// The config's soft daily budgets (`usageDailyTokenBudget`, `usageDailyCostBudgetUsd`,
/// `usageBudgetWarningThresholdPct`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Budgets {
    pub daily_tokens: Option<f64>,
    pub daily_cost_usd: Option<f64>,
    pub warning_pct: Option<f64>,
}

/// What was used today (local day), for the budgets.
#[derive(Debug, Clone, PartialEq)]
pub struct TodayUsage {
    /// Local day `YYYY-MM-DD`.
    pub date: String,
    /// Epoch ms the day ends (keys are dropped after it).
    pub ends_at: i64,
    pub tokens: u64,
    pub cost_usd: f64,
}

impl TodayUsage {
    /// From the summary of `today` taken at `now` (local day).
    pub fn from_summary<Tz: TimeZone>(
        summary: &super::types::UsageSummary,
        now: &chrono::DateTime<Tz>,
    ) -> Self {
        let ends_at = super::periods::period_slots(super::types::UsagePeriod::Today, now)
            .last()
            .map_or(i64::MAX, |s| s.end);
        Self {
            date: super::report::date_key(now.date_naive()),
            ends_at,
            tokens: summary.total_tokens,
            cost_usd: summary.cost_usd,
        }
    }
}

pub const DEFAULT_BUDGET_WARNING_PCT: f64 = 80.0;

#[derive(Debug, Clone, PartialEq)]
pub struct DueBudget {
    pub key: String,
    pub title: String,
    pub body: String,
}

fn compact(n: f64) -> String {
    let n = n.max(0.0);
    for (size, suffix) in [(1e9, "B"), (1e6, "M"), (1e3, "k")] {
        if n >= size {
            let scaled = n / size;
            let text = if scaled >= 100.0 {
                format!("{}", scaled.round() as i64)
            } else {
                let t = format!("{scaled:.1}");
                t.trim_end_matches(".0").to_string()
            };
            return format!("{text}{suffix}");
        }
    }
    format!("{}", n.round() as i64)
}

/// Budget alerts due: once at the warning threshold ("approaching") and once at 100% ("over"), per
/// day and budget. A cost budget is skipped while costs are hidden.
pub fn due_budgets(
    budgets: &Budgets,
    today: &TodayUsage,
    show_cost: bool,
    notified: &HashSet<String>,
) -> Vec<DueBudget> {
    let warn = budgets
        .warning_pct
        .filter(|p| p.is_finite() && *p > 0.0 && *p <= 100.0)
        .unwrap_or(DEFAULT_BUDGET_WARNING_PCT);
    let mut out = Vec::new();
    let mut check =
        |name: &str, label: &str, used: f64, limit: Option<f64>, fmt: &dyn Fn(f64) -> String| {
            let Some(limit) = limit.filter(|l| l.is_finite() && *l > 0.0) else {
                return;
            };
            let pct = used / limit * 100.0;
            for (level, threshold) in [("over", 100.0), ("warn", warn)] {
                if pct < threshold {
                    continue;
                }
                let key = format!("budget|{name}|{}|{level}", today.date);
                if notified.contains(&key) {
                    // The higher level is already said: stay quiet about the lower one too.
                    break;
                }
                let title = if level == "over" {
                    format!(
                        "Daily {label} budget exceeded: {}% used",
                        pct.round() as i64
                    )
                } else {
                    format!("Daily {label} budget: {}% used", pct.round() as i64)
                };
                out.push(DueBudget {
                    key,
                    title,
                    body: format!("{} of {} today.", fmt(used), fmt(limit)),
                });
                break;
            }
        };
    check(
        "tokens",
        "token",
        today.tokens as f64,
        budgets.daily_tokens,
        &|n| format!("{} tokens", compact(n)),
    );
    if show_cost {
        check(
            "cost",
            "cost",
            today.cost_usd,
            budgets.daily_cost_usd,
            &|n| format!("${n:.2}"),
        );
    }
    out
}

// ── notifier ────────────────────────────────────────────────────────────────

/// One notification to deliver.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageAlert {
    pub title: String,
    pub body: String,
}

pub struct AlertConfig {
    pub settings: UsageSettings,
    pub budgets: Budgets,
}

pub struct UsageAlertsOptions {
    /// `usage-alerts.json`.
    pub file: PathBuf,
    pub config: Box<dyn Fn() -> AlertConfig + Send + Sync>,
    pub notify: Box<dyn Fn(UsageAlert) + Send + Sync>,
    /// Running agents of a provider, for the notification text.
    pub running_agents: Option<Box<dyn Fn(UsageProvider) -> usize + Send + Sync>>,
    pub now: Box<dyn Fn() -> i64 + Send + Sync>,
}

#[derive(Serialize, Deserialize, Default, ts_rs::TS)]
struct AlertsFile {
    version: u32,
    notified: Vec<NotifiedEntry>,
}

#[derive(Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
struct NotifiedEntry {
    key: String,
    resets_at: i64,
}

/// Opt-in low-limit notifications. One per (provider, account, window, resetsAt, threshold); the
/// keys are kept in the alerts file so a restart does not repeat them.
pub struct UsageAlerts {
    opts: UsageAlertsOptions,
    /// key -> when it stops mattering (window reset / end of the day), for pruning.
    notified: parking_lot::Mutex<Option<BTreeMap<String, i64>>>,
}

impl UsageAlerts {
    pub fn new(opts: UsageAlertsOptions) -> Self {
        Self {
            opts,
            notified: parking_lot::Mutex::new(None),
        }
    }

    /// Notifies every due alert of `state` (and of the daily budgets when `today` is given); does
    /// nothing while alerts are off.
    pub fn evaluate(&self, state: &UsageState, today: Option<&TodayUsage>) {
        let config = (self.opts.config)();
        if !config.settings.alerts_enabled {
            return;
        }
        let now = (self.opts.now)();
        let mut guard = self.notified.lock();
        let known = guard.get_or_insert_with(|| self.load());
        let mut changed = prune(known, now);
        let seen: HashSet<String> = known.keys().cloned().collect();
        for alert in due_alerts(
            &alertable_windows(state, now),
            &config.settings,
            &seen,
            Some(now),
        ) {
            known.insert(alert.key.clone(), alert.window.resets_at.unwrap_or(now));
            changed = true;
            let running = self
                .opts
                .running_agents
                .as_ref()
                .map(|f| f(alert.window.provider));
            let (title, body) = describe_alert(&alert.window, now, running);
            (self.opts.notify)(UsageAlert { title, body });
        }
        if let Some(today) = today {
            for due in due_budgets(&config.budgets, today, config.settings.show_cost, &seen) {
                // `over` supersedes `warn`: remember both so a later warn is not said after over.
                known.insert(due.key.clone(), today.ends_at);
                if due.key.ends_with("|over") {
                    known.insert(due.key.replace("|over", "|warn"), today.ends_at);
                }
                changed = true;
                (self.opts.notify)(UsageAlert {
                    title: due.title,
                    body: due.body,
                });
            }
        }
        if changed {
            self.save(known);
        }
    }

    fn load(&self) -> BTreeMap<String, i64> {
        let mut map = BTreeMap::new();
        if let Ok(Some(text)) = files::read_text(&self.opts.file, MAX_ALERTS_BYTES)
            && let Ok(file) = serde_json::from_str::<AlertsFile>(&text)
        {
            for e in file.notified {
                map.insert(e.key, e.resets_at);
            }
        }
        map
    }

    fn save(&self, known: &BTreeMap<String, i64>) {
        let file = AlertsFile {
            version: 1,
            notified: known
                .iter()
                .map(|(key, resets_at)| NotifiedEntry {
                    key: key.clone(),
                    resets_at: *resets_at,
                })
                .collect(),
        };
        if let Ok(text) = serde_json::to_string(&file) {
            // The alerts still work for this run when the write fails.
            let _ = files::write_atomic_private(&self.opts.file, text.as_bytes());
        }
    }
}

fn prune(known: &mut BTreeMap<String, i64>, now: i64) -> bool {
    let before = known.len();
    known.retain(|_, until| *until > now);
    known.len() != before
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;
    use crate::usage::codex_line::CodexParsedLimits;
    use crate::usage::types::{ClaudeProfileLimits, IndexingState, ProfileStatus};

    const NOW: i64 = 1_792_756_800_000; // 2026-10-23T12:00:00Z

    fn s() -> UsageSettings {
        UsageSettings::default()
    }
    fn w(kind: LimitWindowKind, used: f64, resets_at: Option<i64>) -> LimitWindow {
        let id = if kind == LimitWindowKind::WeeklyModel {
            "weekly:opus".to_string()
        } else {
            match kind {
                LimitWindowKind::Session => "session".into(),
                LimitWindowKind::Weekly => "weekly".into(),
                _ => "other".into(),
            }
        };
        LimitWindow {
            provider: UsageProvider::Claude,
            account: "default".into(),
            id,
            kind,
            label: match kind {
                LimitWindowKind::Session => "session".into(),
                LimitWindowKind::Weekly => "weekly".into(),
                LimitWindowKind::WeeklyModel => "weekly-model".into(),
                LimitWindowKind::Other => "other".into(),
            },
            used_percent: used,
            resets_at,
        }
    }
    fn soon() -> Option<i64> {
        Some(NOW + 3_600_000)
    }

    #[test]
    fn threshold_levels() {
        assert_eq!(alert_threshold(LimitWindowKind::Session, &s()), 80);
        assert_eq!(alert_threshold(LimitWindowKind::Weekly, &s()), 90);
        assert_eq!(alert_threshold(LimitWindowKind::WeeklyModel, &s()), 90);
        assert_eq!(alert_threshold(LimitWindowKind::Other, &s()), 85);
        let custom = UsageSettings {
            session_alert_percent: 60,
            ..s()
        };
        assert_eq!(alert_threshold(LimitWindowKind::Session, &custom), 60);
    }

    #[test]
    fn due_at_or_over_the_threshold_only() {
        let mut a = w(LimitWindowKind::Session, 80.0, Some(NOW + 1));
        a.id = "a".into();
        let due = due_alerts(
            &[
                w(LimitWindowKind::Session, 79.0, soon()),
                a,
                w(LimitWindowKind::Weekly, 89.0, soon()),
                w(LimitWindowKind::Weekly, 95.0, soon()),
            ],
            &s(),
            &HashSet::new(),
            None,
        );
        let got: Vec<_> = due.iter().map(|d| (d.window.kind, d.threshold)).collect();
        assert_eq!(
            got,
            [
                (LimitWindowKind::Session, 80),
                (LimitWindowKind::Weekly, 90)
            ]
        );
    }

    #[test]
    fn never_alerts_without_a_reset_time_or_after_the_reset() {
        assert!(
            due_alerts(
                &[w(LimitWindowKind::Session, 99.0, None)],
                &s(),
                &HashSet::new(),
                None
            )
            .is_empty()
        );
        assert!(
            due_alerts(
                &[w(LimitWindowKind::Session, 99.0, Some(NOW - 1))],
                &s(),
                &HashSet::new(),
                Some(NOW)
            )
            .is_empty()
        );
    }

    #[test]
    fn does_not_repeat_a_notified_key_and_re_arms_on_a_new_resets_at() {
        let first = w(LimitWindowKind::Session, 90.0, soon());
        let a = due_alerts(std::slice::from_ref(&first), &s(), &HashSet::new(), None).remove(0);
        let known: HashSet<String> = [a.key.clone()].into();
        assert!(due_alerts(&[first], &s(), &known, None).is_empty());
        let next = w(LimitWindowKind::Session, 90.0, Some(NOW + 5 * 3_600_000));
        assert_eq!(due_alerts(&[next], &s(), &known, None).len(), 1);
    }

    #[test]
    fn separates_accounts_and_providers() {
        let base = w(LimitWindowKind::Session, 90.0, soon());
        let mut work = base.clone();
        work.account = "work".into();
        let mut codex = base.clone();
        codex.provider = UsageProvider::Codex;
        let keys: HashSet<_> = [
            alert_key(&base, 80),
            alert_key(&work, 80),
            alert_key(&codex, 80),
        ]
        .into();
        assert_eq!(keys.len(), 3);
    }

    fn profile(status: ProfileStatus, windows: Vec<LimitWindow>) -> ClaudeProfileLimits {
        ClaudeProfileLimits {
            account: "default".into(),
            config_dir: "/x".into(),
            plan: None,
            status,
            windows,
            fetched_at: Some(NOW),
            checked_at: NOW,
        }
    }
    fn state(claude: Vec<ClaudeProfileLimits>, codex: Option<CodexParsedLimits>) -> UsageState {
        UsageState {
            claude,
            codex,
            indexing: IndexingState::default(),
            data_version: 0,
            updated_at: NOW,
        }
    }

    #[test]
    fn skips_claude_accounts_whose_status_is_not_ok() {
        let st = state(
            vec![
                profile(
                    ProfileStatus::Expired,
                    vec![w(LimitWindowKind::Session, 95.0, soon())],
                ),
                profile(
                    ProfileStatus::Ok,
                    vec![w(LimitWindowKind::Weekly, 95.0, soon())],
                ),
            ],
            None,
        );
        let kinds: Vec<_> = alertable_windows(&st, NOW).iter().map(|x| x.kind).collect();
        assert_eq!(kinds, [LimitWindowKind::Weekly]);
    }

    #[test]
    fn skips_codex_limits_older_than_30_minutes() {
        let mut win = w(LimitWindowKind::Weekly, 95.0, Some(NOW + 1000));
        win.provider = UsageProvider::Codex;
        let codex = |at| {
            Some(CodexParsedLimits {
                at,
                account: "default".into(),
                plan: None,
                windows: vec![win.clone()],
            })
        };
        assert_eq!(
            alertable_windows(&state(vec![], codex(NOW - 29 * 60_000)), NOW).len(),
            1
        );
        assert_eq!(
            alertable_windows(&state(vec![], codex(NOW - 31 * 60_000)), NOW).len(),
            0
        );
    }

    #[test]
    fn describes_title_reset_and_running_agents() {
        let (title, body) = describe_alert(
            &w(LimitWindowKind::Session, 85.4, Some(NOW + 42 * 60_000)),
            NOW,
            Some(3),
        );
        assert_eq!(title, "Claude session limit: 85% used");
        let re =
            regex::Regex::new(r"^Resets in 42 min \(\d\d:\d\d\)\. 3 Claude agents are running\.$")
                .unwrap();
        assert!(re.is_match(&body), "{body}");
    }

    #[test]
    fn names_a_non_default_account_and_omits_a_zero_agent_count() {
        let mut win = w(LimitWindowKind::Weekly, 91.0, Some(NOW + 3 * 86_400_000));
        win.account = "work".into();
        let (title, body) = describe_alert(&win, NOW, Some(0));
        assert_eq!(title, "Claude weekly limit (work): 91% used");
        assert!(body.starts_with("Resets in 3 days ("), "{body}");
        assert!(!body.contains("running"));
        assert!(
            describe_alert(&w(LimitWindowKind::Session, 90.0, soon()), NOW, Some(1))
                .1
                .ends_with("1 Claude agent is running.")
        );
    }

    // ── notifier ────────────────────────────────────────────────────────────

    struct Rig {
        alerts: UsageAlerts,
        sent: Arc<Mutex<Vec<UsageAlert>>>,
    }

    fn make(file: PathBuf, settings: UsageSettings, budgets: Budgets, now: Arc<Mutex<i64>>) -> Rig {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sink = sent.clone();
        let alerts = UsageAlerts::new(UsageAlertsOptions {
            file,
            config: Box::new(move || AlertConfig {
                settings: settings.clone(),
                budgets,
            }),
            notify: Box::new(move |a| sink.lock().push(a)),
            running_agents: Some(Box::new(|_| 2)),
            now: Box::new(move || *now.lock()),
        });
        Rig { alerts, sent }
    }
    fn on() -> UsageSettings {
        UsageSettings {
            alerts_enabled: true,
            ..s()
        }
    }
    fn make_default(file: PathBuf, settings: UsageSettings) -> Rig {
        make(
            file,
            settings,
            Budgets::default(),
            Arc::new(Mutex::new(NOW)),
        )
    }
    fn session(used: f64, resets_at: i64) -> UsageState {
        state(
            vec![profile(
                ProfileStatus::Ok,
                vec![w(LimitWindowKind::Session, used, Some(resets_at))],
            )],
            None,
        )
    }

    #[test]
    fn does_nothing_while_alerts_are_off() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("alerts.json");
        let rig = make_default(
            file.clone(),
            UsageSettings {
                alerts_enabled: false,
                ..s()
            },
        );
        rig.alerts.evaluate(&session(95.0, NOW + 600_000), None);
        assert!(rig.sent.lock().is_empty());
        assert!(!file.exists());
    }

    #[test]
    fn notifies_once_per_crossing() {
        let dir = tempfile::tempdir().unwrap();
        let rig = make_default(dir.path().join("alerts.json"), on());
        let st = session(85.0, NOW + 42 * 60_000);
        rig.alerts.evaluate(&st, None);
        rig.alerts.evaluate(&st, None);
        let sent = rig.sent.lock();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].title, "Claude session limit: 85% used");
        assert!(sent[0].body.contains("Resets in 42 min"));
        assert!(sent[0].body.contains("2 Claude agents are running."));
    }

    #[test]
    fn stays_quiet_below_the_threshold_and_uses_the_configured_one() {
        let dir = tempfile::tempdir().unwrap();
        let rig = make_default(
            dir.path().join("alerts.json"),
            UsageSettings {
                session_alert_percent: 60,
                ..on()
            },
        );
        rig.alerts.evaluate(&session(59.0, NOW + 1000), None);
        assert_eq!(rig.sent.lock().len(), 0);
        rig.alerts.evaluate(&session(61.0, NOW + 1000), None);
        assert_eq!(rig.sent.lock().len(), 1);
    }

    #[test]
    fn does_not_repeat_after_a_restart_and_re_arms_with_a_new_resets_at() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("alerts.json");
        let first = make_default(file.clone(), on());
        first.alerts.evaluate(&session(90.0, NOW + 600_000), None);
        assert_eq!(first.sent.lock().len(), 1);
        assert!(file.exists());
        let second = make_default(file, on()); // a new instance reads the file
        second.alerts.evaluate(&session(90.0, NOW + 600_000), None);
        assert_eq!(second.sent.lock().len(), 0);
        second
            .alerts
            .evaluate(&session(90.0, NOW + 6 * 3_600_000), None);
        assert_eq!(second.sent.lock().len(), 1);
    }

    #[test]
    fn prunes_keys_whose_window_has_reset() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("alerts.json");
        let now = Arc::new(Mutex::new(NOW));
        let rig = make(file.clone(), on(), Budgets::default(), now.clone());
        rig.alerts.evaluate(&session(90.0, NOW + 600_000), None);
        *now.lock() = NOW + 2 * 3_600_000;
        rig.alerts
            .evaluate(&session(10.0, NOW + 6 * 3_600_000), None);
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        assert_eq!(v["notified"], serde_json::json!([]));
    }

    #[test]
    fn survives_a_damaged_alerts_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("alerts.json");
        std::fs::write(&file, "{nope").unwrap();
        let rig = make_default(file, on());
        rig.alerts.evaluate(&session(90.0, NOW + 600_000), None);
        assert_eq!(rig.sent.lock().len(), 1);
    }

    // ── budgets ─────────────────────────────────────────────────────────────

    fn today(tokens: u64, usd: f64) -> TodayUsage {
        TodayUsage {
            date: "2026-10-23".into(),
            ends_at: NOW + 43_200_000,
            tokens,
            cost_usd: usd,
        }
    }

    #[test]
    fn budget_warns_at_the_threshold_then_at_100_once_per_day() {
        let b = Budgets {
            daily_tokens: Some(1_000_000.0),
            daily_cost_usd: None,
            warning_pct: Some(75.0),
        };
        let none = HashSet::new();
        assert!(due_budgets(&b, &today(700_000, 0.0), true, &none).is_empty());
        let warn = due_budgets(&b, &today(750_000, 0.0), true, &none);
        assert_eq!(warn.len(), 1);
        assert_eq!(warn[0].title, "Daily token budget: 75% used");
        assert_eq!(warn[0].body, "750k tokens of 1M tokens today.");
        let known: HashSet<String> = [warn[0].key.clone()].into();
        assert!(due_budgets(&b, &today(800_000, 0.0), true, &known).is_empty());
        let over = due_budgets(&b, &today(1_200_000, 0.0), true, &known);
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].title, "Daily token budget exceeded: 120% used");
    }

    #[test]
    fn cost_budget_is_skipped_while_costs_are_hidden_and_defaults_to_80() {
        let b = Budgets {
            daily_tokens: None,
            daily_cost_usd: Some(10.0),
            warning_pct: None,
        };
        let none = HashSet::new();
        assert!(due_budgets(&b, &today(0, 7.9), true, &none).is_empty());
        let due = due_budgets(&b, &today(0, 8.0), true, &none);
        assert_eq!(
            (due[0].title.as_str(), due[0].body.as_str()),
            ("Daily cost budget: 80% used", "$8.00 of $10.00 today.")
        );
        assert!(due_budgets(&b, &today(0, 9.0), false, &none).is_empty());
    }

    #[test]
    fn notifier_sends_budget_alerts_once_and_remembers_them() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("alerts.json");
        let budgets = Budgets {
            daily_tokens: Some(1000.0),
            ..Default::default()
        };
        let rig = make(file.clone(), on(), budgets, Arc::new(Mutex::new(NOW)));
        let st = state(vec![], None);
        rig.alerts.evaluate(&st, Some(&today(900, 0.0)));
        rig.alerts.evaluate(&st, Some(&today(950, 0.0)));
        assert_eq!(rig.sent.lock().len(), 1);
        rig.alerts.evaluate(&st, Some(&today(1100, 0.0)));
        assert_eq!(rig.sent.lock().len(), 2);
        // Over also covers warn, so dropping back below 100% never repeats the warning.
        rig.alerts.evaluate(&st, Some(&today(990, 0.0)));
        assert_eq!(rig.sent.lock().len(), 2);
        let again = make(file, on(), budgets, Arc::new(Mutex::new(NOW)));
        again.alerts.evaluate(&st, Some(&today(1100, 0.0)));
        assert!(again.sent.lock().is_empty());
    }
}
