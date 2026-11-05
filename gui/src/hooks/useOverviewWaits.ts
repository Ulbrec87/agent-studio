import { useEffect, useState } from 'react'
import type { WaitLedgerData } from '@shared/overview/types'
import { transitionOfEvent } from '@shared/overview/waits'

/** How often the ledger is fetched. */
export const WAITS_REFRESH_MS = 30_000

export interface UseOverviewWaitsResult {
  /** The last 9 days of waits and measured time; `null` until the first answer. */
  ledger: WaitLedgerData | null
  error: string | null
}

/**
 * The daemon's wait ledger: fetched on mount, every 30 s while the page is
 * visible, and at once when a pushed event moves an agent into or out of
 * needs-you. The totals are worked out in the renderer (`waitTotals`), so the
 * project filter and the one-second clock need no round trip.
 */
export function useOverviewWaits(): UseOverviewWaitsResult {
  const [ledger, setLedger] = useState<WaitLedgerData | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (typeof window.hydra?.getOverviewWaits !== 'function') return undefined
    let cancelled = false
    // Responses may arrive out of order; only the newest request is applied.
    let latest = 0

    const load = (): void => {
      const request = ++latest
      window.hydra
        .getOverviewWaits()
        .then((response) => {
          if (cancelled || request !== latest) return
          setLedger(response.ledger)
          setError(null)
        })
        .catch((err: unknown) => {
          if (cancelled || request !== latest) return
          setError(err instanceof Error ? err.message : String(err))
        })
    }

    const unsubscribe =
      typeof window.hydra.onOverviewEvents === 'function'
        ? window.hydra.onOverviewEvents((events) => {
            if (events.some((e) => transitionOfEvent(e) !== null)) load()
          })
        : () => undefined

    load()
    const timer = setInterval(() => {
      if (document.visibilityState !== 'hidden') load()
    }, WAITS_REFRESH_MS)

    return () => {
      cancelled = true
      clearInterval(timer)
      unsubscribe()
    }
  }, [])

  return { ledger, error }
}
