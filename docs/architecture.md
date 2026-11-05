# Architecture

## Processes

```
 ┌──────────── agent-studio (TUI client) ────────────┐   local socket    ┌──────── agent-studio daemon ────────┐
 │ ratatui views · keymap · vt100 emulator per agent │◄────────────────►│ owns every PTY (portable-pty)        │
 │ connects; spawns the daemon if it isn't running   │ framed JSON      │ AgentManager · Config · Workspace    │
 │ quitting = detach, or stop agents + daemon        │ req/resp+events  │ Sessions · Overview · Usage · MCP    │
 └───────────────────────────────────────────────────┘                  │ Headless · Notifications · Git       │
                                                                         └──────────────────────────────────────┘
 agent-studio ls | send | kill …  → one-shot clients of the same socket (scripting)
```

* The daemon is the **only** owner of state. The TUI holds view state only (selection, focus, scroll)
  plus a local terminal emulator per attached agent, fed by output events.
* The daemon keeps running after the TUI quits (detach). It stops when asked (`Shutdown`, or quitting the
  TUI with "stop agents").
* Single instance: `daemon.lock` (pid + version + socket name) in the data root. A client that finds a
  stale lock (dead pid) or a version mismatch restarts the daemon (Hydra: `electron/daemon/lifecycle.ts`).

## Transport

* `interprocess` local sockets (tokio): Unix domain socket `<root>/daemon.sock`; Windows named pipe
  `\\.\pipe\agent-studio-<hash(root)>` (`as_core::paths::Paths::daemon_socket_name`).
* Frames: `tokio_util::codec::LengthDelimitedCodec` (u32 BE length, max frame 16 MB) carrying one JSON value each.
* Messages (`as-proto`, adjacently tagged `{"type": "...", "payload": ...}`, snake_case names; `Vec<u8>` fields are base64):
  * client -> daemon: `ClientMsg::Request { id: u64, req: Request }`
  * daemon -> client: `ServerMsg::Response { id, result: Result<Response, String> }` and
    `ServerMsg::Event(Event)` (pushed, no id). A frame that does not parse gets a `Response` with the request's id
    (0 when unknown) and an error string; the connection stays open (an oversized/broken frame closes it).
  * `Request` (P2): `Hello`, `Ping`, `ListAgents`, `GetAgent`, `CreateAgent`, `KillAgent`, `RemoveAgent`,
    `RestartAgent`, `SendInput` (submit queue), `SendRaw` (bytes), `Resize`, `Broadcast`, `RenameAgent`, `SetModel`,
    `ToggleYolo`, `SetGlobalYolo`, `Subscribe`, `Unsubscribe`, `GetConfig`, `UpdateConfig` (JSON patch), `Preflight`,
    `Status`, `Shutdown { stop_agents }`, `ClientFocus(bool)` (P3),
    `ListSessions(ListSessionsOptions)`, `HideSession { session_id, hidden }`, `ListClaudeProfiles`,
    `SetProjectProfile { project_dir, config_dir }`, `ListProviderModels { provider }` (P4, see *Sessions* below),
    `GetOverview`, `GetOverviewHistory { since_ms }`, `GetOverviewWaits`, `GetOverviewEvents { since_ms, limit }`,
    `GetAgentDetails { agent_id }` (P6, see *Overview* below),
    `GetUsageState`, `GetUsageSummary { period, project_key }`, `GetUsageReport { provider, days }` (7-90), `RefreshUsage`,
    `UsageVisible(bool)` (P7, see *Usage* below), `GetUpdateState`, `CheckForUpdates` (P10, see *Updates* below),
    `GetMcpStatus`, `StartHeadlessRun`, `ListHeadlessRuns`, `GetHeadlessRun`, `CancelHeadlessRun`, `GetHeadlessRunLog` (P8, see
    *MCP server* and *Headless runs* below).
  * `Response`: `Ok`, `Pong`, `Flag(bool)`, `Hello(HelloInfo)`, `Agents`, `Agent(Option)`, `Ids`, `Buffer { data }`,
    `Config`, `Preflight`, `Status`, `Sessions`, `Profiles`, `Models` (P4), `Overview`, `OverviewHistory`,
    `OverviewWaits`, `OverviewEvents`, `AgentDetails(Option)` (P6), `UsageState`, `UsageSummary`, `UsageReport` (P7), `UpdateState` (P10), `McpStatus { running, port, error, manager_workspace }`,
    `HeadlessRun`, `HeadlessRuns`, `HeadlessRunLog` (P8).
  * `Event`: `AgentOutput { agent_id, data }`, `AgentStatus { agent_id, status, session_id, model }`, `AgentWaiting`,
    `AgentsChanged`, `ConfigChanged`, `DaemonShuttingDown`, `Notification(StudioNotification)` (P3),
    `OverviewSnapshot`, `OverviewEvents` (P6), `UsageChanged(UsageState)` (P7), `UpdateAvailable(UpdateState)` (P10), `HeadlessRunEvent { run_id, data }`
    (one output line), `HeadlessRunChanged(HeadlessRun)` (started / session id captured / finished) (P8). New phases add variants only. A client that talks to an older daemon
    gets an error for the Overview and Usage requests; the TUI then shows "The overview is not available" and keeps working.
