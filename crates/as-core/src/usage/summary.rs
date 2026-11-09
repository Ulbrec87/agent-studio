//! Period summary of the hourly buckets (Hydra `shared/usage/summary.ts`).

use std::collections::HashMap;

use chrono::{DateTime, TimeZone};

use super::periods::{period_range, period_slots, previous_range};
use super::types::{
    ModelUsage, PreviousUsage, ProviderTokens, ProviderUsage, TokenCounts, USAGE_PROVIDERS,
    UsageBucket, UsagePeriod, UsageProvider, UsageSlot, UsageSummary,
};

/// Buckets are one hour wide, so a bucket is in a range when its hour start is (hour granularity
/// is enough).
fn within<'a>(
    buckets: &'a [UsageBucket],
    start: i64,
    end: i64,
    project_key: Option<&'a str>,
) -> impl Iterator<Item = &'a UsageBucket> {
    buckets.iter().filter(move |b| {
        b.hour >= start && b.hour < end && project_key.is_none_or(|p| b.project_key == p)
    })
}

pub fn summarize<Tz: TimeZone>(
    buckets: &[UsageBucket],
    period: UsagePeriod,
    now: &DateTime<Tz>,
    project_key: Option<&str>,
) -> UsageSummary {
    let range = period_range(period, now);

    let mut providers: Vec<ProviderUsage> = USAGE_PROVIDERS
        .iter()
        .map(|p| ProviderUsage {
            provider: *p,
            tokens: TokenCounts::default(),
            total_tokens: 0,
            messages: 0,
            cost_usd: 0.0,
            cost_partial: false,
        })
        .collect();
    let slot_ranges = period_slots(period, now);
    let mut slots: Vec<UsageSlot> = slot_ranges
        .iter()
        .map(|s| UsageSlot {
            start: s.start,
            label: s.label.clone(),
            tokens_by_provider: ProviderTokens::default(),
        })
        .collect();
    let mut models: HashMap<(UsageProvider, String), ModelUsage> = HashMap::new();

    let (mut messages, mut cost_usd, mut cost_partial) = (0u64, 0.0f64, false);
    for b in within(buckets, range.start, range.end, project_key) {
        let tokens = b.tokens.total();
        messages += b.messages;
        cost_usd += b.cost_usd;
        if b.unpriced_messages > 0 {
            cost_partial = true;
        }
        let p = &mut providers[b.provider.index()];
        p.tokens = p.tokens.add(&b.tokens);
        p.total_tokens += tokens;
        p.messages += b.messages;
        p.cost_usd += b.cost_usd;
        if b.unpriced_messages > 0 {
            p.cost_partial = true;
        }

        let m = models
            .entry((b.provider, b.model.clone()))
            .or_insert_with(|| ModelUsage {
                model: b.model.clone(),
                provider: b.provider,
                total_tokens: 0,
                cost_usd: 0.0,
            });
        m.total_tokens += tokens;
        m.cost_usd += b.cost_usd;

        if let Some(i) = slot_ranges
            .iter()
            .position(|s| b.hour >= s.start && b.hour < s.end)
        {
            slots[i].tokens_by_provider.add(b.provider, tokens);
        }
    }

    let total: u64 = providers.iter().map(|p| p.total_tokens).sum();

    let prev_range = previous_range(period, now);
    let mut prev_count = 0usize;
    let (mut prev_tokens, mut prev_cost) = (0u64, 0.0f64);
    for b in within(buckets, prev_range.start, prev_range.end, project_key) {
        prev_count += 1;
        prev_tokens += b.tokens.total();
        prev_cost += b.cost_usd;
    }
    let previous = (prev_count > 0).then_some(PreviousUsage {
        total_tokens: prev_tokens,
        cost_usd: prev_cost,
    });

    let mut by_model: Vec<ModelUsage> = models.into_values().collect();
    by_model.sort_by(|a, b| {
        b.total_tokens
            .cmp(&a.total_tokens)
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.provider.cmp(&b.provider))
    });

    UsageSummary {
        period,
        range,
        total_tokens: total,
        messages,
        cost_usd,
        cost_partial,
        by_provider: providers,
        by_model,
        slots,
        previous,
        tokens_change_pct: previous
            .filter(|p| p.total_tokens > 0)
            .map(|p| (total as f64 - p.total_tokens as f64) / p.total_tokens as f64 * 100.0),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone};

    use super::*;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(2 * 3600).unwrap()
    }
    fn hour(y: i32, m: u32, d: u32, h: u32) -> i64 {
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
    fn bucket(h: i64) -> UsageBucket {
        UsageBucket {
            hour: h,
            provider: UsageProvider::Claude,
            model: "claude-opus-5".into(),
            project_key: "/work/a".into(),
            tokens: tok(100, 50),
            messages: 2,
            cost_usd: 1.0,
            unpriced_messages: 0,
        }
    }
    fn now() -> DateTime<FixedOffset> {
        tz().with_ymd_and_hms(2026, 10, 22, 15, 30, 0).unwrap()
    }
    fn sum(b: &[UsageBucket], p: UsagePeriod) -> UsageSummary {
        summarize(b, p, &now(), None)
    }

    #[test]
    fn lists_every_provider_in_order_with_zeros_when_empty() {
        let s = sum(&[], UsagePeriod::Today);
        let order: Vec<_> = s.by_provider.iter().map(|p| p.provider).collect();
        assert_eq!(order, [UsageProvider::Claude, UsageProvider::Codex]);
        assert!(
            s.by_provider
                .iter()
                .all(|p| p.total_tokens == 0 && p.messages == 0 && !p.cost_partial)
        );
        assert_eq!(s.total_tokens, 0);
        assert!(s.previous.is_none());
        assert!(s.tokens_change_pct.is_none());
        assert_eq!(s.slots.len(), 24);
    }

    #[test]
    fn totals_tokens_messages_and_cost_per_provider() {
        let mut codex = bucket(hour(2026, 10, 22, 10));
        codex.provider = UsageProvider::Codex;
        codex.model = "gpt-5".into();
        codex.tokens = tok(10, 0);
        codex.messages = 1;
        codex.cost_usd = 0.5;
        let s = sum(&[bucket(hour(2026, 10, 22, 9)), codex], UsagePeriod::Today);
        assert_eq!((s.total_tokens, s.messages), (160, 3));
        assert!((s.cost_usd - 1.5).abs() < 1e-9);
        assert_eq!(
            (
                s.by_provider[0].provider,
                s.by_provider[0].total_tokens,
                s.by_provider[0].messages
            ),
            (UsageProvider::Claude, 150, 2)
        );
        assert_eq!(
            (s.by_provider[1].total_tokens, s.by_provider[1].cost_usd),
            (10, 0.5)
        );
    }

    #[test]
    fn sorts_models_by_tokens_largest_first() {
        let mk = |h: u32, model: &str, input: u64| {
            let mut b = bucket(hour(2026, 10, 22, h));
            b.model = model.into();
            b.tokens = tok(input, 0);
            b
        };
        let s = sum(
            &[
                mk(9, "small", 5),
                mk(9, "big", 500),
                mk(10, "small", 5),
                mk(11, "mid", 100),
            ],
            UsagePeriod::Today,
        );
        let got: Vec<_> = s
            .by_model
            .iter()
            .map(|m| (m.model.as_str(), m.total_tokens))
            .collect();
        assert_eq!(got, [("big", 500), ("mid", 100), ("small", 10)]);
    }

    #[test]
    fn flags_partial_cost_when_a_record_had_no_price() {
        let mut unpriced = bucket(hour(2026, 10, 22, 9));
        unpriced.provider = UsageProvider::Codex;
        unpriced.unpriced_messages = 1;
        let s = sum(
            &[bucket(hour(2026, 10, 22, 9)), unpriced],
            UsagePeriod::Today,
        );
        assert!(s.cost_partial);
        assert!(!s.by_provider[0].cost_partial);
        assert!(s.by_provider[1].cost_partial);
        assert!(!sum(&[bucket(hour(2026, 10, 22, 9))], UsagePeriod::Today).cost_partial);
    }

    #[test]
    fn filters_by_project() {
        let mut b = bucket(hour(2026, 10, 22, 9));
        b.project_key = "/work/b".into();
        b.tokens = tok(1000, 0);
        let s = summarize(
            &[bucket(hour(2026, 10, 22, 9)), b],
            UsagePeriod::Today,
            &now(),
            Some("/work/b"),
        );
        assert_eq!(s.total_tokens, 1000);
        let mut empty = bucket(hour(2026, 10, 22, 9));
        empty.project_key = String::new();
        assert_eq!(
            summarize(&[empty], UsagePeriod::Today, &now(), Some("")).total_tokens,
            150
        );
    }

    #[test]
    fn excludes_buckets_outside_the_range() {
        let s = sum(
            &[
                bucket(hour(2026, 10, 21, 23)), // yesterday
                bucket(hour(2026, 10, 22, 0)),  // in
                bucket(hour(2026, 10, 22, 15)), // in (hour starts before now)
                bucket(hour(2026, 10, 22, 16)), // after now
            ],
            UsagePeriod::Today,
        );
        assert_eq!(s.total_tokens, 300);
    }

    fn pt(claude: u64, codex: u64) -> ProviderTokens {
        ProviderTokens { claude, codex }
    }

    #[test]
    fn slots_serialize_tokens_as_an_object_keyed_by_provider() {
        let s = sum(&[bucket(hour(2026, 10, 22, 9))], UsagePeriod::Today);
        let v = serde_json::to_value(&s.slots[9]).unwrap();
        assert_eq!(
            v["tokensByProvider"],
            serde_json::json!({ "claude": 150, "codex": 0 })
        );
    }

    #[test]
    fn puts_tokens_in_the_right_slots() {
        let mut codex = bucket(hour(2026, 10, 22, 9));
        codex.provider = UsageProvider::Codex;
        codex.tokens = tok(7, 0);
        let s = sum(&[bucket(hour(2026, 10, 22, 9)), codex], UsagePeriod::Today);
        assert_eq!(s.slots[9].tokens_by_provider, pt(150, 7));
        assert_eq!(s.slots[8].tokens_by_provider, pt(0, 0));
        let week = sum(&[bucket(hour(2026, 10, 20, 12))], UsagePeriod::Week);
        assert_eq!(
            (
                week.slots[1].label.as_str(),
                week.slots[1].tokens_by_provider
            ),
            ("Tue", pt(150, 0))
        );
    }

    #[test]
    fn compares_with_the_previous_range() {
        let mk = |d: u32, h: u32, input: u64, cost: f64| {
            let mut b = bucket(hour(2026, 10, d, h));
            b.tokens = tok(input, 0);
            b.cost_usd = cost;
            b
        };
        let s = sum(
            &[
                mk(22, 9, 300, 3.0),
                mk(21, 9, 200, 2.0),
                mk(21, 20, 999, 1.0),
            ], // last: after 15:30 yesterday
            UsagePeriod::Today,
        );
        assert_eq!(
            s.previous,
            Some(PreviousUsage {
                total_tokens: 200,
                cost_usd: 2.0
            })
        );
        assert!((s.tokens_change_pct.unwrap() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn no_change_percentage_without_previous_data_or_when_it_is_zero() {
        let none = sum(&[bucket(hour(2026, 10, 22, 9))], UsagePeriod::Today);
        assert!(none.previous.is_none() && none.tokens_change_pct.is_none());
        let mut zero = bucket(hour(2026, 10, 21, 9));
        zero.tokens = tok(0, 0);
        let s = sum(&[bucket(hour(2026, 10, 22, 9)), zero], UsagePeriod::Today);
        assert_eq!(
            s.previous,
            Some(PreviousUsage {
                total_tokens: 0,
                cost_usd: 1.0
            })
        );
        assert!(s.tokens_change_pct.is_none());
    }

    #[test]
    fn a_decrease_is_a_negative_percentage() {
        let mut a = bucket(hour(2026, 10, 22, 9));
        a.tokens = tok(50, 0);
        let mut b = bucket(hour(2026, 10, 21, 9));
        b.tokens = tok(200, 0);
        let s = sum(&[a, b], UsagePeriod::Today);
        assert!((s.tokens_change_pct.unwrap() + 75.0).abs() < 1e-9);
    }
}
