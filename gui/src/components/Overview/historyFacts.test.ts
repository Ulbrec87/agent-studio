import { describe, expect, it } from 'vitest'
import type { OverviewEvent } from '@shared/overview/types'
import { eventMarkKey, eventText, eventsOfAgent, lastWaitFrom } from './historyFacts'

const NOW = 10_000_000
const HOUR = 3_600_000

function ev(at: number, extra: Partial<OverviewEvent> = {}): OverviewEvent {
  return { id: `a@${at}`, at, kind: 'status', agentId: 'a', name: 'alpha', projectDir: '/w', ...extra }
}

describe('lastWaitFrom', () => {
  it('picks the newest needs-you exit within the last hour', () => {
    const events = [
      ev(NOW - HOUR - 1, { from: 'needs-you', to: 'working', waitedMs: 1000 }),
      ev(NOW - 5000, { from: 'needs-you', to: 'working', waitedMs: 2000, name: 'older' }),
      ev(NOW - 1000, { from: 'needs-you', to: 'idle', waitedMs: 3000, name: 'newest' }),
      ev(NOW - 500, { from: 'working', to: 'needs-you' })
    ]
    expect(lastWaitFrom(events, NOW)).toEqual({ name: 'newest', waitedMs: 3000, endedAt: NOW - 1000 })
  })

  it('is undefined when no wait ended in the last hour', () => {
    expect(lastWaitFrom([ev(NOW - HOUR - 1, { from: 'needs-you', waitedMs: 1 })], NOW)).toBeUndefined()
    expect(lastWaitFrom([], NOW)).toBeUndefined()
  })
})

describe('eventsOfAgent and eventText', () => {
  it('keeps one agent, newest first', () => {
    const events = [ev(1), ev(3), ev(2, { agentId: 'b' })]
    expect(eventsOfAgent(events, 'a').map((e) => e.at)).toEqual([3, 1])
  })

  it('describes each kind and transition', () => {
    expect(eventText(ev(1, { kind: 'appeared', to: 'idle' }))).toBe('started')
    expect(eventText(ev(1, { kind: 'ended' }))).toBe('ended')
    expect(eventText(ev(1, { from: 'working', to: 'needs-you', waitingReason: 'permission' }))).toBe('is waiting for permission')
    expect(eventText(ev(1, { from: 'working', to: 'needs-you', waitingReason: 'question' }))).toBe('is asking a question')
    expect(eventText(ev(1, { from: 'working', to: 'needs-you' }))).toBe('is waiting for you')
    expect(eventText(ev(1, { from: 'idle', to: 'working' }))).toBe('is working')
    expect(eventText(ev(1, { from: 'working', to: 'idle' }))).toBe('went idle')
    expect(eventText(ev(1, { from: 'working', to: 'finished' }))).toBe('finished')
    expect(eventText(ev(1, { from: 'working', to: 'failed' }))).toBe('failed')
  })

  it('adds the wait when leaving needs-you', () => {
    expect(eventText(ev(1, { from: 'needs-you', to: 'working', waitedMs: 252_000 }))).toBe(
      'is working after waiting 4m 12s'
    )
  })

  it('describes a gap', () => {
    expect(eventText(ev(1, { kind: 'resumed', gapMs: 720_000 }))).toBe('Watching resumed \u2014 12m not measured')
  })

  it('keys the mark by status, ended or resumed', () => {
    expect(eventMarkKey(ev(1, { to: 'needs-you' }))).toBe('needs_you')
    expect(eventMarkKey(ev(1, { kind: 'ended' }))).toBe('ended')
    expect(eventMarkKey(ev(1, { kind: 'resumed' }))).toBe('resumed')
  })
})
