import styles from './Charts.module.css'

/** `14:05` in the local time zone. */
export function formatHourMinute(at: number): string {
  return new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false })
}

/** The diagonal hatch for time that was not measured. `id` must be unique in the document. */
export function HatchPattern({ id }: { id: string }) {
  return (
    <pattern id={id} width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
      <line x1="0" y1="0" x2="0" y2="6" className={styles.hatchLine} strokeWidth="2" />
    </pattern>
  )
}

/** `useId()` values contain colons, which break `url(#…)` references. */
export function safeId(raw: string): string {
  return raw.replace(/[^a-zA-Z0-9_-]/g, '')
}

export function plainNumber(n: number): string {
  return (Math.round(n * 10) / 10).toString()
}
