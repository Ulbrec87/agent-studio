// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/core/mapping/claudeCodeMapping.ts

import type { AgentState } from '../types'
import type {
  AgentActivity,
  ClaudeRegistryStatusFields,
  OverviewCounts,
  OverviewSession,
  OverviewStatus,
  WaitingReason
} from './types'

/** Output within this window means a non-Claude agent is working. */
export const ACTIVE_OUTPUT_WINDOW_MS = 5000
/** A working agent with no output for this long is "quiet". */
export const QUIET_AFTER_MS = 5 * 60 * 1000
/** An idle agent is stale after this long. */
export const STALE_AFTER_MS = 24 * 60 * 60 * 1000

export interface DerivedStatus {
  status: OverviewStatus
  waitingReason?: WaitingReason
}

const LIVE_STATUS = new Map<string, OverviewStatus>([
  ['busy', 'working'],
  // `shell` exists only in the registry; assumed to be working like `busy`.
  ['shell', 'working'],
  ['waiting', 'needs-you'],
  ['idle', 'idle']
])

const BACKGROUND_STATE = new Map<string, OverviewStatus>([
  ['working', 'working'],
  ['blocked', 'needs-you'],
  ['done', 'finished'],
  ['stopped', 'finished'],
  ['failed', 'failed']
])

const WAITING_REASON = new Map<string, WaitingReason>([
  ['permission prompt', 'permission'],
  ['input needed', 'question']
])

function mapWaitingFor(waitingFor: unknown): WaitingReason {
  if (typeof waitingFor !== 'string') return 'other'
  return WAITING_REASON.get(waitingFor.trim().toLowerCase()) ?? 'other'
}

/**
 * Maps a Claude Code registry entry to an overview status, or `null` when
 * neither field is recognised so the caller can fall back to output activity.
 *
 * A live process that is busy or waiting wins over `state`; an idle one yields
 * to a `state` that says more (blocked, done, stopped, failed). Never throws.
 */
export function mapClaudeRegistryStatus(fields: ClaudeRegistryStatusFields): DerivedStatus | null {
  try {
    const live = typeof fields.status === 'string' ? LIVE_STATUS.get(fields.status) : undefined
    const background =
      typeof fields.state === 'string' ? BACKGROUND_STATE.get(fields.state) : undefined
    const backgroundSaysMore = background !== undefined && background !== 'working'
    const status = live === 'idle' && backgroundSaysMore ? background : (live ?? background)
    if (!status) return null
    if (status !== 'needs-you') return { status }
    return { status, waitingReason: mapWaitingFor(fields.waitingFor) }
  } catch {
    return null
  }
}

/**
 * Derives the overview status of an agent. The registry entry is only
 * meaningful for Claude agents; other providers use output activity.
 */
export function deriveStatus(
  agent: AgentState,
  activity: AgentActivity,
  registry: ClaudeRegistryStatusFields | null,
  now: number
): DerivedStatus {
  switch (agent.status) {
    case 'starting':
      return { status: 'starting' }
    case 'idle':
      return { status: activity.lastExitCode === null ? 'not-running' : 'finished' }
    case 'errored':
      return { status: 'failed' }
    case 'running': {
      if (agent.provider === 'claude' && registry) {
        const mapped = mapClaudeRegistryStatus(registry)
        if (mapped) return mapped
      }
      const active = now - activity.lastOutputAt < ACTIVE_OUTPUT_WINDOW_MS
      return { status: active ? 'working' : 'idle' }
    }
  }
}

/** Whether the process of a session with this status is over (or never ran). */
export function isEnded(status: OverviewStatus): boolean {
  return status === 'finished' || status === 'failed' || status === 'not-running'
}

/** Idle for at least 24 h. */
export function isStale(status: OverviewStatus, statusSince: number, now: number): boolean {
  return status === 'idle' && now - statusSince >= STALE_AFTER_MS
}

/** How long a working agent has been silent, when at least 5 min; otherwise `undefined`. */
export function quietForMs(
  status: OverviewStatus,
  lastOutputAt: number,
  now: number
): number | undefined {
  if (status !== 'working') return undefined
  const quiet = now - lastOutputAt
  return quiet >= QUIET_AFTER_MS ? quiet : undefined
}

/** Counts sessions per status. Each session is counted once; stale idle ones go under `stale`. */
export function computeCounts(sessions: OverviewSession[], now: number): OverviewCounts {
  const counts: OverviewCounts = {
    working: 0,
    idle: 0,
    stale: 0,
    needsYou: 0,
    open: 0,
    finished: 0,
    failed: 0,
    notRunning: 0,
    total: sessions.length,
    longestWorkingMs: null,
    longestIdleMs: null
  }
  for (const s of sessions) {
    const age = Math.max(0, now - s.statusSince)
    switch (s.status) {
      case 'needs-you':
        counts.needsYou++
        break
      case 'working':
        counts.working++
        counts.longestWorkingMs = Math.max(counts.longestWorkingMs ?? 0, age)
        break
      case 'idle':
        if (s.stale) {
          counts.stale++
        } else {
          counts.idle++
          counts.longestIdleMs = Math.max(counts.longestIdleMs ?? 0, age)
        }
        break
      case 'finished':
        counts.finished++
        break
      case 'failed':
        counts.failed++
        break
      case 'not-running':
        counts.notRunning++
        break
    }
    if (!isEnded(s.status)) counts.open++
  }
  return counts
}
