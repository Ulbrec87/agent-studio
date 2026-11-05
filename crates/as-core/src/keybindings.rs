//! The command catalog and the user keybindings file (Hydra `shared/keybindings.ts` +
//! `electron/config/KeybindingStore.ts`).
//!
//! The catalog lists every command with its label, category, palette/help visibility and the
//! **default keys in TUI syntax**. The UI crate maps ids to its own `Command` values and parses the
//! key strings; this module only knows the strings, so it stays free of terminal types.
//!
//! # Key syntax
//!
//! `keys` is one or more bindings separated by `" | "`. A binding is `<scope> <chord>`:
//!
//! * `prefix g` — after the prefix key (tmux style), valid in every focus;
//! * `app j` — sidebar focus, `grid h` — grid focus, `scroll k` — scroll mode (no prefix needed);
//! * `ctrl+g`, `alt+1` — a *direct* chord (no scope word): works in every focus, so the key never
//!   reaches the agent.
//!
//! A chord is `[ctrl+][alt+][shift+]<key>` with `<key>` a character or `esc enter tab space
//! backspace delete up down left right home end pageup pagedown f1..f12`. `1-9` is a digit range
//! (`prefix 1-9`, `alt+1-9`) that passes the pressed digit as argument. `prefix prefix` is the
//! configured prefix key itself. `none` unbinds the command.
//!
//! The file is `[{ "command": "<id>", "keys": "<keys>" }, ...]` like Hydra's; entries override the
//! defaults per command (the whole `keys` string is replaced).

use serde::{Deserialize, Serialize};

/// One command of the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandDef {
    pub id: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub show_in_palette: bool,
    pub show_in_shortcuts: bool,
    /// Whether this version of the TUI can run the command (the rest of Hydra's list arrives with
    /// later phases; such commands are accepted in the file and ignored).
    pub implemented: bool,
    /// Default bindings (see the module docs); empty = unbound.
    pub default_keys: &'static str,
}

const fn def(
    id: &'static str,
    label: &'static str,
    category: &'static str,
    show_in_palette: bool,
    show_in_shortcuts: bool,
    default_keys: &'static str,
) -> CommandDef {
    CommandDef {
        id,
        label,
        category,
        show_in_palette,
        show_in_shortcuts,
        implemented: true,
        default_keys,
    }
}

const fn later(
    id: &'static str,
    label: &'static str,
    category: &'static str,
    show_in_palette: bool,
    show_in_shortcuts: bool,
) -> CommandDef {
    CommandDef {
        id,
        label,
        category,
        show_in_palette,
        show_in_shortcuts,
        implemented: false,
        default_keys: "",
    }
}

/// Display order of the categories (palette, help overlay).
pub const CATEGORY_ORDER: [&str; 9] = [
    "Navigation",
    "Agents",
    "Grid",
    "Sidebar",
    "Chat",
    "Scroll",
    "Panels",
    "Actions",
    "General",
];

