//! Text for the Overview cards (Hydra `src/components/Overview/{format,historyFacts,sessionFacts}.ts`).

use as_core::overview::waits::{DayZone, LocalZone, UtcZone};
use as_core::overview::{EventKind, OverviewEvent, OverviewSession, OverviewStatus, WaitingReason};
use chrono::{Datelike, Local, TimeZone, Utc};

/// The wall clock the cards read: `now` and the time zone (UTC in tests so snapshots do not
/// depend on the machine).
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub now: i64,
    pub utc: bool,
}

impl Clock {
    pub fn zone(&self) -> &'static dyn DayZone {
        if self.utc { &UtcZone } else { &LocalZone }
    }

    fn parts(&self, at: i64) -> Option<(i32, u32, u32, String, String)> {
        if self.utc {
            let d = Utc.timestamp_millis_opt(at).single()?;
            Some((
                d.year(),
                d.month(),
                d.day(),
                d.format("%H:%M:%S").to_string(),
                d.format("%H:%M").to_string(),
            ))
        } else {
            let d = Local.timestamp_millis_opt(at).single()?;
            Some((
                d.year(),
                d.month(),
                d.day(),
                d.format("%H:%M:%S").to_string(),
                d.format("%H:%M").to_string(),
            ))
        }
    }

    fn ymd(&self, at: i64) -> Option<(i32, u32, u32)> {
        self.parts(at).map(|p| (p.0, p.1, p.2))
    }

    pub fn same_day(&self, a: i64, b: i64) -> bool {
        self.ymd(a) == self.ymd(b)
    }

    /// `14:02:11` in the clock's time zone.
    pub fn time_of_day(&self, at: i64) -> String {
        self.parts(at).map(|p| p.3).unwrap_or_default()
    }

    /// `14:02`, with the date when it is not today (`Oct 21 14:02`).
    pub fn minute(&self, at: i64) -> String {
        let Some((_, month, day, _, hm)) = self.parts(at) else {
            return String::new();
        };
        if self.same_day(at, self.now) {
            hm
        } else {
            format!("{} {day} {hm}", month_name(month))
        }
    }

    /// `14:02:11`, with the date when it is not today.
    pub fn clock(&self, at: i64) -> String {
        let Some((_, month, day, hms, _)) = self.parts(at) else {
            return String::new();
        };
        if self.same_day(at, self.now) {
            hms
        } else {
            format!("{} {day} {hms}", month_name(month))
        }
    }

    /// Relative time for a past moment: `just now`, `12m ago`, `3h ago`, then `yesterday 14:02`,
    /// `Oct 21`, or `Oct 21, 2025` for another year.
    pub fn ago(&self, at: i64) -> String {
        let diff = self.now - at;
        if diff < 60_000 {
            return "just now".into();
        }
        if diff < 3_600_000 {
            return format!("{}m ago", diff / 60_000);
        }
        if diff < 86_400_000 {
            return format!("{}h ago", diff / 3_600_000);
        }
        let (Some((y, m, d, _, hm)), Some((ny, _, _))) = (self.parts(at), self.ymd(self.now))
        else {
            return String::new();
        };
        let yesterday = self.ymd(
            self.zone()
                .shift_days(self.zone().midnight_of(self.now), -1),
        );
        if yesterday == Some((y, m, d)) {
            return format!("yesterday {hm}");
        }
        if y == ny {
            format!("{} {d}", month_name(m))
        } else {
            format!("{} {d}, {y}", month_name(m))
        }
    }
}

fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    NAMES
        .get(month.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("")
}

/// A duration for timers: `45s`, `4m 12s`, `1h 03m`. `compact` drops seconds from a minute on:
/// `12m`, `1h 03m`.
pub fn format_duration(ms: i64, compact: bool) -> String {
    let total = (ms / 1000).max(0);
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        if compact {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {seconds:02}s")
        }
    } else {
        format!("{seconds}s")
    }
}

/// `182431` -> `182,431`.
pub fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Last path segment of a project directory.
pub fn project_name(project_dir: &str) -> String {
    project_dir
        .split(['/', '\\'])
        .rfind(|p| !p.is_empty())
        .unwrap_or(project_dir)
        .to_string()
}

pub fn status_label(session: &OverviewSession) -> &'static str {
    if session.stale {
        return "Idle (stale)";
    }
    match session.status {
        OverviewStatus::NeedsYou => "Needs you",
        OverviewStatus::Working => "Working",
        OverviewStatus::Idle => "Idle",
        OverviewStatus::Starting => "Starting",
        OverviewStatus::Finished => "Finished",
        OverviewStatus::Failed => "Failed",
        OverviewStatus::NotRunning => "Not running",
    }
}

pub fn reason_label(reason: Option<WaitingReason>) -> &'static str {
    match reason {
        Some(WaitingReason::Permission) => "Waiting for permission",
        Some(WaitingReason::Question) => "Asked you a question",
        _ => "Waiting for you",
    }
}

