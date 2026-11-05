import { readFileSync } from 'fs'
import { join } from 'path'
import { describe, expect, it } from 'vitest'
import { limitSeverity, parseClaudeOAuthUsage, pickChipWindows } from './limits'
import type { LimitWindow } from './types'

const fixture = JSON.parse(readFileSync(join(__dirname, 'fixtures/claude-oauth-usage.json'), 'utf8'))

describe('parseClaudeOAuthUsage', () => {
  it('reads limits[] from the real fixture', () => {
    const w = parseClaudeOAuthUsage(fixture, 'default')
    expect(w.map((x) => [x.id, x.kind, x.label, x.usedPercent])).toEqual([
      ['session', 'session', 'Current session (5h)', 4],
      ['weekly', 'weekly', 'Weekly · all models', 0]
    ])
    expect(w[0].resetsAt).toBe(Date.parse('2026-10-23T10:29:59.646885+00:00'))
    expect(w[1].resetsAt).toBe(Date.parse('2026-10-30T01:59:59.646907+00:00'))
    expect(w[0].provider).toBe('claude')
    expect(w[0].account).toBe('default')
  })

  it('falls back to legacy keys', () => {
    const w = parseClaudeOAuthUsage(
      {
        five_hour: { utilization: 12, resets_at: '2026-10-23T10:00:00Z' },
        seven_day: { utilization: 30, resets_at: null },
        seven_day_opus: { utilization: 55.5, resets_at: '2026-10-30T00:00:00Z' },
        seven_day_sonnet: null
      },
      'work'
    )
    expect(w.map((x) => [x.id, x.kind, x.usedPercent])).toEqual([
      ['session', 'session', 12],
      ['weekly', 'weekly', 30],
      ['weekly:opus', 'weekly-model', 55.5]
    ])
    expect(w[1].resetsAt).toBeNull()
  })

  it('does not use legacy keys when limits[] yields windows', () => {
    const w = parseClaudeOAuthUsage(
      { five_hour: { utilization: 99 }, limits: [{ kind: 'session', percent: 3, resets_at: null }] },
      'a'
    )
    expect(w).toHaveLength(1)
    expect(w[0].usedPercent).toBe(3)
  })

  it('maps scoped weekly model limits and unknown kinds', () => {
    const w = parseClaudeOAuthUsage(
      {
        limits: [
          { kind: 'weekly_scoped', percent: 40, resets_at: null, scope: { model: { display_name: 'Claude Opus 4' } } },
          { kind: 'monthly_extra', percent: 10, resets_at: null, scope: null }
        ]
      },
      'a'
    )
    expect(w[0]).toMatchObject({ id: 'weekly:claude-opus-4', kind: 'weekly-model', label: 'Weekly · Claude Opus 4' })
    expect(w[1]).toMatchObject({ id: 'monthly_extra', kind: 'other', label: 'Monthly Extra' })
  })

  it('scales percent: integer 1 is 1%, float 0.5 is 50%', () => {
    const p = (percent: number): number =>
      parseClaudeOAuthUsage({ limits: [{ kind: 'session', percent }] }, 'a')[0].usedPercent
    expect(p(1)).toBe(1)
    expect(p(0)).toBe(0)
    expect(p(0.5)).toBe(50)
    expect(p(250)).toBe(100)
    expect(p(-3)).toBe(0)
  })

  it('skips malformed input without throwing', () => {
    expect(parseClaudeOAuthUsage(null, 'a')).toEqual([])
    expect(parseClaudeOAuthUsage('x', 'a')).toEqual([])
    expect(parseClaudeOAuthUsage({ limits: 'nope' }, 'a')).toEqual([])
    const w = parseClaudeOAuthUsage(
      { limits: [null, 5, { kind: 3 }, { kind: 'session', percent: 'x' }, { kind: 'session', percent: 7, resets_at: 'bad' }] },
      'a'
    )
    expect(w).toHaveLength(1)
    expect(w[0].resetsAt).toBeNull()
  })
})

describe('limitSeverity', () => {
  it('uses 50 and 80 as thresholds', () => {
    expect(limitSeverity(0)).toBe('ok')
    expect(limitSeverity(49.9)).toBe('ok')
    expect(limitSeverity(50)).toBe('warn')
    expect(limitSeverity(79.9)).toBe('warn')
    expect(limitSeverity(80)).toBe('high')
    expect(limitSeverity(100)).toBe('high')
  })
})

describe('pickChipWindows', () => {
  const w = (
    provider: 'claude' | 'codex',
    account: string,
    id: string,
    kind: LimitWindow['kind'],
    usedPercent: number
  ): LimitWindow => ({ provider, account, id, kind, label: id, usedPercent, resetsAt: null })

  it('picks the most-used session and all-models weekly across accounts', () => {
    const r = pickChipWindows([
      w('claude', 'a', 'session', 'session', 10),
      w('codex', 'b', 'session', 'session', 60),
      w('claude', 'a', 'weekly', 'weekly', 20),
      w('claude', 'a', 'weekly:opus', 'weekly-model', 95),
      w('codex', 'b', 'weekly', 'weekly', 35)
    ])
    expect(r.session?.account).toBe('b')
    expect(r.weekly?.usedPercent).toBe(35)
  })

  it('returns nulls when nothing matches', () => {
    expect(pickChipWindows([])).toEqual({ session: null, weekly: null })
  })
})
