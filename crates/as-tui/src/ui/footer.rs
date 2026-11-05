//! Footer: current mode, prefix-pending indicator and key hints.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use as_core::ViewMode;

use crate::app::{App, Focus, Modal};
use crate::keymap::Scope;

fn picker_hints(mode: &'static str) -> (&'static str, String) {
    (
        mode,
        "j/k move \u{00B7} Enter pick \u{00B7} / filter \u{00B7} a ages \u{00B7} p project \u{00B7} s hidden \u{00B7} h hide \u{00B7} Esc close".into(),
    )
}

const SEP: &str = " \u{00B7} ";

/// The key of a command id in one scope (`Prefix`: the label starts with the prefix key, `App`:
/// `sidebar `, `Grid`: `grid `), the shortest one when it has several (`h` over `Left`). The scope
/// word is dropped unless `full` (`Ctrl+A g`), because the mode label already says where a key
/// applies. `None` when the command has no key in that scope (the user unbound or moved it).
fn key_of(app: &App, id: &str, scope: Scope, full: bool) -> Option<String> {
    let word = match scope {
        Scope::Prefix => format!("{} ", app.prefix_label()),
        Scope::App => "sidebar ".to_string(),
        Scope::Grid => "grid ".to_string(),
        Scope::Scroll | Scope::Direct => return None,
    };
    app.keymap
        .labels_for(id)
        .iter()
        .filter(|l| l.starts_with(&word))
        .min_by_key(|l| l.chars().count())
        .map(|l| {
            if full {
                l.clone()
            } else {
                l[word.len()..].to_string()
            }
        })
}

/// `key word` pairs joined with a middle dot; a pair whose command has no binding is left out.
/// An entry with several ids shows their keys joined with `/` (`j/k`).
fn hints(app: &App, items: &[(&[&str], &str)], scope: Scope, full: bool) -> String {
    items
        .iter()
        .filter_map(|(ids, word)| {
            let keys: Vec<String> = ids
                .iter()
                .filter_map(|id| key_of(app, id, scope, full))
                .collect();
            if keys.is_empty() {
                return None;
            }
            // `Ctrl+A ]/{`, not `Ctrl+A ]/Ctrl+A {`.
            let keys: Vec<String> = if full {
                let word = format!("{} ", app.prefix_label());
                keys.iter()
                    .enumerate()
                    .map(|(i, k)| match k.strip_prefix(&word) {
                        Some(rest) if i > 0 => rest.to_string(),
                        _ => k.clone(),
                    })
                    .collect()
            } else {
                keys
            };
            // `h` `j` `k` `l` read better as `hjkl`.
            let joined = if keys.len() > 2 && keys.iter().all(|k| k.chars().count() == 1) {
                keys.concat()
            } else {
                keys.join("/")
            };
            Some(format!("{joined} {word}"))
        })
        .collect::<Vec<_>>()
        .join(SEP)
}

fn prefix_hints(app: &App) -> String {
    hints(
        app,
        &[
            (&["new-agent"], "new"),
            (&["toggle-view"], "grid"),
            (&["command-palette"], "palette"),
            (&["broadcast"], "broadcast"),
            (&["resume-session"], "resume"),
            (&["close-agent"], "kill"),
            (&["restart-agent"], "restart"),
            (&["remove-agent"], "remove"),
            (&["rename-agent"], "rename"),
            (&["toggle-yolo"], "yolo"),
            (&["next-agent", "prev-agent"], "agents"),
            (&["switch-agent-by-index"], "jump"),
            (&["focus-sidebar"], "sidebar"),
            (&["toggle-tile-expand"], "expand"),
            (&["focus-sidebar-search"], "search"),
            (&["show-shortcuts"], "help"),
            (&["quit"], "quit"),
        ],
        Scope::Prefix,
        false,
    )
}

