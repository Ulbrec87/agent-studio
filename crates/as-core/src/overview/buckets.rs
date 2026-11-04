// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/core/history (measured time, bucketing)
//! Measured time, last-hour buckets, count series and timeline segments.

use std::collections::BTreeMap;

use super::types::{HistoryPoint, OverviewStatus};

/// Samples are 5 s apart; more than this between two of them means nothing was measured.
pub const MAX_SAMPLE_GAP_MS: i64 = 15_000;
pub const FIVE_MINUTES_MS: i64 = 5 * 60 * 1000;
pub const HOUR_MS: i64 = 60 * 60 * 1000;

/// Which agents to count; `None` counts all.
pub type AgentFilter<'a> = Option<&'a dyn Fn(&str) -> bool>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: i64,
    pub end: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountKey {
    NeedsYou,
    Working,
    Idle,
    /// Agents whose process ended (only counted from the per-agent statuses of the samples).
    Finished,
    Failed,
}

/// A stretch of measured time during which the counts were those of one sample.
struct Piece<'a> {
    start: i64,
    end: i64,
    needs_you: u32,
    working: u32,
    idle: u32,
    total: u32,
    statuses: &'a BTreeMap<String, OverviewStatus>,
}

fn counts_of(point: &HistoryPoint, filter: AgentFilter) -> (u32, u32, u32, u32) {
    let Some(filter) = filter else {
        return (point.needs_you, point.working, point.idle, point.total);
    };
    let (mut n, mut w, mut i, mut t) = (0, 0, 0, 0);
    for (id, status) in &point.statuses {
        if !filter(id) {
            continue;
        }
        t += 1;
        match status {
            OverviewStatus::NeedsYou => n += 1,
            OverviewStatus::Working => w += 1,
            OverviewStatus::Idle => i += 1,
            _ => {}
        }
    }
    (n, w, i, t)
}

/// The measured pieces inside the window, oldest first. A sample holds until the next one,
/// unless more than `MAX_SAMPLE_GAP_MS` lies between them or a daemon start does: then the
/// sample covers nothing after itself. The newest sample holds up to `now` while it is fresh.
fn pieces<'a>(
    points: &'a [HistoryPoint],
    starts: &[i64],
    now: i64,
    window_ms: i64,
    filter: AgentFilter,
) -> Vec<Piece<'a>> {
    let from = now - window_ms;
    let mut out = Vec::new();
    for (i, p) in points.iter().enumerate() {
        let mut end = match points.get(i + 1) {
            Some(next) => {
                if next.at - p.at > MAX_SAMPLE_GAP_MS
                    || starts.iter().any(|s| *s > p.at && *s <= next.at)
                {
                    continue;
                }
                next.at
            }
            None => {
                if now - p.at > MAX_SAMPLE_GAP_MS {
                    continue;
                }
                now
            }
        };
        let start = p.at.max(from);
        end = end.min(now);
        if end <= start {
            continue;
        }
        let (needs_you, working, idle, total) = counts_of(p, filter);
        out.push(Piece {
            start,
            end,
            needs_you,
            working,
            idle,
            total,
            statuses: &p.statuses,
        });
    }
    out
}

