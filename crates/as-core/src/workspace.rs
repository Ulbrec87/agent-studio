//! Persisted agent set (Hydra `electron/workspace/WorkspaceStore.ts`).
//!
//! Same JSON shape as Hydra: `{ "schemaVersion": 1, "agents": [...] }`, stored in
//! our own file (`Paths::workspace_file`).

use std::path::{Path, PathBuf};

use anyhow::Result;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths::Paths;
use crate::types::{ModelId, ProviderId, WorkMode};
use crate::util::write_atomic;

/// One agent as stored in `workspace.json`. Required keys: `id`, `name`,
/// `projectDir`, `model`, `yolo`, `createdAt` (entries missing one are dropped).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct PersistedWorkspaceAgent {
    pub id: String,
    pub name: String,
    pub project_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderId>,
    pub model: ModelId,
    pub yolo: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_manager: Option<bool>,
    #[serde(default)]
    pub session_id: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_mode: Option<WorkMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    /// Explicit Claude profile chosen when the agent was created (`""` = default profile).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_config_dir: Option<String>,
}

#[derive(Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
struct WorkspaceData<'a> {
    schema_version: u32,
    agents: &'a [PersistedWorkspaceAgent],
}

pub struct WorkspaceStore {
    path: PathBuf,
    cache: Mutex<Vec<PersistedWorkspaceAgent>>,
}

impl WorkspaceStore {
    pub fn open(paths: &Paths) -> Self {
        Self::at(paths.workspace_file())
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let agents = Self::load(&path);
        Self {
            path,
            cache: Mutex::new(agents),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get_agents(&self) -> Vec<PersistedWorkspaceAgent> {
        self.cache.lock().clone()
    }

    /// Replace the persisted set and write it to disk.
    pub fn set_agents(
        &self,
        agents: Vec<PersistedWorkspaceAgent>,
    ) -> Result<Vec<PersistedWorkspaceAgent>> {
        let mut cache = self.cache.lock();
        write_data(&self.path, &agents)?;
        *cache = agents;
        Ok(cache.clone())
    }

    fn load(path: &Path) -> Vec<PersistedWorkspaceAgent> {
        match std::fs::read_to_string(path) {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(parsed) => {
                    return parsed
                        .get("agents")
                        .and_then(Value::as_array)
                        .map(|list| {
                            list.iter()
                                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                                .collect()
                        })
                        .unwrap_or_default();
                }
                Err(err) => {
                    tracing::warn!(%err, "corrupted workspace file, starting empty");
                    let _ = std::fs::copy(path, path.with_extension("json.corrupt"));
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!(%err, "cannot read workspace file"),
        }
        if let Err(err) = write_data(path, &[]) {
            tracing::error!(%err, "failed to create workspace file");
        }
        Vec::new()
    }
}

fn write_data(path: &Path, agents: &[PersistedWorkspaceAgent]) -> Result<()> {
    let data = WorkspaceData {
        schema_version: 1,
        agents,
    };
    write_atomic(path, &serde_json::to_vec_pretty(&data)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn agent(id: &str) -> PersistedWorkspaceAgent {
        PersistedWorkspaceAgent {
            id: id.into(),
            name: "n".into(),
            project_dir: "/p".into(),
            provider: Some(ProviderId::Codex),
            model: "gpt-5.4".into(),
            yolo: true,
            is_manager: None,
            session_id: None,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            last_activity_at: None,
            ended_at: Some("2026-01-02T00:00:00.000Z".into()),
            work_mode: Some(WorkMode::Local),
            worktree_path: None,
            worktree_branch: None,
            claude_config_dir: None,
        }
    }

    #[test]
    fn creates_an_empty_file_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let store = WorkspaceStore::at(&path);
        assert!(store.get_agents().is_empty());
        assert!(path.exists());

        store.set_agents(vec![agent("a1"), agent("a2")]).unwrap();
        let reopened = WorkspaceStore::at(&path);
        assert_eq!(reopened.get_agents(), vec![agent("a1"), agent("a2")]);

        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["schemaVersion"], json!(1));
        assert_eq!(raw["agents"][0]["projectDir"], json!("/p"));
        assert_eq!(raw["agents"][0]["sessionId"], Value::Null);
    }

    #[test]
    fn drops_invalid_entries_and_reads_legacy_ones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let raw = json!({
            "schemaVersion": 1,
            "agents": [
                { "id": "ok", "name": "n", "projectDir": "/p", "model": "sonnet", "yolo": false, "sessionId": null, "createdAt": "t" },
                { "id": "no-model", "name": "n", "projectDir": "/p", "yolo": false, "sessionId": null, "createdAt": "t" },
                { "id": "bad-provider", "name": "n", "projectDir": "/p", "model": "m", "provider": "gemini", "yolo": false, "sessionId": null, "createdAt": "t" },
                "garbage"
            ]
        });
        std::fs::write(&path, raw.to_string()).unwrap();
        let agents = WorkspaceStore::at(&path).get_agents();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "ok");
        assert_eq!(agents[0].provider, None);
    }

    #[test]
    fn corrupted_file_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        std::fs::write(&path, "oops").unwrap();
        assert!(WorkspaceStore::at(&path).get_agents().is_empty());
    }
}
