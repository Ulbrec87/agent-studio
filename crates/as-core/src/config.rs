//! Config store (Hydra `electron/config/ConfigStore.ts`).
//!
//! Loads `config.json`, fills defaults, validates/clamps, saves atomically and
//! notifies subscribers through a `tokio::sync::watch` channel. Hot reload from
//! disk is [`ConfigStore::reload_if_changed`], which the daemon polls (~500 ms
//! like Hydra's `watchFile`).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use parking_lot::Mutex;
use serde_json::{Map, Value};
use tokio::sync::watch;

use crate::paths::Paths;
use crate::types::{
    AppConfig, DEFAULT_TERMINAL_FONT_FAMILY, MAX_CONCURRENT_AGENTS_HARD_LIMIT,
    TERMINAL_FONT_SIZE_RANGE,
};
use crate::usage_settings::normalize_usage_settings;
use crate::util::write_atomic;

pub const MIN_REMOTE_SESSION_TIMEOUT_MINUTES: u32 = 30;
pub const MAX_REMOTE_SESSION_TIMEOUT_MINUTES: u32 = 1440;

struct State {
    config: AppConfig,
    /// `(mtime, len)` of the file as we last wrote or read it.
    stamp: Option<(SystemTime, u64)>,
}

pub struct ConfigStore {
    path: PathBuf,
    state: Mutex<State>,
    tx: watch::Sender<AppConfig>,
}

fn stamp_of(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Clamp/repair values that cannot be expressed by the types alone.
fn sanitize(mut config: AppConfig) -> AppConfig {
    config.max_agents = config.max_agents.clamp(1, MAX_CONCURRENT_AGENTS_HARD_LIMIT);
    config.remote_session_timeout_minutes = config.remote_session_timeout_minutes.clamp(
        MIN_REMOTE_SESSION_TIMEOUT_MINUTES,
        MAX_REMOTE_SESSION_TIMEOUT_MINUTES,
    );
    config.free_terminal_max_count = config.free_terminal_max_count.max(1);
    config.free_terminal_idle_timeout_minutes = config.free_terminal_idle_timeout_minutes.max(1);
    config.terminal_font_size = config.terminal_font_size.clamp(
        *TERMINAL_FONT_SIZE_RANGE.start(),
        *TERMINAL_FONT_SIZE_RANGE.end(),
    );
    if config.terminal_font_family.trim().is_empty() {
        config.terminal_font_family = DEFAULT_TERMINAL_FONT_FAMILY.to_string();
    }
    config
}

/// Apply a JSON patch on top of `base`, field by field: a field whose value
/// does not fit its type is ignored (the rest of the patch still applies).
/// `usage` merges key by key and is normalized.
fn apply_patch(base: &AppConfig, patch: &Value) -> AppConfig {
    let Some(patch) = patch.as_object() else {
        return sanitize(base.clone());
    };
    let Ok(Value::Object(mut merged)) = serde_json::to_value(base) else {
        return sanitize(base.clone());
    };
    for (key, value) in patch {
        if key == "usage" {
            let mut usage: Map<String, Value> = merged
                .get("usage")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            if let Some(p) = value.as_object() {
                usage.extend(p.iter().map(|(k, v)| (k.clone(), v.clone())));
            } else {
                usage.clear();
            }
            let normalized = normalize_usage_settings(&Value::Object(usage));
            if let Ok(v) = serde_json::to_value(normalized) {
                merged.insert("usage".into(), v);
            }
            continue;
        }
        let mut candidate = merged.clone();
        candidate.insert(key.clone(), value.clone());
        if serde_json::from_value::<AppConfig>(Value::Object(candidate.clone())).is_ok() {
            merged = candidate;
        } else {
            tracing::warn!(key = %key, "ignoring config value with an invalid type");
        }
    }
    let config = serde_json::from_value(Value::Object(merged)).unwrap_or_else(|_| base.clone());
    sanitize(config)
}

fn serialize(config: &AppConfig) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(config)?)
}

impl ConfigStore {
    /// Open the store at `<root>/config.json` of `paths`.
    pub fn open(paths: &Paths) -> Self {
        Self::at(paths.config_file())
    }

    /// Open the store at an explicit file path (parent dir is created on save).
    pub fn at(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let config = Self::load(&path);
        let stamp = stamp_of(&path);
        let (tx, _) = watch::channel(config.clone());
        Self {
            path,
            state: Mutex::new(State { config, stamp }),
            tx,
        }
    }

