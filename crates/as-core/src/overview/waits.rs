// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/core/waits/waitTotals.ts, measured.ts
//! How long agents waited on the user, worked out from the ledger the daemon keeps: the moves
//! into and out of needs-you, the stretches that were measured, and the agents waiting now.
//! Pure: the clock and the time zone are arguments.
//!
//! Only measured time counts. A wait that runs on while the daemon was stopped, or the computer
//! slept, counts for the time either side that was measured, and is still one wait. Each
//! agent's waits are added up, so two agents waiting at once for a minute are two minutes.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, Local, TimeZone, Utc};

use super::buckets::{AgentFilter, HOUR_MS, MAX_SAMPLE_GAP_MS};
use super::types::{
    EventKind, LongestLists, LongestWait, MeasuredSpan, OverviewEvent, OverviewStatus, WaitBar,
    WaitDayTotal, WaitDirection, WaitFigure, WaitTotals, WaitTransition, WaitingNow,
};

/// Days of ledger the daemon keeps.
pub const LEDGER_DAYS: i64 = 9;
/// How many local days "the last 7 days" are: today and the six before it.
pub const WAIT_DAYS: i64 = 7;
/// Agents named in each "Longest waits" list.
pub const MAX_LONGEST: usize = 5;

/// What counts as a day: the local one in the app, UTC in tests.
pub trait DayZone {
    /// The start of the day `at` falls in.
    fn midnight_of(&self, at: i64) -> i64;
    /// The start of the day `days` days after (or before, when negative) `midnight`.
    fn shift_days(&self, midnight: i64, days: i64) -> i64;
    /// `2026-10-22` for the day that `at` falls in.
    fn day_key(&self, at: i64) -> String;
}

pub struct LocalZone;
pub struct UtcZone;

const DAY_MS: i64 = 86_400_000;

impl DayZone for UtcZone {
    fn midnight_of(&self, at: i64) -> i64 {
        at.div_euclid(DAY_MS) * DAY_MS
    }
    fn shift_days(&self, midnight: i64, days: i64) -> i64 {
        midnight + days * DAY_MS
    }
    fn day_key(&self, at: i64) -> String {
        Utc.timestamp_millis_opt(at)
            .single()
            .map_or_else(String::new, |d| d.format("%Y-%m-%d").to_string())
    }
}

impl LocalZone {
    fn local_midnight(date: chrono::NaiveDate) -> i64 {
        let naive = date.and_hms_opt(0, 0, 0).unwrap_or_default();
        match Local.from_local_datetime(&naive) {
            chrono::LocalResult::Single(d) => d.timestamp_millis(),
            chrono::LocalResult::Ambiguous(first, _) => first.timestamp_millis(),
            // A day whose midnight does not exist (DST jump at 00:00): the next valid instant.
            chrono::LocalResult::None => naive.and_utc().timestamp_millis(),
        }
    }
}

impl DayZone for LocalZone {
    fn midnight_of(&self, at: i64) -> i64 {
        match Local.timestamp_millis_opt(at).single() {
            Some(d) => Self::local_midnight(d.date_naive()),
            None => UtcZone.midnight_of(at),
        }
    }
    /// Through the calendar, so a day where the clocks change is 23 or 25 hours long.
    fn shift_days(&self, midnight: i64, days: i64) -> i64 {
        match Local.timestamp_millis_opt(midnight).single() {
            Some(d) => {
                let date = d.date_naive() + chrono::Duration::days(days);
                Self::local_midnight(date)
            }
            None => midnight + days * DAY_MS,
        }
    }
    fn day_key(&self, at: i64) -> String {
        match Local.timestamp_millis_opt(at).single() {
            Some(d) => format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day()),
            None => String::new(),
        }
    }
}

/// The move into or out of `needs-you` that an event makes, `None` for any other event. An
/// agent that ends while it waits moves out.
pub fn transition_of_event(event: &OverviewEvent) -> Option<WaitTransition> {
    if event.agent_id.is_empty() {
        return None;
    }
    let direction = match event.kind {
        EventKind::Status => {
            if event.to == Some(OverviewStatus::NeedsYou) {
                Some(WaitDirection::In)
            } else if event.from == Some(OverviewStatus::NeedsYou) {
                Some(WaitDirection::Out)
            } else {
                None
            }
        }
        EventKind::Appeared => {
            (event.to == Some(OverviewStatus::NeedsYou)).then_some(WaitDirection::In)
        }
        EventKind::Ended => {
            (event.from == Some(OverviewStatus::NeedsYou)).then_some(WaitDirection::Out)
        }
        EventKind::Resumed => None,
    }?;
    Some(WaitTransition {
        agent_id: event.agent_id.clone(),
        name: event.name.clone(),
        project_dir: event.project_dir.clone(),
        at: event.at,
        direction,
    })
}