/// Every command. Ids of Hydra's `HydraCommandId` keep their name; the rest are TUI additions.
pub const COMMANDS: &[CommandDef] = &[
    // Navigation
    def(
        "toggle-view",
        "Toggle Chat / Grid View",
        "Navigation",
        true,
        true,
        "prefix g | prefix \\",
    ),
    def(
        "show-overview",
        "Show Overview",
        "Navigation",
        true,
        true,
        "prefix O",
    ),
    def(
        "cycle-view",
        "Cycle Chat / Grid / Overview",
        "Navigation",
        true,
        true,
        "prefix v",
    ),
    def(
        "switch-agent-by-index",
        "Switch to agent by index",
        "Navigation",
        false,
        true,
        "prefix 1-9",
    ),
    def(
        "next-agent",
        "Next Agent",
        "Navigation",
        true,
        true,
        "prefix j | prefix down",
    ),
    def(
        "prev-agent",
        "Previous Agent",
        "Navigation",
        true,
        true,
        "prefix k | prefix up",
    ),
    def(
        "focus-sidebar",
        "Focus Sidebar",
        "Navigation",
        true,
        true,
        "prefix s | grid tab",
    ),
    def(
        "focus-sidebar-search",
        "Focus Sidebar Search",
        "Navigation",
        true,
        true,
        "prefix / | app /",
    ),
    def(
        "focus-terminal",
        "Back to the Terminal / Grid",
        "Navigation",
        false,
        true,
        "app esc | app tab",
    ),
    // Agents
    def(
        "new-agent",
        "New Agent",
        "Agents",
        true,
        true,
        "prefix n | app n",
    ),
    def(
        "new-manager-agent",
        "New Manager Agent",
        "Agents",
        true,
        false,
        "",
    ),
    def(
        "resume-session",
        "Resume Session\u{2026}",
        "Agents",
        true,
        true,
        "prefix o | app o",
    ),
    def(
        "set-profile",
        "Claude Profile of the Project",
        "Agents",
        true,
        true,
        "prefix P | app P",
    ),
    def(
        "close-agent",
        "Stop Selected Agent",
        "Agents",
        true,
        true,
        "prefix x | app x",
    ),
    def(
        "restart-agent",
        "Restart Selected Agent",
        "Agents",
        true,
        true,
        "prefix R | app R",
    ),
    def(
        "remove-agent",
        "Remove Selected Agent",
        "Agents",
        true,
        true,
        "prefix d | prefix delete | app d | app delete",
    ),
    def(
        "rename-agent",
        "Rename Selected Agent",
        "Agents",
        true,
        true,
        "prefix r | app r",
    ),
    def(
        "toggle-yolo",
        "Toggle YOLO for Selected Agent",
        "Agents",
        true,
        true,
        "prefix y | app y",
    ),
    def(
        "toggle-global-yolo",
        "Toggle Global YOLO",
        "Agents",
        true,
        true,
        "prefix Y | app Y",
    ),
    // Grid
    def(
        "toggle-tile-expand",
        "Expand / Collapse Tile",
        "Grid",
        true,
        true,
        "prefix z | grid z",
    ),
    def(
        "broadcast",
        "Broadcast to the Agents of the Tab",
        "Grid",
        true,
        true,
        "prefix b | grid b",
    ),
    def(
        "grid-left",
        "Grid: Focus Tile on the Left",
        "Grid",
        false,
        true,
        "grid left | grid h",
    ),
    def(
        "grid-right",
        "Grid: Focus Tile on the Right",
        "Grid",
        false,
        true,
        "grid right | grid l",
    ),
    def(
        "grid-up",
        "Grid: Focus Tile Above",
        "Grid",
        false,
        true,
        "grid up | grid k",
    ),
    def(
        "grid-down",
        "Grid: Focus Tile Below",
        "Grid",
        false,
        true,
        "grid down | grid j",
    ),
    def(
        "grid-attach",
        "Grid: Type into the Focused Tile",
        "Grid",
        false,
        true,
        "grid enter",
    ),
    def(
        "grid-open-chat",
        "Open Focused Tile in Chat",
        "Grid",
        true,
        true,
        "grid c",
    ),
    def(
        "grid-move-prev",
        "Move Tile Earlier",
        "Grid",
        true,
        true,
        "prefix shift+left | grid <",
    ),
    def(
        "grid-move-next",
        "Move Tile Later",
        "Grid",
        true,
        true,
        "prefix shift+right | grid >",
    ),
    def(
        "grid-prev-tab",
        "Previous Project Tab",
        "Grid",
        true,
        true,
        "grid [",
    ),
    def(
        "grid-next-tab",
        "Next Project Tab",
        "Grid",
        true,
        true,
        "grid ]",
    ),
    def(
        "grid-cycle-columns",
        "Cycle Grid Columns (auto / 2 / 3)",
        "Grid",
        true,
        true,
        "grid C",
    ),
    def(
        "grid-only-running",
        "Toggle Running-only Tiles",
        "Grid",
        true,
        true,
        "grid u",
    ),
    // Sidebar
    def(
        "cursor-down",
        "Sidebar: Cursor Down",
        "Sidebar",
        false,
        true,
        "app down | app j",
    ),
    def(
        "cursor-up",
        "Sidebar: Cursor Up",
        "Sidebar",
        false,
        true,
        "app up | app k",
    ),
    def(
        "activate",
        "Sidebar: Attach Agent / Fold Project",
        "Sidebar",
        false,
        true,
        "app enter | app right | app l",
    ),
    def(
        "toggle-collapse",
        "Sidebar: Fold / Unfold Project",
        "Sidebar",
        false,
        true,
        "app space | app left | app h",
    ),
    def(
        "cycle-sidebar-status",
        "Cycle Sidebar Status Filter",
        "Sidebar",
        true,
        true,
        "app f",
    ),
    def(
        "cycle-sidebar-sort",
        "Cycle Sidebar Sort",
        "Sidebar",
        true,
        true,
        "app S",
    ),
    def(
        "sidebar-narrower",
        "Narrower Sidebar",
        "Sidebar",
        true,
        true,
        "prefix < | app <",
    ),
    def(
        "sidebar-wider",
        "Wider Sidebar",
        "Sidebar",
        true,
        true,
        "prefix > | app >",
    ),
    // Chat
    def(
        "toggle-chat-input",
        "Toggle Chat Input",
        "Chat",
        true,
        true,
        "prefix i | app i",
    ),
    def(
        "send-prefix",
        "Send the Prefix Key to the Agent",
        "Chat",
        false,
        true,
        "prefix prefix",
    ),
    // Scroll
    def(
        "scroll-mode",
        "Scroll Mode (scrollback)",
        "Scroll",
        true,
        true,
        "prefix [ | app [",
    ),
    def(
        "scroll-up",
        "Scroll Up a Line",
        "Scroll",
        false,
        true,
        "scroll up | scroll k",
    ),
    def(
        "scroll-down",
        "Scroll Down a Line",
        "Scroll",
        false,
        true,
        "scroll down | scroll j",
    ),
    def(
        "scroll-page-up",
        "Scroll Up a Page",
        "Scroll",
        false,
        true,
        "scroll pageup | scroll ctrl+b",
    ),
    def(
        "scroll-page-down",
        "Scroll Down a Page",
        "Scroll",
        false,
        true,
        "scroll pagedown | scroll ctrl+f",
    ),
    def(
        "scroll-top",
        "Oldest Output",
        "Scroll",
        false,
        true,
        "scroll g | scroll home",
    ),
    def(
        "scroll-bottom",
        "Latest Output",
        "Scroll",
        false,
        true,
        "scroll G | scroll end",
    ),
    def(
        "scroll-exit",
        "Leave Scroll Mode",
        "Scroll",
        false,
        true,
        "scroll esc | scroll q",
    ),
    // Panels
    def(
        "command-palette",
        "Open Command Palette",
        "Panels",
        false,
        true,
        "prefix p | prefix :",
    ),
    def(
        "show-shortcuts",
        "Show Keyboard Shortcuts",
        "Panels",
        true,
        true,
        "prefix ? | app ?",
    ),
    def(
        "settings",
        "Open Settings",
        "Panels",
        true,
        true,
        "prefix ,",
    ),
    def("git-panel", "Git Panel", "Panels", true, true, "prefix G"),
    def(
        "usage-dashboard",
        "Usage Report",
        "Panels",
        true,
        true,
        "prefix u",
    ),
    def("refresh-usage", "Refresh Usage", "Panels", true, true, ""),
    def(
        "toggle-usage-costs",
        "Show / Hide Usage Costs",
        "Panels",
        true,
        false,
        "",
    ),
    def(
        "toggle-usage-alerts",
        "Usage Alerts On / Off",
        "Panels",
        true,
        false,
        "",
    ),
    def(
        "updates",
        "Check for Updates",
        "Panels",
        true,
        true,
        "prefix U",
    ),
    def(
        "headless",
        "Headless Runs",
        "Panels",
        true,
        true,
        "prefix H",
    ),
    def("mcp-status", "MCP Server Status", "Panels", true, false, ""),
    later("remote-control", "Remote Control", "Panels", true, false),
    def(
        "toggle-editor",
        "Toggle Files Panel",
        "Panels",
        true,
        true,
        "prefix e",
    ),
    def(
        "toggle-terminal",
        "Toggle Terminal",
        "Panels",
        true,
        true,
        "prefix t",
    ),
    def(
        "terminal-new",
        "New Terminal",
        "Panels",
        true,
        true,
        "prefix T",
    ),
    def(
        "terminal-split",
        "Split Terminal Right",
        "Panels",
        true,
        true,
        "prefix |",
    ),
    def(
        "terminal-split-down",
        "Split Terminal Down",
        "Panels",
        true,
        true,
        "prefix -",
    ),
    def(
        "terminal-focus-left",
        "Focus Terminal Pane Left",
        "Panels",
        true,
        false,
        "prefix alt+left",
    ),
    def(
        "terminal-focus-right",
        "Focus Terminal Pane Right",
        "Panels",
        true,
        false,
        "prefix alt+right",
    ),
    def(
        "terminal-focus-up",
        "Focus Terminal Pane Up",
        "Panels",
        true,
        false,
        "prefix alt+up",
    ),
    def(
        "terminal-focus-down",
        "Focus Terminal Pane Down",
        "Panels",
        true,
        false,
        "prefix alt+down",
    ),
    def(
        "terminal-resize-left",
        "Resize Terminal Pane Left",
        "Panels",
        true,
        false,
        "prefix ctrl+left",
    ),
    def(
        "terminal-resize-right",
        "Resize Terminal Pane Right",
        "Panels",
        true,
        false,
        "prefix ctrl+right",
    ),
    def(
        "terminal-resize-up",
        "Resize Terminal Pane Up",
        "Panels",
        true,
        false,
        "prefix ctrl+up",
    ),
    def(
        "terminal-resize-down",
        "Resize Terminal Pane Down",
        "Panels",
        true,
        false,
        "prefix ctrl+down",
    ),
    def(
        "terminal-next-tab",
        "Next Terminal Tab",
        "Panels",
        true,
        true,
        "prefix ]",
    ),
    def(
        "terminal-prev-tab",
        "Previous Terminal Tab",
        "Panels",
        true,
        true,
        "prefix {",
    ),
    def(
        "terminal-next-pane",
        "Next Terminal Pane",
        "Panels",
        true,
        true,
        "prefix }",
    ),
    def(
        "terminal-rename",
        "Rename Terminal",
        "Panels",
        true,
        false,
        "",
    ),
    def(
        "terminal-fullscreen",
        "Terminal Panel: Bottom / Full Screen",
        "Panels",
        true,
        false,
        "",
    ),
    def(
        "terminal-kill",
        "Close Terminal Pane",
        "Panels",
        true,
        true,
        "prefix X",
    ),
    def(
        "file-search",
        "Search Files",
        "Panels",
        true,
        true,
        "prefix f",
    ),
    def(
        "open-in",
        "Open In Editor / File Manager",
        "Panels",
        true,
        true,
        "prefix I",
    ),
    def(
        "import-hydra",
        "Import From Hydra",
        "Actions",
        true,
        false,
        "",
    ),
    later("save-file", "Save File", "Panels", false, true),
    // Actions
    def(
        "open-keybindings-file",
        "Open Keybindings File",
        "Actions",
        true,
        false,
        "",
    ),
    def(
        "export-diagnostics",
        "Export Diagnostics",
        "Actions",
        true,
        false,
        "",
    ),
    // General
    def(
        "close-dialogs",
        "Dismiss Notifications / Return to the Grid",
        "General",
        false,
        true,
        "prefix esc",
    ),
    def("quit", "Quit", "General", true, true, "prefix q | app q"),
];

