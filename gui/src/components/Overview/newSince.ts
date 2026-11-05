import type { OverviewEvent } from '@shared/overview/types'

export const LAST_SEEN_KEY = 'agentStudio.overview.lastSeenAt'
/** How long the divider has to stay in view before the marker goes. */
export const CLEAR_AFTER_MS = 10_000

/** When the user last saw the Overview, `null` on the first ever visit or without storage. */
export function readLastSeen(): number | null {
  try {
    const raw = localStorage.getItem(LAST_SEEN_KEY)
    if (raw === null) return null
    const at = Number(raw)
    return Number.isFinite(at) && at > 0 ? at : null
  } catch {
    return null
  }
}

export function writeLastSeen(at: number): void {
  try {
    localStorage.setItem(LAST_SEEN_KEY, String(at))
  } catch {
    /* storage unavailable */
  }
}

export interface NewSinceMarker {
  /** Where the user left off. */
  at: number
  /** Events newer than `at`. */
  count: number
  /** Index in the newest-first list of the first event the user has already seen. */
  index: number
}

/**
 * The marker for a newest-first list of events: `null` on a first visit (`lastSeenAt` null)
 * or when nothing is newer than `lastSeenAt`.
 */
export function newSinceMarker(eventsNewestFirst: OverviewEvent[], lastSeenAt: number | null): NewSinceMarker | null {
  if (lastSeenAt === null) return null
  let count = 0
  while (count < eventsNewestFirst.length && eventsNewestFirst[count].at > lastSeenAt) count++
  return count === 0 ? null : { at: lastSeenAt, count, index: count }
}