// ── Measured time ───────────────────────────────────────────────────────────

/// Spans in time order with none overlapping: those that touch or overlap are joined.
pub fn join_spans(spans: &[MeasuredSpan]) -> Vec<MeasuredSpan> {
    let mut ordered: Vec<MeasuredSpan> =
        spans.iter().filter(|s| s.end >= s.start).copied().collect();
    ordered.sort_by_key(|s| s.start);
    let mut out: Vec<MeasuredSpan> = Vec::new();
    for span in ordered {
        match out.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => out.push(span),
        }
    }
    out
}

/// The spans that count up to `now`: joined, the newest carried on to `now` while its last
/// point is fresh and no daemon start came after it, and cut at `now`.
pub fn measured_until(spans: &[MeasuredSpan], starts: &[i64], now: i64) -> Vec<MeasuredSpan> {
    let mut joined = join_spans(spans);
    if let Some(newest) = joined.last_mut()
        && newest.end < now
        && now - newest.end <= MAX_SAMPLE_GAP_MS
        && !starts.iter().any(|s| *s > newest.end && *s <= now)
    {
        newest.end = now;
    }
    joined
        .into_iter()
        .map(|s| MeasuredSpan {
            start: s.start,
            end: s.end.min(now),
        })
        .filter(|s| s.end > s.start)
        .collect()
}

/// How much of `from` to `to` the spans cover. They must be joined and in time order.
pub fn covered_within(spans: &[MeasuredSpan], from: i64, to: i64) -> i64 {
    if to <= from {
        return 0;
    }
    let mut sum = 0;
    for span in spans {
        if span.end <= from {
            continue;
        }
        if span.start >= to {
            break;
        }
        sum += span.end.min(to) - span.start.max(from);
    }
    sum
}

// ── Waits ───────────────────────────────────────────────────────────────────

struct Wait {
    agent_id: String,
    from: i64,
    to: i64,
    /// Still under way: the agent needs the user now.
    open: bool,
}

pub struct WaitsInput<'a> {
    pub transitions: &'a [WaitTransition],
    pub spans: &'a [MeasuredSpan],
    /// Each start of the daemon. Without them, a stretch may be carried across a restart.
    pub starts: &'a [i64],
    pub waiting_now: &'a [WaitingNow],
    pub now: i64,
    /// Leaves out the agents it rejects.
    pub agent_filter: AgentFilter<'a>,
}

fn span_start_at(spans: &[MeasuredSpan], at: i64) -> Option<i64> {
    spans
        .iter()
        .find(|s| s.start <= at && at <= s.end)
        .map(|s| s.start)
}

/// The end of the stretch that holds `at`, `at` itself when none does.
fn span_end_at(spans: &[MeasuredSpan], at: i64) -> i64 {
    spans
        .iter()
        .find(|s| s.start <= at && at <= s.end)
        .map_or(at, |s| s.end)
}

fn passes(filter: AgentFilter, id: &str) -> bool {
    filter.is_none_or(|f| f(id))
}

