//! `usage` block of the app config (Hydra `shared/usage/settings.ts`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum UsageChipProvider {
    /// The most used provider.
    #[default]
    Auto,
    Claude,
    Codex,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageSettings {
    /// Opt-in notifications when a plan window nears its limit.
    pub alerts_enabled: bool,
    /// Percent used at which a 5-hour session window alerts.
    pub session_alert_percent: u32,
    /// Percent used at which a weekly (or per-model weekly) window alerts.
    pub weekly_alert_percent: u32,
    /// Which provider the header chip reads.
    pub chip_provider: UsageChipProvider,
    /// Show the estimated API-equivalent cost.
    pub show_cost: bool,
}

impl Default for UsageSettings {
    fn default() -> Self {
        Self {
            alerts_enabled: false,
            session_alert_percent: 80,
            weekly_alert_percent: 90,
            chip_provider: UsageChipProvider::Auto,
            show_cost: true,
        }
    }
}

pub const MIN_ALERT_PERCENT: u32 = 50;
pub const MAX_ALERT_PERCENT: u32 = 100;

fn alert_percent(v: Option<&Value>, fallback: u32) -> u32 {
    match v.and_then(Value::as_f64) {
        Some(n) if n.is_finite() => n
            .round()
            .clamp(f64::from(MIN_ALERT_PERCENT), f64::from(MAX_ALERT_PERCENT))
            as u32,
        _ => fallback,
    }
}

/// Any stored value to a valid block: unknown keys dropped, bad values replaced
/// by the defaults, alert percents clamped to integers in 50-100.
pub fn normalize_usage_settings(raw: &Value) -> UsageSettings {
    let d = UsageSettings::default();
    let Some(o) = raw.as_object() else { return d };
    UsageSettings {
        alerts_enabled: o
            .get("alertsEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(d.alerts_enabled),
        session_alert_percent: alert_percent(o.get("sessionAlertPercent"), d.session_alert_percent),
        weekly_alert_percent: alert_percent(o.get("weeklyAlertPercent"), d.weekly_alert_percent),
        chip_provider: o
            .get("chipProvider")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(d.chip_provider),
        show_cost: o
            .get("showCost")
            .and_then(Value::as_bool)
            .unwrap_or(d.show_cost),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_alerts_off() {
        assert!(!UsageSettings::default().alerts_enabled);
        assert_eq!(
            normalize_usage_settings(&Value::Null),
            UsageSettings::default()
        );
        assert_eq!(
            normalize_usage_settings(&json!([1])),
            UsageSettings::default()
        );
    }

    #[test]
    fn repairs_invalid_values() {
        let s = normalize_usage_settings(
            &json!({ "alertsEnabled": true, "sessionAlertPercent": 5, "chipProvider": "nope", "weeklyAlertPercent": 99.6, "showCost": "x" }),
        );
        assert_eq!(
            s,
            UsageSettings {
                alerts_enabled: true,
                session_alert_percent: 50,
                weekly_alert_percent: 100,
                ..Default::default()
            }
        );
    }
}
