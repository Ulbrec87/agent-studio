export type UsageChipProvider = 'auto' | 'claude' | 'codex'
export const USAGE_CHIP_PROVIDERS: readonly UsageChipProvider[] = ['auto', 'claude', 'codex']

/** The `usage` block of the app config. */
export interface UsageSettings {
  /** Opt-in notifications when a plan window nears its limit. */
  alertsEnabled: boolean
  /** Percent used at which a 5-hour session window alerts. */
  sessionAlertPercent: number
  /** Percent used at which a weekly (or per-model weekly) window alerts. */
  weeklyAlertPercent: number
  /** Which provider the header chip reads; `auto` is the most used. */
  chipProvider: UsageChipProvider
  /** Show the estimated API-equivalent cost. */
  showCost: boolean
}

export const DEFAULT_USAGE_SETTINGS: UsageSettings = {
  alertsEnabled: false,
  sessionAlertPercent: 80,
  weeklyAlertPercent: 90,
  chipProvider: 'auto',
  showCost: true
}

export const MIN_ALERT_PERCENT = 50
export const MAX_ALERT_PERCENT = 100

function alertPercent(v: unknown, fallback: number): number {
  if (typeof v !== 'number' || !Number.isFinite(v)) return fallback
  return Math.min(MAX_ALERT_PERCENT, Math.max(MIN_ALERT_PERCENT, Math.round(v)))
}

/** Any stored value to a valid block: unknown keys dropped, bad values replaced by the defaults, alert percents clamped to 50-100 integers. */
export function normalizeUsageSettings(raw: unknown): UsageSettings {
  const o = typeof raw === 'object' && raw !== null && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
  const d = DEFAULT_USAGE_SETTINGS
  return {
    alertsEnabled: typeof o.alertsEnabled === 'boolean' ? o.alertsEnabled : d.alertsEnabled,
    sessionAlertPercent: alertPercent(o.sessionAlertPercent, d.sessionAlertPercent),
    weeklyAlertPercent: alertPercent(o.weeklyAlertPercent, d.weeklyAlertPercent),
    chipProvider: USAGE_CHIP_PROVIDERS.includes(o.chipProvider as UsageChipProvider)
      ? (o.chipProvider as UsageChipProvider)
      : d.chipProvider,
    showCost: typeof o.showCost === 'boolean' ? o.showCost : d.showCost
  }
}
