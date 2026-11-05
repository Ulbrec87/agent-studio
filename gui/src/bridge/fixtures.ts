// In-memory fixtures served by the mock transport (plain browsers, tests, and the desktop app until G1 wires the daemon).
import {
  DEFAULT_CONFIG,
  type AgentState,
  type AppUpdateState,
  type AppConfig,
  type FreeTerminalLayout
} from '@shared/types'
import type { OverviewCounts, OverviewSession, OverviewSnapshot, OverviewStatus } from '@shared/overview/types'
import type { UsageState } from '@shared/usage/types'
import type { AppInfo } from './transport'

export const MOCK_APP_INFO: AppInfo = {
  name: 'Agent Studio',
  version: '0.1.0',
  dataDir: '~/.config/agent-studio',
  platform: 'browser'
}

export function defaultConfig(): AppConfig {
  // The daemon owns the real config from G1; until then the UI starts from Hydra's defaults.
  return structuredClone(DEFAULT_CONFIG)
}

const NOW = Date.now()
const iso = (msAgo: number): string => new Date(NOW - msAgo).toISOString()

function agent(partial: Partial<AgentState> & Pick<AgentState, 'id' | 'name' | 'projectDir' | 'status'>): AgentState {
  return {
    provider: 'claude',
    model: 'sonnet',
    yolo: false,
    isManager: false,
    sessionId: null,
    initialPrompt: '',
    createdAt: iso(3_600_000),
    workMode: 'local',
    worktreePath: null,
    worktreeBranch: null,
    pid: null,
    restartCount: 0,
    startedAt: iso(3_000_000),
    endedAt: null,
    lastActivityAt: iso(20_000),
    ...partial
  }
}

/** A few agents in different states, in two projects, so the sidebar, chat and grid views have something to show. */
export function fixtureAgents(): AgentState[] {
  return [
    agent({
      id: 'a1b2c3d4',
      name: 'refactor-parser',
      projectDir: '/home/you/projects/agent-studio',
      status: 'running',
      model: 'opus',
      pid: 4242,
      initialPrompt: 'Refactor the parser module'
    }),
    agent({
      id: 'e5f6a7b8',
      name: 'fix-flaky-tests',
      projectDir: '/home/you/projects/agent-studio',
      status: 'idle',
      pid: 4243,
      lastActivityAt: iso(600_000)
    }),
    agent({
      id: 'c9d0e1f2',
      name: 'codex-review',
      projectDir: '/home/you/projects/website',
      status: 'running',
      provider: 'codex',
      model: 'gpt-5.3-codex',
      pid: 4244,
      yolo: true
    }),
    agent({
      id: '0a1b2c3d',
      name: 'migrate-db',
      projectDir: '/home/you/projects/website',
      status: 'errored',
      provider: 'opencode',
      model: 'opencode/big-pickle',
      startedAt: iso(900_000),
      endedAt: iso(300_000)
    })
  ]
}

const OVERVIEW_STATUS: Record<AgentState['status'], OverviewStatus> = {
  running: 'working',
  idle: 'idle',
  errored: 'failed',
  starting: 'starting'
}

export function overviewSnapshot(agents: AgentState[]): OverviewSnapshot {
  const at = Date.now()
  const sessions: OverviewSession[] = agents.map((a, i) => {
    // Show one "needs you" session so the Overview is not all green.
    const status: OverviewStatus = a.id === 'c9d0e1f2' ? 'needs-you' : OVERVIEW_STATUS[a.status]
    return {
      agentId: a.id,
      name: a.name,
      provider: a.provider,
      model: a.model,
      projectDir: a.projectDir,
      worktreeBranch: a.worktreeBranch,
      repoName: a.projectDir.split('/').pop(),
      branch: i < 2 ? 'main' : 'develop',
      status,
      statusSince: at - 120_000 * (i + 1),
      ...(status === 'failed' ? { endedAt: at - 300_000, exitCode: 1 } : {}),
      ...(status === 'needs-you'
        ? { waitingReason: 'permission' as const, waitingText: 'Run `cargo test --workspace`?' }
        : {}),
      stale: false,
      pid: a.pid,
      startedAt: a.startedAt ? Date.parse(a.startedAt) : 0,
      lastOutputAt: Date.parse(a.lastActivityAt)
    }
  })
  const count = (s: OverviewStatus): number => sessions.filter((x) => x.status === s).length
  const counts: OverviewCounts = {
    working: count('working'),
    idle: count('idle'),
    stale: 0,
    needsYou: count('needs-you'),
    open: sessions.filter((s) => !['finished', 'failed', 'not-running'].includes(s.status)).length,
    finished: count('finished'),
    failed: count('failed'),
    notRunning: count('not-running'),
    total: sessions.length,
    longestWorkingMs: count('working') > 0 ? 240_000 : null,
    longestIdleMs: count('idle') > 0 ? 240_000 : null
  }
  return { at, sessions, counts }
}

export function emptyUsageState(): UsageState {
  return {
    claude: [],
    codex: null,
    indexing: { indexing: false, progress: null, lastScanAt: null },
    dataVersion: 0,
    updatedAt: Date.now()
  }
}

/** "Up to date": nothing to download, no error. The desktop app has no self-updater yet (see plan §4). */
export function upToDateState(info: AppInfo): AppUpdateState {
  return {
    supported: true,
    platform: info.platform,
    checking: false,
    available: false,
    downloaded: false,
    downloading: false,
    currentVersion: info.version,
    latestVersion: info.version,
    releaseDate: null,
    releaseNotes: null,
    error: null,
    canAutoInstall: false,
    installMethod: null,
    downloadUrl: null,
    releaseUrl: null
  }
}

export function emptyTerminalLayout(projectDir: string): FreeTerminalLayout {
  return { projectDir, groups: [], activeGroupId: null, panes: [] }
}

/** Shown in an agent's terminal until real PTY output flows (G1). */
export function sampleTerminalBuffer(name: string): string[] {
  return [
    '\x1b[38;5;173m●\x1b[0m \x1b[1mAgent Studio\x1b[0m \x1b[2m(stub bridge, no daemon connected)\x1b[0m\r\n',
    `\x1b[2magent:\x1b[0m ${name}\r\n`,
    '\r\n',
    '\x1b[32m✓\x1b[0m This terminal shows fixture output. Real agents arrive with the daemon bridge (G1).\r\n',
    '\x1b[38;5;173m>\x1b[0m '
  ]
}
