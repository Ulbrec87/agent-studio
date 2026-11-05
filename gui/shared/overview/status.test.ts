import { describe, expect, it } from 'vitest'
import type { AgentState, AgentStatus, ProviderId } from '../types'
import {
  ACTIVE_OUTPUT_WINDOW_MS,
  QUIET_AFTER_MS,
  STALE_AFTER_MS,
  computeCounts,
  deriveStatus,
  isEnded,
  isStale,
  mapClaudeRegistryStatus,
  quietForMs
} from './status'
import type { OverviewSession, OverviewStatus } from './types'

describe('mapClaudeRegistryStatus', () => {
  it.each([
    ['busy', 'working'],
    ['shell', 'working'],
    ['idle', 'idle']
  ])('live status %s -> %s', (status, expected) => {
    expect(mapClaudeRegistryStatus({ status })).toEqual({ status: expected })
  })

  it.each([
    ['working', 'working'],
    ['done', 'finished'],
    ['stopped', 'finished'],
    ['failed', 'failed']
  ])('background state %s -> %s', (state, expected) => {
    expect(mapClaudeRegistryStatus({ state })).toEqual({ status: expected })
  })

  it('maps blocked to needs-you', () => {
    expect(mapClaudeRegistryStatus({ state: 'blocked' })).toEqual({
      status: 'needs-you',
      waitingReason: 'other'
    })
  })

  it.each([
    ['permission prompt', 'permission'],
    ['input needed', 'question'],
    ['  Permission Prompt ', 'permission'],
    ['something else', 'other'],
    [undefined, 'other'],
    [42, 'other']
  ])('waiting with waitingFor %s -> %s', (waitingFor, reason) => {
    expect(mapClaudeRegistryStatus({ status: 'waiting', waitingFor })).toEqual({
      status: 'needs-you',
      waitingReason: reason
    })
  })

  it('lets live busy and waiting win over state', () => {
    expect(mapClaudeRegistryStatus({ status: 'busy', state: 'failed' })).toEqual({
      status: 'working'
    })
    expect(mapClaudeRegistryStatus({ status: 'waiting', state: 'done' })?.status).toBe('needs-you')
  })

  it('lets live idle yield to a state that says more', () => {
    expect(mapClaudeRegistryStatus({ status: 'idle', state: 'failed' })).toEqual({
      status: 'failed'
    })
    expect(mapClaudeRegistryStatus({ status: 'idle', state: 'done' })).toEqual({
      status: 'finished'
    })
    expect(mapClaudeRegistryStatus({ status: 'idle', state: 'blocked' })?.status).toBe('needs-you')
  })

  it('keeps live idle over a working state', () => {
    expect(mapClaudeRegistryStatus({ status: 'idle', state: 'working' })).toEqual({
      status: 'idle'
    })
  })

  it('returns null for unknown or non-string values', () => {
    expect(mapClaudeRegistryStatus({})).toBeNull()
    expect(mapClaudeRegistryStatus({ status: 'zzz', state: 'yyy' })).toBeNull()
    expect(mapClaudeRegistryStatus({ status: 1, state: {} })).toBeNull()
    expect(mapClaudeRegistryStatus({ status: null, state: [] })).toBeNull()
  })

  it('falls back to state when status is unknown', () => {
    expect(mapClaudeRegistryStatus({ status: 'zzz', state: 'done' })).toEqual({
      status: 'finished'
    })
  })

  it('never throws on hostile input', () => {
    expect(mapClaudeRegistryStatus(null as never)).toBeNull()
  })
})

function agent(status: AgentStatus, provider: ProviderId = 'claude'): AgentState {
  return { status, provider } as AgentState
}

const NOW = 1_000_000_000
const recent = { lastOutputAt: NOW - 1000, lastExitCode: null }
const silent = { lastOutputAt: NOW - ACTIVE_OUTPUT_WINDOW_MS, lastExitCode: null }

