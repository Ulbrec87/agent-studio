//! Period windows in the local time zone (Hydra `shared/usage/periods.ts`). Everything is calendar
//! arithmetic on the wall clock, never fixed-millisecond subtraction, so DST changes stay correct.
//! The functions are generic over the zone so tests pin one; production passes `chrono::Local`.

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone,
    Timelike,
};

use super::types::{TimeRange, UsagePeriod};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodSlot {
    pub start: i64,
    pub end: i64,
    pub label: String,
}

const DAY_LABELS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Epoch ms of a wall-clock time. A time that does not exist (spring-forward gap) moves forward by
/// the gap, an ambiguous one (fall back) takes the first, like JavaScript's `Date`.
fn local_ms<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> i64 {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(d) => d.timestamp_millis(),
        LocalResult::Ambiguous(first, _) => first.timestamp_millis(),
        LocalResult::None => {
            for hours in 1..=3 {
                if let Some(d) = tz
                    .from_local_datetime(&(naive + Duration::hours(hours)))
                    .earliest()
                {
                    return d.timestamp_millis();
                }
            }
            naive.and_utc().timestamp_millis()
        }
    }
}

fn midnight<Tz: TimeZone>(tz: &Tz, date: NaiveDate) -> i64 {
    local_ms(tz, date.and_time(NaiveTime::MIN))
}

fn plus_days(date: NaiveDate, days: i64) -> NaiveDate {
    date.checked_add_signed(Duration::days(days))
        .unwrap_or(date)
}

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap_or(NaiveDate::MIN)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    (ymd(ny, nm, 1) - ymd(year, month, 1)).num_days() as u32
}

/// Monday of the week containing `d`. A Sunday belongs to the week that began the previous Monday.
fn week_start(d: NaiveDate) -> NaiveDate {
    plus_days(d, -i64::from(d.weekday().num_days_from_monday()))
}

/// From the start of the period (local time) up to `now`.
pub fn period_range<Tz: TimeZone>(period: UsagePeriod, now: &DateTime<Tz>) -> TimeRange {
    let tz = now.timezone();
    let date = now.date_naive();
    let end = now.timestamp_millis();
    let start = match period {
        UsagePeriod::Today => midnight(&tz, date),
        UsagePeriod::Week => midnight(&tz, week_start(date)),
        UsagePeriod::Month => midnight(&tz, ymd(date.year(), date.month(), 1)),
    };
    TimeRange { start, end }
}

/// The same elapsed span one period earlier. A month that is too short ends at its last day's end.
pub fn previous_range<Tz: TimeZone>(period: UsagePeriod, now: &DateTime<Tz>) -> TimeRange {
    let tz = now.timezone();
    let date = now.date_naive();
    let clock: NaiveTime = now.time();
    let clock = NaiveTime::from_hms_milli_opt(
        clock.hour(),
        clock.minute(),
        clock.second(),
        (clock.nanosecond() / 1_000_000).min(999),
    )
    .unwrap_or(NaiveTime::MIN);
    let at = |d: NaiveDate| local_ms(&tz, d.and_time(clock));
    match period {
        UsagePeriod::Today => {
            let y = plus_days(date, -1);
            TimeRange {
                start: midnight(&tz, y),
                end: at(y),
            }
        }
        UsagePeriod::Week => {
            let offset = i64::from(date.weekday().num_days_from_monday());
            TimeRange {
                start: midnight(&tz, plus_days(date, -offset - 7)),
                end: at(plus_days(date, -7)),
            }
        }
        UsagePeriod::Month => {
            let (year, month) = (date.year(), date.month());
            let (py, pm) = if month == 1 {
                (year - 1, 12)
            } else {
                (year, month - 1)
            };
            let length = days_in_month(py, pm);
            TimeRange {
                start: midnight(&tz, ymd(py, pm, 1)),
                end: if date.day() > length {
                    midnight(&tz, ymd(year, month, 1))
                } else {
                    at(ymd(py, pm, date.day()))
                },
            }
        }
    }
}

