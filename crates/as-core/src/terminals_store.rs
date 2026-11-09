//! `terminals.json`: the free-terminal layout per project, so it survives a daemon restart.
//!
//! Only the *shape* is stored: per tab its split tree and active pane, per pane its label and
//! working directory. The shells themselves do not survive a restart; the manager spawns fresh
//! ones into the saved layout when the project's terminal panel is first used.
//!
//! Parsing is tolerant: an unreadable or corrupt file (kept as `terminals.json.corrupt`) means
//! "no saved layouts", a bad project or tab is skipped, a ratio out of range is clamped.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths::Paths;
use crate::terminal_tree::FreeTerminalNode;
use crate::util::write_atomic;

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct PersistedPane {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct PersistedGroup {
    pub id: String,
    #[serde(default)]
    pub active_pane_id: String,
    pub tree: FreeTerminalNode,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct PersistedProject {
    pub groups: Vec<PersistedGroup>,
    #[serde(default)]
    pub active_group_id: Option<String>,
    #[serde(default)]
    pub panes: Vec<PersistedPane>,
    /// Counter of the default `Terminal N` labels.
    #[serde(default)]
    pub next_label: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct PersistedTerminals {
    pub projects: BTreeMap<String, PersistedProject>,
}

impl PersistedProject {
    /// Repair what a hand edit or an older version may have left: duplicate panes, panes without
    /// an entry, a dangling active pane. `None` when no tab is left.
    fn tidy(mut self) -> Option<Self> {
        let mut seen = HashSet::new();
        let mut groups = Vec::new();
        for mut g in std::mem::take(&mut self.groups) {
            let tree = g.tree.clone().sanitized().retain_unique(&mut seen);
            let Some(tree) = tree else { continue };
            let leaves = tree.leaves();
            if !leaves.contains(&g.active_pane_id.as_str()) {
                g.active_pane_id = leaves.last().map(|s| (*s).to_string()).unwrap_or_default();
            }
            g.tree = tree;
            groups.push(g);
        }
        if groups.is_empty() {
            return None;
        }
        for g in &groups {
            for leaf in g.tree.leaves() {
                if !self.panes.iter().any(|p| p.id == leaf) {
                    self.panes.push(PersistedPane {
                        id: leaf.to_string(),
                        label: "Terminal".into(),
                        cwd: String::new(),
                    });
                }
            }
        }
        if self
            .active_group_id
            .as_ref()
            .is_none_or(|a| !groups.iter().any(|g| &g.id == a))
        {
            self.active_group_id = groups.first().map(|g| g.id.clone());
        }
        self.groups = groups;
        Some(self)
    }
}

pub struct TerminalStore {
    path: PathBuf,
}

impl TerminalStore {
    pub fn open(paths: &Paths) -> Self {
        Self::at(paths.terminals_file())
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The saved layouts; empty when the file is missing, unreadable or corrupt.
    pub fn load(&self) -> PersistedTerminals {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return PersistedTerminals::default();
            }
            Err(err) => {
                tracing::warn!(%err, "cannot read the terminals file");
                return PersistedTerminals::default();
            }
        };
        let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
            tracing::warn!("corrupted terminals file, starting empty");
            let _ = std::fs::copy(&self.path, self.path.with_extension("json.corrupt"));
            return PersistedTerminals::default();
        };
        let mut out = PersistedTerminals::default();
        let Some(projects) = parsed.get("projects").and_then(Value::as_object) else {
            return out;
        };
        for (dir, value) in projects {
            // Tabs are parsed one by one so that a single bad one does not drop the project.
            let mut project = PersistedProject {
                active_group_id: value
                    .get("activeGroupId")
                    .and_then(Value::as_str)
                    .map(String::from),
                next_label: value
                    .get("nextLabel")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(0),
                ..Default::default()
            };
            let list = |key: &str| {
                value
                    .get(key)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            };
            project.groups = list("groups")
                .into_iter()
                .filter_map(|g| serde_json::from_value(g).ok())
                .collect();
            project.panes = list("panes")
                .into_iter()
                .filter_map(|p| serde_json::from_value(p).ok())
                .collect();
            if let Some(project) = project.tidy() {
                out.projects.insert(dir.clone(), project);
            }
        }
        out
    }

    /// Write the layouts atomically. Projects without tabs are left out.
    pub fn save(&self, data: &PersistedTerminals) -> Result<()> {
        let mut data = data.clone();
        data.projects.retain(|_, p| !p.groups.is_empty());
        let file = serde_json::json!({
            "schemaVersion": SCHEMA_VERSION,
            "projects": data.projects,
        });
        write_atomic(&self.path, &serde_json::to_vec_pretty(&file)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_tree::SplitDir;

    fn project() -> PersistedProject {
        let mut tree = FreeTerminalNode::leaf("a");
        tree.split_leaf("a", "b", SplitDir::Horizontal);
        tree.split_leaf("b", "c", SplitDir::Vertical);
        PersistedProject {
            groups: vec![PersistedGroup {
                id: "grp-1".into(),
                active_pane_id: "c".into(),
                tree,
            }],
            active_group_id: Some("grp-1".into()),
            panes: ["a", "b", "c"]
                .iter()
                .map(|id| PersistedPane {
                    id: (*id).into(),
                    label: format!("T {id}"),
                    cwd: std::env::temp_dir().to_string_lossy().into_owned(),
                })
                .collect(),
            next_label: 4,
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = TerminalStore::at(dir.path().join("terminals.json"));
        assert_eq!(store.load(), PersistedTerminals::default(), "missing file");
        let mut data = PersistedTerminals::default();
        data.projects.insert("/work/shop".into(), project());
        data.projects
            .insert("/empty".into(), PersistedProject::default());
        store.save(&data).unwrap();
        let back = store.load();
        assert_eq!(
            back.projects.len(),
            1,
            "projects without tabs are not saved"
        );
        assert_eq!(back.projects["/work/shop"], project());
    }

    #[test]
    fn corrupt_file_is_kept_aside_and_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("terminals.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = TerminalStore::at(&path);
        assert_eq!(store.load(), PersistedTerminals::default());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("terminals.json.corrupt")).unwrap(),
            "{ not json"
        );
        // And saving afterwards works.
        store.save(&PersistedTerminals::default()).unwrap();
    }

    #[test]
    fn bad_parts_are_skipped_and_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("terminals.json");
        let raw = serde_json::json!({
            "projects": {
                "/p": {
                    "groups": [
                        {"id": "g1", "activePaneId": "gone", "tree": {"type": "split", "dir": "horizontal", "ratio": 0,
                            "a": {"type": "leaf", "paneId": "a"}, "b": {"type": "leaf", "paneId": "a"}}},
                        {"id": "bad", "tree": {"type": "banana"}},
                        {"id": "g2", "tree": {"type": "leaf", "paneId": "a"}},
                        7
                    ],
                    "activeGroupId": "missing",
                    "panes": [{"id": "a", "label": "build", "cwd": "/x"}, "junk"]
                },
                "/none": {"groups": []},
                "/worse": 5
            }
        });
        std::fs::write(&path, raw.to_string()).unwrap();
        let loaded = TerminalStore::at(&path).load();
        assert_eq!(loaded.projects.keys().collect::<Vec<_>>(), ["/p"]);
        let p = &loaded.projects["/p"];
        // `a` appeared twice in g1 and once in g2: it keeps its first place only, g2 empties out.
        assert_eq!(p.groups.len(), 1);
        assert_eq!(p.groups[0].tree, FreeTerminalNode::leaf("a"));
        assert_eq!(
            p.groups[0].active_pane_id, "a",
            "a dangling active pane is repaired"
        );
        assert_eq!(p.active_group_id.as_deref(), Some("g1"));
        assert_eq!(
            p.panes,
            vec![PersistedPane {
                id: "a".into(),
                label: "build".into(),
                cwd: "/x".into()
            }]
        );
    }

    #[test]
    fn panes_without_an_entry_get_a_default_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("terminals.json");
        std::fs::write(
            &path,
            r#"{"projects":{"/p":{"groups":[{"id":"g","tree":{"type":"leaf","paneId":"x"}}]}}}"#,
        )
        .unwrap();
        let p = &TerminalStore::at(&path).load().projects["/p"];
        assert_eq!(p.panes[0].id, "x");
        assert_eq!(p.panes[0].label, "Terminal");
    }
}
