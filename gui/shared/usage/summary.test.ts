import { describe, expect, it } from 'vitest'
import { summarize } from './summary'
import { emptyTokens, type TokenCounts, type UsageBucket } from './types'

const hour = (y: number, m: number, d: number, h: number) => new Date(y, m - 1, d, h).getTime()
const tok = (input: number, output = 0): TokenCounts => ({ ...emptyTokens(), input, output })

function bucket(h: number, extra: Partial<UsageBucket> = {}): UsageBucket {
  return {
    hour: h,
    provider: 'claude',
    model: 'claude-opus-5',
    projectKey: '/work/a',
    tokens: tok(100, 50),
    messages: 2,
    costUsd: 1,
    unpricedMessages: 0,
    ...extra
  }
}

const NOW = new Date(2026, 9, 22, 15, 30)

describe('summarize', () => {
  it('lists every provider in order, with zeros when empty', () => {
    const s = summarize([], 'today', NOW)
    expect(s.byProvider.map((p) => p.provider)).toEqual(['claude', 'codex'])
    expect(s.byProvider.every((p) => p.totalTokens === 0 && p.messages === 0 && !p.costPartial)).toBe(true)
    expect(s.totalTokens).toBe(0)
    expect(s.previous).toBeNull()
    expect(s.tokensChangePct).toBeNull()
    expect(s.slots).toHaveLength(24)
  })

  it('totals tokens, messages and cost per provider', () => {
    const s = summarize(
      [
        bucket(hour(2026, 10, 22, 9)),
        bucket(hour(2026, 10, 22, 10), { provider: 'codex', model: 'gpt-5', tokens: tok(10), messages: 1, costUsd: 0.5 })
      ],
      'today',
      NOW
    )
    expect(s.totalTokens).toBe(160)
    expect(s.messages).toBe(3)
    expect(s.costUsd).toBeCloseTo(1.5)
    expect(s.byProvider[0]).toMatchObject({ provider: 'claude', totalTokens: 150, messages: 2 })
    expect(s.byProvider[1]).toMatchObject({ provider: 'codex', totalTokens: 10, costUsd: 0.5 })
  })

  it('sorts models by tokens, largest first', () => {
    const s = summarize(
      [
        bucket(hour(2026, 10, 22, 9), { model: 'small', tokens: tok(5) }),
        bucket(hour(2026, 10, 22, 9), { model: 'big', tokens: tok(500) }),
        bucket(hour(2026, 10, 22, 10), { model: 'small', tokens: tok(5) }),
        bucket(hour(2026, 10, 22, 11), { model: 'mid', tokens: tok(100) })
      ],
      'today',
      NOW
    )
    expect(s.byModel.map((m) => [m.model, m.totalTokens])).toEqual([
      ['big', 500],
      ['mid', 100],
      ['small', 10]
    ])
  })

  it('flags partial cost when a record had no price', () => {
    const s = summarize(
      [bucket(hour(2026, 10, 22, 9)), bucket(hour(2026, 10, 22, 9), { provider: 'codex', unpricedMessages: 1 })],
      'today',
      NOW
    )
    expect(s.costPartial).toBe(true)
    expect(s.byProvider[0].costPartial).toBe(false)
    expect(s.byProvider[1].costPartial).toBe(true)
    expect(summarize([bucket(hour(2026, 10, 22, 9))], 'today', NOW).costPartial).toBe(false)
  })

  it('filters by project', () => {
    const s = summarize(
      [bucket(hour(2026, 10, 22, 9)), bucket(hour(2026, 10, 22, 9), { projectKey: '/work/b', tokens: tok(1000) })],
      'today',
      NOW,
      { projectKey: '/work/b' }
    )
    expect(s.totalTokens).toBe(1000)
    expect(summarize([bucket(hour(2026, 10, 22, 9), { projectKey: '' })], 'today', NOW, { projectKey: '' }).totalTokens).toBe(150)
  })

  it('excludes buckets outside the range', () => {
    const s = summarize(
      [
        bucket(hour(2026, 10, 21, 23)), // yesterday
        bucket(hour(2026, 10, 22, 0)), // in
        bucket(hour(2026, 10, 22, 15)), // in (hour starts before now)
        bucket(hour(2026, 10, 22, 16)) // after now
      ],
      'today',
      NOW
    )
    expect(s.totalTokens).toBe(300)
  })

  it('puts tokens in the right slots', () => {
    const s = summarize(
      [bucket(hour(2026, 10, 22, 9)), bucket(hour(2026, 10, 22, 9), { provider: 'codex', tokens: tok(7) })],
      'today',
      NOW
    )
    expect(s.slots[9].tokensByProvider).toEqual({ claude: 150, codex: 7 })
    expect(s.slots[8].tokensByProvider).toEqual({ claude: 0, codex: 0 })
    const week = summarize([bucket(hour(2026, 10, 20, 12))], 'week', NOW)
    expect(week.slots[1]).toMatchObject({ label: 'Tue', tokensByProvider: { claude: 150, codex: 0 } })
  })

  it('compares with the previous range', () => {
    const s = summarize(
      [
        bucket(hour(2026, 10, 22, 9), { tokens: tok(300), costUsd: 3 }),
        bucket(hour(2026, 10, 21, 9), { tokens: tok(200), costUsd: 2 }),
        bucket(hour(2026, 10, 21, 20), { tokens: tok(999) }) // yesterday after 15:30: not in the span
      ],
      'today',
      NOW
    )
    expect(s.previous).toEqual({ totalTokens: 200, costUsd: 2 })
    expect(s.tokensChangePct).toBeCloseTo(50)
  })

  it('has no change percentage without previous data or when it is zero', () => {
    const none = summarize([bucket(hour(2026, 10, 22, 9))], 'today', NOW)
    expect(none.previous).toBeNull()
    expect(none.tokensChangePct).toBeNull()
    const zero = summarize(
      [bucket(hour(2026, 10, 22, 9)), bucket(hour(2026, 10, 21, 9), { tokens: tok(0) })],
      'today',
      NOW
    )
    expect(zero.previous).toEqual({ totalTokens: 0, costUsd: 1 })
    expect(zero.tokensChangePct).toBeNull()
  })

  it('reports a decrease as a negative percentage', () => {
    const s = summarize(
      [bucket(hour(2026, 10, 22, 9), { tokens: tok(50) }), bucket(hour(2026, 10, 21, 9), { tokens: tok(200) })],
      'today',
      NOW
    )
    expect(s.tokensChangePct).toBeCloseTo(-75)
  })
})