/// Branch to show: the live one, else the app's own worktree branch.
pub fn branch_of(session: &OverviewSession) -> Option<&str> {
    session
        .branch
        .as_deref()
        .or(session.worktree_branch.as_deref())
}

/// `quiet for 12m`, or `None` when the session is not quiet.
pub fn quiet_text(session: &OverviewSession, now: i64) -> Option<String> {
    let quiet_ms = session.quiet_for_ms?;
    let quiet = if session.last_output_at > 0 {
        now - session.last_output_at
    } else {
        quiet_ms
    };
    Some(format!(
        "quiet for {}",
        format_duration(quiet.max(quiet_ms), true)
    ))
}

/// How long a session has been in its status, as a fact: `Working 3m`, `ended 3h ago · exit 1`,
/// `last active 2d ago`.
pub fn status_fact(session: &OverviewSession, clock: &Clock) -> String {
    let since = session.status_since;
    match session.status {
        OverviewStatus::Finished | OverviewStatus::Failed => {
            let at = session.ended_at.unwrap_or(since);
            let exit = match (session.status, session.exit_code) {
                (OverviewStatus::Failed, Some(code)) => format!(" \u{b7} exit {code}"),
                _ => String::new(),
            };
            format!("ended {}{exit}", clock.ago(at))
        }
        OverviewStatus::NotRunning => format!(
            "last active {}",
            clock.ago(session.ended_at.unwrap_or(since))
        ),
        _ => format!(
            "{} {}",
            status_label(session),
            format_duration(clock.now - since, true)
        ),
    }
}

// ── events ──────────────────────────────────────────────────────────────────

fn to_status_text(e: &OverviewEvent) -> &'static str {
    match e.to {
        Some(OverviewStatus::NeedsYou) => match e.waiting_reason {
            Some(WaitingReason::Permission) => "is waiting for permission",
            Some(WaitingReason::Question) => "is asking a question",
            _ => "is waiting for you",
        },
        Some(OverviewStatus::Working) => "is working",
        Some(OverviewStatus::Idle) => "went idle",
        Some(OverviewStatus::Starting) => "is starting",
        Some(OverviewStatus::Finished) => "finished",
        Some(OverviewStatus::Failed) => "failed",
        Some(OverviewStatus::NotRunning) => "stopped",
        None => "changed status",
    }
}

/// One line for an event, without the agent's name (`started`, `is working after waiting 4m
/// 12s`). A `resumed` event has no agent, so its line stands alone.
pub fn event_text(e: &OverviewEvent) -> String {
    match e.kind {
        EventKind::Appeared => "started".into(),
        EventKind::Ended => "ended".into(),
        EventKind::Resumed => format!(
            "Watching resumed \u{2014} {} not measured",
            format_duration(e.gap_ms.unwrap_or(0), true)
        ),
        EventKind::Status => {
            let waited = match (e.from, e.waited_ms) {
                (Some(OverviewStatus::NeedsYou), Some(ms)) => {
                    format!(" after waiting {}", format_duration(ms, false))
                }
                _ => String::new(),
            };
            format!("{}{waited}", to_status_text(e))
        }
    }
}

/// The newest wait that ended within the last hour (leaving needs-you).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastWait {
    pub name: String,
    pub waited_ms: i64,
    pub ended_at: i64,
}

pub fn last_wait_from(events: &[&OverviewEvent], now: i64) -> Option<LastWait> {
    events
        .iter()
        .filter(|e| {
            e.from == Some(OverviewStatus::NeedsYou)
                && e.waited_ms.is_some()
                && e.at >= now - 3_600_000
        })
        .max_by_key(|e| e.at)
        .map(|e| LastWait {
            name: e.name.clone(),
            waited_ms: e.waited_ms.unwrap_or(0),
            ended_at: e.at,
        })
}

pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}\u{2026}")
}

