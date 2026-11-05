import { useCallback, useEffect, useRef, useState } from 'react'
import type { UsagePeriod, UsageState, UsageSummary } from '@shared/usage/types'

/**
 * The usage service state, fetched once and then pushed on change. Owned once in
 * `App` (like the Overview snapshot) and passed down; never subscribe per component.
 */
export function useUsageState(): UsageState | null {
  const [state, setState] = useState<UsageState | null>(null)

  useEffect(() => {
    let cancelled = false
    const unsubscribe = window.hydra.onUsageChanged((next) => {
      if (!cancelled) setState(next)
    })
    window.hydra
      .getUsageState()
      .then((initial) => {
        // A pushed state may already be newer than the initial response.
        if (!cancelled) setState((prev) => (prev && prev.updatedAt > initial.updatedAt ? prev : initial))
      })
      .catch(() => {})
    return () => {
      cancelled = true
      unsubscribe()
    }
  }, [])

  return state
}

/**
 * The summary of one period (optionally of one project directory) for the popover and the Overview card. Fetches while `enabled`, when the period
 * changes and when `dataVersion` changes; keeps showing the last summary of the same
 * period while a refetch runs.
 */
export function useUsageSummary(
  period: UsagePeriod,
  enabled: boolean,
  dataVersion: number | undefined,
  projectKey?: string
): { summary: UsageSummary | null; loading: boolean } {
  const [result, setResult] = useState<{ key: string; summary: UsageSummary } | null>(null)
  const [loading, setLoading] = useState(false)
  const key = `${period}|${projectKey ?? ''}`

  useEffect(() => {
    if (!enabled) return
    let cancelled = false
    setLoading(true)
    const request = projectKey ? window.hydra.getUsageSummary(period, projectKey) : window.hydra.getUsageSummary(period)
    request
      .then((next) => {
        if (!cancelled) setResult({ key, summary: next })
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [period, projectKey, key, enabled, dataVersion])

  return { summary: result && result.key === key && result.summary.period === period ? result.summary : null, loading }
}

const KEEP_ALIVE_MS = 30_000

/** While `active`, tells the service a usage surface is visible (now and every 30 s) so it refreshes the limits faster. */
export function useUsageVisibility(active: boolean): void {
  useEffect(() => {
    if (!active) return
    const ping = (): void => {
      try {
        void Promise.resolve(window.hydra.setUsageVisible()).catch(() => {})
      } catch {
        /* bridge unavailable */
      }
    }
    ping()
    const id = setInterval(ping, KEEP_ALIVE_MS)
    return () => clearInterval(id)
  }, [active])
}

/** Epoch ms that updates every `everyMs` (default one minute) while mounted. */
export function useCoarseNow(everyMs = 60_000): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), everyMs)
    return () => clearInterval(id)
  }, [everyMs])
  return now
}

/** A choice remembered in localStorage (every access guarded: it can be blocked or empty). */
export function usePersistedChoice<T extends string>(
  key: string,
  fallback: T,
  allowed: readonly T[]
): [T, (next: T) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const raw = localStorage.getItem(key)
      return allowed.includes(raw as T) ? (raw as T) : fallback
    } catch {
      return fallback
    }
  })
  const keyRef = useRef(key)
  const set = useCallback((next: T) => {
    setValue(next)
    try {
      localStorage.setItem(keyRef.current, next)
    } catch {
      /* storage unavailable: the choice just is not remembered */
    }
  }, [])
  return [value, set]
}
