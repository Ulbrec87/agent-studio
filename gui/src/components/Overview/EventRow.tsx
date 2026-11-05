import type { OverviewEvent } from '@shared/overview/types'
import { formatTimeOfDay, isSameDay } from './format'
import { eventMarkKey, eventText } from './historyFacts'
import styles from './EventRow.module.css'

interface EventRowProps {
  event: OverviewEvent
  now: number
  /** Makes the agent's name a button that opens its details. */
  onOpenAgent?: (agentId: string, trigger: HTMLElement) => void
}

/** One line of the events log, shared by the Events card and the details drawer. */
export function EventRow({ event, now, onOpenAgent }: EventRowProps) {
  const resumed = event.kind === 'resumed'
  const title = isSameDay(event.at, now) ? undefined : new Date(event.at).toLocaleString()
  return (
    <li className={styles.row}>
      <span className={`${styles.mark} ${styles[`mark_${eventMarkKey(event)}`]}`} aria-hidden="true" />
      <span className={styles.text}>
        {!resumed &&
          (onOpenAgent ? (
            <button type="button" className={styles.name} onClick={(e) => onOpenAgent(event.agentId, e.currentTarget)}>
              {event.name}
            </button>
          ) : (
            <span className={styles.nameText}>{event.name}</span>
          ))}
        {!resumed && ' '}
        {eventText(event)}
      </span>
      <time className={styles.time} dateTime={new Date(event.at).toISOString()} title={title}>
        {formatTimeOfDay(event.at)}
      </time>
    </li>
  )
}
