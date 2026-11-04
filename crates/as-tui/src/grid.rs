//! Grid view model: project tabs, tile order and tile layout. Pure functions, no rendering
//! (Hydra `GridView.tsx`, `useTileOrder.ts`, `GridView.module.css`).

use std::collections::{HashMap, HashSet};

use as_core::{AgentState, AgentStatus, GridColumns};
use ratatui::layout::Rect;

use crate::sidebar::timestamp_ms;

/// Id of the meta tab that lists the running agents of every project (Hydra `RUNNING_PROJECT_ID`).
pub const RUNNING_TAB: &str = "__running__";

/// Narrowest tile in `auto` mode (Hydra: `minmax(480px, 1fr)`, about 60 cells; we are tighter).
pub const MIN_TILE_W: u16 = 40;
/// Narrowest tile for a fixed 2/3 column layout; below it the grid uses fewer columns.
pub const MIN_FIXED_TILE_W: u16 = 30;
/// Shortest tile (border, header and a few lines of output).
pub const MIN_TILE_H: u16 = 8;

pub fn is_active(status: AgentStatus) -> bool {
    matches!(status, AgentStatus::Running | AgentStatus::Starting)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    /// [`RUNNING_TAB`] or the project directory.
    pub key: String,
    pub label: String,
    pub count: usize,
}

/// Last path component of a directory, for tab labels.
pub fn basename(dir: &str) -> String {
    let trimmed = dir.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return dir.to_string();
    }
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

/// Hydra's `recentGroups`: with `max_age_days > 0` an agent that is not active must have been
/// created within that many days.
pub fn recent_enough(agent: &AgentState, max_age_days: u32, now_ms: i64) -> bool {
    if max_age_days == 0 || is_active(agent.status) {
        return true;
    }
    let cutoff = now_ms - i64::from(max_age_days) * 24 * 60 * 60 * 1000;
    timestamp_ms(&agent.created_at) > cutoff
}

/// Agents (indices) that appear in the grid at all.
fn visible(agents: &[AgentState], max_age_days: u32, now_ms: i64) -> Vec<usize> {
    agents
        .iter()
        .enumerate()
        .filter(|(_, a)| recent_enough(a, max_age_days, now_ms))
        .map(|(i, _)| i)
        .collect()
}

/// Tabs in display order: `Running` first, then the projects by recent activity.
pub fn tabs(agents: &[AgentState], only_running: bool, max_age_days: u32, now_ms: i64) -> Vec<Tab> {
    let vis = visible(agents, max_age_days, now_ms);
    let mut projects: Vec<(String, Vec<usize>)> = Vec::new();
    for i in vis {
        let dir = &agents[i].project_dir;
        match projects.iter_mut().find(|(d, _)| d == dir) {
            Some((_, v)) => v.push(i),
            None => projects.push((dir.clone(), vec![i])),
        }
    }
    let activity = |members: &[usize]| {
        members
            .iter()
            .map(|i| {
                timestamp_ms(&agents[*i].last_activity_at).max(timestamp_ms(&agents[*i].created_at))
            })
            .max()
            .unwrap_or(0)
    };
    projects.sort_by(|(da, a), (db, b)| activity(b).cmp(&activity(a)).then_with(|| da.cmp(db)));
    let running = agents
        .iter()
        .filter(|a| is_active(a.status) && recent_enough(a, max_age_days, now_ms))
        .count();
    let mut out = vec![Tab {
        key: RUNNING_TAB.to_string(),
        label: "Running".to_string(),
        count: running,
    }];
    for (dir, members) in projects {
        let count = if only_running {
            members
                .iter()
                .filter(|i| is_active(agents[**i].status))
                .count()
        } else {
            members.len()
        };
        out.push(Tab {
            label: basename(&dir),
            key: dir,
            count,
        });
    }
    out
}