/// Chart slots of the whole period, including those after `now` (they stay empty). On a DST day an
/// hourly slot can be empty or doubled, which is what the wall clock does.
pub fn period_slots<Tz: TimeZone>(period: UsagePeriod, now: &DateTime<Tz>) -> Vec<PeriodSlot> {
    let tz = now.timezone();
    let date = now.date_naive();
    match period {
        UsagePeriod::Today => (0..24)
            .map(|h| PeriodSlot {
                start: local_ms(&tz, date.and_hms_opt(h, 0, 0).unwrap_or_default()),
                end: local_ms(
                    &tz,
                    date.and_hms_opt(h, 0, 0).unwrap_or_default() + Duration::hours(1),
                ),
                label: format!("{h:02}"),
            })
            .collect(),
        UsagePeriod::Week => {
            let first = week_start(date);
            (0..7)
                .map(|i| PeriodSlot {
                    start: midnight(&tz, plus_days(first, i)),
                    end: midnight(&tz, plus_days(first, i + 1)),
                    label: DAY_LABELS[i as usize].to_string(),
                })
                .collect()
        }
        UsagePeriod::Month => (1..=days_in_month(date.year(), date.month()))
            .map(|day| {
                let d = ymd(date.year(), date.month(), day);
                PeriodSlot {
                    start: midnight(&tz, d),
                    end: midnight(&tz, plus_days(d, 1)),
                    label: day.to_string(),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    // A fixed zone (UTC+2) stands in for "local time": the arithmetic is the same.
    fn tz() -> FixedOffset {
        FixedOffset::east_opt(2 * 3600).unwrap()
    }
    fn dt(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<FixedOffset> {
        tz().with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }
    fn d(y: i32, m: u32, day: u32, h: u32, min: u32) -> i64 {
        dt(y, m, day, h, min).timestamp_millis()
    }
    fn r(start: i64, end: i64) -> TimeRange {
        TimeRange { start, end }
    }

    #[test]
    fn period_range_cases() {
        let now = dt(2026, 10, 22, 15, 30); // Thursday
        assert_eq!(
            period_range(UsagePeriod::Today, &now),
            r(d(2026, 10, 22, 0, 0), now.timestamp_millis())
        );
        assert_eq!(
            period_range(UsagePeriod::Week, &now),
            r(d(2026, 10, 19, 0, 0), now.timestamp_millis())
        );
        // a Sunday belongs to the week that began the previous Monday
        assert_eq!(
            period_range(UsagePeriod::Week, &dt(2026, 10, 25, 10, 0)).start,
            d(2026, 10, 19, 0, 0)
        );
        // a Monday starts its own week
        assert_eq!(
            period_range(UsagePeriod::Week, &dt(2026, 10, 19, 0, 0)).start,
            d(2026, 10, 19, 0, 0)
        );
        // the week can start in the previous month
        assert_eq!(
            period_range(UsagePeriod::Week, &dt(2026, 11, 1, 9, 0)).start,
            d(2026, 10, 26, 0, 0)
        );
        assert_eq!(
            period_range(UsagePeriod::Month, &now),
            r(d(2026, 10, 1, 0, 0), now.timestamp_millis())
        );
    }

    #[test]
    fn previous_range_cases() {
        let now = dt(2026, 10, 22, 15, 30);
        assert_eq!(
            previous_range(UsagePeriod::Today, &now),
            r(d(2026, 10, 21, 0, 0), d(2026, 10, 21, 15, 30))
        );
        assert_eq!(
            previous_range(UsagePeriod::Today, &dt(2026, 11, 1, 8, 0)),
            r(d(2026, 10, 31, 0, 0), d(2026, 10, 31, 8, 0))
        );
        assert_eq!(
            previous_range(UsagePeriod::Week, &now),
            r(d(2026, 10, 12, 0, 0), d(2026, 10, 15, 15, 30))
        );
        assert_eq!(
            previous_range(UsagePeriod::Week, &dt(2026, 10, 25, 10, 0)),
            r(d(2026, 10, 12, 0, 0), d(2026, 10, 18, 10, 0))
        );
        assert_eq!(
            previous_range(UsagePeriod::Month, &now),
            r(d(2026, 9, 1, 0, 0), d(2026, 9, 22, 15, 30))
        );
        assert_eq!(
            previous_range(UsagePeriod::Month, &dt(2026, 1, 15, 12, 0)),
            r(d(2025, 12, 1, 0, 0), d(2025, 12, 15, 12, 0))
        );
    }

    #[test]
    fn previous_month_clamps_to_a_shorter_month() {
        assert_eq!(
            previous_range(UsagePeriod::Month, &dt(2026, 3, 31, 10, 0)),
            r(d(2026, 2, 1, 0, 0), d(2026, 3, 1, 0, 0))
        );
        assert_eq!(
            previous_range(UsagePeriod::Month, &dt(2024, 3, 31, 10, 0)),
            r(d(2024, 2, 1, 0, 0), d(2024, 3, 1, 0, 0))
        );
        assert_eq!(
            previous_range(UsagePeriod::Month, &dt(2026, 3, 28, 10, 0)).end,
            d(2026, 2, 28, 10, 0)
        );
        assert_eq!(
            previous_range(UsagePeriod::Month, &dt(2026, 10, 31, 10, 0)).end,
            d(2026, 10, 1, 0, 0)
        );
    }

    #[test]
    fn today_has_24_hourly_slots() {
        let slots = period_slots(UsagePeriod::Today, &dt(2026, 10, 22, 15, 30));
        assert_eq!(slots.len(), 24);
        assert_eq!(
            (slots[0].label.as_str(), slots[0].start),
            ("00", d(2026, 10, 22, 0, 0))
        );
        assert_eq!(
            (slots[23].label.as_str(), slots[23].end),
            ("23", d(2026, 10, 23, 0, 0))
        );
    }

    #[test]
    fn week_has_7_daily_slots() {
        let slots = period_slots(UsagePeriod::Week, &dt(2026, 10, 22, 15, 30));
        let labels: Vec<_> = slots.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]);
        assert_eq!(slots[0].start, d(2026, 10, 19, 0, 0));
        assert_eq!(slots[6].end, d(2026, 10, 26, 0, 0));
    }

    #[test]
    fn month_has_one_slot_per_day() {
        let now = dt(2026, 10, 22, 15, 30);
        assert_eq!(period_slots(UsagePeriod::Month, &now).len(), 31);
        assert_eq!(
            period_slots(UsagePeriod::Month, &dt(2026, 2, 10, 0, 0)).len(),
            28
        );
        assert_eq!(
            period_slots(UsagePeriod::Month, &dt(2024, 2, 10, 0, 0)).len(),
            29
        );
        let slots = period_slots(UsagePeriod::Month, &now);
        assert_eq!(slots[0].label, "1");
        assert_eq!(slots[30].label, "31");
    }

    #[test]
    fn slots_are_contiguous_and_include_the_future() {
        let now = dt(2026, 10, 22, 15, 30);
        let slots = period_slots(UsagePeriod::Week, &now);
        for i in 1..slots.len() {
            assert_eq!(slots[i].start, slots[i - 1].end);
        }
        assert!(slots[6].start > now.timestamp_millis());
    }

    #[test]
    fn utc_zone_works_too() {
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 10, 22, 15, 30, 0)
            .unwrap();
        assert_eq!(
            period_range(UsagePeriod::Today, &now).start,
            chrono::Utc
                .with_ymd_and_hms(2026, 10, 22, 0, 0, 0)
                .unwrap()
                .timestamp_millis()
        );
    }
}
