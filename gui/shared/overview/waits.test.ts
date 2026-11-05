import { describe, expect, it } from 'vitest'
import type { OverviewEvent, WaitTransition } from './types'
import {
  LOCAL_ZONE,
  UTC_ZONE,
  coveredWithin,
  joinSpans,
  lastHourWaits,
  measuredUntil,
  transitionOfEvent,
  waitTotals
} from './waits'

const MIN = 60_000
const HOUR = 60 * MIN
const at = (iso: string): number => Date.parse(`${iso}Z`)
const NOW = at('2026-10-22T12:00:00')

function tr(agentId: string, direction: 'in' | 'out', time: number, extra: Partial<WaitTransition> = {}): WaitTransition {
  return { agentId, name: `agent-${agentId}`, projectDir: `/work/${agentId}`, at: time, direction, ...extra }
}

const spanOf = (from: string, to: string) => ({ start: at(from), end: at(to) })
const DAY_SPAN = spanOf('2026-10-16T00:00:00', '2026-10-22T12:00:00')

function totals(input: Partial<Parameters<typeof waitTotals>[0]>) {
  return waitTotals({
    transitions: [],
    spans: [DAY_SPAN],
    waitingNow: [],
    now: NOW,
    zone: UTC_ZONE,
    ...input
  })
}

