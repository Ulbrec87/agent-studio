# Porting map — Hydra → Agent Studio

Hydra root: `~/PhpstormProjects/hydra`. Status: `todo` · `wip` · `done` · `dropped` · `deferred`.
Update the row (and add new rows) in the same commit that ports the code.

| Hydra source | Rust target | Phase | Status |
|--------------|-------------|-------|--------|
| `shared/types.ts` (Agent*, Provider*, AppConfig, DEFAULT_CONFIG) | `as-core::types`, `as-core::usage_settings` | P1 | done (minus Electron-only terminal keys, plus `prefixKey`) |
| `electron/config/ConfigStore.ts` | `as-core::config::ConfigStore` | P1 | done |
| `electron/config/KeybindingStore.ts`, `shared/keybindings.ts`, `useKeybindings.ts` | `as-core::keybindings` (catalog, defaults in TUI syntax, merge, tolerant parse), `as-tui::{keymap (parse, tables, conflicts), bindings_file (mtime polling)}` | P5 | done (no `mod+` syntax: keys are `prefix x` / `ctrl+x`; commands of later phases are in the catalog but unbound) |
| `electron/agents/providers.ts` | `as-core::agents::providers` | P1 | done |
| `electron/agents/ProviderModelCatalog.ts` | `as-core::agents::model_catalog` (+ `ListProviderModels`) | P4 | done (discovered list replaces the built-in one, which stays the fallback; failures cached 30 s) |
| `electron/agents/shellWrapper.ts` | `as-core::agents::shell` | P1 | done |
| `electron/agents/agentEnv.ts` | `as-core::agents::env` | P1 | done |
| `electron/util/fix-path.ts` | `as-core::agents::shell` (login-shell PATH) | P1 | done (`shell::fixed_path` returns the PATH; daemon applies it in P2) |
| `electron/agents/AgentManager.ts` | `as-core::agents::manager` (+ `agents::startup` trust-prompt gate) | P1/P4 | wip — session discovery (`SessionLocator`, backoff) and `import_sessions` done in P4; worktree → P10 (`TODO(P10)`) |
| `shared/terminalModelDetection.ts` | `as-core::agents::model_detect` | P1 | done |
| `electron/workspace/WorkspaceStore.ts` | `as-core::workspace` | P1 | done |
| `electron/daemon/*` (protocol, server, client, lock, lifecycle) | `as-proto` (messages, codec, client, lifecycle, lock), `as-daemon` (server, registry, handler) ; TUI uses `as_proto::DaemonClient` | P2 | done (no notifications/MCP/overview/usage/headless yet) |
| `electron/daemon/DaemonNotificationService.ts`, `notifications/*` | `as-daemon::notifications` (`Event::Notification`, `Request::ClientFocus`, OS notifications via `notify-rust`, feature `desktop-notifications`) | P3 | done (Overview-tracker `needs-you` source arrives with P6; no headless/usage notifications yet) |
| `src/components/{Sidebar,ChatView,Header,NewAgent,Notifications}`, `useClaudeProfiles`, `ClaudeProfileOptions` (P4: `as-tui::{sessions, new_agent}`, `ui::dialogs`) | `as-tui::ui::{sidebar,chat,header,dialogs,toasts,footer}`, `as-tui::{app,new_agent,sidebar,keymap,theme}` | P3 | done (resume + profile added in P4; no filter chips/sort) |
| `src/components/Terminal/TerminalPane.tsx` (xterm.js) | `as-tui::term::{vt,render,keys}` (vt100 behind `TerminalEmulator`, own cell renderer: `tui-term` not used) | P3 | done |
| `src/hooks/{useNotifications,useNeedsYouTitle}.ts`, quit dialog in `App.tsx` | `as-tui::app` (toasts, `(N) Agent Studio` title, bell, quit dialog) | P3 | done |
| `electron/sessions/SessionCatalog.ts`, `CodexSessionCatalog.ts`, `TranscriptReader.ts` | `as-core::sessions::{claude, codex, transcript}` (+ `locator` = `probeSessionIdFromCatalog`) | P4 | done (per-file mtime/size cache on top of the 5 s snapshot; sidechains and duplicate session files left out) |
| `electron/sessions/claudeProfiles.ts`, `shared/claudeProfiles.ts` | `as-core::sessions::profiles` | P4 | done |
| session/config handlers of `electron/ipc/handlers.ts`, startup import in `electron/daemon/index.ts` | `as-proto` requests `ListSessions`/`HideSession`/`ListClaudeProfiles`/`SetProjectProfile`/`ListProviderModels`, `as-daemon::{handler, services}` | P4 | done (startup import also honours `sessionMaxAgeDays`) |
| `src/components/GridView/{GridView,TerminalTile,BroadcastBar}.tsx`, `src/hooks/{useTileOrder,useViewMode}.ts`, `Header/ViewSwitcher.tsx` | `as-tui::{grid (tabs, tile order, layout math), app::view, ui::grid}` | P5 | done (tiles show a window of the agent's screen instead of a full xterm; drag-and-drop = `<`/`>`; broadcast sends `SendInput` per running agent) |
| `src/components/CommandPalette`, `Shortcuts`, `src/lib/fuzzy.ts` | `as-tui::palette` (nucleo), `ui::dialogs::{help_lines,help,palette}` | P5 | done |
| `src/components/Sidebar/{FilterChips,SortDropdown,FilterDropdown,SearchBar}.tsx`, `useFilterSort.ts` | `as-tui::sidebar` (`StatusChip`, `SortKey`, `ViewOpts`), `ui::sidebar` | P5 | done (single status chip All/Running/Waiting/Idle/Errored + sort created/recent/name/status/provider + search; Hydra's provider/yolo/age dropdown filters not ported) |
| `localStorage` UI state (view, tile order, expanded tile, ...) | `as-tui::state` (`<root>/tui-state.json`) | P5 | done |
| `shared/overview/{types,status,diff,buckets,grouping,waits,waitingText,details}.ts` | `as-core::overview::{types,status,diff,buckets,grouping,waits,waiting_text,details}` (all tests ported) | P6 | done (`CountKey` gained `Finished`/`Failed` for the counts sparklines) |
| `electron/overview/{claudeRegistry,processTree,gitInfo,waitingText,agentDetails}.ts` | `as-core::overview::{claude_registry,process_tree,git_info,transcript_tail}` | P6 | done (`ps` replaced by `sysinfo`, also on Windows; git info is read from `.git` files like Hydra, not the `git` CLI; no `O_NOFOLLOW`: `symlink_metadata` check before opening) |
| `electron/overview/{OverviewTracker,HistoryStore,historyFiles,WaitLedger}.ts` | `as-core::overview::{tracker,history_store,history_files,wait_ledger}` | P6 | done (tracker is synchronous and clock-injected, run from `spawn_blocking` by the daemon; the store has no timer, the daemon calls `flush()` every 5 s) |
| overview wiring in `electron/daemon/{index,DaemonServer,protocol}.ts`, IPC `OVERVIEW_*` | `as-proto` `Get{Overview,OverviewHistory,OverviewWaits,OverviewEvents,AgentDetails}` + events `OverviewSnapshot`/`OverviewEvents`; `as-daemon::services::{overview_loop,publish_overview}` | P6 | done |
| `DaemonNotificationService.connectOverviewEvents` + `hasRegistryMatch` suppression | `as-daemon::{notifications,services}` (`needs_you_body`, 5 s rule skipped for matched Claude agents) | P6 | done |
| `src/hooks/{useOverview,useOverviewHistory,useOverviewWaits,useNeedsYouTitle}.ts`, `Overview/{newSince,format,historyFacts,sessionFacts}.ts` | `as-tui::overview::{state,format}` (merge, marker, refresh timers, needs-you count/title) | P6 | done |
| `src/components/Overview/*` | `as-tui::{overview::model, ui::overview, app::overview}` | P6 | done (cards as ratatui blocks; no usage card yet (P7), no click-to-chart count history, no Stop/Restart buttons in the drawer, no copy-path) |
| `electron/usage/*`, `shared/usage/*` | `as-core::usage` | P7 | todo |
| `src/components/Usage*` | `as-tui::views::usage` | P7 | todo |
| `electron/mcp/*` | `as-daemon::mcp` (rmcp) | P8 | todo |
| `electron/headless/HeadlessOrchestrator.ts` | `as-core::headless` | P8 | todo |
| `src/components/Headless/*` | `as-tui::views::headless` | P8 | todo |
| `electron/git/GitService.ts` | `as-core::git` | P9 | todo |
| `src/components/GitPanel/*` | `as-tui::views::git` (unified diff viewer) | P9 | todo |
| `electron/fs/FileSystemService.ts` | `as-core::fs` | P9 | todo |
| `src/components/EditorPanel/*`, `FileSearchPopup` | `as-tui::views::files` (read-only + `$EDITOR`) | P9 | todo |
| free terminals (daemon routes, `FreeTerminalPanel.tsx`) | `as-core::terminals`, `as-tui::views::terminals` | P9 | todo |
| `src/components/Settings/*`, `electron/skills/SkillScanner.ts` | `as-tui::views::settings`, `as-core::skills` | P9 | todo |
| `EDITOR_REGISTRY` / open-in | `as-core::open_in` | P9 | todo |
| `electron/observability/*` | `tracing` setup in bin (`src/logging.rs`) + `diagnostics` subcommand | P0/P10 | wip |
| `electron/updates/UpdateService.ts` | `as-core::updates` (GitHub release check) | P10 | todo |
| `electron/remote/*`, `hydra-remote/`, `firebase-backend/` | — | P11 | deferred |
| Monaco editor, sounds, electron-builder, Homebrew/AUR/winget | — | — | dropped |