/// The spans of the window covered by samples, oldest first; touching pieces are merged.
pub fn measured_spans(
    points: &[HistoryPoint],
    starts: &[i64],
    now: i64,
    window_ms: i64,
) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for piece in pieces(points, starts, now, window_ms, None) {
        match spans.last_mut() {
            Some(last) if piece.start <= last.end => last.end = last.end.max(piece.end),
            _ => spans.push(Span {
                start: piece.start,
                end: piece.end,
            }),
        }
    }
    spans
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bucket {
    pub start: i64,
    /// Nominal end; the current bucket ends in the future.
    pub end: i64,
    /// Time inside the bucket (up to `now`) that was measured.
    pub measured_ms: i64,
    pub avg_needs_you: f64,
    pub avg_working: f64,
    pub avg_idle: f64,
    pub avg_total: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LastHourData {
    pub buckets: Vec<Bucket>,
    /// Sum over the last hour of the sessions waiting on the user, in ms.
    pub total_waited_ms: i64,
}

/// Time-weighted accumulation of the pieces into `count` buckets of `bucket_ms` from `first`.
fn accumulate(list: &[Piece], first: i64, bucket_ms: i64, count: usize, now: i64) -> Vec<Bucket> {
    #[derive(Clone, Copy, Default)]
    struct Sum {
        ms: f64,
        n: f64,
        w: f64,
        i: f64,
        t: f64,
    }
    let mut sums = vec![Sum::default(); count];
    for piece in list {
        let lo = ((piece.start - first).div_euclid(bucket_ms)).max(0);
        let hi = ((piece.end - 1 - first).div_euclid(bucket_ms)).min(count as i64 - 1);
        let mut b = lo;
        while b <= hi {
            let bs = first + b * bucket_ms;
            let ms = piece.end.min(bs + bucket_ms).min(now) - piece.start.max(bs);
            if ms > 0 {
                let ms = ms as f64;
                let s = &mut sums[b as usize];
                s.ms += ms;
                s.n += f64::from(piece.needs_you) * ms;
                s.w += f64::from(piece.working) * ms;
                s.i += f64::from(piece.idle) * ms;
                s.t += f64::from(piece.total) * ms;
            }
            b += 1;
        }
    }
    sums.iter()
        .enumerate()
        .map(|(b, s)| {
            let avg = |v: f64| if s.ms > 0.0 { v / s.ms } else { 0.0 };
            Bucket {
                start: first + b as i64 * bucket_ms,
                end: first + (b as i64 + 1) * bucket_ms,
                measured_ms: s.ms.round() as i64,
                avg_needs_you: avg(s.n),
                avg_working: avg(s.w),
                avg_idle: avg(s.i),
                avg_total: avg(s.t),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
pub struct BucketOptions {
    pub bucket_ms: i64,
    pub count: usize,
}

impl Default for BucketOptions {
    fn default() -> Self {
        Self {
            bucket_ms: FIVE_MINUTES_MS,
            count: 12,
        }
    }
}

/// Buckets on the clock's marks (the last one is the current, partial one) with the
/// time-weighted average counts over their measured time.
pub fn last_hour_buckets(
    points: &[HistoryPoint],
    starts: &[i64],
    now: i64,
    options: BucketOptions,
    filter: AgentFilter,
) -> LastHourData {
    let BucketOptions { bucket_ms, count } = options;
    let last_start = now.div_euclid(bucket_ms) * bucket_ms;
    let first = last_start - (count as i64 - 1) * bucket_ms;
    let list = pieces(points, starts, now, now - first, filter);
    let buckets = accumulate(&list, first, bucket_ms, count, now);
    let hour_from = now - HOUR_MS;
    let mut total_waited_ms = 0;
    for piece in &list {
        let ms = piece.end - piece.start.max(hour_from);
        if ms > 0 {
            total_waited_ms += i64::from(piece.needs_you) * ms;
        }
    }
    LastHourData {
        buckets,
        total_waited_ms,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeriesBucket {
    pub start: i64,
    pub end: i64,
    pub measured_ms: i64,
    /// Average count over the measured time, `None` when nothing was measured.
    pub value: Option<f64>,
}

/// The average of one count over `bucket_count` equal buckets ending at `now`.
pub fn count_series(
    points: &[HistoryPoint],
    starts: &[i64],
    key: CountKey,
    now: i64,
    window_ms: i64,
    bucket_count: usize,
    filter: AgentFilter,
) -> Vec<SeriesBucket> {
    let bucket_ms = window_ms / bucket_count as i64;
    let first = now - window_ms;
    let mut list = pieces(points, starts, now, window_ms, filter);
    let ended = match key {
        CountKey::Finished => Some(OverviewStatus::Finished),
        CountKey::Failed => Some(OverviewStatus::Failed),
        _ => None,
    };
    if let Some(ended) = ended {
        // The samples keep no aggregate for these: count them from each agent's status and
        // carry the result in `working`, which the shared accumulation then averages.
        for piece in &mut list {
            let n = piece
                .statuses
                .iter()
                .filter(|(id, st)| **st == ended && filter.is_none_or(|f| f(id)))
                .count();
            piece.working = u32::try_from(n).unwrap_or(u32::MAX);
        }
    }
    accumulate(&list, first, bucket_ms, bucket_count, now)
        .into_iter()
        .map(|b| SeriesBucket {
            start: b.start,
            end: b.end,
            measured_ms: b.measured_ms,
            value: (b.measured_ms > 0).then_some(match key {
                CountKey::NeedsYou => b.avg_needs_you,
                CountKey::Working | CountKey::Finished | CountKey::Failed => b.avg_working,
                CountKey::Idle => b.avg_idle,
            }),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentStatus {
    Status(OverviewStatus),
    Unmeasured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub start: i64,
    pub end: i64,
    pub status: SegmentStatus,
}

/// Status segments per agent over the window, oldest first, equal neighbours merged. An agent
/// absent from a sample has no segment there. Unmeasured time is a segment only for an agent
/// seen before it, or, when it opens the window, seen after it (an agent that first shows up
/// after a gap has no segment for it).
pub fn timeline_segments(
    points: &[HistoryPoint],
    starts: &[i64],
    agent_ids: &[String],
    now: i64,
    window_ms: i64,
) -> BTreeMap<String, Vec<Segment>> {
    let from = now - window_ms;
    let list = pieces(points, starts, now, window_ms, None);
    struct Item<'a, 'b> {
        span: Span,
        piece: Option<&'b Piece<'a>>,
    }
    let mut items: Vec<Item> = Vec::new();
    let mut cursor = from;
    for piece in &list {
        if piece.start > cursor {
            items.push(Item {
                span: Span {
                    start: cursor,
                    end: piece.start,
                },
                piece: None,
            });
        }
        items.push(Item {
            span: Span {
                start: piece.start,
                end: piece.end,
            },
            piece: Some(piece),
        });
        cursor = piece.end;
    }
    if cursor < now {
        items.push(Item {
            span: Span {
                start: cursor,
                end: now,
            },
            piece: None,
        });
    }

    let mut result = BTreeMap::new();
    for id in agent_ids {
        let mut segments: Vec<Segment> = Vec::new();
        let last_present = items
            .iter()
            .rposition(|it| it.piece.is_some_and(|p| p.statuses.contains_key(id)));
        let mut seen = false;
        for (idx, item) in items.iter().enumerate() {
            let status = match item.piece {
                Some(piece) => {
                    let Some(status) = piece.statuses.get(id) else {
                        continue;
                    };
                    seen = true;
                    SegmentStatus::Status(*status)
                }
                None => {
                    let leading = idx == 0;
                    if !(seen || (leading && last_present.is_some_and(|l| l > idx))) {
                        continue;
                    }
                    SegmentStatus::Unmeasured
                }
            };
            match segments.last_mut() {
                Some(prev) if prev.status == status && prev.end == item.span.start => {
                    prev.end = item.span.end;
                }
                _ => segments.push(Segment {
                    start: item.span.start,
                    end: item.span.end,
                    status,
                }),
            }
        }
        result.insert(id.clone(), segments);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use OverviewStatus as S;

    const MIN: i64 = 60_000;
    // 2026-01-01 12:00:00 UTC, on a 5 min mark.
    const T0: i64 = 1_767_268_800_000;

    fn point(at: i64, statuses: &[(&str, S)]) -> HistoryPoint {
        let count = |s: S| statuses.iter().filter(|(_, x)| *x == s).count() as u32;
        HistoryPoint {
            at,
            needs_you: count(S::NeedsYou),
            working: count(S::Working),
            idle: count(S::Idle),
            total: statuses.len() as u32,
            statuses: statuses
                .iter()
                .map(|(k, v)| ((*k).to_string(), *v))
                .collect(),
        }
    }

    /// One sample every 5 s from `from` to `to` inclusive.
    fn run(from: i64, to: i64, statuses: &[(&str, S)]) -> Vec<HistoryPoint> {
        let mut out = Vec::new();
        let mut t = from;
        while t <= to {
            out.push(point(t, statuses));
            t += 5000;
        }
        out
    }

    fn span(start: i64, end: i64) -> Span {
        Span { start, end }
    }

    #[test]
    fn spans_cover_samples_and_extend_the_fresh_newest_to_now() {
        let points = run(T0, T0 + MIN, &[("a", S::Working)]);
        assert_eq!(
            measured_spans(&points, &[], T0 + MIN + 3000, 10 * MIN),
            [span(T0, T0 + MIN + 3000)]
        );
    }

    #[test]
    fn spans_break_on_a_gap_above_15s() {
        let mut points = run(T0, T0 + MIN, &[("a", S::Working)]);
        points.extend(run(T0 + 2 * MIN, T0 + 3 * MIN, &[("a", S::Working)]));
        assert_eq!(
            measured_spans(&points, &[], T0 + 3 * MIN, 10 * MIN),
            [span(T0, T0 + MIN), span(T0 + 2 * MIN, T0 + 3 * MIN)]
        );
    }

    #[test]
    fn a_15s_gap_stays_whole() {
        let points = [point(T0, &[]), point(T0 + 15_000, &[])];
        assert_eq!(
            measured_spans(&points, &[], T0 + 15_000, MIN),
            [span(T0, T0 + 15_000)]
        );
    }

    #[test]
    fn spans_break_on_a_start_between_two_samples() {
        let points = run(T0, T0 + 20_000, &[("a", S::Idle)]);
        assert_eq!(
            measured_spans(&points, &[T0 + 7000], T0 + 20_000, MIN),
            [span(T0, T0 + 5000), span(T0 + 10_000, T0 + 20_000)]
        );
    }

    #[test]
    fn before_the_first_sample_and_a_stale_tail_are_unmeasured() {
        let points = run(T0 + 30 * MIN, T0 + 31 * MIN, &[]);
        assert_eq!(
            measured_spans(&points, &[], T0 + 40 * MIN, 60 * MIN),
            [span(T0 + 30 * MIN, T0 + 31 * MIN)]
        );
    }

    #[test]
    fn spans_clip_to_the_window() {
        let points = run(T0, T0 + 10 * MIN, &[]);
        assert_eq!(
            measured_spans(&points, &[], T0 + 10 * MIN, 4 * MIN),
            [span(T0 + 6 * MIN, T0 + 10 * MIN)]
        );
    }

    #[test]
    fn buckets_align_to_5_minute_marks_with_a_partial_last_one() {
        let now = T0 + 7 * MIN;
        let data = last_hour_buckets(&[], &[], now, BucketOptions::default(), None);
        assert_eq!(data.buckets.len(), 12);
        assert_eq!(data.buckets[11].start, T0 + 5 * MIN);
        assert_eq!(data.buckets[0].start, T0 - 50 * MIN);
        assert!(data.buckets.iter().all(|b| b.start % (5 * MIN) == 0));
        assert!(data.buckets.iter().all(|b| b.measured_ms == 0));
    }

    #[test]
    fn buckets_average_by_time_over_measured_time_only() {
        let now = T0 + 5 * MIN;
        let mut points = run(
            T0,
            T0 + 2 * MIN + 25_000,
            &[("a", S::Working), ("b", S::Working), ("c", S::NeedsYou)],
        );
        points.extend(run(T0 + 2 * MIN + 30_000, now, &[("a", S::Idle)]));
        let data = last_hour_buckets(&points, &[], now - 1, BucketOptions::default(), None);
        let b = &data.buckets[11];
        assert_eq!(b.start, T0);
        assert_eq!(b.measured_ms, 5 * MIN - 1);
        assert!((b.avg_needs_you - 0.5).abs() < 0.05);
        assert!((b.avg_working - 1.0).abs() < 0.05);
        assert!((b.avg_idle - 0.5).abs() < 0.05);
        assert!((b.avg_total - 2.0).abs() < 0.05);
    }

    #[test]
    fn a_gap_stays_unmeasured_and_a_partial_bucket_short() {
        let now = T0 + 2 * MIN;
        let mut points = run(T0, T0 + 30_000, &[("a", S::Working)]);
        points.extend(run(T0 + MIN + 30_000, now, &[("a", S::Working)]));
        let data = last_hour_buckets(&points, &[], now, BucketOptions::default(), None);
        let cur = &data.buckets[11];
        assert_eq!(cur.start, T0);
        assert_eq!(cur.measured_ms, 60_000);
        assert!((cur.avg_working - 1.0).abs() < 1e-9);
    }

    #[test]
    fn waited_time_over_the_last_hour() {
        let now = T0 + 20 * MIN;
        let mut points = run(
            T0,
            T0 + 4 * MIN + 55_000,
            &[("a", S::NeedsYou), ("b", S::NeedsYou)],
        );
        points.extend(run(T0 + 5 * MIN, now, &[("a", S::Working), ("b", S::Idle)]));
        assert_eq!(
            last_hour_buckets(&points, &[], now, BucketOptions::default(), None).total_waited_ms,
            2 * 5 * MIN
        );
    }

    #[test]
    fn waits_older_than_an_hour_do_not_count() {
        let now = T0 + 90 * MIN;
        let mut points = run(T0, T0 + 10 * MIN, &[("a", S::NeedsYou)]);
        points.extend(run(T0 + 89 * MIN, now, &[("a", S::Idle)]));
        assert_eq!(
            last_hour_buckets(&points, &[], now, BucketOptions::default(), None).total_waited_ms,
            0
        );
    }

    #[test]
    fn a_filter_recomputes_from_per_agent_statuses() {
        let now = T0 + MIN;
        let points = run(
            T0,
            now,
            &[("a", S::NeedsYou), ("b", S::Working), ("c", S::Working)],
        );
        let filter = |id: &str| id != "a";
        let data = last_hour_buckets(&points, &[], now, BucketOptions::default(), Some(&filter));
        assert_eq!(data.buckets[11].avg_needs_you, 0.0);
        assert_eq!(data.buckets[11].avg_working, 2.0);
        assert_eq!(data.buckets[11].avg_total, 2.0);
        assert_eq!(data.total_waited_ms, 0);
    }

    #[test]
    fn honours_bucket_ms_and_count() {
        let data = last_hour_buckets(
            &[],
            &[],
            T0 + MIN,
            BucketOptions {
                bucket_ms: MIN,
                count: 3,
            },
            None,
        );
        let starts: Vec<i64> = data.buckets.iter().map(|b| b.start).collect();
        assert_eq!(starts, [T0 - MIN, T0, T0 + MIN]);
    }

    fn st(s: S) -> SegmentStatus {
        SegmentStatus::Status(s)
    }
    fn seg(start: i64, end: i64, status: SegmentStatus) -> Segment {
        Segment { start, end, status }
    }
    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn timeline_merges_equal_statuses_and_splits_on_change() {
        let now = T0 + 3 * MIN;
        let mut points = run(T0, T0 + MIN - 5000, &[("a", S::Working)]);
        points.extend(run(T0 + MIN, now, &[("a", S::Idle)]));
        let seg_map = timeline_segments(&points, &[], &ids(&["a"]), now, 3 * MIN);
        assert_eq!(
            seg_map["a"],
            [
                seg(T0, T0 + MIN, st(S::Working)),
                seg(T0 + MIN, now, st(S::Idle))
            ]
        );
    }

    #[test]
    fn timeline_marks_a_gap_unmeasured_for_an_agent_seen_before_it() {
        let now = T0 + 3 * MIN;
        let mut points = run(T0, T0 + 30_000, &[("a", S::Working)]);
        points.extend(run(T0 + 2 * MIN, now, &[("a", S::Working)]));
        let m = timeline_segments(&points, &[], &ids(&["a"]), now, 3 * MIN);
        let statuses: Vec<SegmentStatus> = m["a"].iter().map(|s| s.status).collect();
        assert_eq!(
            statuses,
            [st(S::Working), SegmentStatus::Unmeasured, st(S::Working)]
        );
        assert_eq!(
            m["a"][1],
            seg(T0 + 30_000, T0 + 2 * MIN, SegmentStatus::Unmeasured)
        );
    }

    #[test]
    fn timeline_hatches_the_window_start_only_for_agents_seen_later() {
        let now = T0 + 3 * MIN;
        let points = run(T0 + 2 * MIN, now, &[("a", S::Idle)]);
        let m = timeline_segments(&points, &[], &ids(&["a", "ghost"]), now, 3 * MIN);
        assert_eq!(m["a"][0], seg(T0, T0 + 2 * MIN, SegmentStatus::Unmeasured));
        assert!(m["ghost"].is_empty());
    }

    #[test]
    fn no_segment_while_absent_and_none_for_a_late_arrival_after_a_gap() {
        let now = T0 + 4 * MIN;
        let mut points = run(T0, T0 + 30_000, &[("a", S::Idle)]);
        points.extend(run(T0 + 2 * MIN, now, &[("a", S::Idle), ("b", S::Working)]));
        let m = timeline_segments(&points, &[], &ids(&["a", "b"]), now, 4 * MIN);
        assert_eq!(m["b"], [seg(T0 + 2 * MIN, now, st(S::Working))]);
    }

    #[test]
    fn timeline_splits_at_a_start() {
        let points = run(T0, T0 + 20_000, &[("a", S::Working)]);
        let m = timeline_segments(&points, &[T0 + 7000], &ids(&["a"]), T0 + 20_000, 20_000);
        let statuses: Vec<SegmentStatus> = m["a"].iter().map(|s| s.status).collect();
        assert_eq!(
            statuses,
            [st(S::Working), SegmentStatus::Unmeasured, st(S::Working)]
        );
    }

    #[test]
    fn count_series_averages_one_count_per_bucket() {
        let now = T0 + 15 * MIN;
        let points = run(T0 + 5 * MIN, now, &[("a", S::Working), ("b", S::Idle)]);
        let series = count_series(&points, &[], CountKey::Working, now, 15 * MIN, 3, None);
        let values: Vec<Option<f64>> = series.iter().map(|s| s.value).collect();
        assert_eq!(values, [None, Some(1.0), Some(1.0)]);
        assert_eq!(
            count_series(&points, &[], CountKey::Idle, now, 15 * MIN, 3, None)[1].value,
            Some(1.0)
        );
    }

    #[test]
    fn count_series_counts_finished_and_failed_from_the_statuses() {
        let now = T0 + 10 * MIN;
        let mut points = run(
            T0,
            T0 + 5 * MIN - 5000,
            &[("a", S::Working), ("b", S::Finished)],
        );
        points.extend(run(
            T0 + 5 * MIN,
            now,
            &[("a", S::Failed), ("b", S::Finished)],
        ));
        let finished = count_series(&points, &[], CountKey::Finished, now, 10 * MIN, 2, None);
        assert_eq!(
            finished.iter().map(|s| s.value).collect::<Vec<_>>(),
            [Some(1.0), Some(1.0)]
        );
        let failed = count_series(&points, &[], CountKey::Failed, now, 10 * MIN, 2, None);
        assert_eq!(
            failed.iter().map(|s| s.value).collect::<Vec<_>>(),
            [Some(0.0), Some(1.0)]
        );
        let only_a = |id: &str| id == "a";
        let filtered = count_series(
            &points,
            &[],
            CountKey::Finished,
            now,
            10 * MIN,
            2,
            Some(&only_a),
        );
        assert_eq!(filtered[0].value, Some(0.0));
    }

    #[test]
    fn count_series_applies_the_filter() {
        let now = T0 + 5 * MIN;
        let points = run(T0, now, &[("a", S::Working), ("b", S::Working)]);
        let only_a = |id: &str| id == "a";
        let series = count_series(
            &points,
            &[],
            CountKey::Working,
            now,
            5 * MIN,
            1,
            Some(&only_a),
        );
        assert_eq!(series[0].value, Some(1.0));
    }
}
