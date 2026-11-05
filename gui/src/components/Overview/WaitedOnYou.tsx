// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/dashboard/components/hero/WaitedOnYou.tsx
import type { WaitBar } from '@shared/overview/types'
import { formatDuration } from './format'
import { formatHourMinute } from './chartParts'
import styles from './WaitedOnYou.module.css'

/** The longest bar is at least this long, so a few seconds of waiting do not fill the row. */
const MIN_SCALE_MS = 60_000

interface WaitedOnYouProps {
  /** One per agent that waited, longest first. */
  bars: WaitBar[]
  /** Where the bars begin. */
  from: number
  /** Time between `from` and now that was not measured. */
  unmeasuredMs: number
}

/**
 * Who waited on you in the last hour. A wait still open is a filled amber bar
 * that grows with the clock; one that was answered is an outlined bar.
 * Nothing is rendered when nobody waited.
 */
export function WaitedOnYou({ bars, from, unmeasuredMs }: WaitedOnYouProps) {
  if (bars.length === 0) return null
  const scale = Math.max(MIN_SCALE_MS, ...bars.map((b) => b.ms))
  return (
    <section className={styles.waited} aria-label="Waited on you">
      <h4 className={styles.heading}>Waited on you since {formatHourMinute(from)}</h4>
      <ul className={styles.list}>
        {bars.map((bar) => (
          <li
            key={bar.agentId}
            className={styles.row}
            data-agent={bar.agentId}
            data-state={bar.open ? 'open' : 'answered'}
          >
            <span className={styles.name} title={bar.name}>
              {bar.name}
            </span>
            <span
              className={styles.track}
              role="img"
              aria-label={`${bar.name} waited ${formatDuration(bar.ms)}${bar.open ? ', still waiting' : ', answered'}`}
            >
              <span
                className={`${styles.bar} ${bar.open ? styles.open : styles.answered}`}
                style={{ width: `${Math.max(1, Math.min(100, (bar.ms / scale) * 100))}%` }}
              />
            </span>
            <span className={styles.value}>{formatDuration(bar.ms)}</span>
          </li>
        ))}
      </ul>
      {unmeasuredMs >= 1000 && (
        <p className={styles.note}>Not measured in this time: {formatDuration(unmeasuredMs, { compact: true })}</p>
      )}
    </section>
  )
}
