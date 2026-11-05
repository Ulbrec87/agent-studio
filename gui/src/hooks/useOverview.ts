import { useEffect, useState } from 'react'
import type { OverviewSnapshot } from '@shared/overview/types'

export interface OverviewData {
  snapshot: OverviewSnapshot | null
  loading: boolean
  error: string | null
}

export interface UseOverviewResult extends OverviewData {
  /** Epoch ms, ticks every second while mounted (for live timers). */
  now: number
}

/**
 * The Overview snapshot, fetched once and then pushed by the daemon only when
 * something meaningful changes (no ticking). With `skip`, nothing is
 * subscribed: the caller already has the data from a shared subscription.
 */
export function useOverviewSnapshot(skip = false): OverviewData {
  const [snapshot, setSnapshot] = useState<OverviewSnapshot | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (skip) return
    let cancelled = false
    const unsubscribe = window.hydra.onOverviewSnapshot((next) => {
      if (cancelled) return
      setSnapshot(next)
      setError(null)
      setLoading(false)
    })

    window.hydra
      .getOverview()
      .then((initial) => {
        if (cancelled) return
        // A pushed snapshot may already be newer than the initial response.
        setSnapshot((prev) => (prev && prev.at > initial.at ? prev : initial))
        setError(null)
        setLoading(false)
      })
      .catch((err: unknown) => {
        if (cancelled) return
        setError(err instanceof Error ? err.message : String(err))
        setLoading(false)
      })

    return () => {
      cancelled = true
      unsubscribe()
    }
  }, [skip])

  return { snapshot, loading, error }
}

/** Epoch ms, ticking every second while mounted. */
export function useNow(): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [])
  return now
}

export function useOverview(): UseOverviewResult {
  const data = useOverviewSnapshot()
  const now = useNow()
  return { ...data, now }
}

/** Number of agents that need the user, across all projects. 0 until the first snapshot. */
export function needsYouCount(snapshot: OverviewSnapshot | null): number {
  return snapshot ? snapshot.sessions.filter((s) => s.status === 'needs-you').length : 0
}
