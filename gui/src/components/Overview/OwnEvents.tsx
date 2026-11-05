import type { OverviewEvent } from '@shared/overview/types'
import { EventRow } from './EventRow'
import rowStyles from './EventRow.module.css'
import { eventsOfAgent } from './historyFacts'
import styles from './SessionDetails.module.css'

/** The events of one session for the details drawer, newest first. */
export function OwnEvents({ events, agentId, now }: { events: OverviewEvent[]; agentId: string; now: number }) {
  const own = eventsOfAgent(events, agentId)
  return (
    <section className={styles.section} aria-label="Events">
      <h4 className={styles.sectionTitle}>Events</h4>
      {own.length === 0 ? (
        <p className={styles.muted}>No events yet</p>
      ) : (
        <ul className={rowStyles.list}>
          {own.slice(0, 50).map((e) => (
            <EventRow key={e.id} event={e} now={now} />
          ))}
        </ul>
      )}
    </section>
  )
}