* **Sessions (P4)**: `ListSessions` returns `SessionSummary`s (camelCase like Hydra's `ClaudeSessionSummary`, plus
  `provider` and `hidden`) newest first; the daemon scans off the async runtime (`spawn_blocking`) and adds the config's
  `hiddenSessionIds` (hidden sessions are left out unless `includeHidden`). Filters: provider, `limit`, `maxAgeDays`,
  `projectPathPrefix`, `query` (all words, case-insensitive), `includeSidechains`, `forceRefresh`. OpenCode has no
  catalog (empty list). `HideSession` and `SetProjectProfile` edit the config under its lock and answer `Config`
  (the change is also pushed as `ConfigChanged`). `SetProjectProfile.config_dir`: a dir, `""` = default profile, `None`
  removes the mapping. `CreateAgent` with `resume_session_id` resumes that session; for Claude the profile is the one
  that owns the session file, otherwise the project mapping (`claudeProfileByProject`, longest prefix wins).
  `ListProviderModels` merges nothing: discovered models (`codex app-server`, `opencode models --verbose`) replace the
  built-in list, which is the fallback (Claude: built-in aliases only); cached 5 min (failures 30 s).
* **Overview (P6, `as_core::overview` + `as-daemon::services`)**: the daemon runs one `OverviewTracker` (poll every 2 s and
  100 ms after an agent status event, in `spawn_blocking`). Per poll: running Claude agents are matched to the live
  registry entries `<configDir>/sessions/<pid>.json` of every Claude profile (files > 64 KB, symlinks, `.key` files and
  dead pids are ignored; an entry that fails to parse once is reused for one poll) in three tiers, each across all
  agents so an entry is never taken twice: (1) `sessionId`, (2) the entry pid is the agent's PTY pid **or one of its
  descendants** (`sysinfo` process table, at most every 10 s, only when tiers 1 and the agent pid did not match: the
  PTY child may be the login shell wrapping `claude`), (3) same cwd and start time within 10 s, when unambiguous.
  Status = registry (`busy|shell` working, `waiting` needs-you with reason `permission prompt|input needed|other`, `idle`)
  or, without an entry and for Codex/OpenCode, output within 5 s = working else idle; process states give
  starting/finished (exit seen in this run)/failed/not-running (restored or imported, never ran). `quietFor` (working,
  silent >= 5 min) and stale (idle >= 24 h) are flags. A needs-you Claude session also gets `waitingText` from the last
  256 KB of its transcript ("Run: npm test"; cached per agent on file size/mtime/inode). `OverviewSnapshot` is pushed
  only when something meaningful changed (never for timers; clients tick their own clock), `OverviewEvents` with every
  poll's new events (appeared / status with `waitedMs` when leaving needs-you / ended / resumed after a gap > 15 s).
  `GetAgentDetails` reads the last message and the token counts of the newest reply from the transcript on demand.
  **Notifications**: a Claude agent entering needs-you raises "Needs you" / `<name> needs permission: Run: ls`; the 5 s
  silence notification (`AgentWaiting`) is suppressed for a Claude agent the tracker matched to a registry entry and kept
  for Codex, OpenCode and unmatched Claude agents (the `AgentWaiting` event itself still goes to the clients for the
  sidebar flag).
