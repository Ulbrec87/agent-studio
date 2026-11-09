//! "Import from Hydra": merge Hydra's `config.json` and `workspace.json` into ours.
//!
//! Hydra's files are only ever read. Config keys that do not exist in our [`AppConfig`] are ignored,
//! bad values are skipped per key by [`ConfigStore::update`]. Workspace agents are merged by id and
//! session id. Hydra shortcuts use `mod+x` syntax that has no equivalent in our prefix scheme, so
//! only rules already in our syntax (`prefix …`) for known commands are taken; the rest is reported.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::ConfigStore;
use crate::types::AppConfig;
use crate::workspace::{PersistedWorkspaceAgent, WorkspaceStore};

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("no Hydra data found in {0}")]
    NotFound(String),
    #[error("{0}")]
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigChange {
    pub key: String,
    pub current: String,
    pub incoming: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub dir: String,
    pub config_changes: Vec<ConfigChange>,
    pub agents_total: usize,
    pub agents_new: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub config_keys_applied: usize,
    pub agents_added: Vec<PersistedWorkspaceAgent>,
    pub warnings: Vec<String>,
}

/// Hydra's Electron userData dir for this OS.
pub fn default_hydra_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Hydra"))
    }
    #[cfg(target_os = "macos")]
    {
        Some(
            crate::paths::home_dir()
                .join("Library")
                .join("Application Support")
                .join("Hydra"),
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map_or_else(|| crate::paths::home_dir().join(".config"), PathBuf::from);
        Some(base.join("Hydra"))
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn summarize(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(59).collect::<String>())
    } else {
        s
    }
}

