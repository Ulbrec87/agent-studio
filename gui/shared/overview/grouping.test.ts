import { describe, expect, it } from 'vitest'
import { boardColumns, ENDED_GROUP_LIMIT, groupByRepo, groupByStatus, NO_REPOSITORY } from './grouping'
import type { OverviewSession } from './types'

function s(id: string, status: OverviewSession['status'], extra: Partial<OverviewSession> = {}): OverviewSession {
  return {
    agentId: id,
    name: id,
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

describe('groupByStatus', () => {
  it('orders groups and leaves needs-you out', () => {
    const groups = groupByStatus([
      s('fin', 'finished'),
      s('start', 'starting'),
      s('need', 'needs-you'),
      s('stale', 'idle', { stale: true }),
      s('idle', 'idle'),
      s('fail', 'failed'),
      s('work', 'working'),
      s('nr', 'not-running')
    ])
    expect(groups.map((g) => g.label)).toEqual([
      'Working',
      'Idle',
      'Finished / Failed',
      'Not running',
      'Starting'
    ])
    expect(groups[1].sessions.map((x) => x.agentId)).toEqual(['idle', 'stale'])
    expect(groups[2].sessions.map((x) => x.agentId)).toEqual(['fail', 'fin'])
    expect(groups.flatMap((g) => g.sessions).some((x) => x.agentId === 'need')).toBe(false)
  })

  it('drops empty groups and puts the longest in status first', () => {
    const groups = groupByStatus([s('b', 'working', { statusSince: 20 }), s('a', 'working', { statusSince: 10 })])
    expect(groups).toHaveLength(1)
    expect(groups[0].sessions.map((x) => x.agentId)).toEqual(['a', 'b'])
  })
})

describe('ended ordering and limits', () => {
  it('sorts ended groups newest first and keeps working and idle ascending', () => {
    const groups = groupByStatus([
      s('f-old', 'finished', { statusSince: 10 }),
      s('f-new', 'finished', { statusSince: 30 }),
      s('x-fail', 'failed', { statusSince: 5 }),
      s('nr-old', 'not-running', { statusSince: 1 }),
      s('nr-new', 'not-running', { statusSince: 2 }),
      s('w-new', 'working', { statusSince: 20 }),
      s('w-old', 'working', { statusSince: 10 }),
      s('i-new', 'idle', { statusSince: 20 }),
      s('i-old', 'idle', { statusSince: 10 })
    ])
    const ids = (key: string) => groups.find((g) => g.key === key)?.sessions.map((x) => x.agentId)
    expect(ids('ended')).toEqual(['x-fail', 'f-new', 'f-old'])
    expect(ids('not-running')).toEqual(['nr-new', 'nr-old'])
    expect(ids('working')).toEqual(['w-old', 'w-new'])
    expect(ids('idle')).toEqual(['i-old', 'i-new'])
  })

  it('sets the limit on ended and not-running groups only', () => {
    const groups = groupByStatus([
      s('w', 'working'),
      s('i', 'idle'),
      s('f', 'finished'),
      s('nr', 'not-running'),
      s('st', 'starting')
    ])
    expect(Object.fromEntries(groups.map((g) => [g.key, g.limit]))).toEqual({
      working: undefined,
      idle: undefined,
      ended: ENDED_GROUP_LIMIT,
      'not-running': ENDED_GROUP_LIMIT,
      starting: undefined
    })
    expect(ENDED_GROUP_LIMIT).toBe(10)
    expect(groupByRepo([s('f', 'finished')], () => 'r')[0].limit).toBeUndefined()
  })
})

describe('groupByRepo', () => {
  it('sorts repositories, puts "No repository" last and keeps status order', () => {
    const repo = (x: OverviewSession) => ({ a: 'zeta', b: 'alpha', c: 'alpha' })[x.agentId] ?? null
    const groups = groupByRepo([s('a', 'working'), s('c', 'idle'), s('b', 'working'), s('d', 'finished')], repo)
    expect(groups.map((g) => g.label)).toEqual(['alpha', 'zeta', NO_REPOSITORY])
    expect(groups[0].sessions.map((x) => x.agentId)).toEqual(['b', 'c'])
  })
})

describe('boardColumns', () => {
  it('caps columns, counts hidden and separates stale idle', () => {
    const many = Array.from({ length: 7 }, (_, i) => s(`w${i}`, 'working'))
    const idles = [s('i1', 'idle', { stale: true }), s('i2', 'idle')]
    const cols = boardColumns([...many, ...idles, s('n', 'needs-you'), s('f', 'failed'), s('x', 'finished'), s('nr', 'not-running')])
    const by = Object.fromEntries(cols.map((c) => [c.key, c]))
    expect(by.working.items).toHaveLength(5)
    expect(by.working.hidden).toBe(2)
    expect(by.idle.items.map((x) => x.agentId)).toEqual(['i2', 'i1'])
    expect(by.idle.stale).toBe(1)
    expect(by['needs-you'].total).toBe(1)
    expect(by.ended.items.map((x) => x.agentId)).toEqual(['f', 'x'])
    expect(cols.flatMap((c) => c.items).some((x) => x.agentId === 'nr')).toBe(false)
    expect(by.ended.notRunning).toBe(1)
    expect(by.working.notRunning).toBeUndefined()
  })

  it('lists the newest ended sessions first', () => {
    const cols = boardColumns([s('o', 'finished', { statusSince: 1 }), s('n', 'finished', { statusSince: 9 })])
    expect(cols[3].items.map((x) => x.agentId)).toEqual(['n', 'o'])
  })
})