pub fn command_def(id: &str) -> Option<&'static CommandDef> {
    COMMANDS.iter().find(|c| c.id == id)
}

pub fn is_command_id(id: &str) -> bool {
    command_def(id).is_some()
}

/// Rank of a category for sorting (unknown ones last).
pub fn category_rank(category: &str) -> usize {
    CATEGORY_ORDER
        .iter()
        .position(|c| *c == category)
        .unwrap_or(CATEGORY_ORDER.len())
}

/// `{ "command": ..., "keys": ... }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeybindingRule {
    pub command: String,
    pub keys: String,
}

/// Defaults of every command that has keys.
pub fn default_keybindings() -> Vec<KeybindingRule> {
    COMMANDS
        .iter()
        .filter(|c| !c.default_keys.is_empty())
        .map(|c| KeybindingRule {
            command: c.id.to_string(),
            keys: c.default_keys.to_string(),
        })
        .collect()
}

/// Hydra's `mergeKeybindingsWithDefaults`: one rule per catalog command; an override (known command,
/// non-empty keys) replaces the default, the last override of a command wins. Commands without
/// default and without override are left out.
pub fn merge_with_defaults(rules: &[KeybindingRule]) -> Vec<KeybindingRule> {
    let mut overrides: Vec<(&str, &str)> = Vec::new();
    for rule in rules {
        let keys = rule.keys.trim();
        if keys.is_empty() || !is_command_id(&rule.command) {
            continue;
        }
        match overrides.iter_mut().find(|(c, _)| *c == rule.command) {
            Some(entry) => entry.1 = keys,
            None => overrides.push((rule.command.as_str(), keys)),
        }
    }
    COMMANDS
        .iter()
        .filter_map(|c| {
            let keys = overrides
                .iter()
                .find(|(id, _)| *id == c.id)
                .map_or(c.default_keys, |(_, k)| *k);
            (!keys.is_empty()).then(|| KeybindingRule {
                command: c.id.to_string(),
                keys: keys.to_string(),
            })
        })
        .collect()
}

