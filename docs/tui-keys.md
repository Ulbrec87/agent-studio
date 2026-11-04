# TUI key bindings

Prefix key: **`Ctrl+A`** (config `prefixKey`, e.g. `"ctrl+b"`). Every binding is data: the command catalog
(`as_core::keybindings`, ids and labels of Hydra's `shared/keybindings.ts` plus TUI additions) holds the default keys,
`keybindings.json` overrides them per command, and `prefix ?` is generated from the result.

## Focus

| Focus | Keys |
|-------|------|
| **Terminal** (default in Chat; after `Enter` on a tile in Grid) | every key goes to the selected agent's PTY, except the prefix. Paste is forwarded (bracketed when the agent enabled it). `Shift+PgUp/PgDn` enter scroll mode. |
| **Sidebar** (`App` in the code) | `prefix s` to enter, `Esc`/`Tab` to leave |
| **Grid** (tile navigation, Grid view) | the default focus of the Grid view; `Tab` goes to the sidebar |
| **Broadcast** | the bar at the bottom of the Grid (`prefix b`) |
| **Input** (chat input bar) | `prefix i` toggles the bar |
| **Scroll mode** | `prefix [`, mouse wheel (with `--mouse`) or `Shift+PgUp` (Chat view only) |

The footer shows the current mode; after the prefix it shows `PREFIX` plus the available keys.

## Views

Header tabs: **Chat** (one agent), **Grid** (tiles per project), **Overview** (placeholder until P6). The saved view is
restored at start (`tui-state.json`); when `defaultViewMode` in the config changed since the state was saved, the config
wins. `prefix g` / `prefix \` toggle Chat/Grid (Hydra `mod+\`), `prefix O` shows the Overview (again: back to Chat),
`prefix v` cycles Chat -> Grid -> Overview. (`prefix o` stays "resume session".)

## After the prefix (defaults)

| Key | Command id | Action |
|-----|------------|--------|
| `a`, `Ctrl+A` (the prefix itself) | `send-prefix` | send a literal prefix key (Ctrl+A) to the agent |
| `g`, `\` | `toggle-view` | Chat <-> Grid |
| `O` | `show-overview` | Overview (placeholder) |
| `v` | `cycle-view` | next view |
| `p`, `:` | `command-palette` | command palette |
| `b` | `broadcast` | Grid: broadcast bar (opens the Grid first from Chat) |
| `z` | `toggle-tile-expand` | Grid: expand / collapse the focused tile |
| `Shift+Left` / `Shift+Right` | `grid-move-prev` / `grid-move-next` | Grid: move the focused tile earlier / later |
| `Esc` | `close-dialogs` | dismiss toasts; in the Grid also back to tile navigation from a tile |
| `n` | `new-agent` | new agent dialog |
| `o` | `resume-session` | resume session… (session picker, see below) |
| `P` | `set-profile` | Claude profile of the project under the cursor / of the selected agent |
| `x` | `close-agent` | kill (stop the process, agent stays in the list) |
| `R` | `restart-agent` | restart |
| `d`, `Del` | `remove-agent` | remove (asks for confirmation) |
| `r` | `rename-agent` | rename |
| `y` / `Y` | `toggle-yolo` / `toggle-global-yolo` | YOLO for the agent / global |
| `j`, `Down` / `k`, `Up` | `next-agent` / `prev-agent` | next / previous agent (wraps) |
| `1`-`9` | `switch-agent-by-index` | jump to the n-th agent in sidebar order |
| `s` | `focus-sidebar` | focus the sidebar |
| `/` | `focus-sidebar-search` | focus the sidebar search field |
| `[` | `scroll-mode` | scroll mode (Chat view) |
| `i` | `toggle-chat-input` | toggle the input bar |
| `<` / `>` | `sidebar-narrower` / `sidebar-wider` | sidebar width |
| `?` | `show-shortcuts` | help overlay |
| `q` | `quit` | quit |

## Sidebar focus

`j`/`k`/arrows move the cursor over projects and agents (landing on an agent selects it) ·
`Enter`/`l`/`Right` attach (terminal focus) or fold a project · `Space`/`h`/`Left` fold/unfold ·
`/` search (type, `Enter` keeps it, `Esc` clears; matches name, project, provider, model and status) ·
`f` cycles the **status chip** (All -> Running -> Waiting -> Idle -> Errored; *Running* includes the waiting ones) ·
`S` cycles the **sort** (created, the default and stable one -> recent activity -> name -> status -> provider) ·
`n o P x R d r y Y [ i ? q < >` work without the prefix · `Esc`/`Tab` back to the terminal (the grid in the Grid view).
Manager agents stay pinned on top under every sort and filter. Chips and sort persist in `tui-state.json`; the search
text does not.

## Grid view

Tabs across the top: **Running** (the running agents of every project, newest start first) and one tab per project
(`name count`); the tab strip scrolls to keep the active tab visible. Tiles follow `gridColumns` (`auto` fits as many
40-column tiles as the width allows, a lone last tile of a two-column grid stretches over the row; `2`/`3` fall back to
fewer columns on narrow screens). Rows that do not fit are paged: the grid scrolls by whole rows to keep the focus
visible and subscribes to at most 10 tiles. Agents that are not running and were created before `sessionMaxAgeDays`
are left out (like Hydra).

A tile is a bordered box: header = status glyph, name, `provider · model`, `YOLO`, project name (Running tab); the
body is the agent's live screen. **The PTY is never resized to the tile** (that would reflow Claude's UI in the Chat
view): the tile shows the bottom-most window of the agent's screen (down to the last line with content or the cursor)
starting at the left edge; use `z` or `c` to see more. Border colours: accent = focused, green = you are typing into it,
yellow = waiting, yolo colour = YOLO.

| Key (Grid focus) | Command id | Action |
|------------------|------------|--------|
| `h j k l`, arrows | `grid-left/right/up/down` | move the tile focus (selects that agent) |
| `Enter` | `grid-attach` | type into the focused tile (Terminal focus: keys go to that agent; `prefix Esc` returns) |
| `c` | `grid-open-chat` | open the agent in the Chat view (mouse: click the tile header) |
| `z` | `toggle-tile-expand` | expand to the whole grid area / collapse (persisted) |
| `<` / `>` | `grid-move-prev/next` | move the tile earlier / later (persisted per project; not on the Running tab) |
| `[` / `]` | `grid-prev-tab` / `grid-next-tab` | previous / next tab |
| `u` | `grid-only-running` | project tabs: only running agents (Hydra "Running only") |
| `C` | `grid-cycle-columns` | auto -> 2 -> 3 (stored in the config) |
| `b` | `broadcast` | open the broadcast bar |
| `Tab` | `focus-sidebar` | go to the sidebar |

Mouse (`--mouse`): click a tab to switch, a tile **header** to open that agent in Chat, a tile body to type into it.

### Broadcast bar

`prefix b` / `b`: type, `Enter` sends the text to every **running** agent of the current tab (the Running tab: every
running agent) with the submit-queue semantics of `SendInput`, then shows "Sent to N agents"; the bar stays open.
`Alt+Enter` newline · `Esc` closes. Idle agents are not resumed (the daemon's `Broadcast` request would do that).

## Command palette (`prefix p`, `prefix :`)

Fuzzy list (nucleo) of every palette command with its key hint, `Switch view: Chat/Grid/Overview` and
`Switch to agent: <name>` for every agent (project shown on the right). Type to filter, `Up`/`Down` (`Ctrl+N`/`Ctrl+P`),
`PgUp`/`PgDn` move, `Enter` runs, `Esc` closes.

## Help overlay (`prefix ?`)

Generated from the keymap, grouped by category (Navigation, Agents, Grid, Sidebar, Chat, Scroll, Panels, Actions,
General), so user overrides show up; unbound commands read `(unbound)`. `j`/`k`/arrows, `PgUp`/`PgDn`, `g`/`G` scroll,
`Esc`/`q`/`?` close.

## Keybindings file

`<data root>/keybindings.json`, shape as in Hydra: `[{ "command": "<id>", "keys": "<keys>" }]`. An entry replaces **all**
default keys of that command (merge like Hydra's `mergeKeybindingsWithDefaults`; the last entry of a command wins). The
TUI polls the file every second (and after "Open Keybindings File"), rebuilds the keymap and shows problems as toasts:
invalid JSON, unknown command ids, unparsable keys (the command keeps its default) and conflicts (the user's binding wins,
the other command loses the key). Commands of later phases (`settings`, `git-panel`, ...) are accepted and ignored.

`keys` = one or more bindings joined by `" | "`; each binding is:

| Syntax | Meaning |
|--------|---------|
| `prefix g` | after the prefix key, in every focus |
| `app j` | sidebar focus, no prefix · `grid h` Grid focus · `scroll k` scroll mode |
| `ctrl+g`, `alt+1`, `f5` | a *direct* chord: runs in every focus (not sent to the agent). Cannot be the prefix key |
| `1-9` ranges | only for `switch-agent-by-index`: `prefix 1-9`, `alt+1-9` |
| `prefix prefix` | the prefix key itself (`send-prefix`) |
| `none` | unbind the command |

Chord grammar: `[ctrl+][alt+][shift+]key`, key = one character or `esc enter tab space backspace delete insert up down
left right home end pageup pagedown f1..f12`; `Shift+letter` is the upper-case letter (`prefix R`). Example:

```json
[
  { "command": "toggle-view", "keys": "alt+g | prefix g" },
  { "command": "switch-agent-by-index", "keys": "alt+1-9" },
  { "command": "quit", "keys": "none" }
]
```

The palette command **Open Keybindings File** creates the file with all defaults when missing and opens it in `$VISUAL`
/ `$EDITOR` (`vi`/`notepad` otherwise): the TUI leaves the alternate screen and raw mode, runs the editor, and restores
itself (`as_tui::suspend`, reusable for P9).

## UI state (`tui-state.json`)

Last view, selected agent, sidebar width, sidebar sort and status chip, grid tab, expanded tile, running-only toggle and
the tile order per project; written by the TUI at most once a second when it changed (and on exit), loaded tolerantly.

## Scroll mode

`Up`/`k`, `Down`/`j` line · `PgUp`/`Ctrl+B`, `PgDn`/`Ctrl+F` page · `g`/`Home` oldest · `G`/`End` live ·
`Esc`/`q` leave. 5000 lines of scrollback per agent. Typing in terminal focus snaps back to the live screen.

## Input bar

`Enter` sends the text with submit-queue semantics (`SendInput`) · `Alt+Enter`/`Shift+Enter`/`Ctrl+J` newline ·
`Esc` back to the terminal (the bar stays open; `prefix i` hides it).

## New agent dialog

`Tab`/`Down` next field · `Shift+Tab`/`Up` previous · `Left`/`Right` (or `Space`) change provider, model, reasoning
effort (Codex) · typing in *Model* switches to a free-text id (`Ctrl+K` goes back to the list) · `Tab` in
*Directory* completes directory names · `Space` toggles YOLO · `Alt+Enter` newline in the prompt ·
`Enter` (or `Ctrl+S`) create · `Esc` cancel. The directory must exist; creation errors (missing CLI, max agents) stay
in the dialog.

## Resume session picker (`prefix o`, or *Resume* in the New agent dialog)

Lists the sessions of the provider (Claude and Codex; OpenCode keeps no session files), newest first, with
**age · project · first prompt · messages · branch**. Hidden sessions are left out unless `s`; sessions without prompt and messages are never listed. List mode:
`j`/`k`/arrows, `PgUp`/`PgDn`, `g`/`G` move · `Enter` resumes the selected session (standalone: creates the agent in its
project with its provider; from the dialog: fills directory + provider and returns to the dialog) · `/` fuzzy filter over
project + prompt (type, `Enter` keeps it, `Esc` clears; a new query starts at the best match) · `a` all ages / last
`sessionMaxAgeDays` · `p` this project only (the selected agent's / the dialog's directory) · `s` show hidden ·
`h` hide/unhide the session (`hiddenSessionIds`) · `r` rescan · `Tab` switch Claude/Codex (standalone only) ·
`Esc`/`q` clear the filter, then close.

New agent dialog extras: **Resume** row (`Space`/`o`/`Right` open the picker, `Backspace` clears it; picking a session of
another provider switches the provider) and a **Profile** row, shown only for Claude with more than one profile: it defaults to
the project's mapped profile and, when changed, maps the project (`claudeProfileByProject`) before the agent starts. A
resumed session always uses the profile that owns it. The model list comes from the daemon (`ListProviderModels`).

`prefix P` (or `P` in the sidebar) opens the profile chooser of a project: *no mapping* / each profile; project rows
show a `[label]` chip when the project is mapped to a non-default profile.

## Quit

`prefix q`: nothing running -> leave at once (the daemon stays up, it is cheap). Agents running ->
dialog `Detach` (leave, agents keep running) · `Stop all & quit` (`Shutdown { stop_agents: true }`) · `Cancel`;
`d` / `s` / `c` pick directly, arrows + `Enter` too.

## Passing keys through

All keys are encoded like xterm (`as-tui::term::keys`): Ctrl+letter control bytes, Alt = ESC prefix,
modified arrows `ESC[1;5A`, `Shift+Enter` = LF, `Shift+Tab` = `ESC[Z`, F1-F12, application cursor mode.