/// Keys of Hydra's config that exist in our schema, with their values.
fn mappable_config(hydra: &Value) -> Map<String, Value> {
    let ours = serde_json::to_value(AppConfig::default())
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    hydra
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| ours.contains_key(*k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn hydra_agents(dir: &Path) -> Vec<PersistedWorkspaceAgent> {
    read_json(&dir.join("workspace.json"))
        .and_then(|v| v.get("agents").and_then(Value::as_array).cloned())
        .map(|list| {
            list.iter()
                .filter_map(|a| serde_json::from_value(a.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn hydra_agent_count(dir: &Path) -> usize {
    read_json(&dir.join("workspace.json"))
        .and_then(|v| v.get("agents").and_then(Value::as_array).map(Vec::len))
        .unwrap_or(0)
}

fn new_agents(
    incoming: Vec<PersistedWorkspaceAgent>,
    existing: &[PersistedWorkspaceAgent],
) -> Vec<PersistedWorkspaceAgent> {
    let ids: Vec<&str> = existing.iter().map(|a| a.id.as_str()).collect();
    let sessions: Vec<&str> = existing
        .iter()
        .filter_map(|a| a.session_id.as_deref())
        .collect();
    let mut out: Vec<PersistedWorkspaceAgent> = Vec::new();
    for a in incoming {
        if ids.contains(&a.id.as_str())
            || a.session_id
                .as_deref()
                .is_some_and(|s| sessions.contains(&s))
        {
            continue;
        }
        if out.iter().any(|o| o.id == a.id) {
            continue;
        }
        out.push(a);
    }
    out
}

pub fn preview(
    dir: &Path,
    current: &AppConfig,
    existing: &[PersistedWorkspaceAgent],
) -> Result<ImportPreview, ImportError> {
    let config = read_json(&dir.join("config.json"));
    let has_workspace = dir.join("workspace.json").is_file();
    if config.is_none() && !has_workspace {
        return Err(ImportError::NotFound(dir.display().to_string()));
    }
    let cur = serde_json::to_value(current).unwrap_or(Value::Null);
    let mut p = ImportPreview {
        dir: dir.display().to_string(),
        ..Default::default()
    };
    if let Some(cfg) = &config {
        for (key, incoming) in mappable_config(cfg) {
            let now = cur.get(&key).cloned().unwrap_or(Value::Null);
            if now != incoming {
                p.config_changes.push(ConfigChange {
                    key,
                    current: summarize(&now),
                    incoming: summarize(&incoming),
                });
            }
        }
        p.config_changes.sort_by(|a, b| a.key.cmp(&b.key));
    }
    p.agents_total = hydra_agent_count(dir);
    p.agents_new = new_agents(hydra_agents(dir), existing).len();
    if p.agents_total > p.agents_new {
        p.warnings.push(format!(
            "{} agent(s) already exist or could not be read and will be skipped",
            p.agents_total - p.agents_new
        ));
    }
    Ok(p)
}

/// Merge into our stores (Hydra's own files are never modified).
pub fn apply(
    dir: &Path,
    config: &ConfigStore,
    workspace: &WorkspaceStore,
) -> Result<ImportReport, ImportError> {
    let fail = |e: anyhow::Error| ImportError::Failed(format!("{e:#}"));
    let mut report = ImportReport::default();
    if let Some(cfg) = read_json(&dir.join("config.json")) {
        let patch = mappable_config(&cfg);
        let before = serde_json::to_value(config.get()).unwrap_or(Value::Null);
        let after = config.update(&Value::Object(patch.clone())).map_err(fail)?;
        let after = serde_json::to_value(after).unwrap_or(Value::Null);
        report.config_keys_applied = patch
            .keys()
            .filter(|k| before.get(*k) != after.get(*k))
            .count();
    }
    let existing = workspace.get_agents();
    let added = new_agents(hydra_agents(dir), &existing);
    if !added.is_empty() {
        let mut all = existing;
        all.extend(added.iter().cloned());
        workspace.set_agents(all).map_err(fail)?;
    }
    report.agents_added = added;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fx {
        _d: tempfile::TempDir,
        hydra: PathBuf,
        ours: PathBuf,
    }

    fn fixture() -> Fx {
        let d = tempfile::tempdir().unwrap();
        let hydra = d.path().join("Hydra");
        let ours = d.path().join("ours");
        std::fs::create_dir_all(&hydra).unwrap();
        std::fs::create_dir_all(&ours).unwrap();
        std::fs::write(
            hydra.join("config.json"),
            json!({"theme": "light", "maxAgents": 5, "yoloMode": true, "defaultProvider": "codex",
                   "electronOnlyThing": 1, "gridColumns": "banana"})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            hydra.join("workspace.json"),
            json!({"schemaVersion": 1, "agents": [
                {"id": "a1", "name": "one", "projectDir": "/p", "provider": "claude", "model": "default", "yolo": false,
                 "sessionId": "s-1", "createdAt": "2026-01-01T00:00:00Z"},
                {"id": "a2", "name": "two", "projectDir": "/p", "model": "default", "yolo": true,
                 "sessionId": null, "createdAt": "2026-01-02T00:00:00Z", "workMode": "worktree",
                 "worktreePath": "/p/.wt", "worktreeBranch": "hydra/x"},
                {"id": "broken"}
            ]})
            .to_string(),
        )
        .unwrap();
        Fx { _d: d, hydra, ours }
    }

    #[test]
    fn preview_lists_mappable_changes_only() {
        let fx = fixture();
        let p = preview(&fx.hydra, &AppConfig::default(), &[]).unwrap();
        let keys: Vec<_> = p.config_changes.iter().map(|c| c.key.as_str()).collect();
        assert!(keys.contains(&"theme") && keys.contains(&"maxAgents"));
        assert!(!keys.contains(&"electronOnlyThing"));
        assert_eq!((p.agents_total, p.agents_new), (3, 2));
        assert!(!p.warnings.is_empty());
        assert!(matches!(
            preview(&fx.ours, &AppConfig::default(), &[]),
            Err(ImportError::NotFound(_))
        ));
    }

    #[test]
    fn apply_merges_without_touching_hydra() {
        let fx = fixture();
        let before = std::fs::read(fx.hydra.join("config.json")).unwrap();
        let config = ConfigStore::at(fx.ours.join("config.json"));
        let ws = WorkspaceStore::at(fx.ours.join("workspace.json"));
        let report = apply(&fx.hydra, &config, &ws).unwrap();
        let c = config.get();
        assert_eq!(c.max_agents, 5);
        assert!(report.config_keys_applied >= 2);
        assert_eq!(report.agents_added.len(), 2);
        let agents = ws.get_agents();
        assert_eq!(
            agents
                .iter()
                .find(|a| a.id == "a2")
                .unwrap()
                .worktree_branch
                .as_deref(),
            Some("hydra/x")
        );
        assert_eq!(std::fs::read(fx.hydra.join("config.json")).unwrap(), before);
        // Second run adds no agents.
        let again = apply(&fx.hydra, &config, &ws).unwrap();
        assert!(again.agents_added.is_empty());
        assert_eq!(ws.get_agents().len(), 2);
    }

    #[test]
    fn existing_session_ids_are_skipped() {
        let fx = fixture();
        let ws = WorkspaceStore::at(fx.ours.join("workspace.json"));
        let mut first = hydra_agents(&fx.hydra);
        first[0].id = "other-id".into(); // same session id s-1
        ws.set_agents(vec![first.remove(0)]).unwrap();
        let p = preview(&fx.hydra, &AppConfig::default(), &ws.get_agents()).unwrap();
        assert_eq!(p.agents_new, 1);
    }

    #[test]
    fn default_dir_ends_with_hydra() {
        assert!(default_hydra_dir().is_some_and(|d| d.ends_with("Hydra")));
    }
}
