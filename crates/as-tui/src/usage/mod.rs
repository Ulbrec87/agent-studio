//! Usage and limits in the TUI (Hydra `src/hooks/useUsage.ts` + `src/components/Usage*`): the
//! state pushed by the daemon, the summaries and reports fetched on demand, and the choices of
//! the report overlay. Rendering is in `ui::usage`; this module is plain data.

pub mod format;
pub mod model;

use std::collections::{HashMap, HashSet};

use as_core::usage::report::{UsageReport, UsageReportProvider};
use as_core::usage::{UsagePeriod, UsageState, UsageSummary};

use crate::app::Effect;

/// Days the daemon is asked for; the overlay shows the last 7 or 30 of them.
pub const REPORT_DAYS: u32 = 30;
pub const TREND_WINDOWS: [usize; 2] = [7, 30];

/// The providers of the overlay's segmented toggle, in order.
pub const REPORT_TABS: [UsageReportProvider; 3] = [
    UsageReportProvider::Claude,
    UsageReportProvider::Codex,
    UsageReportProvider::All,
];

pub fn report_tab_label(p: UsageReportProvider) -> &'static str {
    match p {
        UsageReportProvider::Claude => "Claude",
        UsageReportProvider::Codex => "Codex",
        UsageReportProvider::All => "Both",
    }
}

/// Something the UI shows that has to be fetched.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Want {
    /// A period, for all projects (`""`) or one project directory.
    Summary(UsagePeriod, String),
    Report(UsageReportProvider),
}

/// The report overlay (`prefix u`): scroll, provider toggle, trend window, limits filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageOverlay {
    pub scroll: usize,
    pub provider: UsageReportProvider,
    /// Index into [`TREND_WINDOWS`].
    pub trend: usize,
    /// Plan limits: only the windows near their limit.
    pub low_only: bool,
}

impl Default for UsageOverlay {
    fn default() -> Self {
        Self {
            scroll: 0,
            provider: UsageReportProvider::All,
            trend: 0,
            low_only: false,
        }
    }
}

impl UsageOverlay {
    pub fn trend_days(&self) -> usize {
        TREND_WINDOWS[self.trend.min(TREND_WINDOWS.len() - 1)]
    }

    pub fn next_provider(&mut self, delta: isize) {
        let at = REPORT_TABS
            .iter()
            .position(|p| *p == self.provider)
            .unwrap_or(0) as isize;
        let n = REPORT_TABS.len() as isize;
        self.provider = REPORT_TABS[(at + delta).rem_euclid(n) as usize];
    }
}

#[derive(Debug, Default)]
pub struct UsageUi {
    /// `None` until the daemon answered (or when it does not know usage: an older daemon).
    pub state: Option<UsageState>,
    pub summaries: HashMap<(UsagePeriod, String), UsageSummary>,
    pub reports: HashMap<UsageReportProvider, UsageReport>,
    /// The `data_version` each fetched thing was fetched at.
    fetched: HashMap<Want, u64>,
    pending: HashSet<Want>,
    /// Why the usage data could not be loaded.
    pub error: Option<String>,
    /// Period of the Overview card.
    pub card_period: CardPeriod,
    /// A manual refresh is running.
    pub refreshing: bool,
    /// What the daemon was last told about visibility.
    visible_sent: bool,
}

/// The Overview card shows Today / Week / Month (Hydra default: week).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardPeriod(pub UsagePeriod);

impl Default for CardPeriod {
    fn default() -> Self {
        CardPeriod(UsagePeriod::Week)
    }
}

impl CardPeriod {
    pub fn next(self) -> Self {
        CardPeriod(match self.0 {
            UsagePeriod::Today => UsagePeriod::Week,
            UsagePeriod::Week => UsagePeriod::Month,
            UsagePeriod::Month => UsagePeriod::Today,
        })
    }
}

pub fn period_label(p: UsagePeriod) -> &'static str {
    match p {
        UsagePeriod::Today => "Today",
        UsagePeriod::Week => "Week",
        UsagePeriod::Month => "Month",
    }
}

