//! UI state that survives restarts (Hydra keeps it in `localStorage`): last view, selected agent,
//! sidebar width/sort/filter, grid tab, expanded tile, tile order. Stored in `<root>/tui-state.json`.
//!
//! Loading is tolerant: a missing, corrupt or partly wrong file never fails, every field falls back
//! to its default on its own.

use std::collections::HashMap;
use std::path::Path;

use as_core::ViewMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sidebar::{SortKey, StatusChip};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TuiState {
    /// The view that was open when the TUI last ran.
    pub view: Option<ViewMode>,
    /// `defaultViewMode` of the config at that time: when the config changes it, the config wins.
    pub default_view: Option<ViewMode>,
    pub selected_agent: Option<String>,
    pub sidebar_width: Option<u16>,
    pub sidebar_sort: SortKey,
    pub sidebar_status: StatusChip,
    /// Grid tab: `__running__` or a project directory.
    pub grid_tab: Option<String>,
    pub expanded_tile: Option<String>,
    pub grid_only_running: bool,
    /// Project dir -> agent ids in the user's tile order.
    pub tile_order: HashMap<String, Vec<String>>,
    /// Version of the update banner the user dismissed.
    pub dismissed_update: Option<String>,
}

impl TuiState {
    /// Read the file; anything unreadable yields the defaults for that field.
    pub fn load(path: &Path) -> TuiState {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_json(&text),
            Err(_) => TuiState::default(),
        }
    }

    pub fn from_json(text: &str) -> TuiState {
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
            return TuiState::default();
        };
        let mut state = TuiState::default();
        macro_rules! field {
            ($key:literal, $slot:expr) => {
                if let Some(v) = map
                    .get($key)
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                {
                    $slot = v;
                }
            };
        }
        field!("view", state.view);
        field!("defaultView", state.default_view);
        field!("selectedAgent", state.selected_agent);
        field!("sidebarWidth", state.sidebar_width);
        field!("sidebarSort", state.sidebar_sort);
        field!("sidebarStatus", state.sidebar_status);
        field!("gridTab", state.grid_tab);
        field!("expandedTile", state.expanded_tile);
        field!("gridOnlyRunning", state.grid_only_running);
        field!("tileOrder", state.tile_order);
        field!("dismissedUpdate", state.dismissed_update);
        state
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        as_core::util::write_atomic(path, text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TuiState {
        TuiState {
            view: Some(ViewMode::Grid),
            default_view: Some(ViewMode::Chat),
            selected_agent: Some("a1".into()),
            sidebar_width: Some(34),
            sidebar_sort: SortKey::Recent,
            sidebar_status: StatusChip::Waiting,
            grid_tab: Some("/work/api".into()),
            expanded_tile: Some("a1".into()),
            grid_only_running: true,
            tile_order: HashMap::from([("/work/api".to_string(), vec!["b".into(), "a".into()])]),
            dismissed_update: Some("0.2.0".into()),
        }
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tui-state.json");
        let state = sample();
        state.save(&path).unwrap();
        assert_eq!(TuiState::load(&path), state);
    }

    #[test]
    fn missing_or_corrupt_files_give_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            TuiState::load(&dir.path().join("nope.json")),
            TuiState::default()
        );
        assert_eq!(TuiState::from_json("not json"), TuiState::default());
        assert_eq!(TuiState::from_json("[1,2]"), TuiState::default());
    }

    #[test]
    fn one_bad_field_does_not_reset_the_others() {
        let state = TuiState::from_json(
            r#"{"view":"neon","sidebarWidth":40,"sidebarSort":"name","gridTab":7,"tileOrder":{"/p":["a"]}}"#,
        );
        assert_eq!(state.view, None);
        assert_eq!(state.sidebar_width, Some(40));
        assert_eq!(state.sidebar_sort, SortKey::Name);
        assert_eq!(state.grid_tab, None);
        assert_eq!(state.tile_order["/p"], ["a"]);
    }
}