    fn load(path: &Path) -> AppConfig {
        match std::fs::read_to_string(path) {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(parsed @ Value::Object(_)) => {
                    return apply_patch(&AppConfig::default(), &parsed);
                }
                _ => {
                    // Corrupted config: keep a copy, reset to defaults (like Hydra).
                    tracing::warn!(path = %path.display(), "corrupted config, resetting to defaults");
                    let _ = std::fs::copy(path, path.with_extension("json.corrupt"));
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!(path = %path.display(), %err, "cannot read config"),
        }
        let defaults = AppConfig::default();
        if let Err(err) = serialize(&defaults).and_then(|b| write_atomic(path, &b)) {
            tracing::error!(%err, "failed to save default config");
        }
        defaults
    }

    pub fn get(&self) -> AppConfig {
        self.state.lock().config.clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Subscribe to config changes (from `update` or a disk reload).
    pub fn subscribe(&self) -> watch::Receiver<AppConfig> {
        self.tx.subscribe()
    }

    /// Merge a JSON patch (camelCase keys, partial `usage`), validate, save and notify.
    pub fn update(&self, patch: &Value) -> Result<AppConfig> {
        let mut state = self.state.lock();
        let next = apply_patch(&state.config, patch);
        write_atomic(&self.path, &serialize(&next)?)?;
        state.stamp = stamp_of(&self.path);
        let changed = next != state.config;
        state.config = next.clone();
        drop(state);
        if changed {
            self.tx.send_replace(next.clone());
        }
        Ok(next)
    }

    /// Edit the config in place under the store's lock (so two clients changing different parts of
    /// a map, e.g. `claudeProfileByProject`, cannot overwrite each other), then validate, save and notify.
    pub fn modify(&self, edit: impl FnOnce(&mut AppConfig)) -> Result<AppConfig> {
        let mut state = self.state.lock();
        let mut next = state.config.clone();
        edit(&mut next);
        let next = sanitize(next);
        write_atomic(&self.path, &serialize(&next)?)?;
        state.stamp = stamp_of(&self.path);
        let changed = next != state.config;
        state.config = next.clone();
        drop(state);
        if changed {
            self.tx.send_replace(next.clone());
        }
        Ok(next)
    }

    /// Re-read the file when its mtime/size changed; notifies subscribers when
    /// the effective config differs. Returns whether a change was applied.
    pub fn reload_if_changed(&self) -> bool {
        let stamp = stamp_of(&self.path);
        let mut state = self.state.lock();
        if stamp == state.stamp {
            return false;
        }
        state.stamp = stamp;
        let Ok(raw) = std::fs::read_to_string(&self.path) else {
            return false;
        };
        let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(&raw) else {
            return false; // half-written or invalid: keep the current config
        };
        let next = apply_patch(&AppConfig::default(), &parsed);
        if next == state.config {
            return false;
        }
        state.config = next.clone();
        drop(state);
        self.tx.send_replace(next);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_settings::UsageSettings;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn store_in(dir: &Path) -> ConfigStore {
        ConfigStore::at(dir.join("config.json"))
    }

    #[test]
    fn modify_edits_under_the_lock_saves_and_notifies() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        let rx = store.subscribe();
        let next = store
            .modify(|c| {
                c.claude_profile_by_project.insert("/a".into(), "/p".into());
                c.hidden_session_ids.push("s1".into());
            })
            .unwrap();
        assert_eq!(next.claude_profile_by_project["/a"], "/p");
        assert!(rx.has_changed().unwrap());
        let reopened = store_in(dir.path()).get();
        assert_eq!(reopened.hidden_session_ids, ["s1"]);
        assert_eq!(reopened.claude_profile_by_project["/a"], "/p");
    }

    #[test]
    fn update_check_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        let c = store.get();
        assert!(c.check_for_updates, "update checks are on by default");
        let next = store.update(&json!({ "checkForUpdates": false })).unwrap();
        assert!(!next.check_for_updates);
        let reopened = store_in(dir.path()).get();
        assert!(!reopened.check_for_updates);
        // A value of the wrong type is ignored, the rest still applies.
        let next = store.update(&json!({ "checkForUpdates": "no" })).unwrap();
        assert!(!next.check_for_updates);
        let v = serde_json::to_value(&next).unwrap();
        assert_eq!(v["checkForUpdates"], json!(false));
    }

    #[test]
    fn defaults_the_usage_block_alerts_off() {
        let dir = tempfile::tempdir().unwrap();
        let c = store_in(dir.path()).get();
        assert_eq!(c.usage, UsageSettings::default());
        assert!(!c.usage.alerts_enabled);
        assert!(
            dir.path().join("config.json").exists(),
            "defaults are saved"
        );
    }

    #[test]
    fn reads_an_old_config_file_without_usage_block() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            json!({ "maxAgents": 3 }).to_string(),
        )
        .unwrap();
        let c = store_in(dir.path()).get();
        assert_eq!(c.max_agents, 3);
        assert_eq!(c.usage, UsageSettings::default());
    }

    #[test]
    fn repairs_invalid_stored_usage_values() {
        let dir = tempfile::tempdir().unwrap();
        let raw = json!({ "usage": { "alertsEnabled": true, "sessionAlertPercent": 5, "chipProvider": "nope" } });
        std::fs::write(dir.path().join("config.json"), raw.to_string()).unwrap();
        assert_eq!(
            store_in(dir.path()).get().usage,
            UsageSettings {
                alerts_enabled: true,
                session_alert_percent: 50,
                ..Default::default()
            }
        );
    }

    #[test]
    fn merges_a_partial_usage_patch_and_validates_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        store
            .update(&json!({ "usage": { "alertsEnabled": true } }))
            .unwrap();
        store
            .update(&json!({ "usage": { "weeklyAlertPercent": 500 } }))
            .unwrap();
        assert_eq!(
            store.get().usage,
            UsageSettings {
                alerts_enabled: true,
                weekly_alert_percent: 100,
                ..Default::default()
            }
        );
    }

    #[test]
    fn terminal_rendering_keys_have_hydra_defaults_and_are_validated() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        let c = store.get();
        assert_eq!(c.terminal_font_size, 12);
        assert_eq!(
            c.terminal_font_family,
            "\"SF Mono\", \"Menlo\", \"Monaco\", monospace"
        );
        assert!(!c.terminal_enable_webgl);
        assert_eq!(
            store
                .update(&json!({ "terminalFontSize": 99 }))
                .unwrap()
                .terminal_font_size,
            32
        );
        assert_eq!(
            store
                .update(&json!({ "terminalFontSize": 2 }))
                .unwrap()
                .terminal_font_size,
            8
        );
        assert_eq!(
            store
                .update(&json!({ "terminalFontSize": 15 }))
                .unwrap()
                .terminal_font_size,
            15
        );
        // A wrong type is ignored, an empty family falls back to the default.
        assert_eq!(
            store
                .update(&json!({ "terminalFontSize": "big" }))
                .unwrap()
                .terminal_font_size,
            15
        );
        assert_eq!(
            store
                .update(&json!({ "terminalFontFamily": "  " }))
                .unwrap()
                .terminal_font_family,
            "\"SF Mono\", \"Menlo\", \"Monaco\", monospace"
        );
        let c = store
            .update(&json!({ "terminalFontFamily": "Fira Code", "terminalEnableWebgl": true }))
            .unwrap();
        assert_eq!(
            (c.terminal_font_family.as_str(), c.terminal_enable_webgl),
            ("Fira Code", true)
        );
        // Persisted in config.json under the camelCase keys.
        let text = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
        assert!(
            text.contains("\"terminalFontFamily\": \"Fira Code\""),
            "{text}"
        );
    }

    #[test]
    fn clamps_max_agents_and_remote_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        assert_eq!(
            store
                .update(&json!({ "maxAgents": 99 }))
                .unwrap()
                .max_agents,
            10
        );
        assert_eq!(
            store.update(&json!({ "maxAgents": 0 })).unwrap().max_agents,
            1
        );
        assert_eq!(
            store
                .update(&json!({ "remoteSessionTimeoutMinutes": 5 }))
                .unwrap()
                .remote_session_timeout_minutes,
            30
        );
        assert_eq!(
            store
                .update(&json!({ "remoteSessionTimeoutMinutes": 99999 }))
                .unwrap()
                .remote_session_timeout_minutes,
            1440
        );
    }

    #[test]
    fn ignores_values_of_the_wrong_type_but_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        let c = store
            .update(&json!({ "theme": "neon", "maxAgents": 4, "gridColumns": 3 }))
            .unwrap();
        assert_eq!(c.theme, crate::types::ThemeId::Midnight);
        assert_eq!(c.max_agents, 4);
        assert_eq!(c.grid_columns, crate::types::GridColumns::Three);
    }

    #[test]
    fn update_persists_and_notifies() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        let mut rx = store.subscribe();
        store.update(&json!({ "theme": "dark" })).unwrap();
        assert!(rx.has_changed().unwrap());
        assert_eq!(rx.borrow_and_update().theme, crate::types::ThemeId::Dark);
        assert_eq!(
            store_in(dir.path()).get().theme,
            crate::types::ThemeId::Dark
        );
    }

    #[test]
    fn corrupted_file_resets_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "{ not json").unwrap();
        assert_eq!(store_in(dir.path()).get(), AppConfig::default());
        assert!(dir.path().join("config.json.corrupt").exists());
    }

    #[test]
    fn reload_if_changed_picks_up_disk_edits() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        let rx = store.subscribe();
        assert!(!store.reload_if_changed());

        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            json!({ "theme": "light", "maxAgents": 2 }).to_string(),
        )
        .unwrap();
        // Force a distinct mtime: filesystem timestamps can be coarse.
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_modified(SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        drop(f);

        assert!(store.reload_if_changed());
        assert!(rx.has_changed().unwrap());
        assert_eq!(store.get().theme, crate::types::ThemeId::Light);
        assert_eq!(store.get().max_agents, 2);
        assert!(!store.reload_if_changed());
    }

    #[test]
    fn our_own_save_is_not_reported_as_a_disk_change() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        store.update(&json!({ "theme": "dark" })).unwrap();
        assert!(!store.reload_if_changed());
    }
}