* **Usage (P7, `as_core::usage` + `as-daemon::services`)**: one `UsageService` in the daemon, all of it synchronous and run from
  `spawn_blocking` (`usage_loop` calls `tick()` every 5 s, the service decides what is due).
  * *Tokens*: `UsageStore` scans `~/.claude*/projects/**/*.jsonl` (also `subagents/`) and `$CODEX_HOME|~/.codex/{sessions,archived_sessions}`
    incrementally: per file it remembers size, mtime, inode and the byte offset of the last complete line (a partial last line waits, a
    file that shrank or changed inode is forgotten and re-read, a deleted file keeps what it added). Claude lines count once per
    `message.id:requestId`, Codex lines per session id + the `(total, last)` usage signature (resumed/forked files replay history),
    uncarried Codex compactions are billed once the file is quiet for 10 min. Buckets are per (UTC hour, provider, model, project)
    and kept 400 days; cost is the "API-equivalent" estimate of `pricing::SPECS` (unknown models are tokens only, the total says `>=`).
    Day/week (from Monday)/month windows use the local zone (`chrono::Local`). Cache: `usage-cache.json` (mode 600, atomic, at most
    every 30 s and on shutdown; a version or pricing-version change means one full rescan); it holds counts, models, project paths and
    timestamps, never message text or credentials. Rescan every 60 s (cheap: only appended bytes), first full index of ~1 GB of
    logs takes ~20 s in a debug build and is shown as "Indexing usage... 40 %".
  * *Plan limits*: Claude per profile through `GET https://api.anthropic.com/api/oauth/usage` (`Authorization: Bearer`,
    `anthropic-beta: oauth-2025-04-20`, 15 s timeout, blocking `reqwest` + rustls behind the `HttpClient` trait). The token comes from
    `<configDir>/.credentials.json` or, on macOS, the Keychain item `Claude Code-credentials[-<sha256(configDir)[..8]]` through
    `security find-generic-password -w`; it is **never refreshed** (that rotates the refresh token), never logged, never cached, never
    sent to a client: it lives in a `Secret` (redacted `Debug`) from the credential reader to the request and `UsageState` only has
    percentages, reset times and labels. 401/403 = "sign-in expired" (last values kept, the rejected token is not sent again), 429/5xx/
    network errors back off 60 s, 2 min, 5 min, 10 min (a larger `Retry-After` wins), a denied Keychain is not asked again for 24 h,
    two profiles signed in to one account are shown once. Codex limits are passive: the newest `rate_limits` of any rollout line
    (`ESTIMATED`, with its age). Cadence: every 5 min, every 60 s while any client reported `UsageVisible(true)` (a per-connection flag,
    cleared when the client disconnects), and once more (throttled to 60 s) after a Claude agent goes working -> idle.
  * *Alerts* (`usage.alertsEnabled`, default off): once per (provider, account, window, resetsAt, threshold) when a window passes the
    session/weekly alert level (default 80 % / 90 %, other windows 85 %); windows without a reset time, already reset, from a Claude
    account that is not `ok` or from a Codex line older than 30 min never alert. The config's soft daily budgets
    (`usageDailyTokenBudget`, `usageDailyCostBudgetUsd`, `usageBudgetWarningThresholdPct`, default 80 %) alert once at the warning
    level and once at 100 % per local day (the cost budget is skipped while `usage.showCost` is false). Sent keys are kept in
    `usage-alerts.json` (pruned when the window resets / the day ends) so a restart repeats nothing. Delivery is the normal
    notification path: `Event::Notification` with `NotificationKind::UsageLimit` (toast + OS notification when no client is focused).
    Alerts are re-evaluated on every usage change and when the config changes.
  * *Client*: the TUI holds `state` (pushed by `UsageChanged`), summaries per (period, project) and reports per provider, and asks
    again only when `data_version` moved; old values stay on screen meanwhile.