/// Agents of a tab (indices), before the user's tile order is applied. The Running tab lists the
/// active agents of every project, most recently started first.
pub fn tab_agents(
    agents: &[AgentState],
    tab: &str,
    only_running: bool,
    max_age_days: u32,
    now_ms: i64,
) -> Vec<usize> {
    let vis = visible(agents, max_age_days, now_ms);
    if tab == RUNNING_TAB {
        let mut list: Vec<usize> = vis
            .into_iter()
            .filter(|i| is_active(agents[*i].status))
            .collect();
        let started = |i: usize| {
            let a = &agents[i];
            let text = a.started_at.as_deref().unwrap_or(&a.created_at);
            timestamp_ms(text)
        };
        list.sort_by_key(|i| std::cmp::Reverse(started(*i)));
        return list;
    }
    let mut list: Vec<usize> = vis
        .into_iter()
        .filter(|i| agents[*i].project_dir == tab)
        .collect();
    if only_running {
        list.retain(|i| is_active(agents[*i].status));
    }
    // Stable base order: creation time (the order of the sidebar).
    list.sort_by(|a, b| {
        timestamp_ms(&agents[*a].created_at)
            .cmp(&timestamp_ms(&agents[*b].created_at))
            .then_with(|| agents[*a].name.cmp(&agents[*b].name))
    });
    list
}

// ── tile order (Hydra useTileOrder) ──────────────────────────────────────────────────────────

/// Keep the ids of `base` that still exist, in their order, then append the new ones.
pub fn merge_order(base: &[String], ids: &[String]) -> Vec<String> {
    let current: HashSet<&String> = ids.iter().collect();
    let mut out: Vec<String> = base
        .iter()
        .filter(|id| current.contains(id))
        .cloned()
        .collect();
    let have: HashSet<String> = out.iter().cloned().collect();
    out.extend(ids.iter().filter(|id| !have.contains(*id)).cloned());
    out
}

/// Move `from` to the position of `to` (`arrayMove`).
fn array_move(list: &mut Vec<String>, from: usize, to: usize) {
    let item = list.remove(from);
    list.insert(to, item);
}

/// Per-project tile order. `stored` is what the user arranged (persisted); `runtime` freezes the
/// order a project had when first seen, so activity-driven changes of the agent list never shuffle
/// the tiles.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TileOrder {
    stored: HashMap<String, Vec<String>>,
    runtime: HashMap<String, Vec<String>>,
}

impl TileOrder {
    pub fn from_stored(stored: HashMap<String, Vec<String>>) -> Self {
        Self {
            stored,
            runtime: HashMap::new(),
        }
    }

    pub fn stored(&self) -> &HashMap<String, Vec<String>> {
        &self.stored
    }

    /// Ids in display order. The Running tab (and "no project") keeps the order given.
    pub fn ordered(
        &mut self,
        project: Option<&str>,
        ids: &[String],
        running_tab: bool,
    ) -> Vec<String> {
        let Some(project) = project.filter(|_| !running_tab) else {
            return ids.to_vec();
        };
        let next = {
            let base = self.runtime.get(project).map_or(ids, |v| v.as_slice());
            merge_order(base, ids)
        };
        self.runtime.insert(project.to_string(), next);
        match self.stored.get(project) {
            Some(stored) => merge_order(stored, ids),
            None => self.runtime[project].clone(),
        }
    }

    pub fn has_custom(&self, project: Option<&str>) -> bool {
        project.is_some_and(|p| self.stored.contains_key(p))
    }

    /// Put `active` where `over` is. Ignored on the Running tab or for unknown ids; returns
    /// whether the order changed.
    pub fn reorder(
        &mut self,
        project: Option<&str>,
        ids: &[String],
        active: &str,
        over: &str,
        running_tab: bool,
    ) -> bool {
        let Some(project) = project.filter(|_| !running_tab) else {
            return false;
        };
        let mut ordered = self.ordered(Some(project), ids, false);
        let (Some(from), Some(to)) = (
            ordered.iter().position(|i| i == active),
            ordered.iter().position(|i| i == over),
        ) else {
            return false;
        };
        if from == to {
            return false;
        }
        array_move(&mut ordered, from, to);
        self.stored.insert(project.to_string(), ordered);
        true
    }

    pub fn reset(&mut self, project: &str) {
        self.stored.remove(project);
    }
}

// ── layout ───────────────────────────────────────────────────────────────────────────────────

