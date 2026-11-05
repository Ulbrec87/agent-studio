//! The split tree of a free-terminal tab: `Leaf(pane)` or `Split { dir, ratio, a, b }`.
//!
//! Pure data and operations (no PTYs, no UI): the daemon keeps one tree per tab and persists it, the
//! TUI turns it into rectangles. `Horizontal` places `a` left of `b` (a "split right"), `Vertical`
//! places `a` above `b` (a "split down"); `ratio` is the share of `a` in percent.

use serde::{Deserialize, Serialize};

/// Smallest and largest share of the first child (percent) a resize can produce.
pub const MIN_RATIO: u8 = 10;
pub const MAX_RATIO: u8 = 90;
pub const DEFAULT_RATIO: u8 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDir {
    /// Side by side: `a` on the left, `b` on the right.
    Horizontal,
    /// Stacked: `a` on top, `b` below.
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum FreeTerminalNode {
    Leaf {
        #[serde(rename = "paneId")]
        pane_id: String,
    },
    Split {
        dir: SplitDir,
        /// Percent of the space given to `a` (the rest goes to `b`).
        ratio: u8,
        a: Box<FreeTerminalNode>,
        b: Box<FreeTerminalNode>,
    },
}

impl FreeTerminalNode {
    pub fn leaf(pane_id: impl Into<String>) -> Self {
        Self::Leaf {
            pane_id: pane_id.into(),
        }
    }

    /// Panes side by side with equal shares (what tabs of the old, flat layout looked like).
    pub fn row(pane_ids: &[String]) -> Option<Self> {
        let (first, rest) = pane_ids.split_first()?;
        if rest.is_empty() {
            return Some(Self::leaf(first.clone()));
        }
        let share = (100 / pane_ids.len()).clamp(1, 99) as u8;
        Some(Self::Split {
            dir: SplitDir::Horizontal,
            ratio: share,
            a: Box::new(Self::leaf(first.clone())),
            b: Box::new(Self::row(rest)?),
        })
    }

    /// Pane ids in reading order (`a` before `b`).
    pub fn leaves(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Self::Leaf { pane_id } => out.push(pane_id),
            Self::Split { a, b, .. } => {
                a.collect(out);
                b.collect(out);
            }
        }
    }

    pub fn contains(&self, pane: &str) -> bool {
        match self {
            Self::Leaf { pane_id } => pane_id == pane,
            Self::Split { a, b, .. } => a.contains(pane) || b.contains(pane),
        }
    }

    /// Replace the leaf `target` with a split holding it and a new leaf `new_pane` (the new pane
    /// goes right of / below the target). False when `target` is not in the tree.
    pub fn split_leaf(&mut self, target: &str, new_pane: &str, dir: SplitDir) -> bool {
        match self {
            Self::Leaf { pane_id } if pane_id == target => {
                let old = std::mem::replace(self, Self::leaf(String::new()));
                *self = Self::Split {
                    dir,
                    ratio: DEFAULT_RATIO,
                    a: Box::new(old),
                    b: Box::new(Self::leaf(new_pane)),
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { a, b, .. } => {
                a.split_leaf(target, new_pane, dir) || b.split_leaf(target, new_pane, dir)
            }
        }
    }

    /// The tree without the leaf `pane`: its sibling takes the place of their parent split.
    /// `None` when nothing is left.
    pub fn remove_leaf(self, pane: &str) -> Option<Self> {
        match self {
            Self::Leaf { pane_id } if pane_id == pane => None,
            leaf @ Self::Leaf { .. } => Some(leaf),
            Self::Split { dir, ratio, a, b } => match (a.remove_leaf(pane), b.remove_leaf(pane)) {
                (Some(a), Some(b)) => Some(Self::Split {
                    dir,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (only, None) | (None, only) => only,
            },
        }
    }

    /// Move the divider nearest above `pane` along `axis` by `delta` percentage points (positive:
    /// `a` grows, i.e. the divider moves right / down). False when `pane` has no ancestor split
    /// on that axis or the ratio is already at its limit.
    pub fn resize(&mut self, pane: &str, axis: SplitDir, delta: i16) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split { dir, ratio, a, b } => {
                // The deepest matching ancestor wins: try the child that holds the pane first.
                let child = if a.contains(pane) {
                    a
                } else if b.contains(pane) {
                    b
                } else {
                    return false;
                };
                if child.resize(pane, axis, delta) {
                    return true;
                }
                if *dir != axis {
                    return false;
                }
                let next = (i16::from(*ratio) + delta)
                    .clamp(i16::from(MIN_RATIO), i16::from(MAX_RATIO))
                    as u8;
                let changed = next != *ratio;
                *ratio = next;
                changed
            }
        }
    }

    /// Keep each pane id only at its first appearance (`seen` is shared across trees); the sibling
    /// of a dropped duplicate takes its parent's place. `None` when nothing is left.
    pub fn retain_unique(self, seen: &mut std::collections::HashSet<String>) -> Option<Self> {
        match self {
            Self::Leaf { pane_id } => seen
                .insert(pane_id.clone())
                .then_some(Self::Leaf { pane_id }),
            Self::Split { dir, ratio, a, b } => {
                match (a.retain_unique(seen), b.retain_unique(seen)) {
                    (Some(a), Some(b)) => Some(Self::Split {
                        dir,
                        ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    }),
                    (only, None) | (None, only) => only,
                }
            }
        }
    }

    /// Clamp every ratio into range (a hand-edited or corrupt file must not produce empty panes).
    pub fn sanitized(self) -> Self {
        match self {
            leaf @ Self::Leaf { .. } => leaf,
            Self::Split { dir, ratio, a, b } => Self::Split {
                dir,
                ratio: ratio.clamp(MIN_RATIO, MAX_RATIO),
                a: Box::new(a.sanitized()),
                b: Box::new(b.sanitized()),
            },
        }
    }

    /// Rename the panes with `f` (restore: old ids to the freshly spawned ones).
    pub fn map_panes(&mut self, f: &mut impl FnMut(&str) -> String) {
        match self {
            Self::Leaf { pane_id } => *pane_id = f(pane_id),
            Self::Split { a, b, .. } => {
                a.map_panes(f);
                b.map_panes(f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: &FreeTerminalNode) -> Vec<&str> {
        n.leaves()
    }

    #[test]
    fn splitting_right_and_down_builds_a_tree() {
        let mut t = FreeTerminalNode::leaf("a");
        assert!(t.split_leaf("a", "b", SplitDir::Horizontal));
        assert!(t.split_leaf("b", "c", SplitDir::Vertical));
        assert!(!t.split_leaf("zzz", "d", SplitDir::Vertical));
        assert_eq!(ids(&t), ["a", "b", "c"]);
        let FreeTerminalNode::Split { dir, ratio, a, b } = &t else {
            panic!("a split");
        };
        assert_eq!((*dir, *ratio), (SplitDir::Horizontal, DEFAULT_RATIO));
        assert_eq!(**a, FreeTerminalNode::leaf("a"));
        assert!(matches!(
            &**b,
            FreeTerminalNode::Split {
                dir: SplitDir::Vertical,
                ..
            }
        ));
    }

    #[test]
    fn removing_a_leaf_promotes_its_sibling() {
        let mut t = FreeTerminalNode::leaf("a");
        t.split_leaf("a", "b", SplitDir::Horizontal);
        t.split_leaf("b", "c", SplitDir::Vertical);
        let t = t.remove_leaf("b").unwrap();
        assert_eq!(ids(&t), ["a", "c"]);
        assert!(matches!(
            t,
            FreeTerminalNode::Split {
                dir: SplitDir::Horizontal,
                ..
            }
        ));
        let t = t.remove_leaf("a").unwrap();
        assert_eq!(t, FreeTerminalNode::leaf("c"));
        assert_eq!(t.clone().remove_leaf("nope"), Some(t.clone()));
        assert_eq!(t.remove_leaf("c"), None);
    }

    #[test]
    fn resize_moves_the_nearest_divider_on_the_axis_and_clamps() {
        let mut t = FreeTerminalNode::leaf("a");
        t.split_leaf("a", "b", SplitDir::Horizontal);
        t.split_leaf("b", "c", SplitDir::Vertical);
        // `c` sits in a vertical split: the vertical axis resizes that one...
        assert!(t.resize("c", SplitDir::Vertical, 10));
        // ...and the horizontal axis falls through to the outer split.
        assert!(t.resize("c", SplitDir::Horizontal, -20));
        let FreeTerminalNode::Split { ratio, b, .. } = &t else {
            panic!()
        };
        assert_eq!(*ratio, 30);
        let FreeTerminalNode::Split { ratio: inner, .. } = &**b else {
            panic!()
        };
        assert_eq!(*inner, 60);
        // Limits.
        assert!(t.resize("a", SplitDir::Horizontal, -100));
        assert!(
            !t.resize("a", SplitDir::Horizontal, -5),
            "already at the minimum"
        );
        assert!(t.resize("a", SplitDir::Horizontal, 100));
        let FreeTerminalNode::Split { ratio, .. } = &t else {
            panic!()
        };
        assert_eq!(*ratio, MAX_RATIO);
        // No ancestor on that axis, unknown pane, lone leaf.
        assert!(!t.resize("a", SplitDir::Vertical, 5));
        assert!(!t.resize("zzz", SplitDir::Horizontal, 5));
        assert!(!FreeTerminalNode::leaf("a").resize("a", SplitDir::Horizontal, 5));
    }

    #[test]
    fn a_flat_row_of_panes_has_equal_shares() {
        let ids3: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let t = FreeTerminalNode::row(&ids3).unwrap();
        assert_eq!(t.leaves(), ["a", "b", "c"]);
        let FreeTerminalNode::Split { ratio, .. } = t else {
            panic!()
        };
        assert_eq!(ratio, 33);
        assert_eq!(FreeTerminalNode::row(&[]), None);
        assert_eq!(
            FreeTerminalNode::row(&["x".to_string()]),
            Some(FreeTerminalNode::leaf("x"))
        );
    }

    #[test]
    fn json_shape_and_sanitizing() {
        let mut t = FreeTerminalNode::leaf("a");
        t.split_leaf("a", "b", SplitDir::Vertical);
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "type": "split", "dir": "vertical", "ratio": 50,
                "a": {"type": "leaf", "paneId": "a"},
                "b": {"type": "leaf", "paneId": "b"},
            })
        );
        assert_eq!(serde_json::from_value::<FreeTerminalNode>(v).unwrap(), t);
        let wild = FreeTerminalNode::Split {
            dir: SplitDir::Horizontal,
            ratio: 0,
            a: Box::new(FreeTerminalNode::leaf("a")),
            b: Box::new(FreeTerminalNode::leaf("b")),
        };
        let FreeTerminalNode::Split { ratio, .. } = wild.sanitized() else {
            panic!()
        };
        assert_eq!(ratio, MIN_RATIO);
    }

    #[test]
    fn map_panes_renames_every_leaf() {
        let mut t = FreeTerminalNode::leaf("a");
        t.split_leaf("a", "b", SplitDir::Horizontal);
        t.map_panes(&mut |id| format!("new-{id}"));
        assert_eq!(t.leaves(), ["new-a", "new-b"]);
    }
}
