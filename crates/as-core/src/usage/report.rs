//! Daily report of the hourly buckets (Hydra `shared/usage/report.ts`).

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};

use super::claude_line::claude_project_key_label;
use super::types::{TokenCounts, UsageBucket, UsageProvider};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum UsageReportProvider {
    #[default]
    All,
    Claude,
    Codex,
}

impl UsageReportProvider {
    fn matches(self, p: UsageProvider) -> bool {
        match self {
            UsageReportProvider::All => true,
            UsageReportProvider::Claude => p == UsageProvider::Claude,
            UsageReportProvider::Codex => p == UsageProvider::Codex,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageReportModel {
    pub model: String,
    pub tokens: TokenCounts,
    pub total_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageReportProject {
    pub project_key: String,
    /// Last path segment of the project key.
    pub label: String,
    pub total_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageReportDay {
    /// Local calendar day, `YYYY-MM-DD`.
    pub date: String,
    pub tokens: TokenCounts,
    pub total_tokens: u64,
    pub messages: u64,
    pub cost_usd: f64,
    /// Some messages had no price, so `cost_usd` is a lower bound.
    pub cost_partial: bool,
    /// Sorted by `total_tokens`, largest first.
    pub models: Vec<UsageReportModel>,
    /// Sorted by `total_tokens`, largest first.
    pub projects: Vec<UsageReportProject>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub provider: UsageReportProvider,
    /// One entry per local calendar day, oldest first, empty days included.
    pub daily: Vec<UsageReportDay>,
    /// Epoch ms.
    pub generated_at: i64,
}

pub const REPORT_MIN_DAYS: u32 = 7;
pub const REPORT_MAX_DAYS: u32 = 90;

pub fn date_key(d: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

#[derive(Default)]
struct Acc {
    models: HashMap<String, UsageReportModel>,
    projects: HashMap<String, UsageReportProject>,
}

/// Daily report of the last `days` local calendar days ending with the day of `now`.
pub fn build_usage_report<Tz: TimeZone>(
    buckets: &[UsageBucket],
    days: u32,
    provider: UsageReportProvider,
    now: &DateTime<Tz>,
) -> UsageReport {
    let days = days.max(1);
    let tz = now.timezone();
    let today = now.date_naive();

    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<UsageReportDay> = Vec::new();
    let mut accs: Vec<Acc> = Vec::new();
    for i in (0..days).rev() {
        let d = today
            .checked_sub_signed(Duration::days(i64::from(i)))
            .unwrap_or(today);
        let date = date_key(d);
        index.insert(date.clone(), out.len());
        out.push(UsageReportDay {
            date,
            tokens: TokenCounts::default(),
            total_tokens: 0,
            messages: 0,
            cost_usd: 0.0,
            cost_partial: false,
            models: Vec::new(),
            projects: Vec::new(),
        });
        accs.push(Acc::default());
    }

    for b in buckets {
        if !provider.matches(b.provider) {
            continue;
        }
        let Some(local) = tz.timestamp_millis_opt(b.hour).single() else {
            continue;
        };
        let Some(&i) = index.get(&date_key(local.date_naive())) else {
            continue;
        };
        let total = b.tokens.total();
        let day = &mut out[i];
        day.tokens = day.tokens.add(&b.tokens);
        day.total_tokens += total;
        day.messages += b.messages;
        day.cost_usd += b.cost_usd;
        if b.unpriced_messages > 0 {
            day.cost_partial = true;
        }
        let acc = &mut accs[i];
        let m = acc
            .models
            .entry(b.model.clone())
            .or_insert_with(|| UsageReportModel {
                model: b.model.clone(),
                tokens: TokenCounts::default(),
                total_tokens: 0,
                cost_usd: 0.0,
            });
        m.tokens = m.tokens.add(&b.tokens);
        m.total_tokens += total;
        m.cost_usd += b.cost_usd;
        let p = acc
            .projects
            .entry(b.project_key.clone())
            .or_insert_with(|| UsageReportProject {
                project_key: b.project_key.clone(),
                label: claude_project_key_label(&b.project_key),
                total_tokens: 0,
                cost_usd: 0.0,
            });
        p.total_tokens += total;
        p.cost_usd += b.cost_usd;
    }

    for (day, acc) in out.iter_mut().zip(accs) {
        let mut models: Vec<_> = acc.models.into_values().collect();
        models.sort_by(|a, b| {
            b.total_tokens
                .cmp(&a.total_tokens)
                .then_with(|| a.model.cmp(&b.model))
        });
        let mut projects: Vec<_> = acc.projects.into_values().collect();
        projects.sort_by(|a, b| {
            b.total_tokens
                .cmp(&a.total_tokens)
                .then_with(|| a.project_key.cmp(&b.project_key))
        });
        day.models = models;
        day.projects = projects;
    }
    UsageReport {
        provider,
        daily: out,
        generated_at: now.timestamp_millis(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(2 * 3600).unwrap()
    }
    fn at(y: i32, m: u32, d: u32, h: u32) -> i64 {
        tz().with_ymd_and_hms(y, m, d, h, 0, 0)
            .unwrap()
            .timestamp_millis()
    }
    fn tok(input: u64, output: u64) -> TokenCounts {
        TokenCounts {
            input,
            output,
            ..Default::default()
        }
    }
    fn now() -> DateTime<FixedOffset> {
        tz().with_ymd_and_hms(2026, 10, 23, 15, 30, 0).unwrap()
    }
    fn bucket(hour: i64) -> UsageBucket {
        UsageBucket {
            hour,
            provider: UsageProvider::Claude,
            model: "claude-opus-5".into(),
            project_key: "/work/app".into(),
            tokens: tok(100, 50),
            messages: 2,
            cost_usd: 1.0,
            unpriced_messages: 0,
        }
    }
    fn report(b: &[UsageBucket], days: u32, p: UsageReportProvider) -> UsageReport {
        build_usage_report(b, days, p, &now())
    }

    #[test]
    fn lists_every_local_day_oldest_first_empty_days_included() {
        let r = report(&[], 7, UsageReportProvider::All);
        let dates: Vec<_> = r.daily.iter().map(|d| d.date.as_str()).collect();
        assert_eq!(
            dates,
            [
                "2026-10-17",
                "2026-10-18",
                "2026-10-19",
                "2026-10-20",
                "2026-10-21",
                "2026-10-22",
                "2026-10-23"
            ]
        );
        assert!(
            r.daily
                .iter()
                .all(|d| d.total_tokens == 0 && d.models.is_empty() && !d.cost_partial)
        );
        assert_eq!(r.provider, UsageReportProvider::All);
        assert_eq!(r.generated_at, now().timestamp_millis());
    }

    #[test]
    fn crosses_month_boundaries() {
        let n = tz().with_ymd_and_hms(2026, 11, 1, 9, 0, 0).unwrap();
        let r = build_usage_report(&[], 3, UsageReportProvider::All, &n);
        let dates: Vec<_> = r.daily.iter().map(|d| d.date.as_str()).collect();
        assert_eq!(dates, ["2026-10-30", "2026-10-31", "2026-11-01"]);
    }

    #[test]
    fn assigns_hours_to_local_days_at_the_boundaries() {
        let mut a = bucket(at(2026, 10, 22, 23));
        a.tokens = tok(1, 0);
        let mut b = bucket(at(2026, 10, 23, 0));
        b.tokens = tok(10, 0);
        let r = report(&[a, b], 7, UsageReportProvider::All);
        let by: HashMap<_, _> = r
            .daily
            .iter()
            .map(|d| (d.date.as_str(), d.total_tokens))
            .collect();
        assert_eq!(by["2026-10-22"], 1);
        assert_eq!(by["2026-10-23"], 10);
    }

    #[test]
    fn ignores_buckets_outside_the_window() {
        let r = report(
            &[bucket(at(2026, 10, 16, 23)), bucket(at(2026, 10, 24, 1))],
            7,
            UsageReportProvider::All,
        );
        assert_eq!(r.daily.iter().map(|d| d.total_tokens).sum::<u64>(), 0);
    }

    #[test]
    fn filters_by_provider_and_sums_both_for_all() {
        let mut claude = bucket(at(2026, 10, 23, 9));
        claude.tokens = tok(100, 0);
        let mut codex = bucket(at(2026, 10, 23, 10));
        codex.provider = UsageProvider::Codex;
        codex.model = "gpt-5".into();
        codex.tokens = tok(40, 0);
        let buckets = [claude, codex];
        let today = |p| report(&buckets, 7, p).daily.pop().unwrap();
        assert_eq!(today(UsageReportProvider::Claude).total_tokens, 100);
        assert_eq!(today(UsageReportProvider::Codex).total_tokens, 40);
        assert_eq!(today(UsageReportProvider::Codex).models[0].model, "gpt-5");
        let all = today(UsageReportProvider::All);
        assert_eq!(
            (all.total_tokens, all.messages, all.cost_usd),
            (140, 4, 2.0)
        );
    }

    #[test]
    fn merges_models_and_projects_and_sorts_by_tokens_desc() {
        let mk = |h: u32, model: &str, project: &str, input: u64, output: u64| {
            let mut b = bucket(at(2026, 10, 23, h));
            b.model = model.into();
            b.project_key = project.into();
            b.tokens = tok(input, output);
            b
        };
        let buckets = [
            mk(9, "a", "/w/small", 10, 0),
            mk(10, "b", "/w/big", 500, 100),
            mk(11, "a", "/w/small", 20, 0),
        ];
        let day = report(&buckets, 7, UsageReportProvider::All)
            .daily
            .pop()
            .unwrap();
        let models: Vec<_> = day
            .models
            .iter()
            .map(|m| (m.model.as_str(), m.total_tokens, m.tokens.input))
            .collect();
        assert_eq!(models, [("b", 600, 500), ("a", 30, 30)]);
        let projects: Vec<_> = day
            .projects
            .iter()
            .map(|p| (p.label.as_str(), p.project_key.as_str(), p.total_tokens))
            .collect();
        assert_eq!(
            projects,
            [("big", "/w/big", 600), ("small", "/w/small", 30)]
        );
        assert_eq!(day.tokens.input, 530);
    }

    #[test]
    fn marks_a_day_as_partial_when_any_message_was_unpriced() {
        let mut partial = bucket(at(2026, 10, 22, 9));
        partial.unpriced_messages = 1;
        partial.cost_usd = 0.0;
        let r = report(
            &[bucket(at(2026, 10, 23, 9)), partial],
            7,
            UsageReportProvider::All,
        );
        assert!(!r.daily[6].cost_partial);
        assert!(r.daily[5].cost_partial);
    }

    #[test]
    fn labels_an_empty_project_key_as_unknown() {
        let mut b = bucket(at(2026, 10, 23, 9));
        b.project_key = String::new();
        let day = report(&[b], 7, UsageReportProvider::All)
            .daily
            .pop()
            .unwrap();
        assert_eq!(day.projects[0].label, "Unknown");
    }
}