/// Columns of the grid. `auto` fits as many `MIN_TILE_W` columns as the width allows (never more
/// than tiles); 2 and 3 are exact but give way on narrow screens.
pub fn columns(mode: GridColumns, count: usize, width: u16) -> u16 {
    let count = u16::try_from(count).unwrap_or(u16::MAX).max(1);
    let cols = match mode {
        GridColumns::Auto => (width / MIN_TILE_W).max(1),
        GridColumns::Two => 2.min((width / MIN_FIXED_TILE_W).max(1)),
        GridColumns::Three => 3.min((width / MIN_FIXED_TILE_W).max(1)),
    };
    cols.min(count)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    /// Position in the tab's tile list.
    pub index: usize,
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileLayout {
    pub cols: u16,
    /// Rows needed for all tiles.
    pub rows: u16,
    pub visible_rows: u16,
    pub first_row: u16,
    /// Only the tiles on screen.
    pub tiles: Vec<Tile>,
}

impl TileLayout {
    pub fn empty() -> Self {
        Self {
            cols: 1,
            rows: 0,
            visible_rows: 0,
            first_row: 0,
            tiles: Vec::new(),
        }
    }

    pub fn rect_of(&self, index: usize) -> Option<Rect> {
        self.tiles.iter().find(|t| t.index == index).map(|t| t.rect)
    }

    pub fn tile_at(&self, x: u16, y: u16) -> Option<&Tile> {
        let pos = ratatui::layout::Position::new(x, y);
        self.tiles.iter().find(|t| t.rect.contains(pos))
    }
}

fn split(total: u16, parts: u16) -> Vec<u16> {
    let parts = parts.max(1);
    let base = total / parts;
    let extra = total % parts;
    (0..parts).map(|i| base + u16::from(i < extra)).collect()
}

/// Lay `count` tiles out in `area`. `focus` keeps the focused tile on screen by scrolling whole
/// rows (`first_row_hint` is the previous scroll position); `max_visible` caps the tiles on
/// screen (they all hold a subscription). With `expanded`, that tile alone fills the area.
pub fn layout(
    area: Rect,
    count: usize,
    mode: GridColumns,
    focus: Option<usize>,
    first_row_hint: u16,
    max_visible: usize,
    expanded: Option<usize>,
) -> TileLayout {
    if count == 0 || area.is_empty() {
        return TileLayout::empty();
    }
    if let Some(index) = expanded.filter(|i| *i < count) {
        return TileLayout {
            cols: 1,
            rows: 1,
            visible_rows: 1,
            first_row: 0,
            tiles: vec![Tile { index, rect: area }],
        };
    }
    let cols = columns(mode, count, area.width);
    let cols_n = usize::from(cols);
    let rows = u16::try_from(count.div_ceil(cols_n)).unwrap_or(u16::MAX);
    let fit = (area.height / MIN_TILE_H).max(1);
    let cap = u16::try_from((max_visible / cols_n).max(1)).unwrap_or(1);
    let visible_rows = rows.min(fit).min(cap).max(1);
    let max_first = rows - visible_rows;
    let mut first_row = first_row_hint.min(max_first);
    if let Some(f) = focus.filter(|f| *f < count) {
        let row = u16::try_from(f / cols_n).unwrap_or(0);
        if row < first_row {
            first_row = row;
        } else if row >= first_row + visible_rows {
            first_row = row + 1 - visible_rows;
        }
        first_row = first_row.min(max_first);
    }
    let heights = split(area.height, visible_rows);
    let widths = split(area.width, cols);
    // Hydra: in a two-column auto grid a lone last tile stretches over the row.
    let stretch_last = mode == GridColumns::Auto && cols == 2 && count % 2 == 1 && count >= 3;
    let mut tiles = Vec::new();
    let mut y = area.y;
    for (r, h) in heights.iter().enumerate() {
        let row = usize::from(first_row) + r;
        let mut x = area.x;
        for (c, w) in widths.iter().enumerate() {
            let index = row * cols_n + c;
            if index >= count {
                break;
            }
            let span_all = stretch_last && index == count - 1;
            let width = if span_all { area.right() - x } else { *w };
            tiles.push(Tile {
                index,
                rect: Rect::new(x, y, width, *h),
            });
            x += width;
        }
        y += h;
    }
    TileLayout {
        cols,
        rows,
        visible_rows,
        first_row,
        tiles,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Tile index after moving the focus; stays put at the edges.
pub fn move_focus(current: usize, count: usize, cols: u16, dir: Dir) -> usize {
    if count == 0 {
        return 0;
    }
    let cols = usize::from(cols.max(1));
    let last = count - 1;
    match dir {
        Dir::Left => current.saturating_sub(1),
        Dir::Right => (current + 1).min(last),
        Dir::Up => {
            if current >= cols {
                current - cols
            } else {
                current
            }
        }
        Dir::Down => {
            if current + cols <= last {
                current + cols
            } else if current / cols < last / cols {
                // The row below is shorter: land on its last tile.
                last
            } else {
                current
            }
        }
    }
}

/// Where the project tabs go: `(tab index, x offset, width)` of the tabs that fit in `width`,
/// scrolled so that the active tab is fully visible. A tab is drawn as ` label count `.
pub fn tab_slots(tabs: &[Tab], active: usize, width: u16) -> Vec<(usize, u16, u16)> {
    let widths: Vec<u16> = tabs
        .iter()
        .map(|t| {
            u16::try_from(t.label.chars().count() + t.count.to_string().len() + 3)
                .unwrap_or(u16::MAX)
        })
        .collect();
    // Drop tabs from the left until the active one fits.
    let mut start = 0;
    while start < active {
        let used: u32 = widths[start..=active].iter().map(|w| u32::from(*w)).sum();
        if used <= u32::from(width) {
            break;
        }
        start += 1;
    }
    let mut out = Vec::new();
    let mut x = 0u16;
    for (i, tab_w) in widths.iter().enumerate().skip(start) {
        if x >= width {
            break;
        }
        out.push((i, x, (*tab_w).min(width - x)));
        x += tab_w;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, dir: &str, status: AgentStatus, created: &str) -> AgentState {
        let mut a = AgentState::default();
        a.config.id = id.into();
        a.config.name = id.into();
        a.config.project_dir = dir.into();
        a.config.created_at = created.into();
        a.status = status;
        a
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // ── tile order: ports of useTileOrder.test.ts ────────────────────────────────────────

    #[test]
    fn natural_order_without_stored_order() {
        let mut order = TileOrder::default();
        let out = order.ordered(Some("/tmp/project"), &ids(&["a", "b", "c"]), false);
        assert_eq!(out, ["a", "b", "c"]);
        assert!(!order.has_custom(Some("/tmp/project")));
    }

    #[test]
    fn reorder_moves_the_tile_and_is_stored() {
        let mut order = TileOrder::default();
        let list = ids(&["a", "b", "c"]);
        assert!(order.reorder(Some("/tmp/project"), &list, "c", "a", false));
        assert_eq!(
            order.ordered(Some("/tmp/project"), &list, false),
            ["c", "a", "b"]
        );
        assert!(order.has_custom(Some("/tmp/project")));
        assert_eq!(order.stored()["/tmp/project"], ["c", "a", "b"]);
    }

    #[test]
    fn new_agents_go_to_the_end_of_the_stored_order() {
        let mut order = TileOrder::from_stored(HashMap::from([(
            "/tmp/project".to_string(),
            ids(&["b", "a"]),
        )]));
        let out = order.ordered(Some("/tmp/project"), &ids(&["a", "b", "c"]), false);
        assert_eq!(out, ["b", "a", "c"]);
    }

    #[test]
    fn the_first_order_is_frozen_across_list_reshuffles() {
        let mut order = TileOrder::default();
        let p = Some("/tmp/project");
        assert_eq!(
            order.ordered(p, &ids(&["a", "b", "c"]), false),
            ["a", "b", "c"]
        );
        assert_eq!(
            order.ordered(p, &ids(&["c", "a", "b"]), false),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn newly_appearing_agents_are_appended_to_the_frozen_order() {
        let mut order = TileOrder::default();
        let p = Some("/tmp/project");
        assert_eq!(order.ordered(p, &ids(&["a", "b"]), false), ["a", "b"]);
        assert_eq!(
            order.ordered(p, &ids(&["b", "c", "a"]), false),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn stale_ids_are_dropped_from_the_stored_order() {
        let mut order = TileOrder::from_stored(HashMap::from([(
            "/tmp/project".to_string(),
            ids(&["a", "b", "c"]),
        )]));
        let out = order.ordered(Some("/tmp/project"), &ids(&["a", "c"]), false);
        assert_eq!(out, ["a", "c"]);
    }

    #[test]
    fn the_running_tab_keeps_the_given_order_and_ignores_reorders() {
        let mut order = TileOrder::default();
        let list = ids(&["a", "b"]);
        assert_eq!(order.ordered(Some(RUNNING_TAB), &list, true), ["a", "b"]);
        assert!(!order.has_custom(Some(RUNNING_TAB)));
        assert!(!order.reorder(Some(RUNNING_TAB), &list, "b", "a", true));
        assert_eq!(order.ordered(Some(RUNNING_TAB), &list, true), ["a", "b"]);
    }

    #[test]
    fn reset_clears_one_project_only() {
        let mut order = TileOrder::from_stored(HashMap::from([
            ("/tmp/project".to_string(), ids(&["b", "a"])),
            ("/tmp/other".to_string(), ids(&["x", "y"])),
        ]));
        let list = ids(&["a", "b"]);
        assert!(order.has_custom(Some("/tmp/project")));
        order.reset("/tmp/project");
        assert!(!order.has_custom(Some("/tmp/project")));
        assert_eq!(
            order.ordered(Some("/tmp/project"), &list, false),
            ["a", "b"]
        );
        assert_eq!(order.stored()["/tmp/other"], ["x", "y"]);
    }

    #[test]
    fn no_project_and_unknown_ids_are_harmless() {
        let mut order = TileOrder::default();
        assert_eq!(order.ordered(None, &ids(&["a"]), false), ["a"]);
        assert!(!order.has_custom(None));
        assert!(!order.reorder(None, &ids(&["a"]), "a", "a", false));
        assert!(!order.reorder(Some("/p"), &ids(&["a", "b"]), "a", "zzz", false));
        assert!(!order.reorder(Some("/p"), &ids(&["a", "b"]), "a", "a", false));
    }

    // ── tabs ─────────────────────────────────────────────────────────────────────────────

    const NOW: i64 = 1_800_000_000_000;

    fn sample() -> Vec<AgentState> {
        let mut a1 = agent(
            "a1",
            "/work/api",
            AgentStatus::Running,
            "2026-01-01T00:00:00Z",
        );
        a1.started_at = Some("2026-01-03T00:00:00Z".into());
        a1.last_activity_at = "2026-01-03T00:00:00Z".into();
        let a2 = agent("a2", "/work/api", AgentStatus::Idle, "2026-01-02T00:00:00Z");
        let mut w1 = agent(
            "w1",
            "/work/web",
            AgentStatus::Starting,
            "2026-01-04T00:00:00Z",
        );
        w1.started_at = Some("2026-01-05T00:00:00Z".into());
        w1.last_activity_at = "2026-01-05T00:00:00Z".into();
        vec![a1, a2, w1]
    }

    #[test]
    fn tabs_put_running_first_then_projects_by_activity() {
        let t = tabs(&sample(), false, 0, NOW);
        let labels: Vec<_> = t.iter().map(|t| (t.label.as_str(), t.count)).collect();
        assert_eq!(labels, [("Running", 2), ("web", 1), ("api", 2)]);
        assert_eq!(t[0].key, RUNNING_TAB);
        assert_eq!(t[2].key, "/work/api");
        // "Running only" changes the project counts.
        let t = tabs(&sample(), true, 0, NOW);
        assert_eq!(t[2].count, 1);
    }

    #[test]
    fn running_tab_lists_active_agents_newest_start_first() {
        let list = tab_agents(&sample(), RUNNING_TAB, false, 0, NOW);
        assert_eq!(list, [2, 0]);
    }

    #[test]
    fn project_tab_lists_creation_order_and_honours_running_only() {
        let agents = sample();
        assert_eq!(tab_agents(&agents, "/work/api", false, 0, NOW), [0, 1]);
        assert_eq!(tab_agents(&agents, "/work/api", true, 0, NOW), [0]);
        assert!(tab_agents(&agents, "/nope", false, 0, NOW).is_empty());
    }

    #[test]
    fn old_idle_agents_are_hidden_by_the_age_limit() {
        let agents = sample();
        let now = timestamp_ms("2026-01-20T00:00:00Z");
        // a2 was created on Jan 2: older than 7 days and idle -> hidden; the active ones stay.
        assert_eq!(tab_agents(&agents, "/work/api", false, 7, now), [0]);
        assert_eq!(tab_agents(&agents, "/work/api", false, 0, now), [0, 1]);
        let t = tabs(&agents, false, 7, now);
        assert_eq!(
            t.iter().find(|t| t.key == "/work/api").map(|t| t.count),
            Some(1)
        );
    }

    #[test]
    fn basename_handles_separators() {
        assert_eq!(basename("/work/api"), "api");
        assert_eq!(basename("/work/api/"), "api");
        assert_eq!(basename("C:\\work\\web"), "web");
        assert_eq!(basename("/"), "/");
    }

    // ── layout math ──────────────────────────────────────────────────────────────────────

    #[test]
    fn auto_columns_follow_width_and_count() {
        assert_eq!(columns(GridColumns::Auto, 7, 200), 5);
        assert_eq!(columns(GridColumns::Auto, 4, 200), 4);
        assert_eq!(columns(GridColumns::Auto, 2, 200), 2);
        assert_eq!(columns(GridColumns::Auto, 1, 200), 1);
        assert_eq!(columns(GridColumns::Auto, 9, 100), 2);
        assert_eq!(columns(GridColumns::Auto, 9, 79), 1);
        assert_eq!(columns(GridColumns::Auto, 9, 10), 1);
        assert_eq!(columns(GridColumns::Auto, 0, 100), 1);
    }

    #[test]
    fn fixed_columns_give_way_on_narrow_screens_and_few_tiles() {
        assert_eq!(columns(GridColumns::Two, 5, 120), 2);
        assert_eq!(columns(GridColumns::Three, 5, 120), 3);
        assert_eq!(columns(GridColumns::Three, 5, 70), 2);
        assert_eq!(columns(GridColumns::Three, 5, 40), 1);
        assert_eq!(columns(GridColumns::Two, 1, 120), 1);
        assert_eq!(columns(GridColumns::Three, 2, 120), 2);
    }

    fn rects(l: &TileLayout) -> Vec<(usize, Rect)> {
        l.tiles.iter().map(|t| (t.index, t.rect)).collect()
    }

    #[test]
    fn four_tiles_form_a_2x2_grid_that_covers_the_area() {
        let area = Rect::new(10, 2, 100, 40);
        let l = layout(area, 4, GridColumns::Auto, Some(0), 0, 10, None);
        assert_eq!((l.cols, l.rows, l.visible_rows), (2, 2, 2));
        assert_eq!(
            rects(&l),
            [
                (0, Rect::new(10, 2, 50, 20)),
                (1, Rect::new(60, 2, 50, 20)),
                (2, Rect::new(10, 22, 50, 20)),
                (3, Rect::new(60, 22, 50, 20)),
            ]
        );
        let covered: u32 = l
            .tiles
            .iter()
            .map(|t| u32::from(t.rect.width) * u32::from(t.rect.height))
            .sum();
        assert_eq!(covered, 100 * 40);
    }

    #[test]
    fn a_lone_last_tile_stretches_in_a_two_column_auto_grid() {
        let l = layout(
            Rect::new(0, 0, 90, 24),
            3,
            GridColumns::Auto,
            None,
            0,
            10,
            None,
        );
        assert_eq!(l.cols, 2);
        assert_eq!(l.tiles[2].rect, Rect::new(0, 12, 90, 12));
        // Not for fixed columns.
        let l = layout(
            Rect::new(0, 0, 90, 24),
            3,
            GridColumns::Two,
            None,
            0,
            10,
            None,
        );
        assert_eq!(l.tiles[2].rect.width, 45);
    }

    #[test]
    fn seven_tiles_in_a_wide_area() {
        let l = layout(
            Rect::new(0, 0, 200, 40),
            7,
            GridColumns::Auto,
            Some(6),
            0,
            10,
            None,
        );
        assert_eq!((l.cols, l.rows, l.visible_rows), (5, 2, 2));
        assert_eq!(l.tiles.len(), 7);
        assert!(
            l.tiles
                .iter()
                .all(|t| t.rect.right() <= 200 && t.rect.bottom() <= 40)
        );
        // The last row has 2 tiles.
        assert_eq!(l.tiles.iter().filter(|t| t.rect.y > 0).count(), 2);
    }

    #[test]
    fn rows_scroll_to_keep_the_focus_visible_and_respect_the_tile_cap() {
        // 10 tiles, 2 columns, height for 2 rows: 5 rows in total.
        let area = Rect::new(0, 0, 100, 16);
        let l = layout(area, 10, GridColumns::Auto, Some(0), 0, 10, None);
        assert_eq!((l.visible_rows, l.first_row, l.tiles.len()), (2, 0, 4));
        let l = layout(area, 10, GridColumns::Auto, Some(9), 0, 10, None);
        assert_eq!((l.first_row, l.tiles[0].index), (3, 6));
        // Focus inside the window keeps the previous position.
        let l = layout(area, 10, GridColumns::Auto, Some(7), 3, 10, None);
        assert_eq!(l.first_row, 3);
        let l = layout(area, 10, GridColumns::Auto, Some(2), 3, 10, None);
        assert_eq!(l.first_row, 1);
        // A stale hint is clamped.
        let l = layout(area, 10, GridColumns::Auto, None, 99, 10, None);
        assert_eq!(l.first_row, 3);
        // Never more tiles on screen than subscriptions allowed.
        let l = layout(
            Rect::new(0, 0, 200, 80),
            30,
            GridColumns::Auto,
            Some(0),
            0,
            10,
            None,
        );
        assert!(l.tiles.len() <= 10);
    }

    #[test]
    fn tiny_areas_and_expansion() {
        assert_eq!(
            layout(
                Rect::new(0, 0, 0, 0),
                3,
                GridColumns::Auto,
                None,
                0,
                10,
                None
            ),
            TileLayout::empty()
        );
        assert_eq!(
            layout(
                Rect::new(0, 0, 50, 5),
                0,
                GridColumns::Auto,
                None,
                0,
                10,
                None
            ),
            TileLayout::empty()
        );
        let l = layout(
            Rect::new(0, 0, 50, 5),
            3,
            GridColumns::Auto,
            Some(1),
            0,
            10,
            None,
        );
        assert_eq!(l.visible_rows, 1);
        assert_eq!(l.tiles.len(), 1);
        let area = Rect::new(3, 4, 120, 30);
        let l = layout(area, 6, GridColumns::Auto, Some(4), 0, 10, Some(4));
        assert_eq!(rects(&l), [(4, area)]);
        // An expanded index out of range falls back to the normal layout.
        let l = layout(area, 2, GridColumns::Auto, None, 0, 10, Some(9));
        assert_eq!(l.tiles.len(), 2);
    }

    #[test]
    fn tile_lookup_by_position() {
        let l = layout(
            Rect::new(0, 0, 100, 16),
            4,
            GridColumns::Auto,
            None,
            0,
            10,
            None,
        );
        assert_eq!(l.tile_at(0, 0).map(|t| t.index), Some(0));
        assert_eq!(l.tile_at(99, 15).map(|t| t.index), Some(3));
        assert_eq!(l.tile_at(100, 0), None);
    }

    #[test]
    fn focus_moves_over_the_grid() {
        // 5 tiles, 2 columns:  0 1 / 2 3 / 4
        assert_eq!(move_focus(0, 5, 2, Dir::Right), 1);
        assert_eq!(move_focus(1, 5, 2, Dir::Right), 2);
        assert_eq!(move_focus(4, 5, 2, Dir::Right), 4);
        assert_eq!(move_focus(0, 5, 2, Dir::Left), 0);
        assert_eq!(move_focus(3, 5, 2, Dir::Up), 1);
        assert_eq!(move_focus(1, 5, 2, Dir::Up), 1);
        assert_eq!(move_focus(2, 5, 2, Dir::Down), 4);
        assert_eq!(move_focus(3, 5, 2, Dir::Down), 4, "short last row");
        assert_eq!(move_focus(4, 5, 2, Dir::Down), 4);
        assert_eq!(move_focus(0, 0, 2, Dir::Down), 0);
    }

    #[test]
    fn tab_slots_scroll_to_the_active_tab() {
        let tab = |l: &str| Tab {
            key: l.into(),
            label: l.into(),
            count: 1,
        };
        let tabs = vec![tab("Running"), tab("alpha"), tab("beta"), tab("gamma")];
        // widths: 11, 9, 8, 9
        let slots = tab_slots(&tabs, 0, 100);
        assert_eq!(slots.len(), 4);
        assert_eq!(slots[1], (1, 11, 9));
        let slots = tab_slots(&tabs, 3, 20);
        assert_eq!(slots[0].0, 2, "scrolled so that the active tab fits");
        assert!(slots.iter().any(|(i, _, _)| *i == 3));
        let slots = tab_slots(&tabs, 0, 15);
        assert_eq!(
            slots.last().map(|s| s.2),
            Some(4),
            "the last tab is clipped"
        );
    }
}
