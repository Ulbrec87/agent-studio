import { useEffect, useRef, useState } from 'react'
import type { HistoryResponse, OverviewEvent } from '@shared/overview/types'

/** How much history the renderer loads on mount. */
export const HISTORY_WINDOW_MS = 6 * 60 * 60 * 1000
/** How often new points are fetched. */
export const HISTORY_REFRESH_MS = 30_000
const MAX_EVENTS = 1000

export interface UseOverviewHistoryResult {
  history: HistoryResponse | null
  loading: boolean
  error: string | null
}

/** Appends events not yet known (by id), keeping time order and at most `MAX_EVENTS`. */
function mergeEvents(known: OverviewEvent[], incoming: OverviewEvent[]): OverviewEvent[] {
  if (incoming.length === 0) return known
  const ids = new Set(known.map((e) => e.id))
  const fresh = incoming.filter((e) => !ids.has(e.id))
  if (fresh.length === 0) return known
  const merged = [...known, ...fresh]
  merged.sort((a, b) => a.at - b.at)
  return merged.length > MAX_EVENTS ? merged.slice(-MAX_EVENTS) : merged
}

/**
 * Folds a (possibly partial) response into what is held: points, events and
 * starts are added by their time or id, agents are merged, and the scalar
 * fields come from the newer response. Points older than `cutoff` are dropped.
 */
export function mergeHistory(
  held: HistoryResponse | null,
  incoming: HistoryResponse,
  cutoff: number
): HistoryResponse {
  if (!held) return { ...incoming, points: incoming.points.filter((p) => p.at >= cutoff) }
  const known = new Set(held.points.map((p) => p.at))
  const points = [...held.points, ...incoming.points.filter((p) => !known.has(p.at))]
    .filter((p) => p.at >= cutoff)
    .sort((a, b) => a.at - b.at)
  const starts = Array.from(new Set([...held.starts, ...incoming.starts]))
    .filter((s) => s >= cutoff)
    .sort((a, b) => a - b)
  return {
    since: incoming.since,
    startedAt: incoming.startedAt,
    starts,
    points,
    events: mergeEvents(held.events, incoming.events),
    agents: { ...held.agents, ...incoming.agents }
  }
}

/**
 * The Overview history of the last 6 hours: loaded on mount, new events pushed
 * by the daemon as they happen, new points fetched every 30 s (only those
 * after the newest one held).
 */
export function useOverviewHistory(): UseOverviewHistoryResult {
  const [history, setHistory] = useState<HistoryResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const latest = useRef<HistoryResponse | null>(null)

  useEffect(() => {
    if (typeof window.hydra?.getOverviewHistory !== 'function') {
      setLoading(false)
      return undefined
    }
    let cancelled = false
    // Events pushed before the first response arrives would be lost to a response computed earlier.
    let early: OverviewEvent[] = []

    const apply = (next: HistoryResponse | null): void => {
      latest.current = next
      setHistory(next)
    }

    const load = (): void => {
      const now = Date.now()
      const cutoff = now - HISTORY_WINDOW_MS
      const held = latest.current
      const newest = held && held.points.length > 0 ? held.points[held.points.length - 1].at : null
      const since = newest !== null ? newest + 1 : cutoff
      window.hydra
        .getOverviewHistory(since)
        .then((response) => {
          if (cancelled) return
          let merged = mergeHistory(latest.current, response, cutoff)
          if (early.length > 0) {
            merged = { ...merged, events: mergeEvents(merged.events, early) }
            early = []
          }
          apply(merged)
          setError(null)
          setLoading(false)
        })
        .catch((err: unknown) => {
          if (cancelled) return
          setError(err instanceof Error ? err.message : String(err))
          setLoading(false)
        })
    }

    const unsubscribe =
      typeof window.hydra.onOverviewEvents === 'function'
        ? window.hydra.onOverviewEvents((events) => {
            if (cancelled) return
            const held = latest.current
            if (!held) {
              early = [...early, ...events]
              return
            }
            apply({ ...held, events: mergeEvents(held.events, events) })
          })
        : () => undefined

    load()
    const timer = setInterval(() => {
      if (document.visibilityState !== 'hidden') load()
    }, HISTORY_REFRESH_MS)

    return () => {
      cancelled = true
      clearInterval(timer)
      unsubscribe()
    }
  }, [])

  return { history, loading, error }
}