/// The alternatives of a `keys` string; `none` yields no binding.
pub fn split_keys(keys: &str) -> Vec<&str> {
    let keys = keys.trim();
    if keys.eq_ignore_ascii_case("none") || keys.is_empty() {
        return Vec::new();
    }
    keys.split(" | ")
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .collect()
}

/// Result of reading the user's file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedKeybindings {
    /// Valid overrides (known command, non-empty keys).
    pub rules: Vec<KeybindingRule>,
    /// Human-readable problems: broken JSON, unknown commands, malformed entries.
    pub problems: Vec<String>,
}

/// Parse the contents of `keybindings.json` tolerantly: bad entries are reported and skipped.
pub fn parse_rules(text: &str) -> ParsedKeybindings {
    let mut out = ParsedKeybindings::default();
    if text.trim().is_empty() {
        return out;
    }
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(err) => {
            out.problems
                .push(format!("keybindings file is not valid JSON ({err})"));
            return out;
        }
    };
    let Some(items) = value.as_array() else {
        out.problems
            .push("keybindings file must be a JSON array".to_string());
        return out;
    };
    for (i, item) in items.iter().enumerate() {
        let command = item.get("command").and_then(|v| v.as_str());
        let keys = item.get("keys").and_then(|v| v.as_str()).map(str::trim);
        match (command, keys) {
            (Some(command), Some(keys)) if !keys.is_empty() => {
                if is_command_id(command) {
                    out.rules.push(KeybindingRule {
                        command: command.to_string(),
                        keys: keys.to_string(),
                    });
                } else {
                    out.problems.push(format!("unknown command \"{command}\""));
                }
            }
            _ => out
                .problems
                .push(format!("entry {} needs \"command\" and \"keys\"", i + 1)),
        }
    }
    out
}