describe('deriveStatus', () => {
  it('starting -> starting regardless of registry', () => {
    expect(deriveStatus(agent('starting'), recent, { status: 'busy' }, NOW)).toEqual({
      status: 'starting'
    })
  })

  it('idle (exited in this run) -> finished', () => {
    const exited = { lastOutputAt: NOW - 1000, lastExitCode: 0 }
    expect(deriveStatus(agent('idle'), exited, null, NOW)).toEqual({ status: 'finished' })
  })

  it('idle (never ran in this run) -> not-running', () => {
    expect(deriveStatus(agent('idle'), recent, null, NOW)).toEqual({ status: 'not-running' })
  })

  it('errored -> failed', () => {
    expect(deriveStatus(agent('errored'), recent, { status: 'busy' }, NOW)).toEqual({
      status: 'failed'
    })
  })

  it('running Claude uses the registry', () => {
    expect(deriveStatus(agent('running'), silent, { status: 'busy' }, NOW).status).toBe('working')
    expect(
      deriveStatus(agent('running'), recent, { status: 'waiting', waitingFor: 'input needed' }, NOW)
    ).toEqual({ status: 'needs-you', waitingReason: 'question' })
    expect(deriveStatus(agent('running'), recent, { status: 'idle' }, NOW).status).toBe('idle')
  })

  it('running Claude with unknown registry falls back to output', () => {
    expect(deriveStatus(agent('running'), recent, { status: 'zzz' }, NOW).status).toBe('working')
    expect(deriveStatus(agent('running'), silent, { status: 'zzz' }, NOW).status).toBe('idle')
  })

  it('running Claude without a registry entry uses output', () => {
    expect(deriveStatus(agent('running'), recent, null, NOW).status).toBe('working')
    expect(deriveStatus(agent('running'), silent, null, NOW).status).toBe('idle')
  })

  it.each<ProviderId>(['codex', 'opencode'])('running %s ignores the registry', (provider) => {
    expect(deriveStatus(agent('running', provider), silent, { status: 'busy' }, NOW).status).toBe(
      'idle'
    )
    expect(
      deriveStatus(agent('running', provider), recent, { status: 'waiting' }, NOW).status
    ).toBe('working')
  })

  it('output window boundary: 4999 ms working, 5000 ms idle', () => {
    const at = (ms: number) => ({ lastOutputAt: NOW - ms, lastExitCode: null })
    expect(deriveStatus(agent('running', 'codex'), at(4999), null, NOW).status).toBe('working')
    expect(deriveStatus(agent('running', 'codex'), at(5000), null, NOW).status).toBe('idle')
  })
})

describe('isEnded', () => {
  it('is true for finished, failed and not-running only', () => {
    for (const s of ['finished', 'failed', 'not-running'] as const) expect(isEnded(s)).toBe(true)
    for (const s of ['needs-you', 'working', 'idle', 'starting'] as const) expect(isEnded(s)).toBe(false)
  })
})

describe('isStale', () => {
  it('is stale at exactly 24 h idle', () => {
    expect(isStale('idle', NOW - STALE_AFTER_MS, NOW)).toBe(true)
    expect(isStale('idle', NOW - STALE_AFTER_MS + 1, NOW)).toBe(false)
  })

  it('only applies to idle', () => {
    for (const s of ['needs-you', 'working', 'starting', 'finished', 'failed'] as const) {
      expect(isStale(s, 0, NOW)).toBe(false)
    }
  })
})

describe('quietForMs', () => {
  it('returns the duration at 5 min and above for working agents', () => {
    expect(quietForMs('working', NOW - QUIET_AFTER_MS, NOW)).toBe(QUIET_AFTER_MS)
    expect(quietForMs('working', NOW - QUIET_AFTER_MS - 1000, NOW)).toBe(QUIET_AFTER_MS + 1000)
  })

  it('is undefined below 5 min', () => {
    expect(quietForMs('working', NOW - QUIET_AFTER_MS + 1, NOW)).toBeUndefined()
  })

  it('is undefined for non-working agents', () => {
    expect(quietForMs('idle', 0, NOW)).toBeUndefined()
    expect(quietForMs('needs-you', 0, NOW)).toBeUndefined()
  })
})

function session(status: OverviewStatus, ageMs: number, stale = false): OverviewSession {
  return {
    agentId: `${status}-${ageMs}`,
    name: 'a',
    provider: 'claude',
    model: 'sonnet',
    projectDir: '/p',
    worktreeBranch: null,
    status,
    statusSince: NOW - ageMs,
    stale,
    pid: null,
    startedAt: 0,
    lastOutputAt: 0
  }
}

describe('computeCounts', () => {
  it('handles no sessions', () => {
    expect(computeCounts([], NOW)).toEqual({
      working: 0,
      idle: 0,
      stale: 0,
      needsYou: 0,
      open: 0,
      finished: 0,
      failed: 0,
      notRunning: 0,
      total: 0,
      longestWorkingMs: null,
      longestIdleMs: null
    })
  })

  it('counts each session once, with stale apart from idle', () => {
    const counts = computeCounts(
      [
        session('needs-you', 10),
        session('working', 1000),
        session('working', 5000),
        session('idle', 2000),
        session('idle', 9000),
        session('idle', STALE_AFTER_MS + 5, true),
        session('starting', 1),
        session('finished', 1),
        session('failed', 1),
        session('not-running', 1)
      ],
      NOW
    )
    expect(counts).toEqual({
      working: 2,
      idle: 2,
      stale: 1,
      needsYou: 1,
      open: 7,
      finished: 1,
      failed: 1,
      notRunning: 1,
      total: 10,
      longestWorkingMs: 5000,
      longestIdleMs: 9000
    })
  })
})