/// `(mode label, hints)` for the footer.
pub fn mode_and_hints(app: &App) -> (&'static str, String) {
    if app.prefix_pending {
        let mut text = prefix_hints(app);
        if app.modal.is_some() {
            text = modal_prefix_hints(app);
        }
        return ("PREFIX", text);
    }
    match &app.modal {
        Some(Modal::NewAgent(form)) if form.picker.is_some() => return picker_hints("SESSIONS"),
        Some(Modal::Resume(_)) => return picker_hints("RESUME"),
        Some(Modal::Profile(_)) => {
            return (
                "PROFILE",
                "\u{2191}/\u{2193} choose \u{00B7} Enter apply \u{00B7} Esc cancel".into(),
            );
        }
        Some(Modal::NewAgent(form)) if form.mode == crate::new_agent::FormMode::Headless => {
            return ("NEW RUN", "Tab/\u{2193} next \u{00B7} \u{2190}/\u{2192} change \u{00B7} Enter start \u{00B7} Esc back".into());
        }
        Some(Modal::NewAgent(_)) => {
            return ("NEW AGENT", "Tab/\u{2193} next \u{00B7} Shift+Tab prev \u{00B7} \u{2190}/\u{2192} change \u{00B7} Enter create \u{00B7} Esc cancel".into());
        }
        Some(Modal::Rename { .. }) => return ("RENAME", "Enter save \u{00B7} Esc cancel".into()),
        Some(Modal::ConfirmRemove { .. }) => {
            return ("CONFIRM", "y remove \u{00B7} n cancel".into());
        }
        Some(Modal::Quit { .. }) => {
            return ("QUIT", "\u{2190}/\u{2192} choose \u{00B7} Enter confirm \u{00B7} d detach \u{00B7} s stop all \u{00B7} Esc cancel".into());
        }
        Some(Modal::Help) => {
            return (
                "HELP",
                "j/k PgUp/PgDn scroll \u{00B7} g/G top/bottom \u{00B7} Esc closes".into(),
            );
        }
        Some(Modal::Usage(_)) => {
            return (
                "USAGE",
                "j/k scroll \u{00B7} \u{2190}/\u{2192} provider \u{00B7} d 7d/30d \u{00B7} f low/all \u{00B7} r refresh \u{00B7} Esc close".into(),
            );
        }
        Some(Modal::Headless) => {
            return match &app.headless.view {
                Some(_) => (
                    "HEADLESS LOG",
                    "j/k PgUp/PgDn scroll \u{00B7} G follow \u{00B7} r raw/rendered \u{00B7} R reload \u{00B7} x cancel \u{00B7} Esc list".into(),
                ),
                None if app.headless.filtering => (
                    "HEADLESS",
                    "type to search \u{00B7} Enter keep \u{00B7} Esc clear".into(),
                ),
                None => (
                    "HEADLESS",
                    "j/k move \u{00B7} Enter log \u{00B7} n new \u{00B7} / search \u{00B7} s status \u{00B7} x cancel \u{00B7} r refresh \u{00B7} Esc close".into(),
                ),
            };
        }
        Some(Modal::Git) => {
            return (
                "GIT",
                "Tab/1-3 tabs \u{00B7} j/k move \u{00B7} Space stage \u{00B7} c commit \u{00B7} P push \u{00B7} q close".into(),
            );
        }
        Some(Modal::Files) => {
            return (
                "FILES",
                "j/k move \u{00B7} Enter open \u{00B7} f search \u{00B7} E editor \u{00B7} o open in \u{00B7} q close".into(),
            );
        }
        Some(Modal::Settings) => {
            return (
                "SETTINGS",
                "j/k move \u{00B7} Enter/Space change \u{00B7} Esc back".into(),
            );
        }
        Some(Modal::OpenIn) => {
            return (
                "OPEN IN",
                "\u{2191}/\u{2193} choose \u{00B7} Enter open \u{00B7} Esc cancel".into(),
            );
        }
        Some(Modal::Palette(_)) => {
            return (
                "PALETTE",
                "type to filter \u{00B7} \u{2191}/\u{2193} choose \u{00B7} Enter run \u{00B7} Esc close".into(),
            );
        }
        None => {}
    }
    if app.free_terms.open {
        let keys = hints(
            app,
            &[
                (&["toggle-terminal"], "hide"),
                (&["terminal-new"], "new tab"),
                (&["terminal-split"], "split \u{2192}"),
                (&["terminal-split-down"], "split \u{2193}"),
                (&["terminal-next-tab", "terminal-prev-tab"], "tabs"),
                (&["terminal-next-pane"], "pane"),
                (&["terminal-kill"], "close"),
            ],
            Scope::Prefix,
            true,
        );
        return ("TERMINALS", format!("typing into the shell{SEP}{keys}"));
    }
    if app.scroll_mode {
        return (
            "SCROLL",
            "\u{2191}/\u{2193} line \u{00B7} PgUp/PgDn page \u{00B7} g/G top/bottom \u{00B7} Esc/q leave".into(),
        );
    }
    match app.focus {
        Focus::Terminal if app.view == ViewMode::Grid => (
            "TILE",
            format!(
                "typing into the tile{SEP}{}",
                hints(
                    app,
                    &[
                        (&["close-dialogs"], "back to the grid"),
                        (&["toggle-view"], "chat"),
                        (&["show-shortcuts"], "help"),
                    ],
                    Scope::Prefix,
                    true,
                )
            ),
        ),
        Focus::Terminal => (
            "TERMINAL",
            hints(
                app,
                &[
                    (&["show-shortcuts"], "help"),
                    (&["command-palette"], "palette"),
                    (&["toggle-view"], "grid"),
                    (&["new-agent"], "new"),
                    (&["focus-sidebar"], "sidebar"),
                    (&["quit"], "quit"),
                ],
                Scope::Prefix,
                true,
            ),
        ),
        Focus::Grid => (
            "GRID",
            hints(
                app,
                &[
                    (&["grid-left", "grid-down", "grid-up", "grid-right"], "move"),
                    (&["grid-attach"], "type"),
                    (&["grid-open-chat"], "chat"),
                    (&["toggle-tile-expand"], "expand"),
                    (&["grid-prev-tab", "grid-next-tab"], "tab"),
                    (&["grid-move-prev", "grid-move-next"], "reorder"),
                    (&["broadcast"], "broadcast"),
                    (&["grid-only-running"], "running"),
                    (&["grid-cycle-columns"], "columns"),
                ],
                Scope::Grid,
                false,
            ) + SEP
                + "Tab sidebar",
        ),
        Focus::Broadcast => (
            "BROADCAST",
            "Enter send to every running agent of the tab \u{00B7} Alt+Enter newline \u{00B7} Esc close".into(),
        ),
        Focus::App => (
            "APP",
            hints(
                app,
                &[
                    (&["cursor-down", "cursor-up"], "move"),
                    (&["activate"], "attach"),
                    (&["toggle-collapse"], "fold"),
                    (&["focus-sidebar-search"], "search"),
                    (&["cycle-sidebar-status"], "status"),
                    (&["cycle-sidebar-sort"], "sort"),
                    (&["new-agent"], "new"),
                    (&["resume-session"], "resume"),
                ],
                Scope::App,
                false,
            ) + SEP
                + "Esc back",
        ),
        Focus::Overview => (
            "OVERVIEW",
            "Tab card \u{00B7} j/k move \u{00B7} Enter jump/details \u{00B7} m mode \u{00B7} p project \u{00B7} t waits \u{00B7} PgUp/PgDn scroll \u{00B7} Esc close".into(),
        ),
        Focus::Input => (
            "INPUT",
            "Enter send \u{00B7} Alt+Enter newline \u{00B7} Esc terminal".into(),
        ),
    }
}

/// After the prefix inside a modal: only a few global commands work there.
fn modal_prefix_hints(app: &App) -> String {
    hints(
        app,
        &[
            (&["toggle-terminal"], "terminals"),
            (&["command-palette"], "palette"),
            (&["show-shortcuts"], "help"),
            (&["close-dialogs"], "close"),
        ],
        Scope::Prefix,
        false,
    )
}

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let t = &app.theme;
    let bar = Style::default().fg(t.text_secondary).bg(t.surface_raised);
    let (mode, hints) = mode_and_hints(app);
    let mode_style = if app.prefix_pending {
        Style::default().fg(t.accent_text).bg(t.starting)
    } else {
        Style::default().fg(t.accent_text).bg(t.accent)
    };
    let mut spans = vec![
        Span::styled(format!(" {mode} "), mode_style.add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(hints, Style::default().fg(t.text_secondary)),
    ];
    if app.prefix_pending {
        spans.insert(
            1,
            Span::styled(
                format!(" {} \u{2026}", app.prefix_label()),
                Style::default().fg(t.starting).add_modifier(Modifier::BOLD),
            ),
        );
    }
    Line::from(spans).style(bar).render(area, buf);
}
