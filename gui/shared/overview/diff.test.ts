import { describe, expect, it } from 'vitest'
import { diffSnapshots, GAP_AFTER_MS, pointFromSessions, resumedEvent } from './diff'
import type { OverviewSession } from './types'

function s(id: string, status: OverviewSession['status'], extra: Partial<OverviewSession> = {}): OverviewSession {
  return {
    agentId: id,
    name: `name-${id}`,
    provider: 'claude',
    model: 'sonnet',
    projectDir: `/work/${id}`,
    worktreeBranch: null,
    status,
    statusSince: 0,
    stale: false,
    pid: null,
    startedAt: 0,
    lastOutputAt: 0,
    ...extra
  } as OverviewSession
}

describe('diffSnapshots', () => {
  it('emits nothing for the first snapshot', () => {
    expect(diffSnapshots(null, [s('a', 'working'), s('b', 'needs-you')], 1000)).toEqual([])
  })

  it('reports an appeared agent', () => {
    const events = diffSnapshots([], [s('a', 'starting')], 1000)
    expect(events).toEqual([
      {
        id: 'a@1000:appeared',
        at: 1000,
        kind: 'appeared',
        agentId: 'a',
        name: 'name-a',
        projectDir: '/work/a',
        to: 'starting'
      }
    ])
  })

  it('reports a status change with the reason when entering needs-you', () => {
    const events = diffSnapshots([s('a', 'working')], [s('a', 'needs-you', { waitingReason: 'permission' })], 1000)
    expect(events).toMatchObject([{ kind: 'status', from: 'working', to: 'needs-you', waitingReason: 'permission' }])
    expect(events[0].waitedMs).toBeUndefined()
  })

  it('measures the wait when leaving needs-you from the previous statusSince', () => {
    const events = diffSnapshots(
      [s('a', 'needs-you', { statusSince: 4000 })],
      [s('a', 'working', { statusSince: 10_000 })],
      10_000
    )
    expect(events).toMatchObject([{ kind: 'status', from: 'needs-you', to: 'working', waitedMs: 6000 }])
  })

  it('reports an agent removed from Agent Studio, with its wait when it was waiting', () => {
    const events = diffSnapshots([s('a', 'needs-you', { statusSince: 500 }), s('b', 'idle')], [], 2000)
    expect(events).toMatchObject([
      { id: 'a@2000:ended', kind: 'ended', from: 'needs-you', waitedMs: 1500 },
      { id: 'b@2000:ended', kind: 'ended', from: 'idle' }
    ])
    expect(events[1].waitedMs).toBeUndefined()
  })

  it('ignores changes that are not a status change', () => {
    const prev = [s('a', 'idle', { statusSince: 1 })]
    const next = [s('a', 'idle', { statusSince: 1, name: 'renamed', branch: 'x', waitingText: 'y' })]
    expect(diffSnapshots(prev, next, 5000)).toEqual([])
  })
})

describe('pointFromSessions', () => {
  it('counts the three live statuses, all agents in total, and keeps each status', () => {
    const point = pointFromSessions(
      [s('a', 'needs-you'), s('b', 'working'), s('c', 'working'), s('d', 'idle'), s('e', 'finished')],
      7
    )
    expect(point).toEqual({
      at: 7,
      needsYou: 1,
      working: 2,
      idle: 1,
      total: 5,
      statuses: { a: 'needs-you', b: 'working', c: 'working', d: 'idle', e: 'finished' }
    })
  })
})

describe('resumedEvent', () => {
  it('is null without a previous point or within the gap limit', () => {
    expect(resumedEvent(null, 100_000)).toBeNull()
    expect(resumedEvent(100_000 - GAP_AFTER_MS, 100_000)).toBeNull()
  })

  it('reports the gap after the limit', () => {
    expect(resumedEvent(1000, 61_000)).toEqual({
      id: 'resumed@61000',
      at: 61_000,
      kind: 'resumed',
      agentId: '',
      name: '',
      projectDir: '',
      gapMs: 60_000
    })
  })
})
