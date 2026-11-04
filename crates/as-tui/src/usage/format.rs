//! Usage number and time formatting (Hydra `src/components/Usage/format.ts`).

use chrono::{DateTime, TimeZone};

use crate::overview::format::{format_count, format_duration};

/// `48.2M`, `950k`, `12`. One decimal below 100 of a unit, none above.
pub fn format_compact_tokens(n: u64) -> String {
    const UNITS: [(f64, &str); 3] = [(1e9, "B"), (1e6, "M"), (1e3, "k")];
    for (i, (size, suffix)) in UNITS.iter().enumerate() {
        if (n as f64) < *size {
            continue;
        }
        let scaled = n as f64 / size;
        let text = if scaled >= 100.0 {
            format!("{}", scaled.round() as u64)
        } else {
            let t = format!("{scaled:.1}");
            t.strip_suffix(".0").unwrap_or(&t).to_string()
        };
        // 999.95k rounds up to 1000k: say 1M instead.
        if text == "1000" && i > 0 {
            return format!("1{}", UNITS[i - 1].1);
        }
        return format!("{text}{suffix}");
    }
    n.to_string()
}

/// `$31.40`, `$0.42`, `<$0.01` (non-zero below a cent), `$0.00`.
pub fn format_usd(n: f64) -> String {
    if n > 0.0 && n < 0.005 {
        return "<$0.01".to_string();
    }
    let cents = format!("{:.2}", n.max(0.0));
    let (whole, frac) = cents.split_once('.').unwrap_or((&cents, "00"));
    let whole = format_count(whole.parse().unwrap_or(0));
    format!("${whole}.{frac}")
}

/// `182431` -> `182,431`.
pub fn format_exact_tokens(n: u64) -> String {
    format_count(n)
}

/// `just now`, `6 min`, `2 h`, `3 d`: a past span, without "ago".
pub fn format_age(ms: i64) -> String {
    let minutes = ms.max(0) / 60_000;
    if minutes < 1 {
        return "just now".into();
    }
    if minutes < 60 {
        return format!("{minutes} min");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours} h");
    }
    format!("{} d", hours / 24)
}

/// `just now` or `1 min ago`.
pub fn format_updated(at: i64, now: i64) -> String {
    let age = format_age(now - at);
    if age == "just now" {
        age
    } else {
        format!("{age} ago")
    }
}

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// `resets in 1h 48m` under 24 h, else `resets Mon 09:00` (local time).
pub fn format_resets<Tz: TimeZone>(resets_at: Option<i64>, now: i64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(at) = resets_at else {
        return String::new();
    };
    let left = at - now;
    if left <= 0 {
        return "resetting now".into();
    }
    if left < DAY_MS {
        let span = if left < 60_000 {
            "<1m".to_string()
        } else {
            format_duration(left, true)
        };
        return format!("resets in {span}");
    }
    let when: Option<DateTime<Tz>> = tz.timestamp_millis_opt(at).single();
    match when {
        Some(d) => format!("resets {}", d.format("%a %H:%M")),
        None => String::new(),
    }
}

/// `▼ 8% tokens vs last week`, `no change vs yesterday`; `None` when there is nothing to compare.
pub fn format_change(
    change: Option<f64>,
    period: as_core::usage::UsagePeriod,
    noun: &str,
) -> Option<String> {
    use as_core::usage::UsagePeriod;
    let change = change?;
    let versus = match period {
        UsagePeriod::Today => "yesterday",
        UsagePeriod::Week => "last week",
        UsagePeriod::Month => "last month",
    };
    let rounded = change.round() as i64;
    if rounded == 0 {
        return Some(format!("no change vs {versus}"));
    }
    let noun = if noun.is_empty() {
        String::new()
    } else {
        format!(" {noun}")
    };
    Some(format!(
        "{} {}%{noun} vs {versus}",
        if rounded > 0 { '\u{25B2}' } else { '\u{25BC}' },
        rounded.abs()
    ))
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone};

    use super::*;

    #[test]
    fn compacts_by_unit() {
        assert_eq!(format_compact_tokens(12), "12");
        assert_eq!(format_compact_tokens(950_000), "950k");
        assert_eq!(format_compact_tokens(48_200_000), "48.2M");
        assert_eq!(format_compact_tokens(3_000_000_000), "3B");
        assert_eq!(format_compact_tokens(1_500), "1.5k");
    }

    #[test]
    fn rolls_over_at_the_unit_boundary() {
        assert_eq!(format_compact_tokens(999_960), "1M");
        assert_eq!(format_compact_tokens(0), "0");
    }

    #[test]
    fn formats_dollars_and_cents() {
        assert_eq!(format_usd(31.4), "$31.40");
        assert_eq!(format_usd(0.42), "$0.42");
        assert_eq!(format_usd(1234.5), "$1,234.50");
        assert_eq!(format_usd(0.0), "$0.00");
        assert_eq!(format_usd(0.001), "<$0.01");
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(format_exact_tokens(48_213_456), "48,213,456");
    }

    #[test]
    fn age_text() {
        assert_eq!(format_age(10_000), "just now");
        assert_eq!(format_age(6 * 60_000), "6 min");
        assert_eq!(format_age(2 * 3_600_000), "2 h");
        assert_eq!(format_age(3 * 86_400_000), "3 d");
        assert_eq!(format_updated(0, 60_000), "1 min ago");
        assert_eq!(format_updated(0, 1000), "just now");
    }

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    #[test]
    fn resets_are_relative_under_24_hours() {
        let now = tz()
            .with_ymd_and_hms(2026, 10, 23, 12, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            format_resets(Some(now + 108 * 60_000), now, &tz()),
            "resets in 1h 48m"
        );
        assert_eq!(
            format_resets(Some(now + 30_000), now, &tz()),
            "resets in <1m"
        );
    }

    #[test]
    fn resets_use_weekday_and_time_from_24_hours_on() {
        let now = tz()
            .with_ymd_and_hms(2026, 10, 23, 12, 0, 0)
            .unwrap()
            .timestamp_millis();
        let at = tz()
            .with_ymd_and_hms(2026, 10, 26, 9, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(format_resets(Some(at), now, &tz()), "resets Mon 09:00");
    }

    #[test]
    fn resets_unknown_and_past() {
        let now = 1_000_000;
        assert_eq!(format_resets(None, now, &tz()), "");
        assert_eq!(format_resets(Some(now - 1), now, &tz()), "resetting now");
    }

    #[test]
    fn change_text() {
        use as_core::usage::UsagePeriod::*;
        assert_eq!(
            format_change(Some(12.0), Today, "tokens").as_deref(),
            Some("\u{25B2} 12% tokens vs yesterday")
        );
        assert_eq!(
            format_change(Some(-8.4), Week, "").as_deref(),
            Some("\u{25BC} 8% vs last week")
        );
        assert_eq!(
            format_change(Some(0.2), Month, "").as_deref(),
            Some("no change vs last month")
        );
        assert_eq!(format_change(None, Today, ""), None);
    }
}
