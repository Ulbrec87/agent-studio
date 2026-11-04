# TUI key bindings

Prefix key: **`Ctrl+A`** (config `prefixKey`, e.g. `"ctrl+b"`). The tables are data
(`as-tui::keymap::Keymap`, `Vec<(KeyChord, Command)>`), so P5 can load `keybindings.json` over them.

## Focus

| Focus | Keys |
|-------|------|
| **Terminal** (default) | every key goes to the selected agent's PTY, except the prefix. Paste is forwarded (bracketed when the agent enabled it). `Shift+PgUp/PgDn` enter scroll mode. |
| **App** (sidebar) | `prefix s` to enter, `Esc`/`Tab` to leave |
| **Input** (input bar) | `prefix i` toggles the bar |
| **Scroll mode** | `prefix [`, mouse wheel (with `--mouse`) or `Shift+PgUp` |

The footer shows the current mode; after the prefix it shows `PREFIX` plus the available keys.

## After the prefix

| Key | Command |
|-----|---------|
| `a`, `Ctrl+A` (the prefix itself) | send a literal prefix key (Ctrl+A) to the agent |
| `n` | new agent dialog |
| `o` | resume session… (session picker, see below) |
| `P` | Claude profile of the project under the cursor / of the selected agent |
| `x` | kill (stop the process, agent stays in the list) |
| `R` | restart |
| `d`, `Del` | remove (asks for confirmation) |
| `r` | rename |
| `y` | toggle YOLO for the agent |
| `Y` | toggle global YOLO |
| `j`, `Down` / `k`, `Up` | next / previous agent (wraps) |
| `1`-`9` | jump to the n-th agent in sidebar order |
| `s` | focus the sidebar |
| `[` | scroll mode |
| `i` | toggle the input bar |
| `<` / `>` | narrower / wider sidebar |
| `Esc` | dismiss toasts |
| `?` | help overlay (generated from the table) |
| `q` | quit |

## Sidebar focus

`j`/`k`/arrows move the cursor over projects and agents (landing on an agent selects it) ·
`Enter`/`l`/`Right` attach (terminal focus) or fold a project · `Space`/`h`/`Left` fold/unfold ·
`/` filter (type, `Enter` keeps it, `Esc` clears) · `n o P x R d r y Y [ i ? q < >` work without the prefix ·
`Esc`/`Tab` back to the terminal.

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
