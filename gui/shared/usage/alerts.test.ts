import { describe, expect, it } from 'vitest'
import { alertKey, alertThreshold, alertableWindows, describeAlert, dueAlerts } from './alerts'
import { DEFAULT_USAGE_SETTINGS, normalizeUsageSettings } from './settings'
import type { ClaudeProfileLimits, LimitWindow, UsageState } from './types'

const NOW = Date.parse('2026-10-23T12:00:00Z')
const S = { sessionAlertPercent: 80, weeklyAlertPercent: 90 }

function w(kind: LimitWindow['kind'], usedPercent: number, resetsAt: number | null = NOW + 3_600_000, over: Partial<LimitWindow> = {}): LimitWindow {
  const id = kind === 'weekly-model' ? 'weekly:opus' : kind
  return { provider: 'claude', account: 'default', id, kind, label: kind, usedPercent, resetsAt, ...over }
}

describe('alertThreshold', () => {
  it('uses the session, weekly and default levels', () => {
    expect(alertThreshold({ kind: 'session' }, S)).toBe(80)
    expect(alertThreshold({ kind: 'weekly' }, S)).toBe(90)
    expect(alertThreshold({ kind: 'weekly-model' }, S)).toBe(90)
    expect(alertThreshold({ kind: 'other' }, S)).toBe(85)
    expect(alertThreshold({ kind: 'session' }, { ...S, sessionAlertPercent: 60 })).toBe(60)
  })
})

describe('dueAlerts', () => {
  it('is due at or over the threshold only', () => {
    const due = dueAlerts([w('session', 79), w('session', 80, NOW + 1, { id: 'a' }), w('weekly', 89), w('weekly', 95)], S, new Set())
    expect(due.map((d) => [d.window.kind, d.threshold])).toEqual([['session', 80], ['weekly', 90]])
  })

  it('never alerts without a reset time', () => {
    expect(dueAlerts([w('session', 99, null)], S, new Set())).toEqual([])
  })

  it('skips windows already reset when now is given', () => {
    expect(dueAlerts([w('session', 99, NOW - 1)], S, new Set(), NOW)).toEqual([])
  })

  it('does not repeat a notified key and re-arms on a new resetsAt', () => {
    const first = w('session', 90)
    const [a] = dueAlerts([first], S, new Set())
    expect(dueAlerts([first], S, new Set([a.key]))).toEqual([])
    const next = w('session', 90, NOW + 5 * 3_600_000)
    expect(dueAlerts([next], S, new Set([a.key]))).toHaveLength(1)
  })

  it('separates accounts and providers', () => {
    const keys = [
      alertKey(w('session', 90), 80),
      alertKey(w('session', 90, undefined, { account: 'work' }), 80),
      alertKey(w('session', 90, undefined, { provider: 'codex' }), 80)
    ]
    expect(new Set(keys).size).toBe(3)
  })
})

describe('alertableWindows', () => {
  function profile(status: ClaudeProfileLimits['status'], windows: LimitWindow[]): ClaudeProfileLimits {
    return { account: 'default', configDir: '/x', plan: null, status, windows, fetchedAt: NOW, checkedAt: NOW }
  }
  function state(over: Partial<UsageState>): UsageState {
    return {
      claude: [],
      codex: null,
      indexing: { indexing: false, progress: null, lastScanAt: null },
      dataVersion: 0,
      updatedAt: NOW,
      ...over
    }
  }
  const codexWin = w('weekly', 95, NOW + 1000, { provider: 'codex' })

  it('skips Claude accounts whose status is not ok', () => {
    const s = state({ claude: [profile('expired', [w('session', 95)]), profile('ok', [w('weekly', 95)])] })
    expect(alertableWindows(s, NOW).map((x) => x.kind)).toEqual(['weekly'])
  })

  it('skips Codex limits older than 30 minutes', () => {
    const fresh = state({ codex: { at: NOW - 29 * 60_000, plan: null, windows: [codexWin] } as UsageState['codex'] })
    const old = state({ codex: { at: NOW - 31 * 60_000, plan: null, windows: [codexWin] } as UsageState['codex'] })
    expect(alertableWindows(fresh, NOW)).toHaveLength(1)
    expect(alertableWindows(old, NOW)).toHaveLength(0)
  })
})

describe('describeAlert', () => {
  it('writes the title, the reset and the running agents', () => {
    const { title, body } = describeAlert(w('session', 85.4, NOW + 42 * 60_000), NOW, 3)
    expect(title).toBe('Claude session limit: 85% used')
    expect(body).toMatch(/^Resets in 42 min \(\d\d:\d\d\)\. 3 Claude agents are running\.$/)
  })

  it('names a non-default account and omits a zero agent count', () => {
    const { title, body } = describeAlert(w('weekly', 91, NOW + 3 * 86_400_000, { account: 'work' }), NOW, 0)
    expect(title).toBe('Claude weekly limit (work): 91% used')
    expect(body).toMatch(/^Resets in 3 days \(/)
    expect(body).not.toContain('running')
  })
})

describe('normalizeUsageSettings', () => {
  it('fills defaults for missing or invalid values', () => {
    expect(normalizeUsageSettings(undefined)).toEqual(DEFAULT_USAGE_SETTINGS)
    expect(normalizeUsageSettings({ chipProvider: 'x', showCost: 'no', alertsEnabled: 1 })).toEqual(DEFAULT_USAGE_SETTINGS)
  })

  it('clamps alert percents to integers from 50 to 100', () => {
    const s = normalizeUsageSettings({ sessionAlertPercent: 10, weeklyAlertPercent: 100.6 })
    expect(s.sessionAlertPercent).toBe(50)
    expect(s.weeklyAlertPercent).toBe(100)
    expect(normalizeUsageSettings({ sessionAlertPercent: 72.4 }).sessionAlertPercent).toBe(72)
    expect(normalizeUsageSettings({ sessionAlertPercent: NaN }).sessionAlertPercent).toBe(80)
  })
})
