# Porting map — Hydra → Agent Studio

Hydra root: `~/PhpstormProjects/hydra`. Status: `todo` · `wip` · `done` · `dropped` · `deferred`.
Update the row (and add new rows) in the same commit that ports the code.

| Hydra source | Rust target | Phase | Status |
|--------------|-------------|-------|--------|
| `shared/types.ts` (Agent*, Provider*, AppConfig, DEFAULT_CONFIG) | `as-core::types`, `as-core::config` | P1 | todo |
| `electron/config/ConfigStore.ts` | `as-core::config::ConfigStore` | P1 | todo |
| `electron/config/KeybindingStore.ts`, `shared/keybindings.ts` | `as-core::keybindings` (+ TUI key parsing in `as-tui::keymap`) | P5 | todo |
| `electron/agents/providers.ts` | `as-core::agents::providers` | P1 | todo |
| `electron/agents/ProviderModelCatalog.ts` | `as-core::agents::model_catalog` | P4 | todo |
| `electron/agents/shellWrapper.ts` | `as-core::agents::shell` | P1 | todo |
| `electron/agents/agentEnv.ts` | `as-core::agents::env` | P1 | todo |
| `electron/util/fix-path.ts` | `as-core::agents::shell` (login-shell PATH) | P1 | todo |
| `electron/agents/AgentManager.ts` | `as-core::agents::manager` | P1 | todo |
| `shared/terminalModelDetection.ts` | `as-core::agents::model_detect` | P1 | todo |
| `electron/workspace/WorkspaceStore.ts` | `as-core::workspace` | P1 | todo |
| `electron/daemon/*` (protocol, server, client, lock, lifecycle) | `as-proto`, `as-daemon`, `as-tui::client` | P2 | todo |
| `electron/daemon/DaemonNotificationService.ts`, `notifications/*` | `as-daemon::notifications` | P3 | todo |
| `src/components/{Sidebar,ChatView,Header,NewAgent,Notifications}` | `as-tui::views::{sidebar,chat,header,new_agent,toasts}` | P3 | todo |
| `src/components/Terminal/TerminalPane.tsx` (xterm.js) | `as-tui::widgets::terminal` (vt100 + tui-term) | P3 | todo |
| `electron/sessions/SessionCatalog.ts`, `CodexSessionCatalog.ts`, `TranscriptReader.ts` | `as-core::sessions` | P4 | todo |
| `electron/sessions/claudeProfiles.ts`, `shared/claudeProfiles.ts` | `as-core::sessions::profiles` | P4 | todo |
| `src/components/GridView/*` | `as-tui::views::grid` | P5 | todo |
| `src/components/CommandPalette`, `Shortcuts` | `as-tui::views::{palette,help}` | P5 | todo |
| `electron/overview/*`, `shared/overview/*` | `as-core::overview` | P6 | todo |
| `src/components/Overview/*` | `as-tui::views::overview` | P6 | todo |
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