/// Every wait the ledger holds, oldest first: a move into needs-you to the agent's next move
/// out of it.
/// * A move out with no move in held counts from the start of the measured stretch it ended in,
///   or from the agent's previous move when that is later.
/// * A wait with no end runs to `now` while the agent still needs the user (open). For an agent
///   that does not, it ended unseen: it runs to the end of the stretch it began in.
/// * An agent waiting now that no move says began is counted from `since`.
fn waits_of(input: &WaitsInput, spans: &[MeasuredSpan]) -> (Vec<Wait>, BTreeMap<String, String>) {
    let now = input.now;
    let waiting_now: BTreeMap<&str, &WaitingNow> = input
        .waiting_now
        .iter()
        .filter(|w| passes(input.agent_filter, &w.agent_id))
        .map(|w| (w.agent_id.as_str(), w))
        .collect();
    let mut by_agent: BTreeMap<&str, Vec<&WaitTransition>> = BTreeMap::new();
    for t in input.transitions {
        if t.at > now || !passes(input.agent_filter, &t.agent_id) {
            continue;
        }
        by_agent.entry(t.agent_id.as_str()).or_default().push(t);
    }

    let mut waits: Vec<Wait> = Vec::new();
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut unended: BTreeSet<&str> = BTreeSet::new();
    for (agent_id, own) in &mut by_agent {
        own.sort_by_key(|t| t.at);
        let mut began: Option<i64> = None;
        let mut previous = i64::MIN;
        for t in own.iter() {
            names.insert((*agent_id).to_string(), t.name.clone());
            if t.direction == WaitDirection::In {
                began.get_or_insert(t.at);
            } else {
                if began.is_none()
                    && let Some(start) = span_start_at(spans, t.at)
                {
                    began = Some(start.max(previous));
                }
                if let Some(from) = began {
                    waits.push(Wait {
                        agent_id: (*agent_id).to_string(),
                        from,
                        to: t.at,
                        open: false,
                    });
                }
                began = None;
            }
            previous = t.at;
        }
        if let Some(from) = began {
            let open = waiting_now.contains_key(agent_id);
            waits.push(Wait {
                agent_id: (*agent_id).to_string(),
                from,
                to: if open { now } else { span_end_at(spans, from) },
                open,
            });
            unended.insert(*agent_id);
        }
    }
    for (agent_id, w) in &waiting_now {
        names.insert((*agent_id).to_string(), w.name.clone());
        if unended.contains(agent_id) {
            continue;
        }
        let last = by_agent
            .get(agent_id)
            .and_then(|own| own.last())
            .map_or(i64::MIN, |t| t.at);
        waits.push(Wait {
            agent_id: (*agent_id).to_string(),
            from: w.since.max(last),
            to: now,
            open: true,
        });
    }
    waits.retain(|w| w.to > w.from);
    waits.sort_by_key(|w| w.from);
    (waits, names)
}

#[derive(Default)]
struct PeriodTotal {
    figure: WaitFigure,
    by_agent: BTreeMap<String, (i64, u32)>,
}

fn total_in(waits: &[Wait], spans: &[MeasuredSpan], from: i64, to: i64) -> PeriodTotal {
    let mut total = PeriodTotal {
        figure: WaitFigure {
            measured_ms: covered_within(spans, from, to),
            ..Default::default()
        },
        ..Default::default()
    };
    for wait in waits {
        if wait.from >= to || wait.to <= from {
            continue;
        }
        let ms = covered_within(spans, wait.from.max(from), wait.to.min(to));
        if ms <= 0 {
            continue;
        }
        total.figure.ms += ms;
        total.figure.count += 1;
        if wait.open {
            total.figure.open_ms += ms;
        }
        let own = total.by_agent.entry(wait.agent_id.clone()).or_default();
        own.0 += ms;
        own.1 += 1;
    }
    total
}

fn longest_of(
    total: &PeriodTotal,
    names: &BTreeMap<String, String>,
    waiting_now: &BTreeSet<String>,
) -> Vec<LongestWait> {
    let mut list: Vec<LongestWait> = total
        .by_agent
        .iter()
        .map(|(id, (ms, count))| LongestWait {
            agent_id: id.clone(),
            name: names.get(id).cloned().unwrap_or_else(|| id.clone()),
            ms: *ms,
            count: *count,
            waiting_now: waiting_now.contains(id),
        })
        .collect();
    list.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.name.cmp(&b.name)));
    list.truncate(MAX_LONGEST);
    list
}