impl UsageUi {
    pub fn data_version(&self) -> u64 {
        self.state.as_ref().map_or(0, |s| s.data_version)
    }

    pub fn set_state(&mut self, state: UsageState) {
        self.error = None;
        self.state = Some(state);
    }

    pub fn summary(&self, period: UsagePeriod, project: &str) -> Option<&UsageSummary> {
        self.summaries.get(&(period, project.to_string()))
    }

    pub fn set_summary(&mut self, project: String, summary: UsageSummary) {
        let want = Want::Summary(summary.period, project.clone());
        self.landed(&want);
        self.summaries.insert((summary.period, project), summary);
    }

    pub fn set_report(&mut self, report: UsageReport) {
        let want = Want::Report(report.provider);
        self.landed(&want);
        self.reports.insert(report.provider, report);
    }

    fn landed(&mut self, want: &Want) {
        self.pending.remove(want);
        let version = self.data_version();
        self.fetched.insert(want.clone(), version);
    }

    /// A request failed: stop waiting for it (it is asked again when something changes).
    pub fn failed(&mut self, error: String) {
        self.pending.clear();
        self.refreshing = false;
        self.error = Some(error);
    }

    pub fn reset_connection(&mut self) {
        self.pending.clear();
        self.visible_sent = false;
    }

    /// Effects for what is wanted and missing, or older than the data: nothing is asked twice.
    pub fn fetch_effects(&mut self, wants: &[Want]) -> Vec<Effect> {
        if self.state.is_none() || self.error.is_some() {
            return Vec::new();
        }
        let version = self.data_version();
        let mut fx = Vec::new();
        for want in wants {
            if self.pending.contains(want) {
                continue;
            }
            if self.fetched.get(want).is_some_and(|v| *v >= version) {
                continue;
            }
            self.pending.insert(want.clone());
            fx.push(match want {
                Want::Summary(period, project) => Effect::LoadUsageSummary {
                    period: *period,
                    project: project.clone(),
                },
                Want::Report(provider) => Effect::LoadUsageReport {
                    provider: *provider,
                    days: REPORT_DAYS,
                },
            });
        }
        fx
    }

    /// `Some(visible)` when the daemon has to be told the visibility changed.
    pub fn visibility_change(&mut self, visible: bool) -> Option<bool> {
        if self.visible_sent == visible {
            return None;
        }
        self.visible_sent = visible;
        Some(visible)
    }
}