/// Greedy word wrap to `width` columns (long words are cut).
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::new();
        for word in raw.split_whitespace() {
            let mut word = word.to_string();
            while word.chars().count() > width {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                let head: String = word.chars().take(width).collect();
                word = word.chars().skip(width).collect();
                lines.push(head);
            }
            if line.is_empty() {
                line = word;
            } else if line.chars().count() + 1 + word.chars().count() <= width {
                line.push(' ');
                line.push_str(&word);
            } else {
                lines.push(std::mem::take(&mut line));
                line = word;
            }
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use OverviewStatus as S;
    use chrono::DateTime;

    fn utc(now: i64) -> Clock {
        Clock { now, utc: true }
    }
    fn at(iso: &str) -> i64 {
        DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(45_000, false), "45s");
        assert_eq!(format_duration(252_000, false), "4m 12s");
        assert_eq!(format_duration(252_000, true), "4m");
        assert_eq!(format_duration(3_780_000, false), "1h 03m");
        assert_eq!(format_duration(3_780_000, true), "1h 03m");
        assert_eq!(format_duration(-5, false), "0s");
        assert_eq!(format_count(182_431), "182,431");
        assert_eq!(format_count(12), "12");
        assert_eq!(format_count(1000), "1,000");
    }

    #[test]
    fn project_names() {
        assert_eq!(project_name("/work/shop/"), "shop");
        assert_eq!(project_name("C:\\work\\api"), "api");
        assert_eq!(project_name("/"), "/");
    }

    #[test]
    fn clock_and_ago() {
        let now = at("2026-10-22T14:30:00Z");
        let c = utc(now);
        assert_eq!(c.time_of_day(at("2026-10-22T14:02:11Z")), "14:02:11");
        assert_eq!(c.minute(at("2026-10-22T14:02:11Z")), "14:02");
        assert_eq!(c.minute(at("2026-10-21T14:02:11Z")), "Oct 21 14:02");
        assert_eq!(c.clock(at("2026-10-21T14:02:11Z")), "Oct 21 14:02:11");
        assert_eq!(c.ago(now - 30_000), "just now");
        assert_eq!(c.ago(now - 12 * 60_000), "12m ago");
        assert_eq!(c.ago(now - 3 * 3_600_000), "3h ago");
        assert_eq!(c.ago(at("2026-10-21T09:15:00Z")), "yesterday 09:15");
        assert_eq!(c.ago(at("2026-10-10T09:15:00Z")), "Oct 10");
        assert_eq!(c.ago(at("2025-10-10T09:15:00Z")), "Oct 10, 2025");
    }

    fn session(status: S) -> OverviewSession {
        OverviewSession::basic("a", status)
    }

    #[test]
    fn status_facts() {
        let now = at("2026-10-22T14:30:00Z");
        let c = utc(now);
        let mut s = session(S::Working);
        s.status_since = now - 3 * 60_000;
        assert_eq!(status_fact(&s, &c), "Working 3m");
        s.status = S::Idle;
        s.stale = true;
        assert_eq!(status_fact(&s, &c), "Idle (stale) 3m");
        let mut f = session(S::Failed);
        f.ended_at = Some(now - 3 * 3_600_000);
        f.exit_code = Some(1);
        assert_eq!(status_fact(&f, &c), "ended 3h ago \u{b7} exit 1");
        f.status = S::Finished;
        assert_eq!(status_fact(&f, &c), "ended 3h ago");
        let mut n = session(S::NotRunning);
        n.ended_at = Some(now - 2 * 86_400_000);
        assert_eq!(status_fact(&n, &c), "last active Oct 20");
        let mut q = session(S::Working);
        q.quiet_for_ms = Some(12 * 60_000);
        q.last_output_at = now - 12 * 60_000;
        assert_eq!(quiet_text(&q, now).as_deref(), Some("quiet for 12m"));
        assert_eq!(quiet_text(&session(S::Working), now), None);
    }

    fn ev(kind: EventKind) -> OverviewEvent {
        OverviewEvent {
            id: "x".into(),
            at: 1,
            kind,
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
    fn event_texts() {
        assert_eq!(event_text(&ev(EventKind::Appeared)), "started");
        assert_eq!(event_text(&ev(EventKind::Ended)), "ended");
        let mut resumed = ev(EventKind::Resumed);
        resumed.gap_ms = Some(180_000);
        assert_eq!(
            event_text(&resumed),
            "Watching resumed \u{2014} 3m not measured"
        );
        let mut e = ev(EventKind::Status);
        e.from = Some(S::Working);
        e.to = Some(S::NeedsYou);
        e.waiting_reason = Some(WaitingReason::Permission);
        assert_eq!(event_text(&e), "is waiting for permission");
        e.from = Some(S::NeedsYou);
        e.to = Some(S::Working);
        e.waited_ms = Some(252_000);
        assert_eq!(event_text(&e), "is working after waiting 4m 12s");
        e.to = Some(S::Finished);
        e.waited_ms = None;
        assert_eq!(event_text(&e), "finished");
    }

    #[test]
    fn the_last_wait_is_the_newest_one_that_ended_within_the_hour() {
        let now = 10_000_000;
        let mk = |at: i64, waited: Option<i64>, from: Option<S>| OverviewEvent {
            at,
            waited_ms: waited,
            from,
            name: format!("e{at}"),
            ..ev(EventKind::Status)
        };
        let old = mk(now - 4_000_000, Some(5), Some(S::NeedsYou));
        let a = mk(now - 1000, Some(60_000), Some(S::NeedsYou));
        let b = mk(now - 500_000, Some(70_000), Some(S::NeedsYou));
        let other = mk(now - 10, None, Some(S::Working));
        let found = last_wait_from(&[&old, &a, &b, &other], now).unwrap();
        assert_eq!(
            (found.name.as_str(), found.waited_ms),
            (&*format!("e{}", now - 1000), 60_000)
        );
        assert!(last_wait_from(&[&old], now).is_none());
    }

    #[test]
    fn clipping_and_wrapping() {
        assert_eq!(clip("hello", 10), "hello");
        assert_eq!(clip("hello world", 6), "hello\u{2026}");
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("a\n\nb", 5), ["a", "", "b"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    }
}
