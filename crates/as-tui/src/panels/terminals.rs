//! Free terminals (Hydra's `FreeTerminalPanel`): plain shells per project, drawn as a panel at the
//! bottom of the screen (or full screen). Tabs = the layout's groups; the panes of a tab are laid
//! out by a split tree (`FreeTerminalNode`: right / down splits with a ratio). The daemon owns the
//! PTYs and the tree; each pane here has a vt100 emulator fed by `TerminalOutput` events after a
//! `SubscribeTerminal`.

use std::collections::HashMap;

use as_core::terminal_tree::{FreeTerminalNode, SplitDir};
use as_core::terminals::{FreeTerminalGroup, FreeTerminalLayout};
use as_proto::{Request, Response};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use super::{Reply, req};
use crate::app::{Effect, SubState};
use crate::term::{TerminalEmulator, Vt100Emulator, keys};
use crate::widgets::TextArea;

/// Percentage points one resize key press moves a divider.
pub const RESIZE_STEP: i16 = 5;

pub struct PaneTerm {
    pub emu: Box<dyn TerminalEmulator>,
    pub sub: SubState,
    pub sent_size: Option<(u16, u16)>,
}

/// Where the panel draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Geometry {
    pub panel: Rect,
    pub tabs: Rect,
    /// The area the pane tree is laid out in.
    pub body: Rect,
}

/// Panel (bottom ~45 % or the whole content area) with a tab row and the body below it.
pub fn geometry(screen: Rect, fullscreen: bool) -> Geometry {
    // Keep the header (1 row) and footer (1 row) visible.
    let content = Rect::new(
        screen.x,
        screen.y + 1,
        screen.width,
        screen.height.saturating_sub(2),
    );
    let height = if fullscreen {
        content.height
    } else {
        (content.height * 45 / 100).max(8).min(content.height)
    };
    let panel = Rect::new(
        content.x,
        content.bottom().saturating_sub(height),
        content.width,
        height,
    );
    // Border (1 each side) + tab row.
    let inner = Rect::new(
        panel.x + 1,
        panel.y + 1,
        panel.width.saturating_sub(2),
        panel.height.saturating_sub(2),
    );
    let tabs = Rect::new(inner.x, inner.y, inner.width, inner.height.min(1));
    let body = Rect::new(
        inner.x,
        inner.y + tabs.height,
        inner.width,
        inner.height.saturating_sub(tabs.height),
    );
    Geometry { panel, tabs, body }
}

/// Lay a split tree out in `area`: one rectangle per pane plus a one-cell separator per split
/// (a column for side-by-side panes, a row for stacked ones). `ratio` is the share of the first
/// child of the space that is left after the separator.
pub fn tree_rects(area: Rect, node: &FreeTerminalNode) -> (Vec<(String, Rect)>, Vec<Rect>) {
    let (mut panes, mut seps) = (Vec::new(), Vec::new());
    place(area, node, &mut panes, &mut seps);
    (panes, seps)
}

