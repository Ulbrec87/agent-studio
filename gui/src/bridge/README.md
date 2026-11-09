# Bridge: `window.hydra` for Agent Studio

`window.hydra` has exactly the shape of Hydra's `electron/preload.ts` (`HydraAPI` in `hydraApi.ts`), so the copied renderer runs
unchanged. Layers:

| File | Role |
|------|------|
| `transport.ts` | `Transport { request, onEvent }` (Hydra level) and `RawTransport` (protocol level + `onStatus`/`status`), `selectRawTransport()` / `selectTransport()` |
| `tauriTransport.ts` | desktop raw transport: `invoke('app_info')`, `invoke('daemon_request', { request })`, `daemon-event`, `daemon-status`, `daemon_status`, plus `shell` (below) |
| `wsTransport.ts` | browser dev raw transport: `?bridge=ws://127.0.0.1:PORT&token=T` -> the `agent-studio --dev-bridge` WebSocket; its `shell` only offers `log_event` |
| `daemonApi.ts` | **the mapping layer**: Hydra method -> `as-proto` request, daemon event -> Hydra `on*` channel; unwired methods fall through to the mock |
| `mappers.ts` | pure shape adapters (preflight, notification, MCP status, config defaults + GUI-only keys) |
| `streams.ts` | per-agent (and per-free-terminal) output streams: `Subscribe`/`SubscribeTerminal`, held live chunks, de-duplication, streaming `TextDecoder` |
| `generated/` | **generated, do not edit**: TypeScript types of the wire protocol (`Request`, `Response`, `Event` and every as-core payload type) written by `ts-rs`; regenerate with `UPDATE_TS_BINDINGS=1 cargo test -p as-proto --test ts_bindings` (a plain `cargo test` fails when they drift). `protocol.ts` keeps the loose `{ type, payload }` envelopes the transports and mocks use; the mappers import the payload types from here |
| `context.ts` | `BridgeContext` (typed `call`/`send`/`emit`) and `MethodGroup`, what the groups below get from `daemonApi.ts` |
| `gitApi.ts` | G2a: git methods + shape mappers |
| `filesApi.ts` | G2a: files/editor methods, shape mappers and the polling file watcher |
| `terminalsApi.ts` | G2a: free terminals and the preflight "test terminal" |
| `nativeApi.ts` | G2a: open-in (G2b: through the shell), folder suggestions, folder dialog, URL opener, clipboard image |
| `headlessApi.ts`, `skillsApi.ts`, `updatesApi.ts` | G2b: headless runs, skills scan/toggle, update state (see "G2b" below) |
| `diagnosticsApi.ts`, `keybindingsApi.ts`, `quitApi.ts` | G2b: diagnostics export + renderer log, `gui-keybindings.json`, the quit flow |
| `testKit.ts` | test helper: scripted raw transport (`FakeRaw`) and `makeBridge()` |
| `protocol.ts` | protocol envelope types, base64 helpers, `toError` |
| `mockTransport.ts` + `fixtures.ts` | in-memory answers for plain browsers, tests and every method that is not wired yet; `emit()` for tests |
| `bridge.ts` | `call` / `send` / `subscribe` over a Hydra-level transport; fans events out by channel |
| `hydraApi.ts` | generated method table: every method calls the bridge with its own name (+ 3 Agent Studio extensions, below) |
| `index.ts` | `installHydra()`: loads `app_info`, waits up to 4 s for the daemon, builds and installs `window.hydra` before the first render |

Two wire formats:

* **Hydra level** (`bridge.ts` <-> `daemonApi.ts`/mock): a request is `{ method: '<hydra method>', args: [...] }`, an event is
  `{ channel: '<onXxx name>', args: [...] }`; `{ kind: 'app_info' }` asks for the shell's `AppInfo`.
