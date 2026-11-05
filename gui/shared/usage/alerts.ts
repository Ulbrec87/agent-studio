import { limitAlertLevel } from './limits'
import type { UsageSettings } from './settings'
import type { LimitWindow, UsageState } from './types'

type AlertSettings = Pick<UsageSettings, 'sessionAlertPercent' | 'weeklyAlertPercent'>

/** Codex limits come from session logs; older than this they are not trusted for an alert. */
export const CODEX_ALERT_MAX_AGE_MS = 30 * 60_000

export interface DueAlert {
  window: LimitWindow
  threshold: number
  key: string
}

/** Percent used at which `window` alerts: session and weekly levels from the settings, anything else the default. */
export function alertThreshold(window: Pick<LimitWindow, 'kind'>, settings: AlertSettings): number {
  return limitAlertLevel(window, settings)
}

/** Identity of one alert: it repeats only when the window resets (new `resetsAt`) or the threshold changes. */
export function alertKey(window: LimitWindow, threshold: number): string {
  return [window.provider, window.account, window.id, window.resetsAt ?? '', threshold].join('|')
}

/**
 * Windows at or over their threshold that have not been notified. A window without a reset time never
 * alerts (it cannot be deduplicated or re-armed), nor does one whose reset time has already passed.
 */
export function dueAlerts(
  windows: LimitWindow[],
  settings: AlertSettings,
  notified: ReadonlySet<string>,
  now?: number
): DueAlert[] {
  const out: DueAlert[] = []
  for (const window of windows) {
    if (window.resetsAt === null) continue
    if (now !== undefined && window.resetsAt <= now) continue
    const threshold = alertThreshold(window, settings)
    if (window.usedPercent < threshold) continue
    const key = alertKey(window, threshold)
    if (notified.has(key)) continue
    out.push({ window, threshold, key })
  }
  return out
}

/** The windows that may alert: Claude accounts with a fresh sign-in (`ok`), Codex limits read in the last 30 minutes. */
export function alertableWindows(state: UsageState, now: number): LimitWindow[] {
  const out: LimitWindow[] = []
  for (const profile of state.claude) {
    if (profile.status === 'ok') out.push(...profile.windows)
  }
  if (state.codex && now - state.codex.at <= CODEX_ALERT_MAX_AGE_MS) out.push(...state.codex.windows)
  return out
}

function describeDuration(ms: number): string {
  const minutes = Math.max(1, Math.round(ms / 60_000))
  if (minutes < 60) return `${minutes} min`
  const hours = Math.floor(minutes / 60)
  if (hours < 48) {
    const rest = minutes % 60
    return rest === 0 ? `${hours} h` : `${hours} h ${rest} min`
  }
  return `${Math.round(hours / 24)} days`
}

function windowName(w: LimitWindow): string {
  if (w.kind === 'session') return 'session'
  if (w.kind === 'weekly') return 'weekly'
  if (w.kind === 'weekly-model') return `weekly ${w.label.replace(/^Weekly\s*·\s*/i, '')}`.trim()
  return w.label.toLowerCase()
}

/** Title and body of the notification for one alert (`runningAgents` is optional context). */
export function describeAlert(
  window: LimitWindow,
  now: number,
  runningAgents?: number
): { title: string; body: string } {
  const provider = window.provider === 'codex' ? 'Codex' : 'Claude'
  const account = window.provider === 'claude' && window.account !== 'default' ? ` (${window.account})` : ''
  const title = `${provider} ${windowName(window)} limit${account}: ${Math.round(window.usedPercent)}% used`
  let body = ''
  if (window.resetsAt !== null) {
    const at = new Date(window.resetsAt).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' })
    body = `Resets in ${describeDuration(window.resetsAt - now)} (${at}).`
  }
  if (runningAgents && runningAgents > 0) {
    body += `${body ? ' ' : ''}${runningAgents} ${provider} agent${runningAgents === 1 ? ' is' : 's are'} running.`
  }
  return { title, body }
}
