// Adapted from Tokdash (MIT, © JingbiaoMei): src/tokdash/sources/quota/claude.py
import { DEFAULT_USAGE_SETTINGS, type UsageSettings } from './settings'
import type { LimitWindow, LimitWindowKind } from './types'

type Obj = Record<string, unknown>

function isObj(v: unknown): v is Obj {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

function clampPercent(n: number): number {
  return Math.min(100, Math.max(0, n))
}

/** `limits[].percent`: integers are on the 0-100 scale (1 is 1%); a float strictly inside (0,1) is a fraction. */
function limitPercent(v: unknown): number | null {
  if (typeof v !== 'number' || !Number.isFinite(v)) return null
  const fraction = !Number.isInteger(v) && v > 0 && v < 1
  return clampPercent(fraction ? v * 100 : v)
}

function utilizationPercent(v: unknown): number | null {
  if (typeof v !== 'number' || !Number.isFinite(v)) return null
  return clampPercent(v)
}

function parseResetsAt(v: unknown): number | null {
  if (typeof v === 'string' && v.trim()) {
    const ms = Date.parse(v)
    return Number.isFinite(ms) ? ms : null
  }
  if (typeof v === 'number' && Number.isFinite(v) && v > 0) {
    return v < 1e11 ? Math.round(v * 1000) : Math.round(v)
  }
  return null
}

function slug(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '') || 'model'
}

function titleCase(kind: string): string {
  const words = kind.replace(/[_-]+/g, ' ').trim().split(/\s+/).filter(Boolean)
  return words.map((w) => w[0].toUpperCase() + w.slice(1)).join(' ') || 'Other'
}

function build(
  account: string,
  id: string,
  kind: LimitWindowKind,
  label: string,
  usedPercent: number,
  resetsAt: number | null
): LimitWindow {
  return { provider: 'claude', account, id, kind, label, usedPercent, resetsAt }
}

function fromLimits(limits: unknown[], account: string): LimitWindow[] {
  const out: LimitWindow[] = []
  const seen = new Set<string>()
  for (const raw of limits) {
    if (!isObj(raw) || typeof raw.kind !== 'string' || !raw.kind) continue
    const percent = limitPercent(raw.percent)
    if (percent === null) continue
    const kind = raw.kind
    const resetsAt = parseResetsAt(raw.resets_at)
    const scope = isObj(raw.scope) ? raw.scope : null
    const model = scope && isObj(scope.model) ? scope.model : null
    const modelName = model && typeof model.display_name === 'string' ? model.display_name.trim() : ''
    let w: LimitWindow
    if (kind === 'session') {
      w = build(account, 'session', 'session', 'Current session (5h)', percent, resetsAt)
    } else if (kind === 'weekly_all') {
      w = build(account, 'weekly', 'weekly', 'Weekly · all models', percent, resetsAt)
    } else if (kind.startsWith('weekly') && modelName) {
      w = build(account, `weekly:${slug(modelName)}`, 'weekly-model', `Weekly · ${modelName}`, percent, resetsAt)
    } else {
      w = build(account, kind, 'other', titleCase(kind), percent, resetsAt)
    }
    if (seen.has(w.id)) continue
    seen.add(w.id)
    out.push(w)
  }
  return out
}

const LEGACY: Array<{ key: string; id: string; kind: LimitWindowKind; label: string }> = [
  { key: 'five_hour', id: 'session', kind: 'session', label: 'Current session (5h)' },
  { key: 'seven_day', id: 'weekly', kind: 'weekly', label: 'Weekly · all models' },
  { key: 'seven_day_opus', id: 'weekly:opus', kind: 'weekly-model', label: 'Weekly · Opus' },
  { key: 'seven_day_sonnet', id: 'weekly:sonnet', kind: 'weekly-model', label: 'Weekly · Sonnet' }
]

function fromLegacy(body: Obj, account: string): LimitWindow[] {
  const out: LimitWindow[] = []
  for (const l of LEGACY) {
    const raw = body[l.key]
    if (!isObj(raw)) continue
    const percent = utilizationPercent(raw.utilization)
    if (percent === null) continue
    out.push(build(account, l.id, l.kind, l.label, percent, parseResetsAt(raw.resets_at)))
  }
  return out
}

/** Plan-limit windows of one `/api/oauth/usage` response. Never throws; malformed entries are skipped. */
export function parseClaudeOAuthUsage(body: unknown, account: string): LimitWindow[] {
  try {
    if (!isObj(body)) return []
    if (Array.isArray(body.limits)) {
      const windows = fromLimits(body.limits, account)
      if (windows.length > 0) return windows
    }
    return fromLegacy(body, account)
  } catch {
    return []
  }
}

export type LimitSeverity = 'ok' | 'warn' | 'high'

export function limitSeverity(usedPercent: number): LimitSeverity {
  if (usedPercent < 50) return 'ok'
  if (usedPercent < 80) return 'warn'
  return 'high'
}

/** The windows the header chip shows: the most-used session and the most-used all-models weekly. */
export function pickChipWindows(windows: LimitWindow[]): { session: LimitWindow | null; weekly: LimitWindow | null } {
  let session: LimitWindow | null = null
  let weekly: LimitWindow | null = null
  for (const w of windows) {
    if (w.kind === 'session' && (!session || w.usedPercent > session.usedPercent)) session = w
    else if (w.kind === 'weekly' && (!weekly || w.usedPercent > weekly.usedPercent)) weekly = w
  }
  return { session, weekly }
}

/** Percent at which a window counts as "near its limit" when no settings are given. */
export const DEFAULT_LIMIT_ALERTS = {
  session: DEFAULT_USAGE_SETTINGS.sessionAlertPercent,
  weekly: DEFAULT_USAGE_SETTINGS.weeklyAlertPercent,
  other: 85
} as const

/** Alert level of a window; per-model weekly windows use the weekly level. Defaults apply without settings. */
export function limitAlertLevel(
  w: Pick<LimitWindow, 'kind'>,
  settings?: Pick<UsageSettings, 'sessionAlertPercent' | 'weeklyAlertPercent'>
): number {
  if (w.kind === 'session') return settings?.sessionAlertPercent ?? DEFAULT_LIMIT_ALERTS.session
  if (w.kind === 'weekly' || w.kind === 'weekly-model') return settings?.weeklyAlertPercent ?? DEFAULT_LIMIT_ALERTS.weekly
  return DEFAULT_LIMIT_ALERTS.other
}