* **Protocol level** (`daemonApi.ts` <-> shell): the JSON of `as_proto::Request` / `Response` / `Event` (`{ type, payload }`, snake_case
  type names, snake_case message fields, camelCase domain structs, base64 bytes). The Rust shell is generic and has no per-method code:
  `daemon_request(request) -> response` (a daemon error rejects with its message), events on `daemon-event`, connection state on
  `daemon-status` (`{ connected, error? }`; `connected: true` again after a drop means "subscriptions are gone, start over").
  The dev bridge frames the same thing over a WebSocket (`crates/as-gui/src/devbridge.rs`).

### How the layer behaves

* **Output is bytes.** `AgentOutput` carries base64; each agent has its own streaming `TextDecoder` (`{ stream: true }`), so a UTF-8
  character split over two PTY chunks is intact. Typed input becomes UTF-8 bytes for `SendRaw`.
* **Subscriptions.** The daemon pushes output only to subscribed connections. The layer subscribes every non-idle agent on connect and on
  `agents_changed` / status events, forgets agents that disappeared and subscribes again after a reconnect. `getAgentBuffer` re-sends
  `Subscribe` and serves the returned buffer (the renderer merges it with live chunks, like Hydra's own reconnect). The first
  subscription of an agent holds live chunks until the `Subscribe` answer arrived and drops the daemon's documented repeat of the last
  chunk (only when the chunk is at least 4 bytes, so a repeated spinner frame is never eaten).
* **Ordering.** Concurrent Tauri `invoke`s carry no ordering guarantee, so `sendInput` / `sendRawInput` / `resizeAgent` of one agent are
  chained (the next is sent when the previous was answered).
* **Resize.** Hydra's grid tiles call `resizeAgent` too, so (like Hydra, unlike the TUI) a tile resizes the PTY to the tile size. Keeping the
  PTY at the chat pane size needs the renderer to stop resizing from tiles; not done in G1 (limitation).
* **Config.** `GetConfig` / `UpdateConfig` for every key (since G2b that includes `terminalFontFamily`, `terminalFontSize`,
  `terminalEnableWebgl`; a daemon that predates a key gets Hydra's default filled in). `config_changed` events become `onConfigChange`. The
  pre-G2b `localStorage` overlay (`agent-studio.gui-config`) is migrated once per page on the first connection: values that differ from the
  default are sent to the daemon when its config still has the default, then the entry is removed.
* **Keybindings**: see "G2b" below. The path getters return `<data dir>/gui-keybindings.json` and `<data dir>/config.json`.
* **Window focus** is reported with `ClientFocus`, so the daemon only shows OS notifications while the GUI is not focused.
* **Errors**: a rejected request becomes an `Error` with the daemon's message; a response of an unexpected variant is an error.
* **Shape fixes** (everything else already matches `gui/shared/types.ts`): `Preflight` (list per provider, `path`) -> one result with
  `claudePath`; `StudioNotification` (`kind`, `agent_id`) -> `HydraNotification` (`type`, `agentId`); MCP status `manager_workspace` ->
  `managerWorkspace`; the wait ledger is wrapped as `{ ledger }`; global YOLO also writes `globalYolo` into the config (Hydra does).

### G2a: git, files/editor, free terminals, open-in and native

**Git** (`gitApi.ts`). Shapes: the daemon's `GitStatus` (file list with flags) becomes Hydra's `{ branch, ahead, behind, modified, staged, untracked }`
(a folder that is not a repository is the same empty status Hydra returns; a detached HEAD is branch `HEAD`); the log's `hash` is the 8 character
`shortHash`; branches are **local only** (the panel has no notion of remote ones); `gitWorktreeCreate` answers `{ worktreePath, branch }`;
`gitWorktreeRemove` passes `force: true` and deletes the branch (Hydra uses `--force` and `branch -D`); `gitPush` answers `void`. Semantics:
* `gitCommit(dir, msg, files?)`: with files it is `GitCommit { files }` (stages and commits only those; Hydra staged them but committed everything
  staged). Without files Hydra does `git add -A`: the bridge reads the status, `GitStage`s every path that is not fully staged, then `GitCommit { files: [] }`.
* `gitFileContents` is HEAD versus the **work tree** (what the diff viewer shows): the bridge asks the daemon twice (`staged: true` gives HEAD/index,
  `staged: false` gives index/work tree) and combines `original` of the first with `modified` of the second.
* `getGitDiff` is the unstaged diff (Hydra's `git diff`); `gitFetchPr` passes the identifier (`123`, `#123`, URL) as text, `gitPrFileDiff` the number.
  `gh` errors (no GitHub remote, not logged in) reach the PR tab as the error text.

**Files / editor** (`filesApi.ts`). Hydra's methods take an **agent id**: the project directory comes from the agent list (cached; `GetAgent` on a
miss). Paths are project-relative with `/` and `'.'` for the root (the bridge sends `''`). `readDir` hides Hydra's ignored names (`node_modules`,
`dist`, `build`, ...) and dot entries except `.env.example`, folders first. `readFile` returns `{ content, path }`; a binary file or one above the
daemon's 1 MiB preview cap is an error (Hydra: 5 MB). `writeFile` is the new `FsWriteFile` (5 MiB cap, atomic, path-safe, answers `true`).
`searchFiles` is the daemon's fuzzy, `.gitignore`-aware search, over-fetched and filtered with Hydra's ignore list; a blank query gives `[]`.

*Polling watcher* (replaces Hydra's recursive `fs.watch`; the daemon has no watcher, only `FsMtime`): `watchDir(agentId)` starts polling every
**1 s**, `unwatchDir` stops it (the editor panel does both). It follows what the renderer is actually showing, at most 64 paths per agent: the
project root, every directory it listed with `readDir` (a directory's mtime moves when an entry is created, deleted or renamed -> `{ eventType:
'rename', path: <dir> }`) and every file it opened with `readFile` (-> `{ eventType: 'change', path }`; a file that vanishes or comes back is a
`rename`). Baselines are taken at registration (before the listing, so a change in between is reported at the next poll); our own `writeFile`
is acknowledged silently, but the atomic write touches its directory, so the tree refreshes once after a save (Hydra's watcher also fires on saves).
A change in a directory the tree never listed is not seen. The renderer only reloads its tree on these events; an open tab is not reloaded.

**Free terminals** (`terminalsApi.ts`). The daemon's `FreeTerminalLayout` is Hydra's layout plus an optional split `tree` per group; groups carry
`paneIds` in reading order, which is what the renderer shows (side by side), so layouts pass through unchanged and keep the split info for later.
`SpawnedTerminal` is already `{ terminalId, groupId, layout }`; `listFreeTerminals` adds `exited: false` (the daemon drops exited terminals).
Output is bytes: every pane of every layout the bridge sees (`getFreeTerminalLayout`, `spawnFreeTerminal`, layout events) is subscribed
(`SubscribeTerminal`) so `onFreeTerminalOutput(id, projectDir, text)` has a streaming `TextDecoder` per terminal; `getFreeTerminalBuffer` re-sends
`SubscribeTerminal` and returns the whole scrollback (`{ exists: false }` when the daemon does not know the terminal). Input and resizes of a
terminal are chained (`serial`). `terminal_exit` -> `onFreeTerminalExit`, `terminal_layout_changed` -> `onFreeTerminalLayoutChanged`.
*Reconnect*: subscriptions are per connection and a restarted daemon respawns a project's saved shells (new ids) on the first `GetTerminalLayout`,
so after `connected: true` the bridge asks the layout of every project that had terminals and emits `onFreeTerminalLayoutChanged` (the panel only
reads layouts when it mounts). The window does **not** reload on a reconnect (see AGENTLOG).
*Test terminal* (preflight modal, "try `claude` in a shell"): a free terminal of a scratch project (the data dir) whose output/exit go to
`onTestTerminalOutput/Exit`; its layout events are not forwarded and `killTestTerminal` (modal closed) removes it, so it never shows in a layout.

**Open-in and native** (`nativeApi.ts`). `getInstalledEditors` = `ListOpenTargets` mapped to Hydra's `EditorId`s (`files` = `finder`; the daemon's
`$EDITOR` target has no counterpart); `openInEditor`/`openInApp`/`openPath` = `OpenIn` (editor, falling back to the default application like Hydra;
`openPath` = default application via the `files` target), answering `false` when nothing could open the path. The daemon starts the program, so a
daemon without a display (systemd service started outside the session) cannot open GUI editors. `listDirectories` = new `FsListDirs`.
`selectDirectory` = Tauri dialog plugin (`dialog:allow-open`), `null` in a browser (no native dialog in the dev bridge; the New Agent dialog still
takes a typed path). `openExternal` = Tauri opener plugin (`opener:allow-open-url`, scope `http(s)://*` and `mailto:*`; Hydra's own http/https/mailto
check runs first), `window.open` in a browser. `writeClipboardImage` = `navigator.clipboard.write` with a PNG blob (no plugin: supported by WebView2,
WKWebView and current WebKitGTK; returns `false` when the webview refuses).

### G2b: headless, skills, updates, diagnostics, keybindings, quit flow, open-in through the shell

**The shell** (`RawTransport.shell`, `ShellBridge` in `transport.ts`). Besides `daemon_request` the Tauri shell has a few commands of its own,
reached through `ctx.shell` (`supports(cmd)` / `invoke(cmd, args)` / `on(event, cb)`): `log_event`, `open_in`, `list_open_targets`, `quit_app`,
and the window event `confirm-quit`. They exist only in the desktop app; the dev bridge's `shell` supports `log_event` only; groups fall back
to the daemon (`open_in`, `list_open_targets`) or to a documented no-op (`quit_app`) where a command is missing.

**Headless runs** (`headlessApi.ts`). The five calls map 1:1 to `StartHeadlessRun`, `ListHeadlessRuns`, `GetHeadlessRun`, `CancelHeadlessRun`,
`GetHeadlessRunLog` (the run and log structs are Hydra's, camelCase). Hydra only announces output lines (`onHeadlessRunEvent { runId, data }`);
the bridge also turns `HeadlessRunChanged` (started, session id captured, finished) into an event with empty `data`, so the panel refreshes at
once instead of at its 4 s poll. A daemon error (bad folder, empty prompt, preflight failed) rejects with its message.

**Skills** (`skillsApi.ts`). `scanSkills` = `ScanSkills` + `scannedAt`. `toggleSkill({ provider, id, enabled })` looks the skill up in a fresh scan
because the daemon needs the profile (`configDir`) or the file: a Claude plugin (`id` = plugin key) is toggled in every profile that has it, a Codex
skill (`id` = directory name, first match) by its `SKILL.md`. Claude's own user skills have no switch in Claude Code (Hydra wrote a meaningless
`enabledPlugins` entry), so the answer is `{ success: false }` and the panel rescans (the toggle snaps back). Errors are `{ success: false }`, never exceptions.

**Updates** (`updatesApi.ts`). `GetUpdateState` / `CheckForUpdates` / event `UpdateAvailable` -> Hydra's `AppUpdateState`: `supported: true`,
`available` = status `available`, `latestVersion`, `releaseNotes` (markdown), `error` for offline / rate limit / failed (not for "no release yet"),
`canAutoInstall: false`, `downloaded`/`downloading: false`, `releaseDate: null`, `downloadUrl` = the archive for this OS and CPU (else the release page),
`releaseUrl` = the release page, `installMethod` = `brew` for Homebrew, `direct` for a plain binary, `unknown` for cargo/winget/scoop/choco, and the
extension field `upgradeHint` (the daemon's sentence: "Run `brew upgrade agent-studio`", "Run `cargo install ...`", "Download <url>") that the banner and
the panel show. `checkForUpdates` also announces `checking: true` and then the result through `onUpdateStateChange`. The app never replaces itself:
`downloadUpdate` answers the state with an `error` saying so (plus the hint), `installUpdateAndRestart` answers `false`, `runBrewUpgrade` answers
`{ ok: false, error: <hint> }`; the UI never offers them (`canAutoInstall` is false) except "Upgrade via brew" for a Homebrew install, which then shows the
hint. `openUpdateDownload` opens the asset (or release page) with the opener plugin (`window.open` in a browser) after the http(s) check.
Tauri's own updater is out of scope.

**Diagnostics and logging** (`diagnosticsApi.ts`). `exportDiagnostics` = `ExportDiagnostics` -> `{ path, error }` (the folder holds the newest logs, including the
GUI log, and a redacted snapshot); the desktop app then shows the folder in the file manager (`open_in` files); a browser tab never does. `logEvent`
(fire and forget) goes to the shell's `log_event` -> `<data>/logs/agent-studio-gui.<date>` (JSON lines, `tracing` target `renderer`, fields `event`,
`message`, `trace_id`, `agent_id`, `session_id`, `project_id`, `service`, `meta`); through the dev bridge it reaches the same file; with no shell at all
(plain browser, tests) it is a console line. `as_gui::gui_log` redacts values under credential-looking keys (`token`, `secret`, `password`, `authorization`,
`cookie`, ...), scrubs `token=...` / `Bearer ...` / `sk-ant-...` in free text and caps field sizes; it never fails.

**Keybindings** (`keybindingsApi.ts`). Hydra's `mod+x` rules with the user's overrides in **`<data>/gui-keybindings.json`** (the daemon's `keybindings.json` is the TUI's
prefix-key file and is never read here). The daemon only reads the raw file (`GetGuiKeybindings` -> `{ path, exists, rules, error, stamp }`, tolerant: a
missing, empty, corrupt or too big file is reported, not an error) and creates a template on request (`CreateGuiKeybindingsFile`, atomic: temporary file linked
into place, never replaces an existing file). Merging with the defaults and the validation of command ids is TypeScript (`parseRules`, `mergeKeybindingsWithDefaults`,
as in Hydra's `KeybindingStore`). `onKeybindingsChange` polls the stamp once a second after (re)connecting (works through Tauri and the dev bridge) and fires only when the
merged result changes; a broken file means the defaults. Differences from Hydra: the file is not created at start and not rewritten to append missing defaults;
it is created from the defaults when the user opens it (`openPath` of that path, i.e. Settings > Shortcuts > Open File or the "Open Keybindings File" command).

**Quit flow** (`quitApi.ts`, shell `src/quit.rs` + `shell.rs`). Closing the main window asks the daemon for its status. Nothing running (or no daemon answer within 2 s): the app exits at
once. Agents running: the window stays open and the shell emits `confirm-quit` with the count; the renderer's dialog (Hydra's) answers with `confirmQuit()` ("Quit All": `quit_app
{ stopAgents: true }` = `Shutdown { stop_agents: true }`, then exit; the daemon stops too), `quitBackground()` ("Close": `quit_app { stopAgents: false }`, the window exits, daemon and agents
keep running) or Cancel (the renderer closes its dialog, nothing else happens). Browser dev bridge: no window, `onConfirmQuit` never fires and both methods answer `false`.
Not handled yet (G3): Cmd+Q / the app menu / the tray, which exit without asking.

**Open-in** (`nativeApi.ts`). In the desktop app `openInEditor` / `openInApp` / `openPath` / `getInstalledEditors` run in the shell process (`open_in`, `list_open_targets` through
`as_core::open_in`), because a daemon started by systemd/launchd outside the graphical session has no display. Without a shell (dev bridge) they stay `OpenIn` / `ListOpenTargets` requests.

**Agent restarts** (extension `onAgentRestarted`, from the new event `AgentRestarted`). The daemon announces every restart (restart button, CLI/TUI/MCP, YOLO or model switch, global YOLO)
before the new process writes; `useAgents` drops its copy of the screen, so the old session no longer stays under the new one.

### Agent Studio extensions of `window.hydra`

| Method | Meaning |
|--------|---------|
| `getDaemonStatus()` / `onDaemonStatus(cb)` | `{ connected, error? }`; drives the "Connecting to the daemon..." / "Daemon not reachable" banner (`components/Daemon`) |
| `onAgentsChanged(cb)` | the agent list changed elsewhere (CLI/TUI/MCP) or the connection came back: `useAgents` reloads the list |
| `onAgentRestarted(cb)` | `{ agentId }`: a new process replaced the agent's old one; `useAgents` clears the agent's output (G2b) |
| `AppUpdateState.upgradeHint` | field, not a method: the daemon's "how to upgrade" sentence (G2b) |

### Running the front end against a real daemon in a browser

```bash
cd crates/as-gui && cargo run -- --dev-bridge --home /tmp/as-dev     # prints: DEV_BRIDGE ws://127.0.0.1:PORT token=TOKEN
cd gui && npm run dev                                                # http://localhost:1420/?bridge=ws://127.0.0.1:PORT&token=TOKEN
```

The dev bridge is a dev/test tool (browser automation, screenshots): loopback only, random port and token, never started without the flag.

## Methods

Status: `stub` = answered by the mock (fixtures / empty states / documented no-op), `wired` = real daemon or native call.
Tick them off here as G1/G2a/G2b landed (`wired.test.ts` keeps this table in step with `WIRED_METHODS` / `WIRED_CHANNELS` in `daemonApi.ts`). `send` = fire-and-forget (`ipcRenderer.send`), `on` = subscription returning an unsubscribe.

| Method | Kind | Phase | Status |
|--------|------|-------|--------|
| `preflight` | invoke | G1 | wired |
| `createAgent` | invoke | G1 | wired |
| `killAgent` | invoke | G1 | wired |
| `removeAgent` | invoke | G1 | wired |
| `restartAgent` | invoke | G1 | wired |
| `listAgents` | invoke | G1 | wired |
| `toggleYolo` | invoke | G1 | wired |
| `renameAgent` | invoke | G1 | wired |
| `setAgentModel` | invoke | G1 | wired |
| `getAgentBuffer` | invoke | G1 | wired |
| `sendInput` | send | G1 | wired |
| `sendRawInput` | send | G1 | wired |
| `resizeAgent` | send | G1 | wired |
| `broadcast` | invoke | G1 | wired |
| `onAgentOutput` | on | G1 | wired |
| `onAgentStatus` | on | G1 | wired |
| `getOverview` | invoke | G1 | wired |
| `getOverviewAgentDetails` | invoke | G1 | wired |
| `getOverviewHistory` | invoke | G1 | wired |
| `getOverviewWaits` | invoke | G1 | wired |
| `onOverviewEvents` | on | G1 | wired |
| `onOverviewSnapshot` | on | G1 | wired |
| `getUsageState` | invoke | G1 | wired |
| `getUsageSummary` | invoke | G1 | wired |
| `getUsageReport` | invoke | G1 | wired |
| `refreshUsage` | invoke | G1 | wired |
| `setUsageVisible` | invoke | G1 | wired |
| `onUsageChanged` | on | G1 | wired |
| `getConfig` | invoke | G1 | wired |
| `setConfig` | invoke | G1 | wired |
| `onConfigChange` | on | G1 | wired |
| `getKeybindings` | invoke | G1 | wired |
| `getKeybindingsPath` | invoke | G1 | wired |
| `getConfigPath` | invoke | G1 | wired |
| `onKeybindingsChange` | on | G1 | wired |
| `toggleGlobalYolo` | invoke | G1 | wired |
| `writeClipboardImage` | invoke | G2 | wired |
| `selectDirectory` | invoke | G2 | wired |
| `listDirectories` | invoke | G2 | wired |
| `listClaudeProfiles` | invoke | G1 | wired |
| `openInEditor` | invoke | G2 | wired |
| `openInApp` | invoke | G2 | wired |
| `openPath` | invoke | G2 | wired |
| `openExternal` | invoke | G2 | wired |
| `getInstalledEditors` | invoke | G2 | wired |
| `listClaudeSessions` | invoke | G1 | wired |
| `listProviderModels` | invoke | G1 | wired |
| `onConfirmQuit` | on | G1 | wired |
| `confirmQuit` | invoke | G1 | wired |
| `quitBackground` | invoke | G1 | wired |
| `startHeadlessRun` | invoke | G2 | wired |
| `listHeadlessRuns` | invoke | G2 | wired |
| `getHeadlessRun` | invoke | G2 | wired |
| `cancelHeadlessRun` | invoke | G2 | wired |
| `getHeadlessRunLog` | invoke | G2 | wired |
| `onHeadlessRunEvent` | on | G2 | wired |
| `logEvent` | send | G1 | wired |
| `exportDiagnostics` | invoke | G2 | wired |
| `getUpdateState` | invoke | G2 | wired |
| `checkForUpdates` | invoke | G2 | wired |
| `downloadUpdate` | invoke | G2 | wired |
| `installUpdateAndRestart` | invoke | G2 | wired |
| `runBrewUpgrade` | invoke | G2 | wired |
| `openUpdateDownload` | invoke | G2 | wired |
| `onUpdateStateChange` | on | G2 | wired |
| `getMcpServerStatus` | invoke | G1 | wired |
| `onNotification` | on | G1 | wired |
| `dismissNotification` | send | G1 | wired |
| `readDir` | invoke | G2 | wired |
| `readFile` | invoke | G2 | wired |
| `writeFile` | invoke | G2 | wired |
| `watchDir` | send | G2 | wired |
| `unwatchDir` | send | G2 | wired |
| `searchFiles` | invoke | G2 | wired |
| `onFsWatchEvent` | on | G2 | wired |
| `getGitStatus` | invoke | G2 | wired |
| `getGitLog` | invoke | G2 | wired |
| `getGitDiff` | invoke | G2 | wired |
| `getGitDiffStats` | invoke | G2 | wired |
| `gitCommit` | invoke | G2 | wired |
| `gitPush` | invoke | G2 | wired |
| `gitListBranches` | invoke | G2 | wired |
| `gitCheckout` | invoke | G2 | wired |
| `gitCreateBranch` | invoke | G2 | wired |
| `gitWorktreeCreate` | invoke | G2 | wired |
| `gitWorktreeRemove` | invoke | G2 | wired |
| `gitFileContents` | invoke | G2 | wired |
| `gitFetchPr` | invoke | G2 | wired |
| `gitPrFileDiff` | invoke | G2 | wired |
| `enableRemoteControl` | invoke | G2 | stub |
| `disableRemoteControl` | invoke | G2 | stub |
| `getRemoteControlState` | invoke | G2 | stub |
| `onRemoteStateChange` | on | G2 | stub |
| `scanSkills` | invoke | G2 | wired |
| `toggleSkill` | invoke | G2 | wired |
| `spawnTestTerminal` | invoke | G2 | wired |
| `sendTestTerminalInput` | send | G2 | wired |
| `resizeTestTerminal` | send | G2 | wired |
| `killTestTerminal` | send | G2 | wired |
| `onTestTerminalOutput` | on | G2 | wired |
| `onTestTerminalExit` | on | G2 | wired |
| `spawnFreeTerminal` | invoke | G2 | wired |
| `sendFreeTerminalInput` | send | G2 | wired |
| `resizeFreeTerminal` | send | G2 | wired |
| `killFreeTerminal` | send | G2 | wired |
| `activateFreeTerminal` | send | G2 | wired |
| `getFreeTerminalBuffer` | invoke | G2 | wired |
| `getFreeTerminalLayout` | invoke | G2 | wired |
| `listFreeTerminals` | invoke | G2 | wired |
| `onFreeTerminalOutput` | on | G2 | wired |
| `onFreeTerminalExit` | on | G2 | wired |
| `onFreeTerminalLayoutChanged` | on | G2 | wired |
After G2b only the four P11 remote-control rows are `stub` (108 of 112 methods are wired). `dismissNotification` is wired as a documented no-op (toasts are
renderer state; the daemon dismisses nothing).

Notes: `enableRemoteControl`, `disableRemoteControl`, `getRemoteControlState`, `onRemoteStateChange` are deferred to P11 (kept
so the API shape matches; the UI is removed: no component calls them, and the only remote-flavoured setting left in the UI is Hydra's
"remote error reporting" checkbox, a config flag nothing reads). Hydra's preload has no window-control methods, so there is nothing else Electron-only to stub.
