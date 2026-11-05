import { formatDuration } from '../Overview/format'

const UNITS: Array<{ size: number; suffix: string }> = [
  { size: 1e9, suffix: 'B' },
  { size: 1e6, suffix: 'M' },
  { size: 1e3, suffix: 'k' }
]

/** `48.2M`, `950k`, `12`. One decimal below 100 of a unit, none above. */
export function formatCompactTokens(n: number): string {
  const value = Math.max(0, Math.round(n))
  for (let i = 0; i < UNITS.length; i++) {
    const { size, suffix } = UNITS[i]
    if (value < size) continue
    const scaled = value / size
    const text = scaled >= 100 ? String(Math.round(scaled)) : scaled.toFixed(1).replace(/\.0$/, '')
    // 999.95k rounds up to 1000k: say 1M instead.
    if (text === '1000' && i > 0) return `1${UNITS[i - 1].suffix}`
    return `${text}${suffix}`
  }
  return String(value)
}

/** `$31.40`, `$0.42`, `<$0.01` (non-zero below a cent), `$0.00`. */
export function formatUsd(n: number): string {
  if (n > 0 && n < 0.005) return '<$0.01'
  return `$${n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`
}

/** `182431` -> `182,431`. */
export function formatExactTokens(n: number): string {
  return Math.round(n).toLocaleString('en-US')
}

/** `just now`, `6 min`, `2 h`, `3 d`: a past span, without "ago". */
export function formatAge(ms: number): string {
  const minutes = Math.floor(Math.max(0, ms) / 60_000)
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes} min`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} h`
  return `${Math.floor(hours / 24)} d`
}

/** `Updated 1 min ago` text without the prefix: `just now` or `1 min ago`. */
export function formatUpdated(at: number, now: number): string {
  const age = formatAge(now - at)
  return age === 'just now' ? age : `${age} ago`
}

const DAY_MS = 24 * 60 * 60 * 1000

/** `resets in 1h 48m` under 24 h, else `resets Mon 09:00` (weekday and time follow the locale). */
export function formatResets(resetsAt: number | null, now: number): string {
  if (resetsAt === null) return ''
  const left = resetsAt - now
  if (left <= 0) return 'resetting now'
  if (left < DAY_MS) return `resets in ${left < 60_000 ? '<1m' : formatDuration(left, { compact: true })}`
  const date = new Date(resetsAt)
  const weekday = date.toLocaleDateString([], { weekday: 'short' })
  const time = date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  return `resets ${weekday} ${time}`
}

const VERSUS = { today: 'yesterday', week: 'last week', month: 'last month' } as const

/** `▼ 8% tokens vs last week`, `no change vs yesterday`; `null` when there is nothing to compare. */
export function formatChange(
  change: number | null,
  period: keyof typeof VERSUS,
  noun = ''
): string | null {
  if (change === null) return null
  const rounded = Math.round(change)
  if (rounded === 0) return `no change vs ${VERSUS[period]}`
  return `${rounded > 0 ? '▲' : '▼'} ${Math.abs(rounded)}%${noun ? ` ${noun}` : ''} vs ${VERSUS[period]}`
}