/// Sum of a report's models over the last `days` days: `(model, tokens, cost)`, largest first.
pub fn models_over(report: &UsageReport, days: usize) -> Vec<(String, u64, f64)> {
    let mut map: HashMap<&str, (u64, f64)> = HashMap::new();
    for day in report.daily.iter().rev().take(days) {
        for m in &day.models {
            let e = map.entry(m.model.as_str()).or_default();
            e.0 += m.total_tokens;
            e.1 += m.cost_usd;
        }
    }
    let mut out: Vec<_> = map
        .into_iter()
        .map(|(m, (t, c))| (m.to_string(), t, c))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Sum of a report's projects over the last `days` days: `(label, tokens, cost)`, largest first.
pub fn projects_over(report: &UsageReport, days: usize) -> Vec<(String, u64, f64)> {
    let mut map: HashMap<&str, (&str, u64, f64)> = HashMap::new();
    for day in report.daily.iter().rev().take(days) {
        for p in &day.projects {
            let e = map
                .entry(p.project_key.as_str())
                .or_insert((p.label.as_str(), 0, 0.0));
            e.1 += p.total_tokens;
            e.2 += p.cost_usd;
        }
    }
    let mut out: Vec<_> = map
        .into_values()
        .map(|(l, t, c)| (l.to_string(), t, c))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

#[cfg(test)]
mod tests {
    use as_core::usage::report::build_usage_report;
    use as_core::usage::types::{TokenCounts, UsageBucket, UsageProvider};
    use chrono::{FixedOffset, TimeZone};

    use super::model::fixtures::usage_state;
    use super::*;

    #[test]
    fn nothing_is_asked_before_the_state_and_nothing_twice() {
        let mut ui = UsageUi::default();
        let wants = [Want::Summary(UsagePeriod::Today, String::new())];
        assert!(ui.fetch_effects(&wants).is_empty());
        ui.set_state(usage_state());
        assert_eq!(ui.fetch_effects(&wants).len(), 1);
        assert!(ui.fetch_effects(&wants).is_empty()); // pending
    }

    #[test]
    fn a_new_data_version_asks_again() {
        let mut ui = UsageUi::default();
        ui.set_state(usage_state());
        let want = Want::Report(UsageReportProvider::All);
        assert_eq!(ui.fetch_effects(std::slice::from_ref(&want)).len(), 1);
        let report = build_usage_report(&[], 7, UsageReportProvider::All, &chrono::Utc::now());
        ui.set_report(report);
        assert!(ui.fetch_effects(std::slice::from_ref(&want)).is_empty());
        let mut next = usage_state();
        next.data_version += 1;
        ui.set_state(next);
        assert_eq!(ui.fetch_effects(&[want]).len(), 1);
        // the old report stays on screen meanwhile
        assert!(ui.reports.contains_key(&UsageReportProvider::All));
    }

    #[test]
    fn a_failure_stops_the_asking_until_a_state_arrives() {
        let mut ui = UsageUi::default();
        ui.set_state(usage_state());
        let wants = [Want::Summary(UsagePeriod::Week, String::new())];
        ui.fetch_effects(&wants);
        ui.failed("boom".into());
        assert!(ui.fetch_effects(&wants).is_empty());
        ui.set_state(usage_state());
        assert_eq!(ui.fetch_effects(&wants).len(), 1);
    }

    #[test]
    fn visibility_is_reported_only_on_change() {
        let mut ui = UsageUi::default();
        assert_eq!(ui.visibility_change(false), None);
        assert_eq!(ui.visibility_change(true), Some(true));
        assert_eq!(ui.visibility_change(true), None);
        ui.reset_connection();
        assert_eq!(ui.visibility_change(true), Some(true));
        assert_eq!(ui.visibility_change(false), Some(false));
    }

    #[test]
    fn overlay_cycles_providers() {
        let mut o = UsageOverlay::default();
        assert_eq!(o.provider, UsageReportProvider::All);
        o.next_provider(1);
        assert_eq!(o.provider, UsageReportProvider::Claude);
        o.next_provider(-1);
        assert_eq!(o.provider, UsageReportProvider::All);
        assert_eq!(o.trend_days(), 7);
    }

    #[test]
    fn models_and_projects_are_summed_over_the_window() {
        let tz = FixedOffset::east_opt(0).unwrap();
        let now = tz.with_ymd_and_hms(2026, 10, 23, 15, 0, 0).unwrap();
        let bucket = |day: u32, model: &str, project: &str, input: u64| UsageBucket {
            hour: tz
                .with_ymd_and_hms(2026, 10, day, 9, 0, 0)
                .unwrap()
                .timestamp_millis(),
            provider: UsageProvider::Claude,
            model: model.into(),
            project_key: project.into(),
            tokens: TokenCounts {
                input,
                ..Default::default()
            },
            messages: 1,
            cost_usd: 1.0,
            unpriced_messages: 0,
        };
        let buckets = [
            bucket(23, "a", "/w/x", 10),
            bucket(22, "a", "/w/x", 5),
            bucket(22, "b", "/w/y", 100),
            bucket(1, "b", "/w/y", 1000), // outside 7 days
        ];
        let report = build_usage_report(&buckets, 30, UsageReportProvider::All, &now);
        assert_eq!(
            models_over(&report, 7),
            [("b".to_string(), 100, 1.0), ("a".to_string(), 15, 2.0)]
        );
        assert_eq!(models_over(&report, 30)[0], ("b".to_string(), 1100, 2.0));
        assert_eq!(projects_over(&report, 7)[0].0, "y");
    }
}