describe('waitTotals', () => {
  it('adds up a wait from its entry to its exit', () => {
    const t = totals({ transitions: [tr('a', 'in', at('2026-10-22T10:00:00')), tr('a', 'out', at('2026-10-22T10:05:00'))] })
    expect(t.today).toMatchObject({ ms: 5 * MIN, count: 1, openMs: 0 })
    expect(t.week).toMatchObject({ ms: 5 * MIN, count: 1 })
    expect(t.longest.today).toEqual([{ agentId: 'a', name: 'agent-a', ms: 5 * MIN, count: 1, waitingNow: false }])
  })

  it('counts only measured time and keeps a wait across a gap as one wait', () => {
    const t = totals({
      spans: [spanOf('2026-10-22T09:55:00', '2026-10-22T10:02:00'), spanOf('2026-10-22T10:08:00', '2026-10-22T12:00:00')],
      transitions: [tr('a', 'in', at('2026-10-22T10:00:00')), tr('a', 'out', at('2026-10-22T10:10:00'))]
    })
    expect(t.today.ms).toBe(4 * MIN)
    expect(t.today.count).toBe(1)
    expect(t.today.measuredMs).toBe(119 * MIN)
  })

  it('counts a wait from the start of the span when its entry is not held', () => {
    const t = totals({
      spans: [spanOf('2026-10-22T10:00:00', '2026-10-22T12:00:00')],
      transitions: [tr('a', 'out', at('2026-10-22T10:05:00'))]
    })
    expect(t.today).toMatchObject({ ms: 5 * MIN, count: 1 })
  })

  it('does not count an exit that no measured span holds', () => {
    const t = totals({
      spans: [spanOf('2026-10-22T11:00:00', '2026-10-22T12:00:00')],
      transitions: [tr('a', 'out', at('2026-10-22T10:05:00'))]
    })
    expect(t.today.count).toBe(0)
  })

  it('splits a wait at midnight and counts it once per period', () => {
    const now = at('2026-10-22T01:00:00')
    const t = totals({
      now,
      spans: [spanOf('2026-10-21T20:00:00', '2026-10-22T01:00:00')],
      transitions: [tr('a', 'in', at('2026-10-21T23:30:00')), tr('a', 'out', at('2026-10-22T00:30:00'))]
    })
    expect(t.today).toMatchObject({ ms: 30 * MIN, count: 1 })
    expect(t.week).toMatchObject({ ms: 60 * MIN, count: 1 })
    expect(t.byDay).toHaveLength(7)
    expect(t.byDay[5]).toMatchObject({ day: '2026-10-21', ms: 30 * MIN, count: 1 })
    expect(t.byDay[6]).toMatchObject({ day: '2026-10-22', ms: 30 * MIN, count: 1 })
  })

  it('lists seven days oldest first with unmeasured days marked', () => {
    const t = totals({ spans: [spanOf('2026-10-21T00:00:00', '2026-10-22T12:00:00')] })
    expect(t.byDay.map((d) => d.day)).toEqual([
      '2026-10-16', '2026-10-17', '2026-10-18', '2026-10-19', '2026-10-20', '2026-10-21', '2026-10-22'
    ])
    expect(t.byDay.map((d) => d.measured)).toEqual([false, false, false, false, false, true, true])
    expect(t.byDay[6].measuredMs).toBe(12 * HOUR)
    expect(t.week.measuredMs).toBe(36 * HOUR)
  })

  it('counts two agents waiting at once for both', () => {
    const t = totals({
      transitions: [
        tr('a', 'in', at('2026-10-22T10:00:00')),
        tr('b', 'in', at('2026-10-22T10:00:00')),
        tr('b', 'out', at('2026-10-22T10:01:00')),
        tr('a', 'out', at('2026-10-22T10:01:00'))
      ]
    })
    expect(t.today).toMatchObject({ ms: 2 * MIN, count: 2 })
    expect(t.longest.today.map((l) => l.agentId)).toEqual(['a', 'b'])
  })

  it('runs an open wait to now and marks it', () => {
    const t = totals({
      transitions: [tr('a', 'in', at('2026-10-22T11:50:00'))],
      waitingNow: [{ agentId: 'a', name: 'agent-a', since: at('2026-10-22T11:50:00') }]
    })
    expect(t.today).toMatchObject({ ms: 10 * MIN, count: 1, openMs: 10 * MIN })
    expect(t.byDay[6].openMs).toBe(10 * MIN)
    expect(t.longest.today[0]).toMatchObject({ agentId: 'a', waitingNow: true })
  })

  it('counts an agent waiting now whose entry was never seen from its since', () => {
    const t = totals({ waitingNow: [{ agentId: 'z', name: 'zed', since: at('2026-10-22T11:30:00') }] })
    expect(t.today).toMatchObject({ ms: 30 * MIN, count: 1, openMs: 30 * MIN })
    expect(t.longest.today[0].name).toBe('zed')
  })

  it('ends an unended wait of an agent that is not waiting now at the end of its span', () => {
    const t = totals({
      spans: [spanOf('2026-10-22T09:00:00', '2026-10-22T10:30:00'), spanOf('2026-10-22T11:00:00', '2026-10-22T12:00:00')],
      transitions: [tr('a', 'in', at('2026-10-22T10:00:00'))]
    })
    expect(t.today).toMatchObject({ ms: 30 * MIN, count: 1, openMs: 0 })
  })

  it('carries a wait across a daemon restart while the agent still waits', () => {
    const t = totals({
      spans: [spanOf('2026-10-22T09:00:00', '2026-10-22T10:30:00'), spanOf('2026-10-22T11:00:00', '2026-10-22T12:00:00')],
      transitions: [tr('a', 'in', at('2026-10-22T10:00:00'))],
      waitingNow: [{ agentId: 'a', name: 'agent-a', since: at('2026-10-22T11:00:00') }]
    })
    expect(t.today).toMatchObject({ ms: 30 * MIN + HOUR, count: 1 })
  })

  it('ignores a second entry while a wait is open', () => {
    const t = totals({
      transitions: [
        tr('a', 'in', at('2026-10-22T10:00:00')),
        tr('a', 'in', at('2026-10-22T10:02:00')),
        tr('a', 'out', at('2026-10-22T10:04:00'))
      ]
    })
    expect(t.today).toMatchObject({ ms: 4 * MIN, count: 1 })
  })

  it('filters by agent', () => {
    const transitions = [
      tr('a', 'in', at('2026-10-22T10:00:00')),
      tr('a', 'out', at('2026-10-22T10:05:00')),
      tr('b', 'in', at('2026-10-22T10:00:00')),
      tr('b', 'out', at('2026-10-22T10:01:00'))
    ]
    const t = totals({
      transitions,
      agentFilter: (id) => id === 'b',
      waitingNow: [{ agentId: 'a', name: 'agent-a', since: at('2026-10-22T11:00:00') }]
    })
    expect(t.today).toMatchObject({ ms: MIN, count: 1 })
    expect(t.longest.today.map((l) => l.agentId)).toEqual(['b'])
  })

  it('lists at most five agents, longest first', () => {
    const transitions: WaitTransition[] = []
    for (let i = 1; i <= 7; i++) {
      transitions.push(tr(`x${i}`, 'in', at('2026-10-22T10:00:00')))
      transitions.push(tr(`x${i}`, 'out', at('2026-10-22T10:00:00') + i * MIN))
    }
    const t = totals({ transitions })
    expect(t.longest.week.map((l) => l.agentId)).toEqual(['x7', 'x6', 'x5', 'x4', 'x3'])
  })

  it('counts waits of earlier days in the week but not today', () => {
    const t = totals({
      transitions: [tr('a', 'in', at('2026-10-20T10:00:00')), tr('a', 'out', at('2026-10-20T10:10:00'))]
    })
    expect(t.today.count).toBe(0)
    expect(t.week).toMatchObject({ ms: 10 * MIN, count: 1 })
    expect(t.byDay[4]).toMatchObject({ day: '2026-10-20', ms: 10 * MIN, count: 1 })
  })

  it('has nothing without a ledger', () => {
    const t = totals({ spans: [] })
    expect(t.today).toEqual({ ms: 0, count: 0, measuredMs: 0, openMs: 0 })
    expect(t.byDay.every((d) => !d.measured)).toBe(true)
  })

  it('works in the local time zone', () => {
    const now = new Date(2026, 9, 22, 12, 0, 0).getTime()
    const t = waitTotals({
      transitions: [tr('a', 'in', new Date(2026, 9, 22, 8, 0).getTime()), tr('a', 'out', new Date(2026, 9, 22, 8, 10).getTime())],
      spans: [{ start: new Date(2026, 9, 20).getTime(), end: now }],
      waitingNow: [],
      now
    })
    expect(t.today.ms).toBe(10 * MIN)
    expect(t.byDay[6].day).toBe('2026-10-22')
    expect(LOCAL_ZONE.dayKey(now)).toBe('2026-10-22')
  })
})