/// How long agents waited today (from local midnight) and over the last 7 days (today and the
/// six before it), cut into local days, with the five agents that waited longest in each.
pub fn wait_totals(input: &WaitsInput, zone: &dyn DayZone) -> WaitTotals {
    let now = input.now;
    let spans = measured_until(input.spans, input.starts, now);
    let (waits, names) = waits_of(input, &spans);
    let waiting_now: BTreeSet<String> = input
        .waiting_now
        .iter()
        .filter(|w| passes(input.agent_filter, &w.agent_id))
        .map(|w| w.agent_id.clone())
        .collect();
    let today = zone.midnight_of(now);
    let week_start = zone.shift_days(today, -(WAIT_DAYS - 1));

    let today_total = total_in(&waits, &spans, today, now);
    let week_total = total_in(&waits, &spans, week_start, now);
    let mut by_day = Vec::new();
    let mut start = week_start;
    while start <= today {
        let end = zone.shift_days(start, 1).min(now);
        let figure = total_in(&waits, &spans, start, end).figure;
        by_day.push(WaitDayTotal {
            day: zone.day_key(start),
            measured: figure.measured_ms > 0,
            figure,
        });
        start = zone.shift_days(start, 1);
    }
    WaitTotals {
        today: today_total.figure,
        week: week_total.figure,
        by_day,
        longest: LongestLists {
            today: longest_of(&today_total, &names, &waiting_now),
            week: longest_of(&week_total, &names, &waiting_now),
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastHourWaits {
    /// Longest first.
    pub bars: Vec<WaitBar>,
    /// Where the bars begin: an hour back, or the daemon's start when that is later.
    pub from: i64,
    /// Time in `from`..`now` that was not measured.
    pub unmeasured_ms: i64,
}

/// Who waited on the user in the last hour, one bar per agent, longest first, and how much of
/// that hour was not measured. `from_floor` moves the beginning of the window (the caller passes
/// the daemon's start when later).
pub fn last_hour_waits(
    transitions: &[WaitTransition],
    waiting_now: &[WaitingNow],
    spans: &[MeasuredSpan],
    now: i64,
    starts: &[i64],
    filter: AgentFilter,
    from_floor: Option<i64>,
) -> LastHourWaits {
    let from = (now - HOUR_MS).max(from_floor.unwrap_or(0));
    let measured = measured_until(spans, starts, now);
    let input = WaitsInput {
        transitions,
        spans,
        starts: &[],
        waiting_now,
        now,
        agent_filter: filter,
    };
    let (waits, names) = waits_of(&input, &measured);
    let total = total_in(&waits, &measured, from, now);
    let open: BTreeSet<&str> = waits
        .iter()
        .filter(|w| w.open)
        .map(|w| w.agent_id.as_str())
        .collect();
    let mut bars: Vec<WaitBar> = total
        .by_agent
        .iter()
        .map(|(id, (ms, count))| WaitBar {
            agent_id: id.clone(),
            name: names.get(id).cloned().unwrap_or_else(|| id.clone()),
            ms: *ms,
            count: *count,
            open: open.contains(id.as_str()),
        })
        .collect();
    bars.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.name.cmp(&b.name)));
    LastHourWaits {
        bars,
        from,
        unmeasured_ms: (now - from - covered_within(&measured, from, now)).max(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;
    const HOUR: i64 = 60 * MIN;

    fn at(iso: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(&format!("{iso}Z"))
            .unwrap()
            .timestamp_millis()
    }
    fn now() -> i64 {
        at("2026-10-22T12:00:00")
    }
    fn tr(agent: &str, direction: WaitDirection, time: i64) -> WaitTransition {
        WaitTransition {
            agent_id: agent.to_string(),
            name: format!("agent-{agent}"),
            project_dir: format!("/work/{agent}"),
            at: time,
            direction,
        }
    }
    fn span_of(from: &str, to: &str) -> MeasuredSpan {
        MeasuredSpan {
            start: at(from),
            end: at(to),
        }
    }
    fn day_span() -> MeasuredSpan {
        span_of("2026-10-16T00:00:00", "2026-10-22T12:00:00")
    }
    fn wn(agent: &str, since: &str) -> WaitingNow {
        WaitingNow {
            agent_id: agent.into(),
            name: format!("agent-{agent}"),
            since: at(since),
        }
    }
    use WaitDirection::{In, Out};

    struct Input {
        transitions: Vec<WaitTransition>,
        spans: Vec<MeasuredSpan>,
        waiting_now: Vec<WaitingNow>,
        now: i64,
    }
    impl Input {
        fn new() -> Self {
            Self {
                transitions: vec![],
                spans: vec![day_span()],
                waiting_now: vec![],
                now: now(),
            }
        }
        fn totals(&self) -> WaitTotals {
            self.totals_filtered(None)
        }
        fn totals_filtered(&self, f: AgentFilter) -> WaitTotals {
            wait_totals(
                &WaitsInput {
                    transitions: &self.transitions,
                    spans: &self.spans,
                    starts: &[],
                    waiting_now: &self.waiting_now,
                    now: self.now,
                    agent_filter: f,
                },
                &UtcZone,
            )
        }
    }
    fn fig(ms: i64, count: u32) -> (i64, u32) {
        (ms, count)
    }
    fn of(f: &WaitFigure) -> (i64, u32) {
        (f.ms, f.count)
    }

    #[test]
    fn adds_up_a_wait_from_entry_to_exit() {
        let mut i = Input::new();
        i.transitions = vec![
            tr("a", In, at("2026-10-22T10:00:00")),
            tr("a", Out, at("2026-10-22T10:05:00")),
        ];
        let t = i.totals();
        assert_eq!(of(&t.today), fig(5 * MIN, 1));
        assert_eq!(t.today.open_ms, 0);
        assert_eq!(of(&t.week), fig(5 * MIN, 1));
        assert_eq!(
            t.longest.today,
            [LongestWait {
                agent_id: "a".into(),
                name: "agent-a".into(),
                ms: 5 * MIN,
                count: 1,
                waiting_now: false
            }]
        );
    }

    #[test]
    fn counts_only_measured_time_and_keeps_a_wait_across_a_gap_as_one() {
        let mut i = Input::new();
        i.spans = vec![
            span_of("2026-10-22T09:55:00", "2026-10-22T10:02:00"),
            span_of("2026-10-22T10:08:00", "2026-10-22T12:00:00"),
        ];
        i.transitions = vec![
            tr("a", In, at("2026-10-22T10:00:00")),
            tr("a", Out, at("2026-10-22T10:10:00")),
        ];
        let t = i.totals();
        assert_eq!(t.today.ms, 4 * MIN);
        assert_eq!(t.today.count, 1);
        assert_eq!(t.today.measured_ms, 119 * MIN);
    }

    #[test]
    fn exit_without_entry_counts_from_the_start_of_the_span() {
        let mut i = Input::new();
        i.spans = vec![span_of("2026-10-22T10:00:00", "2026-10-22T12:00:00")];
        i.transitions = vec![tr("a", Out, at("2026-10-22T10:05:00"))];
        assert_eq!(of(&i.totals().today), fig(5 * MIN, 1));
    }

    #[test]
    fn exit_outside_every_span_does_not_count() {
        let mut i = Input::new();
        i.spans = vec![span_of("2026-10-22T11:00:00", "2026-10-22T12:00:00")];
        i.transitions = vec![tr("a", Out, at("2026-10-22T10:05:00"))];
        assert_eq!(i.totals().today.count, 0);
    }

    #[test]
    fn splits_a_wait_at_midnight_and_counts_it_once_per_period() {
        let mut i = Input::new();
        i.now = at("2026-10-22T01:00:00");
        i.spans = vec![span_of("2026-10-21T20:00:00", "2026-10-22T01:00:00")];
        i.transitions = vec![
            tr("a", In, at("2026-10-21T23:30:00")),
            tr("a", Out, at("2026-10-22T00:30:00")),
        ];
        let t = i.totals();
        assert_eq!(of(&t.today), fig(30 * MIN, 1));
        assert_eq!(of(&t.week), fig(60 * MIN, 1));
        assert_eq!(t.by_day.len(), 7);
        assert_eq!(t.by_day[5].day, "2026-10-21");
        assert_eq!(of(&t.by_day[5].figure), fig(30 * MIN, 1));
        assert_eq!(t.by_day[6].day, "2026-10-22");
        assert_eq!(of(&t.by_day[6].figure), fig(30 * MIN, 1));
    }

    #[test]
    fn lists_seven_days_with_unmeasured_days_marked() {
        let mut i = Input::new();
        i.spans = vec![span_of("2026-10-21T00:00:00", "2026-10-22T12:00:00")];
        let t = i.totals();
        let days: Vec<&str> = t.by_day.iter().map(|d| d.day.as_str()).collect();
        assert_eq!(
            days,
            [
                "2026-10-16",
                "2026-10-17",
                "2026-10-18",
                "2026-10-19",
                "2026-10-20",
                "2026-10-21",
                "2026-10-22"
            ]
        );
        let measured: Vec<bool> = t.by_day.iter().map(|d| d.measured).collect();
        assert_eq!(measured, [false, false, false, false, false, true, true]);
        assert_eq!(t.by_day[6].figure.measured_ms, 12 * HOUR);
        assert_eq!(t.week.measured_ms, 36 * HOUR);
    }

    #[test]
    fn two_agents_waiting_at_once_count_for_both() {
        let mut i = Input::new();
        i.transitions = vec![
            tr("a", In, at("2026-10-22T10:00:00")),
            tr("b", In, at("2026-10-22T10:00:00")),
            tr("b", Out, at("2026-10-22T10:01:00")),
            tr("a", Out, at("2026-10-22T10:01:00")),
        ];
        let t = i.totals();
        assert_eq!(of(&t.today), fig(2 * MIN, 2));
        let ids: Vec<&str> = t
            .longest
            .today
            .iter()
            .map(|l| l.agent_id.as_str())
            .collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn an_open_wait_runs_to_now_and_is_marked() {
        let mut i = Input::new();
        i.transitions = vec![tr("a", In, at("2026-10-22T11:50:00"))];
        i.waiting_now = vec![wn("a", "2026-10-22T11:50:00")];
        let t = i.totals();
        assert_eq!(of(&t.today), fig(10 * MIN, 1));
        assert_eq!(t.today.open_ms, 10 * MIN);
        assert_eq!(t.by_day[6].figure.open_ms, 10 * MIN);
        assert_eq!(t.longest.today[0].agent_id, "a");
        assert!(t.longest.today[0].waiting_now);
    }

    #[test]
    fn an_agent_waiting_now_with_no_entry_counts_from_since() {
        let mut i = Input::new();
        i.waiting_now = vec![WaitingNow {
            agent_id: "z".into(),
            name: "zed".into(),
            since: at("2026-10-22T11:30:00"),
        }];
        let t = i.totals();
        assert_eq!(of(&t.today), fig(30 * MIN, 1));
        assert_eq!(t.today.open_ms, 30 * MIN);
        assert_eq!(t.longest.today[0].name, "zed");
    }

    #[test]
    fn an_unended_wait_of_an_agent_not_waiting_ends_with_its_span() {
        let mut i = Input::new();
        i.spans = vec![
            span_of("2026-10-22T09:00:00", "2026-10-22T10:30:00"),
            span_of("2026-10-22T11:00:00", "2026-10-22T12:00:00"),
        ];
        i.transitions = vec![tr("a", In, at("2026-10-22T10:00:00"))];
        let t = i.totals();
        assert_eq!(of(&t.today), fig(30 * MIN, 1));
        assert_eq!(t.today.open_ms, 0);
    }

    #[test]
    fn a_wait_carries_across_a_restart_while_the_agent_still_waits() {
        let mut i = Input::new();
        i.spans = vec![
            span_of("2026-10-22T09:00:00", "2026-10-22T10:30:00"),
            span_of("2026-10-22T11:00:00", "2026-10-22T12:00:00"),
        ];
        i.transitions = vec![tr("a", In, at("2026-10-22T10:00:00"))];
        i.waiting_now = vec![wn("a", "2026-10-22T11:00:00")];
        assert_eq!(of(&i.totals().today), fig(30 * MIN + HOUR, 1));
    }

    #[test]
    fn a_second_entry_while_open_is_ignored() {
        let mut i = Input::new();
        i.transitions = vec![
            tr("a", In, at("2026-10-22T10:00:00")),
            tr("a", In, at("2026-10-22T10:02:00")),
            tr("a", Out, at("2026-10-22T10:04:00")),
        ];
        assert_eq!(of(&i.totals().today), fig(4 * MIN, 1));
    }

    #[test]
    fn filters_by_agent() {
        let mut i = Input::new();
        i.transitions = vec![
            tr("a", In, at("2026-10-22T10:00:00")),
            tr("a", Out, at("2026-10-22T10:05:00")),
            tr("b", In, at("2026-10-22T10:00:00")),
            tr("b", Out, at("2026-10-22T10:01:00")),
        ];
        i.waiting_now = vec![wn("a", "2026-10-22T11:00:00")];
        let only_b = |id: &str| id == "b";
        let t = i.totals_filtered(Some(&only_b));
        assert_eq!(of(&t.today), fig(MIN, 1));
        let ids: Vec<&str> = t
            .longest
            .today
            .iter()
            .map(|l| l.agent_id.as_str())
            .collect();
        assert_eq!(ids, ["b"]);
    }

    #[test]
    fn lists_at_most_five_longest_first() {
        let mut i = Input::new();
        for n in 1..=7 {
            i.transitions
                .push(tr(&format!("x{n}"), In, at("2026-10-22T10:00:00")));
            i.transitions.push(tr(
                &format!("x{n}"),
                Out,
                at("2026-10-22T10:00:00") + n * MIN,
            ));
        }
        let t = i.totals();
        let ids: Vec<&str> = t.longest.week.iter().map(|l| l.agent_id.as_str()).collect();
        assert_eq!(ids, ["x7", "x6", "x5", "x4", "x3"]);
    }

    #[test]
    fn earlier_days_count_in_the_week_but_not_today() {
        let mut i = Input::new();
        i.transitions = vec![
            tr("a", In, at("2026-10-20T10:00:00")),
            tr("a", Out, at("2026-10-20T10:10:00")),
        ];
        let t = i.totals();
        assert_eq!(t.today.count, 0);
        assert_eq!(of(&t.week), fig(10 * MIN, 1));
        assert_eq!(t.by_day[4].day, "2026-10-20");
        assert_eq!(of(&t.by_day[4].figure), fig(10 * MIN, 1));
    }

    #[test]
    fn nothing_without_a_ledger() {
        let mut i = Input::new();
        i.spans = vec![];
        let t = i.totals();
        assert_eq!(t.today, WaitFigure::default());
        assert!(t.by_day.iter().all(|d| !d.measured));
    }

    #[test]
    fn works_in_the_local_time_zone() {
        let local = |d: u32, h: u32, m: u32| {
            Local
                .with_ymd_and_hms(2026, 10, d, h, m, 0)
                .unwrap()
                .timestamp_millis()
        };
        let now = local(22, 12, 0);
        let transitions = [tr("a", In, local(22, 8, 0)), tr("a", Out, local(22, 8, 10))];
        let spans = [MeasuredSpan {
            start: local(20, 0, 0),
            end: now,
        }];
        let t = wait_totals(
            &WaitsInput {
                transitions: &transitions,
                spans: &spans,
                starts: &[],
                waiting_now: &[],
                now,
                agent_filter: None,
            },
            &LocalZone,
        );
        assert_eq!(t.today.ms, 10 * MIN);
        assert_eq!(t.by_day[6].day, "2026-10-22");
        assert_eq!(LocalZone.day_key(now), "2026-10-22");
    }

    #[test]
    fn joins_touching_spans() {
        let s = |a, b| MeasuredSpan { start: a, end: b };
        assert_eq!(
            join_spans(&[s(5, 9), s(0, 5), s(12, 13)]),
            [s(0, 9), s(12, 13)]
        );
    }

    #[test]
    fn carries_the_newest_span_to_now_while_fresh() {
        let n = now();
        let s = |a, b| MeasuredSpan { start: a, end: b };
        assert_eq!(
            measured_until(&[s(n - HOUR, n - 6000)], &[], n),
            [s(n - HOUR, n)]
        );
        assert_eq!(
            measured_until(&[s(n - HOUR, n - 20_000)], &[], n),
            [s(n - HOUR, n - 20_000)]
        );
    }

    #[test]
    fn does_not_carry_a_span_across_a_daemon_start() {
        let n = now();
        let s = |a, b| MeasuredSpan { start: a, end: b };
        assert_eq!(
            measured_until(&[s(n - HOUR, n - 6000)], &[n - 3000], n),
            [s(n - HOUR, n - 6000)]
        );
    }

    #[test]
    fn coverage_of_a_window() {
        let s = |a, b| MeasuredSpan { start: a, end: b };
        let spans = [s(0, 10), s(20, 30)];
        assert_eq!(covered_within(&spans, 5, 25), 10);
        assert_eq!(covered_within(&spans, 12, 18), 0);
    }

    fn lh(
        transitions: &[WaitTransition],
        waiting: &[WaitingNow],
        spans: &[MeasuredSpan],
        f: AgentFilter,
        from: Option<i64>,
    ) -> LastHourWaits {
        last_hour_waits(transitions, waiting, spans, now(), &[], f, from)
    }
    fn bar(id: &str, ms: i64, open: bool) -> WaitBar {
        WaitBar {
            agent_id: id.into(),
            name: format!("agent-{id}"),
            ms,
            count: 1,
            open,
        }
    }

    #[test]
    fn last_hour_has_a_bar_per_agent_longest_first() {
        let spans = [span_of("2026-10-22T10:00:00", "2026-10-22T12:00:00")];
        let transitions = [
            tr("a", In, at("2026-10-22T11:10:00")),
            tr("a", Out, at("2026-10-22T11:12:00")),
            tr("b", In, at("2026-10-22T11:20:00")),
            tr("b", Out, at("2026-10-22T11:30:00")),
            tr("c", In, at("2026-10-22T11:55:00")),
        ];
        let r = lh(
            &transitions,
            &[wn("c", "2026-10-22T11:55:00")],
            &spans,
            None,
            None,
        );
        assert_eq!(
            r.bars,
            [
                bar("b", 10 * MIN, false),
                bar("c", 5 * MIN, true),
                bar("a", 2 * MIN, false)
            ]
        );
        assert_eq!(r.from, now() - HOUR);
        assert_eq!(r.unmeasured_ms, 0);
    }

    #[test]
    fn last_hour_cuts_a_wait_at_the_start_of_the_hour() {
        let spans = [span_of("2026-10-22T10:00:00", "2026-10-22T12:00:00")];
        let r = lh(
            &[
                tr("a", In, at("2026-10-22T10:30:00")),
                tr("a", Out, at("2026-10-22T11:10:00")),
            ],
            &[],
            &spans,
            None,
            None,
        );
        assert_eq!(r.bars[0].ms, 10 * MIN);
    }

    #[test]
    fn last_hour_tells_how_much_was_not_measured() {
        let r = lh(
            &[],
            &[],
            &[span_of("2026-10-22T11:30:00", "2026-10-22T12:00:00")],
            None,
            None,
        );
        assert!(r.bars.is_empty());
        assert_eq!(r.unmeasured_ms, 30 * MIN);
    }

    #[test]
    fn last_hour_begins_at_the_daemon_start_when_later() {
        let spans = [span_of("2026-10-22T10:00:00", "2026-10-22T12:00:00")];
        let from = at("2026-10-22T11:40:00");
        let r = lh(&[], &[], &spans, None, Some(from));
        assert_eq!(r.from, from);
        assert_eq!(r.unmeasured_ms, 0);
    }

    #[test]
    fn last_hour_filters_agents() {
        let spans = [span_of("2026-10-22T10:00:00", "2026-10-22T12:00:00")];
        let none = |_: &str| false;
        let r = lh(
            &[
                tr("a", In, at("2026-10-22T11:10:00")),
                tr("a", Out, at("2026-10-22T11:12:00")),
            ],
            &[],
            &spans,
            Some(&none),
            None,
        );
        assert!(r.bars.is_empty());
    }

    fn base_event() -> OverviewEvent {
        OverviewEvent {
            id: "x".into(),
            at: 5,
            kind: EventKind::Status,
            agent_id: "a".into(),
            name: "n".into(),
            project_dir: "/p".into(),
            from: None,
            to: None,
            waiting_reason: None,
            waited_ms: None,
            gap_ms: None,
        }
    }

    #[test]
    fn transition_of_event_reads_moves_in_and_out() {
        use OverviewStatus as S;
        let e = |kind, from, to| OverviewEvent {
            kind,
            from,
            to,
            ..base_event()
        };
        let dir = |ev: OverviewEvent| transition_of_event(&ev).map(|t| t.direction);
        assert_eq!(
            dir(e(EventKind::Status, Some(S::Working), Some(S::NeedsYou))),
            Some(In)
        );
        assert_eq!(
            dir(e(EventKind::Status, Some(S::NeedsYou), Some(S::Working))),
            Some(Out)
        );
        assert_eq!(
            dir(e(EventKind::Appeared, None, Some(S::NeedsYou))),
            Some(In)
        );
        assert_eq!(dir(e(EventKind::Ended, Some(S::NeedsYou), None)), Some(Out));
        assert_eq!(
            transition_of_event(&e(EventKind::Status, Some(S::Working), Some(S::NeedsYou)))
                .unwrap()
                .at,
            5
        );
    }

    #[test]
    fn transition_of_event_passes_over_everything_else() {
        use OverviewStatus as S;
        let e = |kind, from, to| OverviewEvent {
            kind,
            from,
            to,
            ..base_event()
        };
        assert!(
            transition_of_event(&e(EventKind::Status, Some(S::Working), Some(S::Idle))).is_none()
        );
        assert!(transition_of_event(&e(EventKind::Appeared, None, Some(S::Working))).is_none());
        let resumed = OverviewEvent {
            kind: EventKind::Resumed,
            agent_id: String::new(),
            gap_ms: Some(5),
            ..base_event()
        };
        assert!(transition_of_event(&resumed).is_none());
    }
}