* **MCP server (P8, `as-daemon::mcp`)**: started with the daemon (`DaemonOptions::mcp`) on `127.0.0.1:<random port>` with
  `rmcp` 3.8 (streamable HTTP, sessions, mounted at `/mcp` of an `axum` router). Tools (Hydra's `hydra_*` renamed, same parameters
  and limits, input validated, errors are tool errors with `isError`): `studio_list_agents`, `studio_create_agent` (never a manager;
  `projectDir` must be an existing absolute folder; respects `maxAgents`), `studio_send_prompt`, `studio_get_output` (recent output
  as plain text: escape sequences and control characters removed, `\r` redraws collapsed, last N lines, at most 100 000
  characters, the end is kept), `studio_broadcast`, `studio_kill_agent`, `studio_restart_agent`. **Auth**: every request needs
  `Authorization: Bearer <token>`; the token is random per daemon start (two v4 UUIDs), kept in memory and in the workspace's
  `.mcp.json` (`headers`, mode 0600 on Unix), which Claude Code reads for HTTP servers. rmcp also rejects non-loopback `Host`
  headers (DNS rebinding). **Manager workspace** (`<root>/manager-workspace/`, rewritten at every start because port and token
  change): `.mcp.json` (server `studio`), `CLAUDE.md` (Hydra's text with the `studio_*` names) and `.claude/settings.local.json`
  (`enabledMcpjsonServers: ["studio"]`; without it Claude asks to approve the server at start and the daemon-typed first prompt
  answers that dialog instead). `CreateAgent { is_manager: true }`: the daemon replaces the folder with the workspace, forces
  Claude and drops `resume_session_id`; it fails when the server is not running. No limit on the number of managers (Hydra has none).
* **Headless runs (P8, `as_core::headless`)**: `HeadlessOrchestrator` spawns the provider CLI with `build_headless_args` (Claude:
  `-p <prompt> --output-format stream-json --verbose --model <m> [--resume <id>]`) through the same shell wrapping, env sanitising and
  Claude profile env as agents (Windows: retry through `cmd.exe /c`; Unix: own process group), stdin closed. Files in
  `<root>/headless-runs/`: `<id>.jsonl` (stdout lines as they come; stderr as `{"type":"stderr","text":..}`) and `<id>.meta.json`
  (`{"schemaVersion":1,"run":{..}}`, written at every state change); legacy logs without meta are backfilled, a run found
  `running` at start (the daemon died) becomes `errored`. Statuses `running | completed | errored | canceled`; the session id is the
  first `session_id`/`sessionId` of a stdout line; exit code != 0 is `errored` with `CLI exited with code N: <stderr tail>`. Cancel
  = SIGTERM to the process group, SIGKILL after 3 s (Windows: `taskkill /T /F`); the daemon cancels every run at shutdown.
  `ListHeadlessRuns { query, status, limit }` (newest first; the query searches id, prompt, project, model, provider, status, session,
  error), `GetHeadlessRunLog { tailLines (400), maxChars (120 000) }` -> `{ content, totalLines, returnedLines, truncated }` (streams
  the file; the oldest characters are cut when over the cap). Finished runs raise `Notification` `HeadlessCompleted` ("Run <id>
  finished successfully") or `HeadlessErrored` ("Run <id> failed: <error>"). There is no retention (Hydra has none either).
* **P9 requests** (`as_daemon::handler::p9`; all blocking work under `spawn_blocking`):
  * *git* (`as_core::git::GitRunner`, shells out to `git`/`gh`): `GitStatus`, `GitLog`, `GitDiff` (whole/file/staged), `GitDiffStats`,
    `GitStage`/`GitUnstage`, `GitCommit` (stages the listed files, else commits the index), `GitPush`, `GitBranches`, `GitCheckout`,
    `GitCreateBranch`, `GitFileContents`, `GitWorktreeCreate`/`GitWorktreeRemove`, `GitPrFetch`/`GitPrFileDiff` (read-only `gh`).
    `GIT_TERMINAL_PROMPT=0`, timeouts (30 s, 60 s for push/gh), mutations serialized per repository root. A folder that is not a
    repository answers an empty `GitStatus { is_repo: false }`; a missing `git`/`gh` is an error text the panel shows.
  * *files*: `FsReadDir`, `FsReadFile` (1 MiB cap, binary and too-large placeholders), `FsMtime` (the viewer polls it every second;
    there is no watcher), `FsSearch` (gitignore-aware walk of at most 20 000 files, `nucleo-matcher` ranking). Relative paths use `/`;
    absolute paths, `..` and symlinks that leave the project are refused (`OutsideProject`).
  * *free terminals* (`as_core::terminals::TerminalManager`, shells per project, `agents::pty::pump` shared reader/waiter threads,
    the same `PtyBackend` as agents so the Windows DSR reply applies; the agent manager uses `pump_with_eof` of the same module): `SpawnTerminal` (new tab, or a pane of `group_id`: with `split_pane` / `split_dir` `horizontal` = to the right, `vertical` = below; both optional, an old client that sends neither splits the active pane to the right),
    `TerminalInput`, `TerminalResize`, `KillTerminal`, `RenameTerminal`, `SubscribeTerminal` (returns the scrollback, then
    `Event::TerminalOutput` goes to that connection only; subscribe first, snapshot second, so a chunk may repeat but never gets lost),
    `UnsubscribeTerminal`, `GetTerminalLayout`, `ActivateTerminal`, `ResizeTerminalPane { project_dir, pane_id, axis, delta }` (moves the divider nearest above the pane along `axis` by `delta` percentage points; answers the layout), `ListTerminals`. Events to everyone: `TerminalExit`,
    `TerminalLayoutChanged { project_dir, layout }` (groups, panes, labels; an exited terminal leaves the layout like in Hydra). **Layout tree**: a group (tab) carries `tree`, a `FreeTerminalNode` (`as_core::terminal_tree`): `{"type":"leaf","paneId":..}` or `{"type":"split","dir":"horizontal"|"vertical","ratio":1-99,"a":..,"b":..}` (`horizontal` = `a` left of `b`, `vertical` = `a` above `b`, `ratio` = percent for `a`, kept in 10-90 by resizes); `pane_ids` stays the leaves in reading order, so a client that ignores `tree` (or a daemon that never sent one) still draws the panes side by side. Closing a pane promotes its sibling.
    Lifecycle from the config at use time: `explicit` keeps all, `lru` evicts the least recently active terminal when spawning past
    `freeTerminalMaxCount`, `idle` is swept every minute (`freeTerminalIdleTimeoutMinutes` without input). Scrollback cap =
    `freeTerminalScrollbackLines` x 200 bytes. **Persistence**: the layout (not the shells) is written to `terminals.json` (atomic, debounced 500 ms after every layout change, once more before the shells are closed at shutdown and never after): per project the tabs with their trees, the active tab / pane, and per pane its label and working directory. At daemon start the file is only read (tolerantly: a corrupt file is kept as `terminals.json.corrupt` and means "nothing saved"). The first `GetTerminalLayout` (or `SpawnTerminal`) of a project spawns fresh shells into the saved layout, each in its saved folder (the project folder when that is gone), under new terminal ids and tab ids, at most `freeTerminalMaxCount` in total (further panes are dropped from the tree; with no room at all the saved layout is kept for later); a project that was never opened keeps its saved layout in the next write. Shells do not survive a daemon stop, their history does not either.
  * *skills*: `ScanSkills`, `ToggleSkill` (Claude: `enabledPlugins` of the profile's `settings.json`, atomic write, other keys kept;
    Codex: `SKILL.md` <-> `SKILL.md.disabled`, only for files the scan lists).
  * *open-in*: `ListOpenTargets` (editors found on PATH or as macOS app bundles, file manager, terminal, `$EDITOR`), `OpenIn`.
  * *Hydra import*: `PreviewHydraImport`/`ApplyHydraImport` (`as_core::hydra_import`; Hydra's files are only read; unknown config keys
    ignored, bad values skipped per key, agents merged by id and session id, only `prefix ...` keybindings are taken), also the CLI
    `agent-studio import-hydra [--dir D] [-y]`. `ExportDiagnostics` writes `<root>/diagnostics/<timestamp>/{snapshot.json, logs/}`
    (last 5 logs, 1 MiB each; keys named token/secret/password/credential/apikey/authorization are redacted).
  * *worktree mode*: `CreateAgent { work_mode: worktree }` creates `<repo>/.worktrees/<branch>` on branch `studio/<name>-<id>`
    before the spawn; the agent's `project_dir` is the worktree. Removing the agent deletes the worktree and branch only when clean.
* **TUI panels (P9)**: `Modal::{Git, Files, Settings, OpenIn}` take all keys (`Esc`/`q` closes); the free terminals are a panel
  (`FreeTerms`) at the bottom (45 %) or full screen: while it is open every key goes to the active shell except the prefix and the
  direct bindings. Panel state lives in `as_tui::panels::*` and talks to the daemon only through the generic `Effect::Req { key, req }`
  -> `Msg::Reply { key, result }` (keys `git:*`, `fs:*`, `term:*`, `set:*`, `open:*`, `diag:*`), so each panel is unit-tested without a daemon.
  The pane tree of the active tab is laid out in the client (`panels::terminals::tree_rects`: a one-cell separator per split, `neighbor` finds
  the pane in a direction); resizes are applied locally at once and confirmed by the daemon's layout.
* **Diff viewer and syntax highlighting (`as_tui::panels::diff_view`, `as_tui::highlight`)**: `DiffView` keeps the unified rows (from
  `as_core::git::parse_unified_diff`) and a derived side-by-side form (`pair_rows`: the n-th removed line of a run meets the n-th added one,
  the shorter side is padded, headers span both columns). The renderer switches layout from `diffLayout` (`unified` | `split`, `s` in the Git
  panel) and the panel width (split needs a 100-column body, else a note and the unified view). Highlighting is behind the cargo feature
  `highlight` (default for the binary; `as-tui` alone builds without it): `syntect` with `default-features = false` and
  `default-syntaxes`, `default-themes`, `regex-fancy` (pure-Rust regex engine, no `onig`), themes `InspiredGitHub` (light) and
  `base16-eighties.dark` / `base16-ocean.dark` (dark / midnight), foreground colors only. Lines are highlighted lazily (visible window + 60 lines,
  at most 30 ms per frame, the rest on the next frames), per (path, mtime, theme) for the Files preview (4 files cached), files over 512 KB stay plain; diffs
  keep two parser states (old and new side) restarted at every hunk.
* **Modals and the prefix (P10)**: the panel modals (Help, Usage, Git, Files, Settings, Open in, Headless) let the prefix through:
  `prefix` + one of *terminals toggle*, *palette*, *help*, *close-dialogs* (keys from the keymap, so overrides apply) closes the
  panel and runs the command; any other key after the prefix is dropped. Forms (new agent, rename, resume, profile, palette, quit,
  confirmations) and panels that currently take typed text (a git prompt, the file search, a Settings value, the headless filter) keep
  `Ctrl+A` for themselves. The footer shows `PREFIX` and the four allowed keys.
* **Mouse (P10)**: off by default (`mouse` in `config.json`, `--mouse`, `AGENT_STUDIO_MOUSE=1`; read once at start). With it on, a click on
  the header tabs (Chat/Grid/Overview, `ui::header::tab_at`) switches the view, a click on a sidebar row selects/folds, a click on a
  tile focuses it, the wheel scrolls the Chat scrollback. The terminal's own selection needs `Shift`+drag while the mouse is captured.
* **Updates (P10, `as_core::updates`)**: `GetUpdateState` (no network) / `CheckForUpdates` (asks now, also with `checkForUpdates: false`)
  answer `Response::UpdateState`; `Event::UpdateAvailable(UpdateState)` is pushed once per newly found version. The daemon checks 10 s
  after start and every 6 h when `checkForUpdates` is on: one `GET https://api.github.com/repos/kawashita86/agent-studio/releases/latest`
  through the `HttpClient` seam of the usage limits (empty bearer = no `Authorization` header). `404` = no release yet, network errors /
  rate limits only change `UpdateState.status` (`offline`, `rate_limited`, `failed`), drafts and prereleases are ignored, versions are
  compared as semver. The state carries the install method guessed from `current_exe()` (cargo, Homebrew, winget, scoop, chocolatey,
  direct, unknown) and a one-sentence upgrade `hint`; for a direct install the hint is the download URL of the archive for this OS and CPU
  (`agent-studio-<tag>-<target triple>.tar.gz|zip`). The binary is never replaced. `AGENT_STUDIO_UPDATE_URL` overrides the API URL (tests,
  manual checks). The TUI shows a dismissible banner (stacked with the toasts; `prefix Esc` dismisses it for that version, remembered in
  `tui-state.json` as `dismissedUpdate`), the palette command *Check for Updates* (`prefix U`) and a line in Settings > About. The CLI
  `agent-studio update-check` runs the same check in-process, without a daemon.
* **Overview history (`<root>/overview-history/v1-YYYY-MM-DD.jsonl`, mode 600, folder 700)**: one writer (the daemon).
  Lines: `{"start":<ms>}` (each daemon start; time between the last point and the next one is *unmeasured*),
  `{"point":[at,needsYou,working,idle,total,{agentId:status}]}` every 5 s (a trailing `1` marks a delta line: only the
  changed statuses, `null` for a gone agent; the first point of a run and of each file is full; agents ended for over an
  hour and not-running ones are not written), `{"event":{...}}` (written field by field). Flushed every 5 s and on
  shutdown; files of days that ended more than 8 days ago are deleted, then the oldest until the total is <= 20 MB; a
  day over 2 MB goes on in `-2`, `-3`. Memory: 1 000 events, 6 h of points; the wait ledger (moves in/out of needs-you,
  measured stretches, starts; 9 days) is rebuilt from every file at start. Unknown formats, links, other file types and
  files > 4 MB are never read; malformed lines and records dated > 1 min ahead are skipped.
* **Startup import**: after the daemon is up, when `importSessionsOnStartup` is set, every catalog's sessions
  (`sessionImportLimit`, `sessionMaxAgeDays`, `sessionImportProjectPrefix`, minus `hiddenSessionIds`, minus sessions
  without prompt and messages) become idle `sess-<id8>` agents without process, each with the provider that saved it; they
  are not persisted in `workspace.json` and the first input resumes them.
* **Session-id discovery**: after every spawn the manager asks the `SessionLocator` (catalog based) at once and then after
  2.5 s x 1.6^n (max 30 s) until it settles, the process exits or the daemon stops; a match is by project dir (prefix),
  latest user prompt hint, or "modified since start - 5 min"; ids of other agents are skipped.
* Requests of one connection are answered **in order** (so `SendRaw` keystrokes never reorder); use several
  connections or pipeline by id for parallelism.
* Handshake: first request is `Hello { client_version }`; the reply carries daemon version, `PROTOCOL_VERSION` and pid.
  Version policy (`as_proto::lifecycle::ensure_daemon`): on a `major.minor` or protocol mismatch an **idle** daemon
  (no running agent) is shut down (`Shutdown { stop_agents: false }`) and replaced; a daemon with running agents is
  never restarted automatically: the client keeps using it (warning) when the protocol matches and fails with an
  actionable error (`agent-studio stop --force`) when it does not.
* Notifications (P3, `as-daemon::notifications`, rules of Hydra's `DaemonNotificationService`): `agent_waiting` ("Agent Finished" / "<name> is waiting for input"), `agent_idle` ("Agent Completed"), `agent_errored` ("Agent Error"), `agent_started` ("Agent Started"); status notifications are de-duplicated per agent. Every notification is broadcast as `Event::Notification`; an OS notification (`notify-rust`, cargo feature `desktop-notifications`, default on) is shown only while no client reported `ClientFocus(true)`.
* Output streaming: `Subscribe { agent_id }` returns the buffered output (<= 2 MB raw bytes) and from then on
  `Event::AgentOutput` chunks are pushed to **that connection only**. Subscriptions belong to the connection: after a
  reconnect, subscribe again. Status/waiting/agents-changed/config events go to every client. A client whose outgoing
  queue overflows (8192 messages) is disconnected instead of silently losing PTY bytes.
* PTY output is **bytes** (`Vec<u8>`), not `String`: a chunk can split a UTF-8 sequence.
* Shutdown: `Shutdown { stop_agents: true }` kills all agents (waits up to 5 s), saves the workspace, broadcasts
  `DaemonShuttingDown`, exits. `stop_agents: false` is refused while any agent is active (the TUI's "Detach" simply
  disconnects). SIGTERM/Ctrl+C behave like `stop_agents: true`; SIGHUP is ignored.
* Lock file `daemon.lock`: `{ pid, version, socket, started_at }`, written after the socket is bound. A daemon counts as
  alive only if the pid exists **and** the socket accepts a connection; otherwise lock + socket file are removed.
* Spawning (`spawn_daemon`): `current_exe daemon --home <root>`, stdio null; Unix: own process group; Windows:
  `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW`. Clients retry connecting with backoff for 5 s.

## Data root (`as_core::paths::Paths`)

`AGENT_STUDIO_HOME` or the OS config dir (`~/.config/agent-studio`, `~/Library/Application Support/agent-studio`,
`%APPDATA%\agent-studio`):

| File | Content |
|------|---------|
| `config.json` | `AppConfig` (Hydra `shared/types.ts` `AppConfig`, camelCase keys, defaults filled in) |
| `keybindings.json` | user overrides `[{ "command": "...", "keys": "..." }]`, keys in TUI syntax (see `docs/tui-keys.md`); polled by the TUI every second |
| `tui-state.json` | UI state of the TUI: last view, selected agent, sidebar width/sort/status chip, grid tab, expanded tile, tile order, running-only toggle (written by the TUI only, tolerant load) |
| `workspace.json` | persisted agents, restored at daemon start |
| `terminals.json` | free-terminal layout per project (tabs, split trees, pane labels and folders); shells are respawned lazily after a restart |
| `daemon.lock`, `daemon.sock` | daemon single instance + socket |
| `logs/` | `tracing` JSONL logs, daily rotation |
| `headless-runs/` | `<id>.jsonl` log + `<id>.meta.json` index entry per headless run |
| `overview-history/` | overview events and samples (8 days, ≤ 20 MB) |
| `usage-cache.json` | usage store: file offsets, hourly buckets, dedupe keys, newest Codex limits (mode 600) |
| `usage-alerts.json` | usage alerts already sent (key + when it stops mattering) |
| `manager-workspace/` | cwd of the MCP manager agent (`.mcp.json` with the bearer token, `CLAUDE.md`, `.claude/settings.local.json`); rewritten at every daemon start |

## External files read (never written, except skill toggles)

| Path | Used for |
|------|----------|
| `~/.claude*/projects/<slug>/<session>.jsonl` (+ `$CLAUDE_CONFIG_DIR`, optional `sessions-index.json`) | session catalog (P4), resume, usage, transcript history |
| `~/.claude*/sessions/<pid>.json` | live status / "needs you" (undocumented) |
| `~/.claude*/.credentials.json` / macOS keychain (`security`) | OAuth token for the plan-limits endpoint (read-only, never refreshed, never logged) |
| `~/.codex/{sessions,archived_sessions}/**/rollout-*.jsonl` (`$CODEX_HOME`) | Codex session catalog (P4) + usage (tokens and passive limits) |
| `~/.claude/settings.json`, `~/.claude/plugins`, `~/.codex/skills` | skills tab |

## Network access

Only two outgoing requests exist, both from the daemon (or the one-shot CLI) with a short timeout: the Claude plan limits
(`api.anthropic.com/api/oauth/usage`, with the user's own OAuth token, opt-out `usage.*`) and the update check
(`api.github.com/.../releases/latest`, no credential, opt-out `checkForUpdates: false`). Agents talk to their providers on their own.

## Startup dialogs of Claude

`agents::startup::TrustGate` holds the automatic startup input (`/rename`, the first prompt) while one of Claude's first-run dialogs is on
the agent's output: folder trust, project MCP server approval, custom API key. They are matched by tolerant phrases
(`DIALOG_PHRASES`, ANSI stripped, non-alphanumerics removed); answering = Enter or a digit typed by the user.

## Crates

| Crate | Responsibility |
|-------|---------------|
| `as-core` | all domain services, synchronous or tokio-based, no UI, no socket (`agents::{manager, startup, model_catalog}`, `sessions::{claude, codex, transcript, profiles, locator}`) |
| `as-proto` | `Request`/`Response`/`Event` enums and frame codec helpers |
| `as-daemon` | socket server, client registry, event fan-out, lock/lifecycle, MCP server, OS notifications |
| `as-tui` | daemon client, app state, views, widgets, keymap, theme |
| bin `agent-studio` | clap entry point, logging init, dispatch |
