import { describe, expect, it } from 'vitest'
import { buildUsageReport } from './report'
import { emptyTokens, type TokenCounts, type UsageBucket } from './types'

const at = (y: number, m: number, d: number, h: number, min = 0) => new Date(y, m - 1, d, h, min).getTime()
const tok = (input: number, output = 0): TokenCounts => ({ ...emptyTokens(), input, output })
const NOW = new Date(2026, 9, 23, 15, 30)

function bucket(hour: number, extra: Partial<UsageBucket> = {}): UsageBucket {
  return {
    hour,
    provider: 'claude',
    model: 'claude-opus-5',
    projectKey: '/work/app',
    tokens: tok(100, 50),
    messages: 2,
    costUsd: 1,
    unpricedMessages: 0,
    ...extra
  }
}

describe('buildUsageReport', () => {
  it('lists every local day oldest first, empty days included', () => {
    const r = buildUsageReport([], { days: 7, provider: 'all', now: NOW })
    expect(r.daily.map((d) => d.date)).toEqual([
      '2026-10-17', '2026-10-18', '2026-10-19', '2026-10-20', '2026-10-21', '2026-10-22', '2026-10-23'
    ])
    expect(r.daily.every((d) => d.totalTokens === 0 && d.models.length === 0 && !d.costPartial)).toBe(true)
    expect(r.provider).toBe('all')
    expect(r.generatedAt).toBe(NOW.getTime())
  })

  it('crosses month boundaries', () => {
    const r = buildUsageReport([], { days: 3, provider: 'all', now: new Date(2026, 10, 1, 9) })
    expect(r.daily.map((d) => d.date)).toEqual(['2026-10-30', '2026-10-31', '2026-11-01'])
  })

  it('assigns hours to local days at the boundaries', () => {
    const r = buildUsageReport(
      [bucket(at(2026, 10, 22, 23), { tokens: tok(1) }), bucket(at(2026, 10, 23, 0), { tokens: tok(10) })],
      { days: 7, provider: 'all', now: NOW }
    )
    const byDate = Object.fromEntries(r.daily.map((d) => [d.date, d.totalTokens]))
    expect(byDate['2026-10-22']).toBe(1)
    expect(byDate['2026-10-23']).toBe(10)
  })

  it('ignores buckets outside the window', () => {
    const r = buildUsageReport(
      [bucket(at(2026, 10, 16, 23)), bucket(at(2026, 10, 24, 1))],
      { days: 7, provider: 'all', now: NOW }
    )
    expect(r.daily.reduce((s, d) => s + d.totalTokens, 0)).toBe(0)
  })

  it('filters by provider and sums both for all', () => {
    const buckets = [
      bucket(at(2026, 10, 23, 9), { provider: 'claude', tokens: tok(100) }),
      bucket(at(2026, 10, 23, 10), { provider: 'codex', model: 'gpt-5', tokens: tok(40) })
    ]
    const today = (p: 'all' | 'claude' | 'codex') =>
      buildUsageReport(buckets, { days: 7, provider: p, now: NOW }).daily[6]
    expect(today('claude').totalTokens).toBe(100)
    expect(today('codex').totalTokens).toBe(40)
    expect(today('codex').models.map((m) => m.model)).toEqual(['gpt-5'])
    expect(today('all').totalTokens).toBe(140)
    expect(today('all').messages).toBe(4)
    expect(today('all').costUsd).toBe(2)
  })

  it('merges models and projects and sorts them by tokens desc', () => {
    const buckets = [
      bucket(at(2026, 10, 23, 9), { model: 'a', projectKey: '/w/small', tokens: tok(10) }),
      bucket(at(2026, 10, 23, 10), { model: 'b', projectKey: '/w/big', tokens: tok(500, 100) }),
      bucket(at(2026, 10, 23, 11), { model: 'a', projectKey: '/w/small', tokens: tok(20) })
    ]
    const day = buildUsageReport(buckets, { days: 7, provider: 'all', now: NOW }).daily[6]
    expect(day.models.map((m) => [m.model, m.totalTokens, m.tokens.input])).toEqual([
      ['b', 600, 500],
      ['a', 30, 30]
    ])
    expect(day.projects.map((p) => [p.label, p.projectKey, p.totalTokens])).toEqual([
      ['big', '/w/big', 600],
      ['small', '/w/small', 30]
    ])
    expect(day.tokens.input).toBe(530)
  })

  it('marks a day as partial when any message was unpriced', () => {
    const buckets = [
      bucket(at(2026, 10, 23, 9)),
      bucket(at(2026, 10, 22, 9), { unpricedMessages: 1, costUsd: 0 })
    ]
    const r = buildUsageReport(buckets, { days: 7, provider: 'all', now: NOW })
    expect(r.daily[6].costPartial).toBe(false)
    expect(r.daily[5].costPartial).toBe(true)
  })

  it('labels an empty project key as Unknown', () => {
    const day = buildUsageReport([bucket(at(2026, 10, 23, 9), { projectKey: '' })], { days: 7, provider: 'all', now: NOW }).daily[6]
    expect(day.projects[0].label).toBe('Unknown')
  })
})
