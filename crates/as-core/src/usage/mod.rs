//! Usage and plan limits (Hydra `shared/usage/*` + `electron/usage/*`): log parsers, pricing,
//! periods, summaries and reports, the incremental store, Claude's OAuth limits client, alerts and
//! the service that schedules them.

pub mod alerts;
pub mod claude_line;
pub mod codex_line;
pub mod credentials;
pub mod files;
pub mod limits;
pub mod limits_client;
pub mod periods;
pub mod pricing;
pub mod report;
pub mod service;
pub mod store;
pub mod summary;
pub mod types;

pub use types::*;

/// RFC 3339 / ISO 8601 timestamp to epoch ms (`Date.parse` for the shapes the logs use).
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(d.timestamp_millis());
    }
    // `2026-10-22T17:45:01.123` (no zone) is local time in JS; logs always carry a zone, so treat
    // a zone-less stamp as UTC rather than guessing.
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .map(|n| n.and_utc().timestamp_millis())
}
