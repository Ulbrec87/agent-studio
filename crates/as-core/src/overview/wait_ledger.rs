//! What the wait totals are worked out from, kept for the last 9 days: every move into or out of
//! `needs-you`, and the measured stretches (runs of points no more than 15 s apart, broken by
//! each daemon start). Hydra `electron/overview/WaitLedger.ts`.
//!
//! The history files hold all of it, but the history store keeps only the newest 1 000 events
//! and 6 h of points in memory, so the store seeds this from every file it restores (`append`)
//! and then feeds it live. Compact: a transition is a few fields, a stretch is two numbers.

use super::buckets::MAX_SAMPLE_GAP_MS;
use super::types::{MeasuredSpan, OverviewEvent, WaitLedgerData, WaitTransition};
use super::waits::{LEDGER_DAYS, transition_of_event};

/// The ledger keeps this long.
pub const LEDGER_MS: i64 = LEDGER_DAYS * 24 * 60 * 60 * 1000;

#[derive(Debug, Default)]
pub struct WaitLedger {
    transitions: Vec<WaitTransition>,
    spans: Vec<MeasuredSpan>,
    starts: Vec<i64>,
    /// The next point goes on the newest stretch if it is near enough.
    continuing: bool,
}

impl WaitLedger {
    pub fn add_events(&mut self, events: &[OverviewEvent]) {
        self.transitions
            .extend(events.iter().filter_map(transition_of_event));
    }

    /// A point taken at `at`: the newest stretch goes on to it, or a new one begins.
    pub fn add_point(&mut self, at: i64) {
        match self.spans.last_mut() {
            Some(last)
                if self.continuing && at >= last.end && at - last.end <= MAX_SAMPLE_GAP_MS =>
            {
                last.end = at;
            }
            Some(last) if at <= last.end => {} // clock went back: adds nothing
            _ => self.spans.push(MeasuredSpan { start: at, end: at }),
        }
        self.continuing = true;
    }

    /// The daemon started at `at`: nothing was measured from the last point to the next one.
    pub fn add_start(&mut self, at: i64) {
        self.starts.push(at);
        self.continuing = false;
    }

    /// Adds the data of a stretch of history that comes after everything held, for example the
    /// next day's file. A stretch that goes on from the last one is joined to it.
    pub fn append(&mut self, data: WaitLedgerData) {
        self.transitions.extend(data.transitions);
        let mut rest: &[MeasuredSpan] = &data.spans;
        if let (Some(last), Some(first)) = (self.spans.last_mut(), data.spans.first())
            && first.start >= last.end
            && first.start - last.end <= MAX_SAMPLE_GAP_MS
            && !data
                .starts
                .iter()
                .any(|s| *s > last.end && *s <= first.start)
        {
            last.end = last.end.max(first.end);
            rest = &data.spans[1..];
        }
        self.spans.extend_from_slice(rest);
        self.continuing = data
            .spans
            .last()
            .is_some_and(|newest| data.starts.iter().all(|s| *s <= newest.end));
        self.starts.extend(data.starts);
    }

    /// Forgets what is older than 9 days before `now`.
    pub fn prune(&mut self, now: i64) {
        let cutoff = now - LEDGER_MS;
        self.transitions.retain(|t| t.at >= cutoff);
        self.spans.retain(|s| s.end >= cutoff);
        for s in &mut self.spans {
            s.start = s.start.max(cutoff);
        }
        self.starts.retain(|s| *s >= cutoff);
    }

    /// A copy of what is held, transitions in time order.
    pub fn data(&self) -> WaitLedgerData {
        let mut transitions = self.transitions.clone();
        transitions.sort_by_key(|t| t.at);
        WaitLedgerData {
            transitions,
            spans: self.spans.clone(),
            starts: self.starts.clone(),
        }
    }
}
