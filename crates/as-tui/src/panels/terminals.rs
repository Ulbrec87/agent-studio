//! Free terminals (Hydra's `FreeTerminalPanel`): plain shells per project, drawn as a panel at the
//! bottom of the screen (or full screen). Tabs = the layout's groups, split = the panes of a group
//! (side by side). The daemon owns the PTYs; each pane here has a vt100 emulator fed by
//! `TerminalOutput` events after a `SubscribeTerminal`.

use std::collections::HashMap;

use as_core::terminals::{FreeTerminalGroup, FreeTerminalLayout};
use as_proto::{Request, Response};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use super::{Reply, req};
use crate::app::{Effect, SubState};
use crate::term::{TerminalEmulator, Vt100Emulator, keys};
use crate::widgets::TextArea;

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
    pub panes: Vec<Rect>,
}

/// Panel (bottom ~45 % or the whole content area) with a tab row and `panes` side-by-side areas
/// separated by one column.
pub fn geometry(screen: Rect, fullscreen: bool, panes: usize) -> Geometry {
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
    let n = u16::try_from(panes.max(1)).unwrap_or(1);
    let seps = n - 1;
    let each = body.width.saturating_sub(seps) / n;
    let mut rects = Vec::new();
    let mut x = body.x;
    for i in 0..n {
        let w = if i == n - 1 {
            body.right().saturating_sub(x)
        } else {
            each
        };
        rects.push(Rect::new(x, body.y, w, body.height));
        x += w + 1;
    }
    Geometry {
        panel,
        tabs,
        panes: rects,
    }
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

    /// Panes of the active tab, in order.
    pub fn visible_panes(&self) -> Vec<String> {
        self.active_group()
            .map(|g| g.pane_ids.clone())
            .unwrap_or_default()
    }

    pub fn geometry(&self) -> Geometry {
        geometry(self.screen, self.fullscreen, self.visible_panes().len())
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
                fx.extend(self.spawn(None));
            } else if !empty {
                self.want_first = false;
            }
            fx.extend(self.sync());
        }
        fx
    }

    fn spawn(&mut self, group_id: Option<String>) -> Vec<Effect> {
        self.spawning = true;
        self.want_first = false;
        let (cols, rows) = self
            .geometry()
            .panes
            .first()
            .map_or((80, 24), |r| (r.width.max(2), r.height.max(2)));
        vec![req(
            "term:spawn",
            Request::SpawnTerminal {
                project_dir: self.project_dir.clone(),
                cwd: None,
                group_id,
                cols,
                rows,
            },
        )]
    }

    /// `prefix T`.
    pub fn new_tab(&mut self) -> Vec<Effect> {
        self.spawn(None)
    }

    /// `prefix |`: another pane in the active tab (or a first terminal).
    pub fn split(&mut self) -> Vec<Effect> {
        let group = self.active_group().map(|g| g.id.clone());
        self.spawn(group)
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
        let geo = self.geometry();
        let mut fx = Vec::new();
        for (id, rect) in self.visible_panes().into_iter().zip(geo.panes) {
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
            ("term:activate", Ok(Response::TerminalLayout(l))) => {
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

    #[test]
    fn geometry_splits_the_body_evenly() {
        let g = geometry(Rect::new(0, 0, 100, 40), false, 2);
        assert_eq!(g.panel.bottom(), 39, "the footer row stays visible");
        assert!(g.panel.height >= 8);
        assert_eq!(g.panes.len(), 2);
        assert_eq!(g.panes[0].width + 1 + g.panes[1].width, g.tabs.width);
        let full = geometry(Rect::new(0, 0, 100, 40), true, 1);
        assert_eq!(full.panel.height, 38);
        // Tiny screens never underflow.
        let tiny = geometry(Rect::new(0, 0, 4, 3), false, 3);
        assert_eq!(tiny.panes.len(), 3);
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
        let fx = t.split();
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
