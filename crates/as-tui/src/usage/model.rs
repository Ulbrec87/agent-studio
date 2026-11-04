//! What the usage surfaces show, worked out from a [`UsageState`] (Hydra
//! `src/components/Usage/model.ts`).

use std::collections::HashSet;

use as_core::UsageChipProvider;
use as_core::UsageSettings;
use as_core::usage::limits::{limit_alert_level, pick_chip_windows};
use as_core::usage::types::{
    ClaudeProfileLimits, LimitWindow, LimitWindowKind, ProfileStatus, UsageProvider, UsageState,
};

/// Values older than this render dimmed.
pub const STALE_AFTER_MS: i64 = 15 * 60_000;

/// A window with the time its value was read.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedWindow {
    pub window: LimitWindow,
    /// Epoch ms of the fetch / log line, `None` when unknown.
    pub at: Option<i64>,
}

pub fn is_stale(at: Option<i64>, now: i64) -> bool {
    at.is_none_or(|a| now - a > STALE_AFTER_MS)
}

fn all_timed_windows(state: &UsageState) -> Vec<TimedWindow> {
    let mut out = Vec::new();
    for profile in &state.claude {
        for w in &profile.windows {
            out.push(TimedWindow {
                window: w.clone(),
                at: profile.fetched_at,
            });
        }
    }
    if let Some(codex) = &state.codex {
        for w in &codex.windows {
            out.push(TimedWindow {
                window: w.clone(),
                at: Some(codex.at),
            });
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChipWindows {
    pub session: Option<TimedWindow>,
    pub weekly: Option<TimedWindow>,
}

/// The most-used session and weekly window over every visible account, or of one provider
/// (`chipProvider`); a provider with no session and no weekly window falls back to all.
pub fn chip_windows(state: Option<&UsageState>, chip_provider: UsageChipProvider) -> ChipWindows {
    let Some(state) = state else {
        return ChipWindows::default();
    };
    let mut timed = all_timed_windows(state);
    let wanted = match chip_provider {
        UsageChipProvider::Auto => None,
        UsageChipProvider::Claude => Some(UsageProvider::Claude),
        UsageChipProvider::Codex => Some(UsageProvider::Codex),
    };
    if let Some(p) = wanted {
        let own: Vec<TimedWindow> = timed
            .iter()
            .filter(|t| t.window.provider == p)
            .cloned()
            .collect();
        if own.iter().any(|t| {
            matches!(
                t.window.kind,
                LimitWindowKind::Session | LimitWindowKind::Weekly
            )
        }) {
            timed = own;
        }
    }
    let (session, weekly) = pick_chip_windows(timed.iter().map(|t| &t.window));
    let find = |w: Option<&LimitWindow>| {
        w.and_then(|w| timed.iter().find(|t| std::ptr::eq(&t.window, w)).cloned())
    };
    ChipWindows {
        session: find(session),
        weekly: find(weekly),
    }
}

/// `Claude · alternative` / `Codex`: where a value comes from.
pub fn source_label(w: &LimitWindow) -> String {
    match w.provider {
        UsageProvider::Codex => "Codex".to_string(),
        UsageProvider::Claude => format!("Claude {}", w.account),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LimitGroup {
    pub key: String,
    pub provider: UsageProvider,
    pub name: String,
    /// `Pro`, `Pro · alternative`, or `""`.
    pub subtitle: String,
    pub profile: Option<ClaudeProfileLimits>,
    pub windows: Vec<TimedWindow>,
    /// Epoch ms of the Codex log line, for the source text.
    pub codex_at: Option<i64>,
}

/// One group per signed-in Claude account (all of them when none is signed in), then Codex.
pub fn build_limit_groups(state: &UsageState) -> Vec<LimitGroup> {
    let signed_in: Vec<&ClaudeProfileLimits> = state
        .claude
        .iter()
        .filter(|p| p.status != ProfileStatus::NotSignedIn)
        .collect();
    let shown: Vec<&ClaudeProfileLimits> = if signed_in.is_empty() {
        state.claude.iter().collect()
    } else {
        signed_in
    };
    let many = shown.len() > 1;
    let mut groups: Vec<LimitGroup> = shown
        .iter()
        .map(|profile| {
            let parts: Vec<&str> = [
                profile.plan.as_deref(),
                many.then_some(profile.account.as_str()),
            ]
            .into_iter()
            .flatten()
            .filter(|p| !p.is_empty())
            .collect();
            LimitGroup {
                key: format!("claude:{}", profile.account),
                provider: UsageProvider::Claude,
                name: "Claude".into(),
                subtitle: parts.join(" \u{b7} "),
                profile: Some((*profile).clone()),
                windows: profile
                    .windows
                    .iter()
                    .map(|w| TimedWindow {
                        window: w.clone(),
                        at: profile.fetched_at,
                    })
                    .collect(),
                codex_at: None,
            }
        })
        .collect();
    if let Some(codex) = &state.codex {
        groups.push(LimitGroup {
            key: "codex".into(),
            provider: UsageProvider::Codex,
            name: "Codex".into(),
            subtitle: codex.plan.clone().unwrap_or_default(),
            profile: None,
            windows: codex
                .windows
                .iter()
                .map(|w| TimedWindow {
                    window: w.clone(),
                    at: Some(codex.at),
                })
                .collect(),
            codex_at: Some(codex.at),
        });
    }
    groups
}

pub const LOW_MAX_ROWS: usize = 2;

/// The windows at or above their alert level, most used first, at most [`LOW_MAX_ROWS`], as
/// `(group key, window id)`.
pub fn low_windows(groups: &[LimitGroup], settings: &UsageSettings) -> HashSet<(String, String)> {
    let mut near: Vec<(&str, &LimitWindow)> = groups
        .iter()
        .flat_map(|g| g.windows.iter().map(move |t| (g.key.as_str(), &t.window)))
        .filter(|(_, w)| w.used_percent >= f64::from(limit_alert_level(w.kind, settings)))
        .collect();
    near.sort_by(|a, b| b.1.used_percent.total_cmp(&a.1.used_percent));
    near.into_iter()
        .take(LOW_MAX_ROWS)
        .map(|(g, w)| (g.to_string(), w.id.clone()))
        .collect()
}

/// Newest time any value was read, for the "Updated" text.
pub fn newest_fetch(state: &UsageState) -> i64 {
    all_timed_windows(state)
        .iter()
        .filter_map(|t| t.at)
        .fold(state.updated_at, i64::max)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub title: &'static str,
    pub hint: Option<&'static str>,
}

/// Why an account shows no (or only old) limits, `None` when it is fine.
pub fn profile_problem(p: &ClaudeProfileLimits) -> Option<Problem> {
    let plain = |title| Some(Problem { title, hint: None });
    match p.status {
        ProfileStatus::Ok => None,
        ProfileStatus::NotSignedIn => plain("Not signed in."),
        ProfileStatus::Expired => Some(Problem {
            title: "Sign-in expired",
            hint: Some("Start a Claude agent in this profile to refresh the sign-in."),
        }),
        ProfileStatus::KeychainDenied => {
            plain("Agent Studio could not read this sign-in from the Keychain.")
        }
        ProfileStatus::Unavailable => {
            plain("No plan limits for this sign-in. Token usage still shows below.")
        }
        ProfileStatus::RateLimited | ProfileStatus::Error if p.windows.is_empty() => {
            plain("Limits unavailable \u{2014} will retry shortly.")
        }
        ProfileStatus::RateLimited | ProfileStatus::Error => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LimitTile {
    pub key: String,
    pub provider: UsageProvider,
    /// `Claude · session (5h)`, `Claude alternative · weekly`, `Codex · weekly`.
    pub label: String,
    /// Set for a plan window.
    pub timed: Option<TimedWindow>,
    /// Set for an account that has a problem instead of (or besides) windows.
    pub problem: Option<Problem>,
    pub dimmed: bool,
    /// Codex values come from session logs, not from the provider.
    pub estimated: bool,
    pub at: Option<i64>,
}

fn window_name(w: &LimitWindow) -> String {
    match w.kind {
        LimitWindowKind::Session => "session (5h)".into(),
        LimitWindowKind::Weekly => "weekly".into(),
        LimitWindowKind::WeeklyModel => {
            let rest = w
                .label
                .strip_prefix("Weekly")
                .unwrap_or(&w.label)
                .trim_start();
            let rest = rest.strip_prefix('\u{b7}').unwrap_or(rest);
            format!("weekly {}", rest.trim()).trim().to_string()
        }
        LimitWindowKind::Other => w.label.clone(),
    }
}

/// Flat tiles for the Overview card: Claude accounts in order (session, weekly, ...), then Codex.
/// The account is named only when more than one Claude account is shown.
pub fn build_limit_tiles(state: &UsageState) -> Vec<LimitTile> {
    let groups = build_limit_groups(state);
    let claude_count = groups
        .iter()
        .filter(|g| g.provider == UsageProvider::Claude)
        .count();
    let mut tiles = Vec::new();
    for g in &groups {
        let source = match (&g.profile, g.provider) {
            (_, UsageProvider::Codex) => "Codex".to_string(),
            (Some(p), _) if claude_count > 1 => format!("Claude {}", p.account),
            _ => "Claude".to_string(),
        };
        let problem = g.profile.as_ref().and_then(profile_problem);
        let dimmed = g
            .profile
            .as_ref()
            .is_some_and(|p| p.status != ProfileStatus::Ok);
        if problem.is_some() {
            tiles.push(LimitTile {
                key: format!("{}:problem", g.key),
                provider: g.provider,
                label: g
                    .profile
                    .as_ref()
                    .map_or_else(|| source.clone(), |p| format!("Claude {}", p.account)),
                timed: None,
                problem,
                dimmed: false,
                estimated: false,
                at: None,
            });
        }
        for t in &g.windows {
            tiles.push(LimitTile {
                key: format!("{}:{}", g.key, t.window.id),
                provider: g.provider,
                label: format!("{source} \u{b7} {}", window_name(&t.window)),
                timed: Some(t.clone()),
                problem: None,
                dimmed,
                estimated: g.provider == UsageProvider::Codex,
                at: t.at,
            });
        }
    }
    tiles
}

#[cfg(test)]
pub mod fixtures {
    use as_core::usage::codex_line::CodexParsedLimits;
    use as_core::usage::types::{IndexingState, LimitWindowKind};

    use super::*;

    /// 2026-10-22 14:30 UTC (the Overview tests' clock).
    pub const NOW: i64 = 1_792_679_400_000;

    pub fn win(
        account: &str,
        kind: LimitWindowKind,
        used: f64,
        resets_at: Option<i64>,
    ) -> LimitWindow {
        let (id, label) = match kind {
            LimitWindowKind::Session => ("session", "Current session (5h)"),
            _ => ("weekly", "Weekly \u{b7} all models"),
        };
        LimitWindow {
            provider: UsageProvider::Claude,
            account: account.into(),
            id: id.into(),
            kind,
            label: label.into(),
            used_percent: used,
            resets_at,
        }
    }

    pub fn profile(account: &str) -> ClaudeProfileLimits {
        ClaudeProfileLimits {
            account: account.into(),
            config_dir: format!("/home/u/.claude-{account}"),
            plan: Some("Pro".into()),
            status: ProfileStatus::Ok,
            windows: vec![
                win(
                    account,
                    LimitWindowKind::Session,
                    29.0,
                    Some(NOW + 108 * 60_000),
                ),
                win(
                    account,
                    LimitWindowKind::Weekly,
                    4.0,
                    Some(NOW + 4 * 86_400_000),
                ),
            ],
            fetched_at: Some(NOW - 60_000),
            checked_at: NOW - 60_000,
        }
    }

    pub fn usage_state() -> UsageState {
        let mut alt = profile("alternative");
        alt.windows = vec![
            win(
                "alternative",
                LimitWindowKind::Session,
                20.0,
                Some(NOW + 3_600_000),
            ),
            win(
                "alternative",
                LimitWindowKind::Weekly,
                26.0,
                Some(NOW + 3 * 86_400_000),
            ),
        ];
        let mut work = profile("work");
        work.plan = None;
        work.status = ProfileStatus::Expired;
        work.windows = vec![];
        work.fetched_at = None;
        UsageState {
            claude: vec![profile("default"), alt, work],
            codex: None,
            indexing: IndexingState {
                indexing: false,
                progress: None,
                last_scan_at: Some(NOW - 60_000),
            },
            data_version: 1,
            updated_at: NOW - 60_000,
        }
    }

    pub fn codex(at: i64, used: f64) -> CodexParsedLimits {
        let mut w = win(
            "default",
            LimitWindowKind::Session,
            used,
            Some(NOW + 60_000),
        );
        w.provider = UsageProvider::Codex;
        CodexParsedLimits {
            at,
            account: "default".into(),
            plan: Some("Plus".into()),
            windows: vec![w],
        }
    }
}

#[cfg(test)]
mod tests {
    use as_core::usage::types::LimitWindowKind;

    use super::fixtures::*;
    use super::*;

    #[test]
    fn the_chip_shows_the_highest_session_and_weekly_across_accounts() {
        let w = chip_windows(Some(&usage_state()), UsageChipProvider::Auto);
        assert_eq!(w.session.unwrap().window.used_percent, 29.0);
        assert_eq!(w.weekly.unwrap().window.used_percent, 26.0);
    }

    #[test]
    fn no_windows_no_chip_windows() {
        let mut s = usage_state();
        s.claude = vec![{
            let mut p = profile("work");
            p.status = ProfileStatus::Expired;
            p.windows.clear();
            p.fetched_at = None;
            p
        }];
        assert_eq!(
            chip_windows(Some(&s), UsageChipProvider::Auto),
            ChipWindows::default()
        );
        assert_eq!(
            chip_windows(None, UsageChipProvider::Auto),
            ChipWindows::default()
        );
    }

    #[test]
    fn the_chip_provider_setting_picks_a_provider_and_falls_back_to_all() {
        let mut s = usage_state();
        s.codex = Some(codex(NOW - 60_000, 70.0));
        let codex_only = chip_windows(Some(&s), UsageChipProvider::Codex);
        assert_eq!(
            codex_only.session.unwrap().window.provider,
            UsageProvider::Codex
        );
        // Auto: the most used session is Codex's 70%.
        assert_eq!(
            chip_windows(Some(&s), UsageChipProvider::Auto)
                .session
                .unwrap()
                .window
                .used_percent,
            70.0
        );
        // Claude with no windows falls back to all.
        let mut only_codex = usage_state();
        only_codex.claude.clear();
        only_codex.codex = Some(codex(NOW, 10.0));
        let w = chip_windows(Some(&only_codex), UsageChipProvider::Claude);
        assert_eq!(w.session.unwrap().window.provider, UsageProvider::Codex);
    }

    #[test]
    fn staleness_is_15_minutes() {
        assert!(!is_stale(Some(NOW - 60_000), NOW));
        assert!(is_stale(Some(NOW - 20 * 60_000), NOW));
        assert!(is_stale(None, NOW));
    }

    #[test]
    fn groups_hide_not_signed_in_accounts_while_another_is_signed_in() {
        let mut s = usage_state();
        let mut old = profile("old");
        old.status = ProfileStatus::NotSignedIn;
        old.windows.clear();
        s.claude = vec![profile("default"), old];
        let g = build_limit_groups(&s);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].subtitle, "Pro");
    }

    #[test]
    fn groups_name_accounts_when_there_are_several_and_add_codex() {
        let mut s = usage_state();
        s.codex = Some(codex(NOW - 6 * 60_000, 10.0));
        let g = build_limit_groups(&s);
        let subs: Vec<_> = g.iter().map(|g| g.subtitle.as_str()).collect();
        assert_eq!(
            subs,
            [
                "Pro \u{b7} default",
                "Pro \u{b7} alternative",
                "work",
                "Plus"
            ]
        );
        assert_eq!(g[3].codex_at, Some(NOW - 6 * 60_000));
    }

    #[test]
    fn low_windows_are_the_ones_over_their_level_most_used_first_at_most_two() {
        let mut s = usage_state();
        s.claude = vec![{
            let mut p = profile("default");
            p.windows = vec![
                win(
                    "default",
                    LimitWindowKind::Session,
                    85.0,
                    Some(NOW + 600_000),
                ),
                win("default", LimitWindowKind::Weekly, 40.0, None),
            ];
            p
        }];
        let groups = build_limit_groups(&s);
        let low = low_windows(&groups, &UsageSettings::default());
        assert_eq!(
            low,
            HashSet::from([("claude:default".to_string(), "session".to_string())])
        );
        assert!(
            low_windows(
                &build_limit_groups(&usage_state()),
                &UsageSettings::default()
            )
            .is_empty()
        );
    }

    #[test]
    fn newest_fetch_is_the_latest_of_state_and_windows() {
        let s = usage_state();
        assert_eq!(newest_fetch(&s), NOW - 60_000);
    }

    #[test]
    fn problems_per_status() {
        let mk = |status, windows: bool| {
            let mut p = profile("a");
            p.status = status;
            if !windows {
                p.windows.clear();
            }
            profile_problem(&p)
        };
        assert!(mk(ProfileStatus::Ok, true).is_none());
        assert_eq!(
            mk(ProfileStatus::Expired, true).unwrap().title,
            "Sign-in expired"
        );
        assert!(
            mk(ProfileStatus::Expired, true)
                .unwrap()
                .hint
                .unwrap()
                .contains("Start a Claude agent")
        );
        assert!(
            mk(ProfileStatus::KeychainDenied, false)
                .unwrap()
                .title
                .contains("Keychain")
        );
        assert_eq!(
            mk(ProfileStatus::RateLimited, false).unwrap().title,
            "Limits unavailable \u{2014} will retry shortly."
        );
        assert!(mk(ProfileStatus::RateLimited, true).is_none());
        assert!(
            mk(ProfileStatus::Unavailable, false)
                .unwrap()
                .title
                .starts_with("No plan limits")
        );
        assert_eq!(
            mk(ProfileStatus::NotSignedIn, false).unwrap().title,
            "Not signed in."
        );
    }

    #[test]
    fn tiles_name_the_account_only_with_several_claude_accounts() {
        let tiles = build_limit_tiles(&usage_state());
        let labels: Vec<_> = tiles.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Claude default \u{b7} session (5h)",
                "Claude default \u{b7} weekly",
                "Claude alternative \u{b7} session (5h)",
                "Claude alternative \u{b7} weekly",
                "Claude work"
            ]
        );
        assert!(tiles[4].problem.is_some());
        let mut one = usage_state();
        one.claude.truncate(1);
        one.codex = Some(codex(NOW - 60_000, 10.0));
        let tiles = build_limit_tiles(&one);
        assert_eq!(tiles[0].label, "Claude \u{b7} session (5h)");
        let last = tiles.last().unwrap();
        assert_eq!(
            (last.label.as_str(), last.estimated),
            ("Codex \u{b7} session (5h)", true)
        );
    }

    #[test]
    fn an_expired_account_shows_its_last_values_dimmed() {
        let mut s = usage_state();
        s.claude = vec![{
            let mut p = profile("default");
            p.status = ProfileStatus::Expired;
            p
        }];
        let tiles = build_limit_tiles(&s);
        assert!(tiles[0].problem.is_some());
        assert!(tiles[1].dimmed);
    }
}