/// What "Open keybindings file" writes when the file does not exist yet (Hydra writes the defaults).
pub fn default_file_contents() -> String {
    let mut text = serde_json::to_string_pretty(&default_keybindings()).unwrap_or_default();
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(command: &str, keys: &str) -> KeybindingRule {
        KeybindingRule {
            command: command.into(),
            keys: keys.into(),
        }
    }

    fn keys_of(rules: &[KeybindingRule], id: &str) -> Option<String> {
        rules
            .iter()
            .find(|r| r.command == id)
            .map(|r| r.keys.clone())
    }

    #[test]
    fn ids_are_unique_and_defaults_are_well_formed() {
        let mut ids: Vec<_> = COMMANDS.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate command id");
        for c in COMMANDS {
            assert!(
                CATEGORY_ORDER.contains(&c.category),
                "{} has an unknown category",
                c.id
            );
            for alt in split_keys(c.default_keys) {
                let scope = alt.split(' ').next().unwrap_or("");
                assert!(
                    ["prefix", "app", "grid", "scroll"].contains(&scope) || !alt.contains(' '),
                    "{}: bad default {alt}",
                    c.id
                );
            }
        }
    }

    #[test]
    fn hydra_commands_are_all_present() {
        for id in [
            "toggle-view",
            "show-overview",
            "switch-agent-by-index",
            "new-agent",
            "close-agent",
            "restart-agent",
            "toggle-yolo",
            "toggle-global-yolo",
            "toggle-editor",
            "toggle-terminal",
            "toggle-chat-input",
            "terminal-new",
            "terminal-split",
            "terminal-kill",
            "focus-sidebar-search",
            "show-shortcuts",
            "file-search",
            "save-file",
            "command-palette",
            "settings",
            "git-panel",
            "usage-dashboard",
            "updates",
            "close-dialogs",
            "headless",
            "remote-control",
            "export-diagnostics",
            "open-keybindings-file",
        ] {
            assert!(is_command_id(id), "{id}");
        }
        assert!(!is_command_id("nope"));
    }

    #[test]
    fn merges_partial_overrides_onto_defaults() {
        let merged = merge_with_defaults(&[rule("new-agent", "prefix N")]);
        assert_eq!(keys_of(&merged, "new-agent").as_deref(), Some("prefix N"));
        assert_eq!(
            keys_of(&merged, "toggle-view").as_deref(),
            Some("prefix g | prefix \\")
        );
    }

    #[test]
    fn merge_ignores_unknown_commands_and_empty_keys_and_last_wins() {
        let merged = merge_with_defaults(&[
            rule("nope", "ctrl+x"),
            rule("quit", "   "),
            rule("new-agent", "alt+n"),
            rule("new-agent", "alt+m"),
        ]);
        assert_eq!(keys_of(&merged, "nope"), None);
        assert_eq!(
            keys_of(&merged, "quit").as_deref(),
            Some("prefix q | app q")
        );
        assert_eq!(keys_of(&merged, "new-agent").as_deref(), Some("alt+m"));
    }

    #[test]
    fn commands_without_default_can_be_bound() {
        let merged = merge_with_defaults(&[rule("open-keybindings-file", "ctrl+k")]);
        assert_eq!(
            keys_of(&merged, "open-keybindings-file").as_deref(),
            Some("ctrl+k")
        );
        assert_eq!(
            keys_of(&merge_with_defaults(&[]), "open-keybindings-file"),
            None
        );
    }

    #[test]
    fn parses_the_file_tolerantly() {
        let parsed = parse_rules(
            r#"[{"command":"new-agent","keys":"prefix N"},
                {"command":"bogus","keys":"x"},
                {"command":"quit"},
                {"keys":"x"},
                7]"#,
        );
        assert_eq!(parsed.rules, vec![rule("new-agent", "prefix N")]);
        assert_eq!(parsed.problems.len(), 4);
        assert!(parsed.problems[0].contains("unknown command \"bogus\""));

        assert!(parse_rules("").problems.is_empty());
        assert!(parse_rules("{}").problems[0].contains("array"));
        assert!(parse_rules("[").problems[0].contains("JSON"));
        assert!(parse_rules("[]").rules.is_empty());
    }

    #[test]
    fn splits_alternatives_and_none() {
        assert_eq!(
            split_keys("prefix g | prefix \\"),
            ["prefix g", "prefix \\"]
        );
        assert_eq!(split_keys("alt+1-9"), ["alt+1-9"]);
        assert!(split_keys("none").is_empty());
        assert!(split_keys(" ").is_empty());
    }

    #[test]
    fn default_file_round_trips() {
        let parsed = parse_rules(&default_file_contents());
        assert!(parsed.problems.is_empty());
        assert_eq!(parsed.rules, default_keybindings());
    }
}
