//! Usage and limits: the report overlay's keys, what is fetched and when, and the visibility
//! reports. All of it is `impl App`.

use super::*;

use as_core::usage::report::UsageReportProvider;

use crate::usage::{TREND_WINDOWS, UsageOverlay};

impl App {
    /// What the screen shows of usage and therefore needs from the daemon: today's total (the
    /// header chip falls back to it), everything the overlay shows, the Overview card's period.
    fn usage_wants(&self) -> Vec<Want> {
        let mut wants = vec![Want::Summary(UsagePeriod::Today, String::new())];
        if let Some(Modal::Usage(overlay)) = &self.modal {
            for p in [UsagePeriod::Today, UsagePeriod::Week, UsagePeriod::Month] {
                wants.push(Want::Summary(p, String::new()));
            }
            wants.push(Want::Report(overlay.provider));
        }
        if self.view == ViewMode::Overview {
            let project = self
                .overview
                .effective_project()
                .unwrap_or_default()
                .to_string();
            wants.push(Want::Summary(self.usage.card_period.0, project));
        }
        wants
    }

    /// A usage surface is on screen: the overlay or the Overview.
    pub fn usage_visible(&self) -> bool {
        matches!(self.modal, Some(Modal::Usage(_))) || self.view == ViewMode::Overview
    }

    /// Fetch what is missing and tell the daemon about visibility changes. Cheap; called every
    /// frame (hash lookups only).
    pub fn usage_effects(&mut self) -> Vec<Effect> {
        let wants = self.usage_wants();
        let mut fx = self.usage.fetch_effects(&wants);
        if let Some(visible) = self.usage.visibility_change(self.usage_visible()) {
            fx.push(Effect::UsageVisible(visible));
        }
        fx
    }

    /// `prefix u`: the report overlay.
    pub(super) fn open_usage_report(&mut self) -> Vec<Effect> {
        if self.usage.state.is_none() && self.usage.error.is_some() {
            self.toast(
                ToastKind::Warn,
                "Usage",
                "Usage is not available: restart the daemon (agent-studio stop) after an update",
            );
            return Vec::new();
        }
        self.modal = Some(Modal::Usage(UsageOverlay::default()));
        self.dirty = true;
        Vec::new()
    }

    pub(super) fn refresh_usage(&mut self) -> Vec<Effect> {
        if self.usage.refreshing {
            return Vec::new();
        }
        self.usage.refreshing = true;
        self.dirty = true;
        vec![Effect::RefreshUsage]
    }

    /// Keys of the report overlay; it stays open until `Esc`/`q`/`u`.
    pub(super) fn usage_overlay_key(
        &mut self,
        mut overlay: UsageOverlay,
        key: KeyEvent,
    ) -> Vec<Effect> {
        let mut fx = Vec::new();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'u') => return Vec::new(),
            KeyCode::Down | KeyCode::Char('j') => overlay.scroll += 1,
            KeyCode::Up | KeyCode::Char('k') => overlay.scroll = overlay.scroll.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => overlay.scroll += 10,
            KeyCode::PageUp => overlay.scroll = overlay.scroll.saturating_sub(10),
            KeyCode::Home | KeyCode::Char('g') => overlay.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => overlay.scroll = usize::MAX / 2,
            KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => overlay.next_provider(1),
            KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => overlay.next_provider(-1),
            KeyCode::Char('1') => overlay.provider = UsageReportProvider::Claude,
            KeyCode::Char('2') => overlay.provider = UsageReportProvider::Codex,
            KeyCode::Char('3') => overlay.provider = UsageReportProvider::All,
            KeyCode::Char('d') => overlay.trend = (overlay.trend + 1) % TREND_WINDOWS.len(),
            KeyCode::Char('f') => overlay.low_only = !overlay.low_only,
            KeyCode::Char('r') => fx.extend(self.refresh_usage()),
            _ => {}
        }
        self.modal = Some(Modal::Usage(overlay));
        self.dirty = true;
        fx
    }
}
