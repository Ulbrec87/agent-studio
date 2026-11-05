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

The footer shows the current mode; after the prefix it shows `PREFIX` plus the available keys. The key hints are generated from the
effective bindings (defaults + `keybindings.json` + the configured prefix), so an override shows up at once; a command you unbound
(`none`) is left out of the hint line.

## Views

Header tabs: **Chat** (one agent), **Grid** (tiles per project), **Overview** (cards, see below; an amber badge with the
number of agents that need you shows next to the tab in every view, and the terminal title becomes `(2) Agent Studio`). The saved view is
restored at start (`tui-state.json`); when `defaultViewMode` in the config changed since the state was saved, the config
wins. `prefix g` / `prefix \` toggle Chat/Grid (Hydra `mod+\`), `prefix O` shows the Overview (again: back to Chat),
`prefix v` cycles Chat -> Grid -> Overview. (`prefix o` stays "resume session".)

## After the prefix (defaults)

| Key | Command id | Action |
|-----|------------|--------|
| `a`, `Ctrl+A` (the prefix itself) | `send-prefix` | send a literal prefix key (Ctrl+A) to the agent |
| `g`, `\` | `toggle-view` | Chat <-> Grid |
| `O` | `show-overview` | Overview |
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
| `u` | `usage-dashboard` | usage report overlay (see below) |
| `?` | `show-shortcuts` | help overlay |
| `G` | `git-panel` | git panel (see below) |
| `e` | `toggle-editor` | files panel |
| `f` | `file-search` | fuzzy file search (opens the files panel) |
| `t` | `toggle-terminal` | show / hide the free terminals (opens a first shell) |
| `T` | `terminal-new` | new terminal tab |
| `\|` | `terminal-split` | split the active pane to the right: another shell side by side |
| `-` | `terminal-split-down` | split the active pane downwards: another shell below |
| `Alt+Left/Right/Up/Down` | `terminal-focus-left/right/up/down` | focus the neighbouring pane in that direction (palette entries too) |
| `Ctrl+Left/Right/Up/Down` | `terminal-resize-left/right/up/down` | move the divider next to the active pane by 5 % in that direction |
| `]` / `{` | `terminal-next-tab` / `terminal-prev-tab` | next / previous terminal tab |
| `}` | `terminal-next-pane` | next pane of the tab |
| `X` | `terminal-kill` | close the active pane (kill its shell) |
| *(palette)* | `terminal-rename`, `terminal-fullscreen` | rename the active terminal; panel bottom / full screen |
| `U` | `updates` | check for updates now (banner + toast; see *Updates* below) |
| `,` | `settings` | settings |
| `I` | `open-in` | open the project in an editor / file manager / terminal |
| *(palette)* | `import-hydra`, `export-diagnostics` | import from Hydra (also CLI `import-hydra`); write a diagnostics folder |
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

## Overview view

One scrollable page of cards (Hydra's Overview, modeled on agent-lookout): two columns when the main area is >= 110
columns wide (Counts on top, then Needs you | Last hour, Sessions | Events, Timeline, Waits), one column otherwise.
The focused card has a thick accent border; the page scrolls to keep it in view. The sidebar is still there
(`prefix s`); `Esc`/`Tab` in the sidebar come back to the cards.

| Key (Overview focus) | Action |
|----------------------|--------|
| `Tab` / `Shift+Tab` | next / previous card (Needs you, Sessions, Events, Last hour, Timeline, Waits) |
| `j`/`k`, arrows | move the cursor (or scroll) inside the focused card |
| `g`/`G`, `Home`/`End` | first / last item of the card |
| `PgUp`/`PgDn` | scroll the page |
| `Enter` | **Needs you**: **Jump** (select the agent, switch to Chat, keys go to its terminal) · **Sessions**/**Events**: open the details drawer (on a "Show N more" row: expand/collapse the group) |
| `d`, `Space` | open the details drawer of the item under the cursor |
| `m` | Sessions layout: List -> Repos -> Board |
| `p` | project filter: All -> each project -> All (applies to every card) |
| `t` | Waits: longest waiters today / 7 days |

**Details drawer** (right side): status + since, reason and what it asks, agent, folder, repo/branch, start time, PID,
tokens of the newest reply, waits in the last hour, last message (read from the transcript every 2 s while open), own
events and own timeline row. `Enter`/`J` jump, `j`/`k`/`PgUp`/`PgDn` scroll, `Esc`/`q` close.

Cards: **Counts** (working, idle, stale, needs you, finished, failed, each with a last-hour sparkline) · **Needs you**
(longest wait first: name, reason, project/branch, what it asks, live timer; plus the "waited on you" bars of the last
hour) · **Last hour** (12 bars of 5 min: needs-you amber / working green / idle grey, `░` = not measured) · **Sessions**
(grouped Working, Idle, Finished/Failed newest first, Not running; "quiet for 12m", "ended 3h ago · exit 1", groups of
ended sessions show 10 and "Show N more") · **Events** (newest first, `── new since 14:02 ──` marks what happened while
you were elsewhere; it goes away after 10 s in view) · **Timeline** (one row per session, each cell a slice of the last
hour) · **Waits** (today / 7 days totals, by-day bars, five longest waiters). Time is shown in local time. History and
the wait ledger are refreshed when the view opens and every 30 s. The **Usage & limits** card is on top (once the daemon reported usage): plan limits left, tokens of Today/Week/Month with a daily bar chart right; `Tab` reaches it, `t` cycles its period, `Enter` / `u` open the report. It follows the project filter (`p`) for tokens; limits are per account.

## Command palette (`prefix p`, `prefix :`)

Fuzzy list (nucleo) of every palette command with its key hint, `Switch view: Chat/Grid/Overview` and
`Switch to agent: <name>` for every agent (project shown on the right). Type to filter, `Up`/`Down` (`Ctrl+N`/`Ctrl+P`),
`PgUp`/`PgDn` move, `Enter` runs, `Esc` closes.

## Usage chip and report (`prefix u`)

The header chip reads `● 5h 42% · wk 18%` (the most used session and weekly window, green < 50 %, amber < 80 %, red from there,
bold when high, dimmed when older than 15 min; the dot is the provider's color, Claude terracotta in every theme). Without limits it
shows `12.3M today`, before any data `Usage`; it is left out when the header is too narrow. `usage.chipProvider` picks the provider.

`prefix u` (command `usage-dashboard`, also in the palette) opens the overlay: **Plan limits** per account with reset times and
bars (`f` All / Low = only windows over their alert level), **Tokens** Today / Week / Month per provider (+ API-equivalent cost and
change vs the previous period), **Report** with `Claude | Codex | Both` (`1`/`2`/`3`, `Tab`, arrows), `d` 7 d / 30 d daily trend
(cost, or tokens when costs are hidden) and per-model / per-project tables. `j/k`, `PgUp/PgDn`, `g/G` scroll, `r` refreshes (scan +
limits poll), `Esc`/`q`/`u` close. Palette: *Usage Report*, *Refresh Usage*, *Show / Hide Usage Costs*, *Usage Alerts On / Off*.
`usage.showCost=false` hides every dollar amount (chip, card, overlay, notifications). While the overlay or the Overview is shown
the TUI tells the daemon (`UsageVisible`), which then polls the plan limits every 60 s instead of every 5 min.

## Headless runs (`prefix H`) and manager agents

`prefix H` (command `headless`, palette *Headless Runs*) opens the panel: one row per run, newest first: status glyph
(`◐` running, `✔` completed, `✖` errored, `⊘` canceled), age, `provider/model`, project folder, prompt excerpt.
List keys: `j/k`, `PgUp/PgDn`, `g/G` move, `/` search (id, prompt, project, model, provider, status, session id, error; `Enter` keeps,
`Esc` clears), `s`/`Tab` cycle the status chip (all / running / completed / errored / canceled), `n` new run, `Enter` log, `x` cancel
the run (running ones only), `r` refresh, `Esc`/`q` close.
`n` opens the *New headless run* dialog (the New agent dialog without name, YOLO and manager rows): directory (Tab completes),
provider, model, reasoning effort (Codex), *Resume* (the session picker of the New agent dialog; fills directory and provider) and a
required multi-line prompt (`Alt+Enter` newline, `Enter` starts, `Esc` goes back to the list). Starting opens the log.
Log viewer: Claude's `stream-json` is rendered as assistant text, one line per tool call (`⚙ Bash cargo test`), one line per tool
result (`↳ first line (+N lines)`), stderr lines and a summary (`✔ done · 3 turns · 8.2s · $0.0421`); other CLIs print plain text.
`r` toggles the raw lines, `j/k`/`PgUp/PgDn`/`g` scroll, `G`/`End`/`f` follow the end again (the viewer follows while the run
writes; scrolling up keeps your place), `R` reloads, `x` cancels, `Esc`/`Backspace` back to the list. The daemon notifies when a run
completes or fails (toast + OS notification when unfocused).

*New Manager Agent* (palette) opens the New agent dialog with the **Manager** row on; the row is also reachable in the normal
dialog (`Space`). A manager runs Claude in the daemon's manager workspace (no directory, provider or resume to choose) with the
`studio_*` MCP tools, is pinned at the top of the sidebar under `★ manager`, and gets *"What can you do as the Agent Studio Manager
agent? ..."* as first message unless you type another. The toggle is off (with the reason) when the MCP server is not running.
*MCP Server Status* (palette) shows the port and workspace as a toast; the help overlay (`prefix ?`) ends with the same line.

## Help overlay (`prefix ?`)

Generated from the keymap, grouped by category (Navigation, Agents, Grid, Sidebar, Chat, Scroll, Panels, Actions,
General), so user overrides show up; unbound commands read `(unbound)`. `j`/`k`/arrows, `PgUp`/`PgDn`, `g`/`G` scroll,
`Esc`/`q`/`?` close.

## Prefix inside panels

The panel modals (help, usage, git, files, settings, open in, headless) take every key, but let the **prefix** through for four global
commands: `prefix t` (toggle the terminals panel), `prefix p` / `prefix :` (command palette), `prefix ?` (help) and `prefix Esc`
(`close-dialogs`: close the panel). The panel closes (or is replaced by the palette / help) and the command runs; any other key after
the prefix is dropped (the footer shows `PREFIX` with these four keys while it waits). The forms (new agent, rename, resume, profile,
palette, quit, confirmations) and panels that currently take text (a git commit/PR prompt, the file search, a Settings value, the headless
search) keep `Ctrl+A` for themselves, as text editing keys do. Choice: not every global command, because most of them would act on the
view *behind* the panel (kill an agent, switch the view) while its state is hidden.

## Mouse

Off by default, because capturing the mouse takes native text selection away from the terminal (hold `Shift`, or `Option` in some macOS
terminals, while dragging to select text; most terminals also copy with that). Turn it on with `"mouse": true` in `config.json` (Settings >
General, applies at the next start), `agent-studio --mouse` or `AGENT_STUDIO_MOUSE=1`. With the mouse captured:

| Where | Click / wheel |
|-------|---------------|
| header tabs `Chat` `Grid` `Overview` | switch the view |
| sidebar row | select the agent and type into it / fold a project |
| Grid tab strip, tile header, tile body | switch tab / open the agent in Chat / type into the tile |
| Chat pane | focus the terminal; the wheel scrolls the scrollback (3 lines per notch, enters scroll mode) |

Modals own the screen: clicks outside them do nothing.

## Updates

The daemon asks GitHub Releases for a newer version 10 s after it starts and every 6 h (`checkForUpdates`, default on; Settings >
General). A newer stable release shows a banner at the bottom right: `Update available: X.Y.Z` with how to upgrade for the way you
installed (`brew upgrade agent-studio`, `cargo install --git ... --locked --force`, or the download URL of the archive for your OS and
CPU). `prefix Esc` dismisses it for that version (remembered). `prefix U` / palette *Check for Updates* asks now and answers with a toast
(also when automatic checks are off), Settings > About shows the state (`Enter`/`c` there checks too). The binary never replaces itself.
CLI: `agent-studio update-check`.

## Keybindings file

`<data root>/keybindings.json`, shape as in Hydra: `[{ "command": "<id>", "keys": "<keys>" }]`. An entry replaces **all**
default keys of that command (merge like Hydra's `mergeKeybindingsWithDefaults`; the last entry of a command wins). The
TUI polls the file every second (and after "Open Keybindings File"), rebuilds the keymap and shows problems as toasts:
invalid JSON, unknown command ids, unparsable keys (the command keeps its default) and conflicts (the user's binding wins,
the other command loses the key). Commands the TUI does not implement (`remote-control`, `save-file`) are accepted and ignored.

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

## Git panel (`prefix G`; `gitPanelDisplayMode`: `overlay` = centered, `split` = right 60 %)

Works on the selected agent's folder (the worktree for worktree agents). Tabs: `Tab`/`Shift+Tab` or `1` `2` `3`.
The title shows branch, ahead/behind and the diff stats. A missing `git`/`gh` or a folder that is not a repository is shown as a message.

| Tab | Keys |
|-----|------|
| **Changes** | `j`/`k` file (the diff follows) · `Enter`/`l` focus the diff · `Space` stage / unstage · `a` stage all · `c` commit message (Enter commits the staged files) · `P` push (asks `y`/`n`) · `n`/`N` next / previous hunk · `s` unified / side by side · `r` refresh · `q` close |
| diff focus | `j`/`k`, `PgUp`/`PgDn`, `g`/`G` scroll · `n`/`N` hunk · `s` unified / side by side · `h`/`Esc` back to the list |
| **Branches** | `j`/`k` · `Enter` checkout (asks first when the work tree is dirty) · `n` new branch |
| **PR review** | `/` PR number or URL (needs `gh`, read-only) · `j`/`k` file · `Enter` diff of the file · `s` unified / side by side |

Markers are `git status --short` style (`M `, ` M`, `MM`, `A `, `??`). The diff is colored: hunk headers (accent), added / removed lines on
a green / red background (theme colors `diff_add_bg` / `diff_del_bg`), line numbers in the gutter, and the code syntax-highlighted by the file's
extension (needs the `highlight` build feature; unknown languages stay plain).

**Layouts** (`diffLayout` in the config, Settings > Panels > *Diff layout*; `s` toggles it and saves the choice): *unified* is one column
with both line numbers; *split* shows the old file on the left and the new one on the right, aligned per hunk (a run of removed lines meets
the run of added lines, the shorter side is padded), long lines end in `…`. Side by side needs a git panel body of at least **100 columns**;
on a narrower panel the unified diff is shown and the status row says so. Scrolling and `n`/`N` hunk jumps work in both layouts and the top row
stays in view when you switch.

## Files panel (`prefix e`; `prefix f` opens it with the search popup; `editorPanelDisplayMode`)

Tree on the left, preview with line numbers on the right (syntax-highlighted by extension when the build has the `highlight` feature: only the
visible lines plus a margin are highlighted, cached per file and mtime, and files over 512 KB stay plain; binary and too-large files show a
placeholder; the open file is re-read when its mtime changes, polled every second). `j`/`k` move · `Enter`/`l` expand / open · `h` collapse / parent · `.` hidden files ·
`f` or `/` search (type, `Up`/`Down`, `Enter` opens) · `Tab` focus the preview (`j`/`k`/`PgUp`/`PgDn` scroll) · `E` open in `$VISUAL`/`$EDITOR`
(the TUI suspends) · `y` copy the absolute path (OSC 52) · `o` open-in picker · `q`/`Esc` close.

## Free terminals (`prefix t`)

A panel with a tab row (one tab per group, `|2` marks a tab with several panes) and the shells below. With the panel open **every key goes to the shell**;
use the prefix for app commands (`t` hides the panel, shells keep running; they are per project: the project of the selected agent).
`T` new tab · `|` split right · `-` split down · `]` `{` tabs · `}` next pane · `X` close the pane (the tab disappears when its last shell ends) · `Shift+PgUp/PgDn` scroll the shell's history.

Panes form a **split tree**: any pane can be split again in either direction, so `a | (b / c)` is two splits. A new pane starts in the working directory of the
pane it splits. **Focus by direction**: `prefix Alt+Left/Right/Up/Down` (the nearest pane that lies beyond the active one and shares rows / columns with it).
**Resize**: `prefix Ctrl+Left/Right/Up/Down` moves the divider next to the active pane by 5 % towards the key (on the nearest split of that axis; limits 10 %-90 %).
The dividers around the active pane are drawn in the accent color. (`Alt`/`Ctrl` + arrows are free after the prefix; `prefix Up/Down` stay "next / previous agent".
If your terminal swallows those chords, rebind the `terminal-focus-*` / `terminal-resize-*` ids in `keybindings.json`, or use the palette.)

**The layout survives a daemon restart**: the tabs, split trees, pane labels and folders are saved in `terminals.json`. The shells are *not* running any more; the
first time you open a project's terminal panel after a restart, fresh shells are started in the saved folders and the layout reappears (at most
`freeTerminalMaxCount` shells in total).

## Settings (`prefix ,`)

Sections on the left (`j`/`k`, `Enter`/`l` to enter, `Esc`/`h` back): General, Terminal / shell, Panels, Usage, Claude profiles (read-only),
Skills, Keybindings, Data & diagnostics, About. Values: `Space`/`Left`/`Right` toggle or cycle, `+`/`-` step numbers, `Enter` edits text
and numbers (validated; the daemon's answer is compared with what was sent and a clamped value is reported). Every change is an `UpdateConfig`
patch applied at once. **Skills**: `Tab` switches Claude / Codex, `Space` toggles, `r` rescans. **Keybindings**: effective bindings,
`Enter` opens `keybindings.json` in `$EDITOR`. **Data**: *Import from Hydra* (preview, `Enter` confirms) and *Export diagnostics*.

## Open in (`prefix I`, `o` in the files panel)

Lists the editors found on this machine (VS Code, Cursor, Windsurf, Antigravity, Zed), `$EDITOR` (runs in the TUI's terminal), the file
manager and a terminal window. `Enter` opens the project folder (or the file).