fn place(
    area: Rect,
    node: &FreeTerminalNode,
    panes: &mut Vec<(String, Rect)>,
    seps: &mut Vec<Rect>,
) {
    match node {
        FreeTerminalNode::Leaf { pane_id } => panes.push((pane_id.clone(), area)),
        FreeTerminalNode::Split { dir, ratio, a, b } => {
            let horizontal = *dir == SplitDir::Horizontal;
            let total = if horizontal { area.width } else { area.height };
            let avail = total.saturating_sub(1);
            let first = (u32::from(avail) * u32::from(*ratio) / 100) as u16;
            // Both children keep at least one cell whenever there is room for that.
            let first = if avail >= 2 {
                first.clamp(1, avail - 1)
            } else {
                first.min(avail)
            };
            let second = avail - first;
            let (ra, sep, rb) = if horizontal {
                (
                    Rect::new(area.x, area.y, first, area.height),
                    Rect::new(area.x + first, area.y, total.min(1), area.height),
                    Rect::new(area.x + first + 1, area.y, second, area.height),
                )
            } else {
                (
                    Rect::new(area.x, area.y, area.width, first),
                    Rect::new(area.x, area.y + first, area.width, total.min(1)),
                    Rect::new(area.x, area.y + first + 1, area.width, second),
                )
            };
            seps.push(sep);
            place(ra, a, panes, seps);
            place(rb, b, panes, seps);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// The pane next to `from` in direction `dir`: the nearest one that lies entirely beyond it and
/// shares some rows (left / right) or columns (up / down); the one with the larger overlap wins
/// a tie. `None` when `from` is at that edge.
pub fn neighbor(panes: &[(String, Rect)], from: &str, dir: Dir) -> Option<String> {
    let c = panes.iter().find(|(id, _)| id == from)?.1;
    let overlap =
        |lo1: u16, hi1: u16, lo2: u16, hi2: u16| hi1.min(hi2).saturating_sub(lo1.max(lo2));
    panes
        .iter()
        .filter(|(id, _)| id != from)
        .filter_map(|(id, r)| {
            let (gap, shared) = match dir {
                Dir::Right if r.x >= c.right() => {
                    (r.x - c.right(), overlap(c.y, c.bottom(), r.y, r.bottom()))
                }
                Dir::Left if r.right() <= c.x => {
                    (c.x - r.right(), overlap(c.y, c.bottom(), r.y, r.bottom()))
                }
                Dir::Down if r.y >= c.bottom() => {
                    (r.y - c.bottom(), overlap(c.x, c.right(), r.x, r.right()))
                }
                Dir::Up if r.bottom() <= c.y => {
                    (c.y - r.bottom(), overlap(c.x, c.right(), r.x, r.right()))
                }
                _ => return None,
            };
            (shared > 0).then(|| (id.clone(), gap, shared))
        })
        .min_by_key(|(_, gap, shared)| (*gap, std::cmp::Reverse(*shared)))
        .map(|(id, _, _)| id)
}

#[derive(Default)]
pub struct FreeTerms {
    pub open: bool,
    pub fullscreen: bool,
    pub project_dir: String,
    pub layouts: HashMap<String, FreeTerminalLayout>,
    pub terms: HashMap<String, PaneTerm>,
    pub rename: Option<(String, TextArea)>,
    pub screen: Rect,
    /// Open a first terminal when the project has none (after the layout arrived).
    want_first: bool,
    /// A spawn is on its way (do not start a second one for the same trigger).
    spawning: bool,
}

impl FreeTerms {
    pub fn layout(&self) -> Option<&FreeTerminalLayout> {
        self.layouts.get(&self.project_dir)
    }

    pub fn active_group(&self) -> Option<&FreeTerminalGroup> {
        let l = self.layout()?;
        let id = l.active_group_id.as_ref()?;
        l.groups.iter().find(|g| &g.id == id)
    }

    pub fn active_pane(&self) -> Option<String> {
        let g = self.active_group()?;
        Some(g.active_pane_id.clone()).filter(|p| g.pane_ids.contains(p))
    }

    pub fn label_of(&self, pane_id: &str) -> String {
        self.layout()
            .and_then(|l| l.panes.iter().find(|p| p.id == pane_id))
            .map_or_else(|| pane_id.to_string(), |p| p.label.clone())
    }

    /// Panes of the active tab, in reading order.
    pub fn visible_panes(&self) -> Vec<String> {
        self.active_group()
            .map(|g| g.pane_ids.clone())
            .unwrap_or_default()
    }

    pub fn geometry(&self) -> Geometry {
        geometry(self.screen, self.fullscreen)
    }

    /// Rectangle of every pane of the active tab and the separators between them.
    pub fn pane_rects(&self) -> (Vec<(String, Rect)>, Vec<Rect>) {
        let body = self.geometry().body;
        self.active_group()
            .and_then(FreeTerminalGroup::effective_tree)
            .map_or_else(Default::default, |t| tree_rects(body, &t))
    }

    /// `prefix t`: show / hide the panel for `project_dir`.
    pub fn toggle(&mut self, project_dir: &str, screen: Rect) -> Vec<Effect> {
        if self.open {
            self.open = false;
            self.rename = None;
            return Vec::new();
        }
        self.open = true;
        self.screen = screen;
        if self.project_dir != project_dir {
            self.project_dir = project_dir.to_string();
        }
        self.want_first = true;
        vec![req(
            "term:layout",
            Request::GetTerminalLayout {
                project_dir: project_dir.to_string(),
            },
        )]
    }

    pub fn on_layout(&mut self, project_dir: &str, layout: FreeTerminalLayout) -> Vec<Effect> {
        let mut fx = Vec::new();
        let current = project_dir == self.project_dir;
        let empty = layout.groups.is_empty();
        // Drop emulators of terminals that are gone from every project we know.
        self.layouts.insert(project_dir.to_string(), layout);
        let alive: Vec<String> = self
            .layouts
            .values()
            .flat_map(|l| l.panes.iter().map(|p| p.id.clone()))
            .collect();
        self.terms.retain(|id, _| alive.contains(id));
        if let Some(l) = self.layouts.get(project_dir) {
            for p in &l.panes {
                if !self.terms.contains_key(&p.id) {
                    self.terms.insert(
                        p.id.clone(),
                        PaneTerm {
                            emu: Box::new(Vt100Emulator::new(80, 24)),
                            sub: SubState::Requested(Vec::new()),
                            sent_size: None,
                        },
                    );
                    fx.push(req(
                        format!("term:sub:{}", p.id),
                        Request::SubscribeTerminal {
                            terminal_id: p.id.clone(),
                        },
                    ));
                }
            }
        }
        if current && self.open {
            if empty && self.want_first && !self.spawning {
                fx.extend(self.spawn(None, None, None));
            } else if !empty {
                self.want_first = false;
            }
            fx.extend(self.sync());
        }
        fx
    }

    /// Start a shell: a new tab, or (with `group_id`) a split of `split_pane` in `split_dir`.
    fn spawn(
        &mut self,
        group_id: Option<String>,
        split_pane: Option<String>,
        split_dir: Option<SplitDir>,
    ) -> Vec<Effect> {
        self.spawning = true;
        self.want_first = false;
        // The size of the pane it will get, roughly; `sync` corrects it once the layout is back.
        let (panes, _) = self.pane_rects();
        let target = split_pane
            .as_ref()
            .and_then(|p| panes.iter().find(|(id, _)| id == p))
            .or_else(|| panes.first())
            .map(|(_, r)| *r);
        let (cols, rows) = target.map_or((80, 24), |r| match split_dir {
            Some(SplitDir::Vertical) if group_id.is_some() => (r.width, r.height / 2),
            Some(_) | None if group_id.is_some() => (r.width / 2, r.height),
            _ => (r.width, r.height),
        });
        vec![req(
            "term:spawn",
            Request::SpawnTerminal {
                project_dir: self.project_dir.clone(),
                cwd: None,
                group_id,
                split_pane,
                split_dir,
                cols: cols.max(2),
                rows: rows.max(2),
            },
        )]
    }

    /// `prefix T`.
    pub fn new_tab(&mut self) -> Vec<Effect> {
        self.spawn(None, None, None)
    }

    /// `prefix |` (right) / `prefix -` (down): split the active pane (or start a first terminal).
    pub fn split(&mut self, dir: SplitDir) -> Vec<Effect> {
        let group = self.active_group().map(|g| g.id.clone());
        let pane = self.active_pane();
        self.spawn(group, pane, Some(dir))
    }

    /// Focus the neighbouring pane in `dir` (nothing at the edge of the tab).
    pub fn focus_dir(&mut self, dir: Dir) -> Vec<Effect> {
        let (Some(group), Some(active)) = (
            self.active_group().map(|g| g.id.clone()),
            self.active_pane(),
        ) else {
            return Vec::new();
        };
        let (panes, _) = self.pane_rects();
        match neighbor(&panes, &active, dir) {
            Some(next) => self.activate(Some(group), Some(next)),
            None => Vec::new(),
        }
    }

    /// Move the divider next to the active pane in the direction of the key (`Left`/`Up` shrink the
    /// first child of the nearest split on that axis, `Right`/`Down` grow it).
    pub fn resize_dir(&mut self, dir: Dir) -> Vec<Effect> {
        let Some(pane) = self.active_pane() else {
            return Vec::new();
        };
        let (axis, delta) = match dir {
            Dir::Left => (SplitDir::Horizontal, -RESIZE_STEP),
            Dir::Right => (SplitDir::Horizontal, RESIZE_STEP),
            Dir::Up => (SplitDir::Vertical, -RESIZE_STEP),
            Dir::Down => (SplitDir::Vertical, RESIZE_STEP),
        };
        // Apply it locally right away; the daemon's layout confirms (or corrects) it.
        let project = self.project_dir.clone();
        let mut changed = false;
        if let Some(l) = self.layouts.get_mut(&project)
            && let Some(g) = l.groups.iter_mut().find(|g| g.pane_ids.contains(&pane))
        {
            let mut tree = g.effective_tree();
            changed = tree.as_mut().is_some_and(|t| t.resize(&pane, axis, delta));
            g.tree = tree;
        }
        if !changed {
            return Vec::new();
        }
        let mut fx = vec![req(
            "term:resize",
            Request::ResizeTerminalPane {
                project_dir: project,
                pane_id: pane,
                axis,
                delta,
            },
        )];
        fx.extend(self.sync());
        fx
    }

    pub fn kill_active(&mut self) -> Vec<Effect> {
        match self.active_pane() {
            Some(id) => vec![req("term:kill", Request::KillTerminal { terminal_id: id })],
            None => Vec::new(),
        }
    }

    pub fn cycle_tab(&mut self, delta: isize) -> Vec<Effect> {
        let Some(l) = self.layout() else {
            return Vec::new();
        };
        let n = l.groups.len();
        if n < 2 {
            return Vec::new();
        }
        let cur = l
            .active_group_id
            .as_ref()
            .and_then(|a| l.groups.iter().position(|g| &g.id == a))
            .unwrap_or(0);
        let next = (cur as isize + delta).rem_euclid(n as isize) as usize;
        let group = l.groups[next].id.clone();
        self.activate(Some(group), None)
    }

    pub fn cycle_pane(&mut self) -> Vec<Effect> {
        let Some(g) = self.active_group() else {
            return Vec::new();
        };
        if g.pane_ids.len() < 2 {
            return Vec::new();
        }
        let cur = g
            .pane_ids
            .iter()
            .position(|p| *p == g.active_pane_id)
            .unwrap_or(0);
        let (group, pane) = (
            g.id.clone(),
            g.pane_ids[(cur + 1) % g.pane_ids.len()].clone(),
        );
        self.activate(Some(group), Some(pane))
    }

    fn activate(&mut self, group_id: Option<String>, pane_id: Option<String>) -> Vec<Effect> {
        // Update locally right away; the daemon's layout event confirms it.
        if let Some(l) = self.layouts.get_mut(&self.project_dir)
            && let Some(gid) = &group_id
        {
            l.active_group_id = Some(gid.clone());
            if let (Some(p), Some(g)) = (&pane_id, l.groups.iter_mut().find(|g| &g.id == gid)) {
                g.active_pane_id.clone_from(p);
            }
        }
        let mut fx = vec![req(
            "term:activate",
            Request::ActivateTerminal {
                project_dir: self.project_dir.clone(),
                group_id,
                pane_id,
            },
        )];
        fx.extend(self.sync());
        fx
    }

    pub fn start_rename(&mut self) {
        if let Some(id) = self.active_pane() {
            self.rename = Some((id.clone(), TextArea::single_line(&self.label_of(&id))));
        }
    }

    pub fn toggle_fullscreen(&mut self) -> Vec<Effect> {
        self.fullscreen = !self.fullscreen;
        self.sync()
    }

    /// Size every visible emulator to its pane and tell the daemon about changes.
    pub fn sync(&mut self) -> Vec<Effect> {
        if !self.open {
            return Vec::new();
        }
        let (rects, _) = self.pane_rects();
        let mut fx = Vec::new();
        for (id, rect) in rects {
            let (cols, rows) = (rect.width.max(2), rect.height.max(2));
            if let Some(t) = self.terms.get_mut(&id)
                && t.sent_size != Some((cols, rows))
            {
                t.emu.resize(cols, rows);
                t.sent_size = Some((cols, rows));
                fx.push(Effect::TermResize(id, cols, rows));
            }
        }
        fx
    }

    pub fn on_resize(&mut self, screen: Rect) -> Vec<Effect> {
        self.screen = screen;
        self.sync()
    }

    pub fn on_reply(&mut self, key: &str, result: Reply) -> Vec<Effect> {
        match (key, result) {
            ("term:layout", Ok(Response::TerminalLayout(l))) => {
                let dir = l.project_dir.clone();
                self.on_layout(&dir, l)
            }
            ("term:spawn", Ok(Response::TerminalSpawned(s))) => {
                self.spawning = false;
                let dir = s.layout.project_dir.clone();
                self.on_layout(&dir, s.layout)
            }
            ("term:spawn", Err(_)) => {
                self.spawning = false;
                Vec::new()
            }
            ("term:activate" | "term:resize", Ok(Response::TerminalLayout(l))) => {
                let dir = l.project_dir.clone();
                self.on_layout(&dir, l)
            }
            (k, result) if k.starts_with("term:sub:") => self.on_subscribed(
                &k["term:sub:".len()..],
                result.map(|r| match r {
                    Response::Buffer { data } => data,
                    _ => Vec::new(),
                }),
            ),
            _ => Vec::new(),
        }
    }

    fn on_subscribed(&mut self, id: &str, result: Result<Vec<u8>, String>) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(t) = self.terms.get_mut(id) else {
            return fx;
        };
        match result {
            Ok(buffer) => {
                let pending = match std::mem::replace(&mut t.sub, SubState::Live) {
                    SubState::Requested(p) => p,
                    _ => Vec::new(),
                };
                t.emu.process(&buffer);
                // Queries inside the replayed history were answered long ago.
                t.emu.take_replies();
                t.emu.take_bell();
                t.emu.process(&pending);
                let replies = t.emu.take_replies();
                if !replies.is_empty() {
                    fx.push(Effect::TermInput(id.to_string(), replies));
                }
                t.emu.take_bell();
            }
            Err(_) => t.sub = SubState::Failed,
        }
        fx
    }

    pub fn on_output(&mut self, id: &str, data: &[u8]) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(t) = self.terms.get_mut(id) else {
            return fx;
        };
        match &mut t.sub {
            SubState::Requested(pending) => pending.extend_from_slice(data),
            SubState::Live => {
                t.emu.process(data);
                let replies = t.emu.take_replies();
                if !replies.is_empty() {
                    fx.push(Effect::TermInput(id.to_string(), replies));
                }
                if t.emu.take_bell() {
                    fx.push(Effect::Bell);
                }
            }
            SubState::Failed => {}
        }
        fx
    }

    pub fn on_exit(&mut self, id: &str) {
        self.terms.remove(id);
    }

    /// The connection was lost: the daemon's terminals are gone with it.
    pub fn reset(&mut self) {
        self.terms.clear();
        self.layouts.clear();
        self.spawning = false;
    }

    /// Keys while the panel is open: everything goes to the active shell (the prefix and the
    /// direct bindings were handled before), except while renaming.
    pub fn key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if let Some((id, input)) = &mut self.rename {
            match key.code {
                KeyCode::Esc => self.rename = None,
                KeyCode::Enter => {
                    let (id, label) = (id.clone(), input.text());
                    self.rename = None;
                    return vec![req(
                        "term:rename",
                        Request::RenameTerminal {
                            terminal_id: id,
                            label,
                        },
                    )];
                }
                _ => {
                    input.handle_key(&key);
                }
            }
            return Vec::new();
        }
        let Some(id) = self.active_pane() else {
            return Vec::new();
        };
        let Some(t) = self.terms.get_mut(&id) else {
            return Vec::new();
        };
        if key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
        {
            let page = usize::from(t.emu.size().1.saturating_sub(1)).max(1);
            let cur = t.emu.scrollback();
            t.emu.set_scrollback(if key.code == KeyCode::PageUp {
                cur + page
            } else {
                cur.saturating_sub(page)
            });
            return Vec::new();
        }
        let bytes = keys::encode_key(key, t.emu.application_cursor());
        if bytes.is_empty() {
            return Vec::new();
        }
        t.emu.set_scrollback(0);
        vec![Effect::TermInput(id, bytes)]
    }

    pub fn paste(&mut self, text: &str) -> Vec<Effect> {
        let Some(id) = self.active_pane() else {
            return Vec::new();
        };
        let Some(t) = self.terms.get_mut(&id) else {
            return Vec::new();
        };
        let bytes = keys::encode_paste(text, t.emu.bracketed_paste());
        t.emu.set_scrollback(0);
        vec![Effect::TermInput(id, bytes)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use as_core::terminals::{FreeTerminalPane, SpawnedTerminal};

    fn layout(dir: &str, groups: &[(&str, &[&str])], active: &str) -> FreeTerminalLayout {
        FreeTerminalLayout {
            project_dir: dir.into(),
            groups: groups
                .iter()
                .map(|(g, panes)| FreeTerminalGroup {
                    id: (*g).into(),
                    pane_ids: panes.iter().map(|p| (*p).into()).collect(),
                    active_pane_id: panes.last().map(|p| (*p).into()).unwrap_or_default(),
                    tree: None,
                })
                .collect(),
            active_group_id: Some(active.into()),
            panes: groups
                .iter()
                .flat_map(|(_, p)| p.iter())
                .map(|p| FreeTerminalPane {
                    id: (*p).into(),
                    label: format!("T-{p}"),
                    exited: false,
                })
                .collect(),
        }
    }

    fn open_panel() -> FreeTerms {
        let mut t = FreeTerms::default();
        t.toggle("/p", Rect::new(0, 0, 100, 40));
        t
    }

    fn row(ids: &[&str]) -> FreeTerminalNode {
        FreeTerminalNode::row(&ids.iter().map(|s| (*s).to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn geometry_keeps_the_header_and_footer_rows_visible() {
        let g = geometry(Rect::new(0, 0, 100, 40), false);
        assert_eq!(g.panel.bottom(), 39, "the footer row stays visible");
        assert!(g.panel.height >= 8);
        assert_eq!(g.body.width, g.tabs.width);
        assert_eq!(g.body.y, g.tabs.bottom());
        let full = geometry(Rect::new(0, 0, 100, 40), true);
        assert_eq!(full.panel.height, 38);
        // Tiny screens never underflow.
        let tiny = geometry(Rect::new(0, 0, 4, 3), false);
        assert!(tiny.body.width <= 4);
    }

    #[test]
    fn tree_rects_tile_the_area_with_one_cell_separators() {
        let area = Rect::new(10, 5, 81, 21);
        // a | (b / c) with a 50 % first split.
        let mut tree = FreeTerminalNode::leaf("a");
        tree.split_leaf("a", "b", SplitDir::Horizontal);
        tree.split_leaf("b", "c", SplitDir::Vertical);
        let (panes, seps) = tree_rects(area, &tree);
        let rect = |id: &str| panes.iter().find(|(p, _)| p == id).unwrap().1;
        assert_eq!(rect("a"), Rect::new(10, 5, 40, 21));
        assert_eq!(rect("b"), Rect::new(51, 5, 40, 10));
        assert_eq!(rect("c"), Rect::new(51, 16, 40, 10));
        assert_eq!(
            seps,
            [Rect::new(50, 5, 1, 21), Rect::new(51, 15, 40, 1)],
            "a column and a row"
        );
        // Everything inside the area, nothing overlapping.
        let all: Vec<Rect> = panes.iter().map(|(_, r)| *r).chain(seps.clone()).collect();
        for (i, r) in all.iter().enumerate() {
            assert!(
                area.contains((r.x, r.y).into())
                    && r.right() <= area.right()
                    && r.bottom() <= area.bottom()
            );
            for o in &all[i + 1..] {
                assert!(!r.intersects(*o), "{r:?} overlaps {o:?}");
            }
        }
        // The ratio moves the divider.
        tree.resize("a", SplitDir::Horizontal, 20);
        let (panes, _) = tree_rects(area, &tree);
        assert_eq!(panes[0].1.width, 56);
    }

    #[test]
    fn tree_rects_never_underflow_on_tiny_areas() {
        for w in 0..4 {
            for h in 0..4 {
                let (panes, seps) = tree_rects(Rect::new(0, 0, w, h), &row(&["a", "b", "c"]));
                assert_eq!(panes.len(), 3);
                assert_eq!(seps.len(), 2);
            }
        }
        // A two-cell width still gives both panes a cell.
        let (panes, _) = tree_rects(Rect::new(0, 0, 3, 1), &row(&["a", "b"]));
        assert!(panes.iter().all(|(_, r)| r.width >= 1));
    }

    #[test]
    fn a_flat_layout_without_a_tree_is_a_row() {
        let mut t = open_panel();
        t.on_layout("/p", layout("/p", &[("g1", &["a", "b"])], "g1"));
        let (panes, seps) = t.pane_rects();
        assert_eq!(panes.len(), 2);
        assert_eq!(seps.len(), 1);
        assert_eq!(panes[0].1.y, panes[1].1.y, "side by side");
    }

    /// `a | (b / c)`, `c` active.
    fn three_pane_layout() -> FreeTerminalLayout {
        let mut l = layout("/p", &[("g1", &["a", "b", "c"])], "g1");
        let mut tree = FreeTerminalNode::leaf("a");
        tree.split_leaf("a", "b", SplitDir::Horizontal);
        tree.split_leaf("b", "c", SplitDir::Vertical);
        l.groups[0].tree = Some(tree);
        l
    }

    fn open_three() -> FreeTerms {
        let mut t = open_panel();
        t.on_layout("/p", three_pane_layout());
        t
    }

    #[test]
    fn neighbors_by_direction() {
        let t = open_three();
        let (panes, _) = t.pane_rects();
        let n = |from: &str, d| neighbor(&panes, from, d);
        assert!(n("a", Dir::Right).is_some_and(|p| p == "b" || p == "c"));
        assert_eq!(n("b", Dir::Left).as_deref(), Some("a"));
        assert_eq!(n("c", Dir::Left).as_deref(), Some("a"));
        assert_eq!(n("b", Dir::Down).as_deref(), Some("c"));
        assert_eq!(n("c", Dir::Up).as_deref(), Some("b"));
        assert_eq!(n("a", Dir::Left), None);
        assert_eq!(n("a", Dir::Up), None);
        assert_eq!(n("c", Dir::Down), None);
        assert_eq!(n("c", Dir::Right), None);
        assert_eq!(n("zzz", Dir::Right), None);
    }

    #[test]
    fn a_tie_in_overlap_goes_to_the_first_pane() {
        let panes = vec![
            ("a".to_string(), Rect::new(0, 0, 10, 10)),
            ("b".to_string(), Rect::new(11, 0, 10, 5)),
            ("c".to_string(), Rect::new(11, 5, 10, 5)),
            ("far".to_string(), Rect::new(22, 0, 10, 10)),
        ];
        assert_eq!(neighbor(&panes, "a", Dir::Right).as_deref(), Some("b"));
        // The pane that overlaps more wins over the one that merely touches the rows.
        let panes = vec![
            ("a".to_string(), Rect::new(0, 3, 10, 6)),
            ("b".to_string(), Rect::new(11, 0, 10, 5)),
            ("c".to_string(), Rect::new(11, 5, 10, 5)),
        ];
        assert_eq!(neighbor(&panes, "a", Dir::Right).as_deref(), Some("c"));
    }

    #[test]
    fn focus_follows_the_direction_and_tells_the_daemon() {
        let mut t = open_three();
        assert_eq!(t.active_pane().as_deref(), Some("c"));
        let fx = t.focus_dir(Dir::Up);
        assert!(
            matches!(&fx[0], Effect::Req { req: Request::ActivateTerminal { pane_id: Some(p), .. }, .. } if p == "b")
        );
        assert_eq!(t.active_pane().as_deref(), Some("b"));
        t.focus_dir(Dir::Left);
        assert_eq!(t.active_pane().as_deref(), Some("a"));
        assert!(t.focus_dir(Dir::Left).is_empty(), "nothing beyond the edge");
    }

    #[test]
    fn splits_ask_for_a_direction_and_a_pane() {
        let mut t = open_three();
        t.on_reply("term:sub:c", Ok(Response::Buffer { data: Vec::new() }));
        let fx = t.split(SplitDir::Vertical);
        assert!(matches!(
            &fx[..],
            [Effect::Req {
                req: Request::SpawnTerminal {
                    group_id: Some(g),
                    split_pane: Some(p),
                    split_dir: Some(SplitDir::Vertical),
                    ..
                },
                ..
            }] if g == "g1" && p == "c"
        ));
        let fx = t.split(SplitDir::Horizontal);
        assert!(matches!(
            &fx[..],
            [Effect::Req {
                req: Request::SpawnTerminal {
                    split_dir: Some(SplitDir::Horizontal),
                    ..
                },
                ..
            }]
        ));
    }

    #[test]
    fn resize_keys_move_the_divider_locally_and_in_the_daemon() {
        let mut t = open_three();
        t.sync();
        // `c` is below `b`: Down grows the upper pane (the divider moves down).
        let fx = t.resize_dir(Dir::Down);
        assert!(matches!(
            &fx[0],
            Effect::Req {
                req: Request::ResizeTerminalPane { pane_id, axis: SplitDir::Vertical, delta: 5, .. },
                ..
            } if pane_id == "c"
        ));
        let FreeTerminalNode::Split { b, .. } = t.active_group().unwrap().tree.clone().unwrap()
        else {
            panic!()
        };
        assert!(matches!(*b, FreeTerminalNode::Split { ratio: 55, .. }));
        // The panes were resized to the new geometry.
        assert!(fx.iter().any(|e| matches!(e, Effect::TermResize(..))));
        // Left moves the outer divider (c has no horizontal split of its own).
        let fx = t.resize_dir(Dir::Left);
        assert!(matches!(
            &fx[0],
            Effect::Req {
                req: Request::ResizeTerminalPane {
                    axis: SplitDir::Horizontal,
                    delta: -5,
                    ..
                },
                ..
            }
        ));
        // At the limit nothing is sent.
        for _ in 0..20 {
            t.resize_dir(Dir::Left);
        }
        assert!(t.resize_dir(Dir::Left).is_empty());
        // The daemon's answer replaces the local guess.
        let mut l = three_pane_layout();
        if let Some(FreeTerminalNode::Split { ratio, .. }) = &mut l.groups[0].tree {
            *ratio = 70;
        }
        t.on_reply("term:resize", Ok(Response::TerminalLayout(l)));
        assert!(matches!(
            t.active_group().unwrap().tree,
            Some(FreeTerminalNode::Split { ratio: 70, .. })
        ));
    }

    #[test]
    fn sync_sizes_every_pane_of_the_tree() {
        let mut t = open_panel();
        // The layout arrives: every pane of the tree gets its own size.
        let fx = t.on_layout("/p", three_pane_layout());
        assert_eq!(
            fx.iter()
                .filter(|e| matches!(e, Effect::TermResize(..)))
                .count(),
            3
        );
        let (panes, _) = t.pane_rects();
        for (id, r) in panes {
            assert_eq!(
                t.terms[&id].sent_size,
                Some((r.width.max(2), r.height.max(2)))
            );
        }
    }

    #[test]
    fn an_empty_project_gets_a_first_terminal_once() {
        let mut t = FreeTerms::default();
        let fx = t.toggle("/p", Rect::new(0, 0, 100, 40));
        assert!(matches!(&fx[..], [Effect::Req { key, .. }] if key == "term:layout"));
        let fx = t.on_layout("/p", layout("/p", &[], ""));
        assert!(matches!(
            &fx[..],
            [Effect::Req {
                req: Request::SpawnTerminal { group_id: None, .. },
                ..
            }]
        ));
        // Another empty layout while the spawn is pending does not spawn again.
        assert!(t.on_layout("/p", layout("/p", &[], "")).is_empty());
        let spawned = SpawnedTerminal {
            terminal_id: "a".into(),
            group_id: "g1".into(),
            layout: layout("/p", &[("g1", &["a"])], "g1"),
        };
        let fx = t.on_reply("term:spawn", Ok(Response::TerminalSpawned(spawned)));
        assert!(fx.iter().any(|e| matches!(e, Effect::Req { req: Request::SubscribeTerminal { terminal_id }, .. } if terminal_id == "a")));
        assert!(
            fx.iter()
                .any(|e| matches!(e, Effect::TermResize(id, ..) if id == "a"))
        );
    }

    #[test]
    fn subscribe_replays_the_buffer_and_holds_back_live_output() {
        let mut t = open_panel();
        t.on_layout("/p", layout("/p", &[("g1", &["a"])], "g1"));
        assert!(t.on_output("a", b"live").is_empty());
        t.on_reply(
            "term:sub:a",
            Ok(Response::Buffer {
                data: b"old ".to_vec(),
            }),
        );
        let a = &t.terms["a"];
        assert!(matches!(a.sub, SubState::Live));
        let screen = a.emu.size();
        assert!(screen.0 >= 2);
        t.on_output("a", b"more");
        // A cursor-position query in the output is answered through the daemon.
        let fx = t.on_output("a", b"\x1b[6n");
        assert!(
            matches!(&fx[..], [Effect::TermInput(id, bytes)] if id == "a" && bytes.starts_with(b"\x1b["))
        );
    }

    #[test]
    fn typing_goes_to_the_active_pane_and_the_prefix_commands_work() {
        let mut t = open_panel();
        t.on_layout(
            "/p",
            layout("/p", &[("g1", &["a", "b"]), ("g2", &["c"])], "g1"),
        );
        t.on_reply("term:sub:a", Ok(Response::Buffer { data: Vec::new() }));
        t.on_reply("term:sub:b", Ok(Response::Buffer { data: Vec::new() }));
        let fx = t.key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(
            fx,
            vec![Effect::TermInput("b".into(), b"x".to_vec())],
            "the last pane of a group is active"
        );
        let fx = t.cycle_pane();
        assert!(
            matches!(&fx[0], Effect::Req { req: Request::ActivateTerminal { pane_id: Some(p), .. }, .. } if p == "a")
        );
        assert_eq!(t.active_pane().as_deref(), Some("a"));
        t.cycle_tab(1);
        assert_eq!(t.active_group().map(|g| g.id.as_str()), Some("g2"));
        t.cycle_tab(1);
        assert_eq!(t.active_group().map(|g| g.id.as_str()), Some("g1"), "wraps");
        let fx = t.split(SplitDir::Horizontal);
        assert!(
            matches!(&fx[..], [Effect::Req { req: Request::SpawnTerminal { group_id: Some(g), .. }, .. }] if g == "g1")
        );
        let fx = t.kill_active();
        assert!(
            matches!(&fx[..], [Effect::Req { req: Request::KillTerminal { terminal_id }, .. }] if terminal_id == "a")
        );
    }

    #[test]
    fn rename_edits_the_label() {
        let mut t = open_panel();
        t.on_layout("/p", layout("/p", &[("g1", &["a"])], "g1"));
        t.start_rename();
        assert!(t.rename.is_some());
        for _ in 0..10 {
            t.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        }
        for c in "build".chars() {
            t.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let fx = t.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(&fx[..], [Effect::Req { req: Request::RenameTerminal { label, .. }, .. }] if label == "build")
        );
        assert!(t.rename.is_none());
    }

    #[test]
    fn exited_terminals_lose_their_emulator() {
        let mut t = open_panel();
        t.on_layout("/p", layout("/p", &[("g1", &["a", "b"])], "g1"));
        assert_eq!(t.terms.len(), 2);
        t.on_layout("/p", layout("/p", &[("g1", &["b"])], "g1"));
        assert_eq!(t.terms.len(), 1);
        assert!(t.terms.contains_key("b"));
    }
}
