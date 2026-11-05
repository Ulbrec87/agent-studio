/**
 * Formats a duration for timers: `45s`, `4m 12s`, `1h 03m`.
 * With `compact`, seconds are dropped from a minute on: `12m`, `1h 03m`.
 */
export function formatDuration(ms: number, options: { compact?: boolean } = {}): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000))
  const hours = Math.floor(totalSeconds / 3600)
  const minutes = Math.floor((totalSeconds % 3600) / 60)
  const seconds = totalSeconds % 60
  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, '0')}m`
  if (minutes > 0) return options.compact ? `${minutes}m` : `${minutes}m ${String(seconds).padStart(2, '0')}s`
  return `${seconds}s`
}

/** Last path segment of a project directory. */
export function projectName(projectDir: string): string {
  const parts = projectDir.split(/[\\/]+/).filter(Boolean)
  return parts[parts.length - 1] ?? projectDir
}

/** Absolute time for a timestamp: `14:02:11`, with the date when it is not today. */
export function formatClock(at: number, now: number = Date.now()): string {
  const date = new Date(at)
  const time = date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
  return date.toDateString() === new Date(now).toDateString()
    ? time
    : `${date.toLocaleDateString([], { day: 'numeric', month: 'short' })} ${time}`
}

/** `182431` -> `182,431`. */
export function formatCount(n: number): string {
  return n.toLocaleString('en-US')
}

/** `14:02:11` in local time. */
export function formatTimeOfDay(at: number): string {
  return new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

/** `14:02`, with the date when it is not today (`Oct 21 14:02`). */
export function formatMinute(at: number, now: number = Date.now()): string {
  const date = new Date(at)
  const time = date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  return date.toDateString() === new Date(now).toDateString()
    ? time
    : `${date.toLocaleDateString([], { day: 'numeric', month: 'short' })} ${time}`
}

/** True when both timestamps fall on the same local day. */
export function isSameDay(a: number, b: number): boolean {
  return new Date(a).toDateString() === new Date(b).toDateString()
}

/**
 * Relative time for a past moment: `just now`, `12m ago`, `3h ago`, then `yesterday 14:02`,
 * `Oct 21`, or `Oct 21, 2025` for another year.
 */
export function formatAgo(at: number, now: number = Date.now()): string {
  const diff = now - at
  if (diff < 60_000) return 'just now'
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`
  const date = new Date(at)
  const today = new Date(now)
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1)
  if (date.toDateString() === yesterday.toDateString()) {
    return `yesterday ${date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`
  }
  return date.getFullYear() === today.getFullYear()
    ? date.toLocaleDateString([], { day: 'numeric', month: 'short' })
    : date.toLocaleDateString([], { day: 'numeric', month: 'short', year: 'numeric' })
}
