import type { ModelId, ProviderId } from '../types'

/** What an agent is doing right now, as shown in the Overview. */
export type OverviewStatus =
  | 'needs-you'
  | 'working'
  | 'idle'
  | 'starting'
  | 'finished'
  | 'failed'
  /** Restored or imported at startup and not run in this daemon run. */
  | 'not-running'

/** Why a session waits on the user. */
export type WaitingReason = 'permission' | 'question' | 'other'

/** The status fields of a Claude Code registry entry. Undocumented, so all `unknown`. */
export interface ClaudeRegistryStatusFields {
  /** Live process status: `busy`, `waiting`, `idle` or `shell`. */
  status?: unknown
  /** Background session state: `working`, `blocked`, `done`, `failed` or `stopped`. */
  state?: unknown
  /** Present while waiting, for example `permission prompt`. */
  waitingFor?: unknown
}

/** What the daemon knows about an agent's PTY, beyond `AgentState`. */
export interface AgentActivity {
  /** Epoch ms of the last PTY output. */
  lastOutputAt: number
  /** Exit code of the last run, `null` while running or when unknown. */
  lastExitCode: number | null
}

export interface OverviewSession {
  agentId: string
  name: string
  provider: ProviderId
  model: ModelId
  projectDir: string
  worktreeBranch: string | null
  /** Repository name, read from git files by the daemon (worktrees group under their main repo). */
  repoName?: string
  /** Live branch of `projectDir`; absent when detached or not a repository. */
  branch?: string
  /** Short commit id, set when HEAD is detached. */
  commit?: string
  status: OverviewStatus
  /** Epoch ms when the derived status last changed (kept by the tracker). */
  statusSince: number
  /** Epoch ms when its process ended, or its last activity when unknown; set for finished, failed and not-running. */
  endedAt?: number
  /** Exit code of the last run, when known. */
  exitCode?: number
  waitingReason?: WaitingReason
  /** What the session asks, one line (Step 6). */
  waitingText?: string
  /** Working with no output for at least `QUIET_AFTER_MS`. */
  quietForMs?: number
  /** Idle for at least `STALE_AFTER_MS`. */
  stale: boolean
  pid: number | null
  /** Epoch ms, `0` when the agent never started. */
  startedAt: number
  lastOutputAt: number
}

export interface OverviewCounts {
  working: number
  /** Idle sessions that are not stale. */
  idle: number
  stale: number
  needsYou: number
  /** Sessions whose process is alive or starting (not finished, failed or not-running). */
  open: number
  finished: number
  failed: number
  /** Restored or imported sessions that have not run in this daemon run. */
  notRunning: number
  total: number
  /** Longest time in `working`, `null` when none. */
  longestWorkingMs: number | null
  /** Longest time in `idle` among non-stale sessions, `null` when none. */
  longestIdleMs: number | null
}

export interface OverviewSnapshot {
  at: number
  sessions: OverviewSession[]
  counts: OverviewCounts
}

/** A change worth logging (Step 8). */
export interface OverviewEvent {
  /** Unique and stable, `<agentId>@<at>:<kind>` (`resumed@<at>` for gaps). */
  id: string
  at: number
  /** `resumed`: watching resumed after an unmeasured gap of `gapMs`; it has no agent. */
  kind: 'appeared' | 'status' | 'ended' | 'resumed'
  /** `''` for `resumed`. */
  agentId: string
  name: string
  projectDir: string
  from?: OverviewStatus
  to?: OverviewStatus
  waitingReason?: WaitingReason
  /** Set when leaving `needs-you`: how long the user was waited on. */
  waitedMs?: number
  /** Set on `resumed`: how long nothing was measured. */
  gapMs?: number
}

/** One 5 s sample of the counts per status (Step 8). */
export interface HistoryPoint {
  at: number
  needsYou: number
  working: number
  idle: number
  total: number
  /** Status per agent, for the Timeline. Names live in `HistoryResponse.agents`. */
  statuses: Record<string, OverviewStatus>
}

/** What the history response knows about an agent that appears in its points or events. */
export interface HistoryAgent {
  name: string
  projectDir: string
}

/** Answer of `GET /overview/history?since=<ms>`. */
export interface HistoryResponse {
  /** Where the history held begins; time before it is unmeasured. */
  since: number
  /** When this run of the daemon started watching. */
  startedAt: number
  /** Each start of the daemon (restored from disk and this run's), oldest first. */
  starts: number[]
  /** Oldest first. */
  points: HistoryPoint[]
  /** Oldest first. */
  events: OverviewEvent[]
  agents: Record<string, HistoryAgent>
}

/** A move of an agent into (`in`) or out of (`out`) `needs-you`. */
export interface WaitTransition {
  agentId: string
  name: string
  projectDir: string
  at: number
  direction: 'in' | 'out'
}

/** A stretch of time the daemon was measuring: points no more than 15 s apart, with no daemon start between. */
export interface MeasuredSpan {
  start: number
  end: number
}

/**
 * What the wait totals are worked out from: the last 9 days of moves into and
 * out of `needs-you` and of measured stretches, oldest first.
 */
export interface WaitLedgerData {
  transitions: WaitTransition[]
  spans: MeasuredSpan[]
  /** Each start of the daemon, oldest first; a start ends a measured stretch. */
  starts: number[]
}

/** Answer of `GET /overview/waits`. The renderer works the totals out, so the project filter and the clock stay client-side. */
export interface WaitsResponse {
  ledger: WaitLedgerData
}

/** An agent that needs the user now. */
export interface WaitingNow {
  agentId: string
  name: string
  /** Epoch ms when the wait began (the session's `statusSince`). */
  since: number
}

/** How long agents waited in a stretch of time. Only measured time counts. */
export interface WaitFigure {
  ms: number
  /** Waits that took measured time in the stretch. */
  count: number
  /** Measured time in the stretch. */
  measuredMs: number
  /** Part of `ms` that belongs to waits still open. */
  openMs: number
}

export interface WaitDayTotal extends WaitFigure {
  /** Local day, `2026-10-22`. */
  day: string
  /** False when nothing was measured that day. */
  measured: boolean
}

export interface LongestWait {
  agentId: string
  name: string
  ms: number
  count: number
  waitingNow: boolean
}

/** Time agents waited on the user (Step 11). */
export interface WaitTotals {
  today: WaitFigure
  /** Today and the six days before it. */
  week: WaitFigure
  /** The 7 days, oldest first. */
  byDay: WaitDayTotal[]
  /** At most five each, longest first. */
  longest: { today: LongestWait[]; week: LongestWait[] }
}

/** One bar of "Waited on you". */
export interface WaitBar {
  agentId: string
  name: string
  ms: number
  /** How many waits took measured time in the window. */
  count: number
  /** The agent needs the user now. */
  open: boolean
}

/** What the details drawer shows, read on demand from the transcript. */
export interface AgentDetails {
  lastMessage: { text: string; truncated: boolean } | { unavailable: string }
  /** Token counts of the newest reply; `cached` is the part of `input` read from a cache. */
  tokens: { input: number; cached?: number; output: number } | null
}
