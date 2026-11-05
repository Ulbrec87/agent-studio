import { Fragment, useCallback, useEffect, useMemo, useState } from 'react'
import type { HistoryResponse } from '@shared/overview/types'
import { Card, EmptyState } from './Card'
import { EventRow } from './EventRow'
import rowStyles from './EventRow.module.css'
import { formatMinute, formatTimeOfDay } from './format'
import { CLEAR_AFTER_MS, newSinceMarker, readLastSeen, writeLastSeen } from './newSince'
import styles from './EventsCard.module.css'

const PAGE = 200

interface EventsCardProps {
  history: HistoryResponse | null
  /** Project directory to show, `''` for all. */
  filter: string
  now: number
  onOpenAgent: (agentId: string, trigger: HTMLElement) => void
}

/**
 * The time the user last saw the Overview. Written when the card goes away or the
 * document is hidden, re-read when the document is shown again. `null` once the marker has been cleared.
 */
function useLastSeen(): [number | null, () => void] {
  const [lastSeen, setLastSeen] = useState<number | null>(readLastSeen)
  useEffect(() => {
    const onVisibility = () => {
      if (document.visibilityState === 'hidden') writeLastSeen(Date.now())
      else setLastSeen(readLastSeen())
    }
    document.addEventListener('visibilitychange', onVisibility)
    return () => {
      document.removeEventListener('visibilitychange', onVisibility)
      writeLastSeen(Date.now())
    }
  }, [])
  const clear = useCallback(() => setLastSeen(null), [])
  return [lastSeen, clear]
}

export function EventsCard({ history, filter, now, onOpenAgent }: EventsCardProps) {
  const [lastSeen, clearMarker] = useLastSeen()
  const [limit, setLimit] = useState(PAGE)
  const [dividerEl, setDivider] = useState<HTMLLIElement | null>(null)

  const events = useMemo(() => {
    if (!history) return []
    const all = history.events.filter((e) => {
      if (filter === '' || e.kind === 'resumed') return true
      return (history.agents[e.agentId]?.projectDir ?? e.projectDir) === filter
    })
    return all.sort((a, b) => b.at - a.at)
  }, [history, filter])

  const marker = useMemo(() => newSinceMarker(events, lastSeen), [events, lastSeen])
  const markerAt = marker?.at

  // The marker goes once its divider has been in view for 10 s.
  useEffect(() => {
    if (!dividerEl || markerAt === undefined) return undefined
    let timer: ReturnType<typeof setTimeout> | undefined
    const start = () => {
      if (timer === undefined) timer = setTimeout(clearMarker, CLEAR_AFTER_MS)
    }
    const stop = () => {
      clearTimeout(timer)
      timer = undefined
    }
    if (typeof IntersectionObserver === 'undefined') {
      start()
      return stop
    }
    const observer = new IntersectionObserver((entries) => {
      const last = entries[entries.length - 1]
      if (last?.isIntersecting) start()
      else stop()
    })
    observer.observe(dividerEl)
    return () => {
      observer.disconnect()
      stop()
    }
  }, [dividerEl, markerAt, clearMarker])

  const divider = (at: number) => (
    <li ref={setDivider} className={styles.divider} role="separator">
      New since {formatTimeOfDay(at)}
    </li>
  )

  const shown = Math.max(limit, marker ? marker.count + 10 : 0)
  const visible = events.slice(0, shown)
  const since = history ? history.since : null

  return (
    <Card title="Events" area="events" meta={marker ? `${marker.count} new` : undefined}>
      {events.length === 0 ? (
        <EmptyState>No events yet</EmptyState>
      ) : (
        <div className={styles.host}>
          <div className={styles.scroll} tabIndex={0} role="region" aria-label="Events log">
            <ul className={rowStyles.list}>
              {visible.map((e, i) => (
                <Fragment key={e.id}>
                  {marker && i === marker.index && divider(marker.at)}
                  <EventRow event={e} now={now} onOpenAgent={onOpenAgent} />
                </Fragment>
              ))}
              {marker && marker.index >= visible.length && visible.length === events.length && divider(marker.at)}
            </ul>
            {events.length > visible.length && (
              <button type="button" className={styles.more} onClick={() => setLimit(shown + PAGE)}>
                Show more
              </button>
            )}
            {since !== null && since > 0 && (
              <p className={styles.footer}>Started watching {formatMinute(since, now)}</p>
            )}
          </div>
        </div>
      )}
    </Card>
  )
}