describe('measured spans', () => {
  it('joins touching spans', () => {
    expect(joinSpans([{ start: 5, end: 9 }, { start: 0, end: 5 }, { start: 12, end: 13 }])).toEqual([
      { start: 0, end: 9 },
      { start: 12, end: 13 }
    ])
  })

  it('carries the newest span to now while it is fresh', () => {
    const spans = [{ start: NOW - HOUR, end: NOW - 6000 }]
    expect(measuredUntil(spans, [], NOW)).toEqual([{ start: NOW - HOUR, end: NOW }])
    expect(measuredUntil([{ start: NOW - HOUR, end: NOW - 20_000 }], [], NOW)).toEqual([
      { start: NOW - HOUR, end: NOW - 20_000 }
    ])
  })

  it('does not carry a span across a daemon start', () => {
    const spans = [{ start: NOW - HOUR, end: NOW - 6000 }]
    expect(measuredUntil(spans, [NOW - 3000], NOW)).toEqual([{ start: NOW - HOUR, end: NOW - 6000 }])
  })

  it('measures coverage of a window', () => {
    const spans = [{ start: 0, end: 10 }, { start: 20, end: 30 }]
    expect(coveredWithin(spans, 5, 25)).toBe(10)
    expect(coveredWithin(spans, 12, 18)).toBe(0)
  })
})

describe('lastHourWaits', () => {
  const spans = [spanOf('2026-10-22T10:00:00', '2026-10-22T12:00:00')]

  it('gives a bar per agent, longest first, with open ones marked', () => {
    const transitions = [
      tr('a', 'in', at('2026-10-22T11:10:00')),
      tr('a', 'out', at('2026-10-22T11:12:00')),
      tr('b', 'in', at('2026-10-22T11:20:00')),
      tr('b', 'out', at('2026-10-22T11:30:00')),
      tr('c', 'in', at('2026-10-22T11:55:00'))
    ]
    const r = lastHourWaits(transitions, [{ agentId: 'c', name: 'agent-c', since: at('2026-10-22T11:55:00') }], spans, NOW)
    expect(r.bars).toEqual([
      { agentId: 'b', name: 'agent-b', ms: 10 * MIN, count: 1, open: false },
      { agentId: 'c', name: 'agent-c', ms: 5 * MIN, count: 1, open: true },
      { agentId: 'a', name: 'agent-a', ms: 2 * MIN, count: 1, open: false }
    ])
    expect(r.from).toBe(NOW - HOUR)
    expect(r.unmeasuredMs).toBe(0)
  })

  it('cuts a wait at the start of the hour', () => {
    const r = lastHourWaits([tr('a', 'in', at('2026-10-22T10:30:00')), tr('a', 'out', at('2026-10-22T11:10:00'))], [], spans, NOW)
    expect(r.bars[0].ms).toBe(10 * MIN)
  })

  it('tells how much of the hour was not measured', () => {
    const r = lastHourWaits([], [], [spanOf('2026-10-22T11:30:00', '2026-10-22T12:00:00')], NOW)
    expect(r.bars).toEqual([])
    expect(r.unmeasuredMs).toBe(30 * MIN)
  })

  it('begins at the daemon start when that is later', () => {
    const from = at('2026-10-22T11:40:00')
    const r = lastHourWaits([], [], spans, NOW, { from })
    expect(r.from).toBe(from)
    expect(r.unmeasuredMs).toBe(0)
  })

  it('filters agents', () => {
    const r = lastHourWaits(
      [tr('a', 'in', at('2026-10-22T11:10:00')), tr('a', 'out', at('2026-10-22T11:12:00'))],
      [],
      spans,
      NOW,
      { agentFilter: () => false }
    )
    expect(r.bars).toEqual([])
  })
})

describe('transitionOfEvent', () => {
  const base: OverviewEvent = { id: 'x', at: 5, kind: 'status', agentId: 'a', name: 'n', projectDir: '/p' }
  it('reads moves into and out of needs-you', () => {
    expect(transitionOfEvent({ ...base, from: 'working', to: 'needs-you' })).toMatchObject({ direction: 'in', at: 5 })
    expect(transitionOfEvent({ ...base, from: 'needs-you', to: 'working' })).toMatchObject({ direction: 'out' })
    expect(transitionOfEvent({ ...base, kind: 'appeared', to: 'needs-you' })).toMatchObject({ direction: 'in' })
    expect(transitionOfEvent({ ...base, kind: 'ended', from: 'needs-you' })).toMatchObject({ direction: 'out' })
  })
  it('passes over everything else', () => {
    expect(transitionOfEvent({ ...base, from: 'working', to: 'idle' })).toBeNull()
    expect(transitionOfEvent({ ...base, kind: 'appeared', to: 'working' })).toBeNull()
    expect(transitionOfEvent({ ...base, kind: 'resumed', agentId: '', gapMs: 5 })).toBeNull()
  })
})
