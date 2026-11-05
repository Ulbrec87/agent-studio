import { useId, useMemo, useState } from 'react'
import type { KeyboardEvent, MouseEvent } from 'react'
import { lastHourBuckets, type AgentFilter, type Bucket } from '@shared/overview/buckets'
import type { HistoryPoint } from '@shared/overview/types'
import { formatDuration } from './format'
import { HatchPattern, formatHourMinute, plainNumber, safeId } from './chartParts'
import styles from './Charts.module.css'
import own from './LastHourChart.module.css'

const HEIGHT = 88
const BAR_GAP_PCT = 0.9
/** The math is rerun at most this often although `now` ticks every second. */
const COARSE_MS = 5000

interface LastHourChartProps {
  points: HistoryPoint[]
  starts: number[]
  now: number
  agentFilter?: AgentFilter
}

function visibleMs(b: Bucket, now: number): number {
  return Math.max(0, Math.min(b.end, now) - b.start)
}

function readout(b: Bucket): string {
  const range = `${formatHourMinute(b.start)}-${formatHourMinute(b.end)}`
  if (b.measuredMs <= 0) return `${range}: not measured`
  return (
    `${range}: needs you ${plainNumber(b.avgNeedsYou)}, working ${plainNumber(b.avgWorking)}, ` +
    `idle ${plainNumber(b.avgIdle)} on average`
  )
}

export function LastHourChart({ points, starts, now, agentFilter }: LastHourChartProps) {
  const hatchId = safeId(useId()) + 'h'
  const coarse = Math.floor(now / COARSE_MS) * COARSE_MS
  const { buckets, totalWaitedMs } = useMemo(
    () => lastHourBuckets(points, starts, coarse, { agentFilter }),
    [points, starts, coarse, agentFilter]
  )
  const [active, setActive] = useState<number | null>(null)

  const count = buckets.length
  const scale = Math.max(1, ...buckets.map((b) => b.avgTotal))
  const anyMeasured = buckets.some((b) => b.measuredMs > 0)
  const slot = 100 / count
  const px = (n: number) => (n / scale) * HEIGHT

  const onMouseMove = (e: MouseEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect()
    if (rect.width <= 0) return
    const i = Math.floor(((e.clientX - rect.left) / rect.width) * count)
    setActive(Math.min(count - 1, Math.max(0, i)))
  }

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const move = (to: number) => {
      e.preventDefault()
      setActive(Math.min(count - 1, Math.max(0, to)))
    }
    if (e.key === 'ArrowRight') move(active === null ? 0 : active + 1)
    else if (e.key === 'ArrowLeft') move(active === null ? count - 1 : active - 1)
    else if (e.key === 'Home') move(0)
    else if (e.key === 'End') move(count - 1)
    else if (e.key === 'Escape') setActive(null)
  }

  const waited =
    !anyMeasured
      ? 'Nothing measured in the last hour yet'
      : totalWaitedMs >= 1000
        ? `Agents waited on you ${formatDuration(totalWaitedMs, { compact: true })} in the last hour`
        : 'Nothing waited on you in the last hour'

  return (
    <div>
      <p className={styles.line}>{waited}</p>
      <div
        className={styles.chart}
        role="group"
        tabIndex={0}
        aria-label="Last hour, one bar per five minutes. Use the arrow keys to read a bar."
        onMouseMove={onMouseMove}
        onMouseLeave={() => setActive(null)}
        onBlur={() => setActive(null)}
        onKeyDown={onKeyDown}
      >
        <svg className={styles.svg} height={HEIGHT} aria-hidden="true" focusable="false">
          <defs>
            <HatchPattern id={hatchId} />
          </defs>
          {buckets.map((b, i) => {
            const x = i * slot + BAR_GAP_PCT / 2
            const w = slot - BAR_GAP_PCT
            const measured = b.measuredMs > 0
            const partial = measured && b.measuredMs < visibleMs(b, now) - COARSE_MS
            const needsH = px(b.avgNeedsYou)
            const workingH = px(b.avgWorking)
            const totalH = Math.max(px(b.avgTotal), needsH + workingH)
            return (
              <g key={b.start} data-bucket={i} data-bar={measured ? '' : undefined}>
                {active === i && <rect className={styles.highlight} x={`${i * slot}%`} width={`${slot}%`} y={0} height={HEIGHT} />}
                {(!measured || partial) && (
                  <rect x={`${x}%`} width={`${w}%`} y={0} height={HEIGHT} fill={`url(#${hatchId})`} data-unmeasured="" />
                )}
                {measured && (
                  <>
                    <rect className={styles.barIdle} x={`${x}%`} width={`${w}%`} y={HEIGHT - totalH} height={totalH} />
                    <rect className={styles.barWorking} x={`${x}%`} width={`${w}%`} y={HEIGHT - needsH - workingH} height={workingH} />
                    <rect className={styles.barNeeds} x={`${x}%`} width={`${w}%`} y={HEIGHT - needsH} height={needsH} />
                  </>
                )}
              </g>
            )
          })}
          <line className={styles.baseline} x1="0" x2="100%" y1={HEIGHT} y2={HEIGHT} />
        </svg>
        <div className={styles.axis} aria-hidden="true">
          {buckets.map((b, i) =>
            new Date(b.start).getMinutes() % 15 === 0 ? (
              <span
                key={b.start}
                className={`${styles.axisLabel} ${i === 0 ? styles.axisLabelFirst : ''}`}
                style={{ left: `${i * slot}%` }}
              >
                {formatHourMinute(b.start)}
              </span>
            ) : null
          )}
        </div>
      </div>
      <p className={styles.readout} aria-live="polite" data-testid="last-hour-readout">
        {active === null ? <span className={own.hint}>Point at a bar, or focus the chart and press the arrow keys.</span> : readout(buckets[active])}
      </p>
      <ul className={styles.legend} aria-label="Legend">
        <li className={styles.legendItem}><span className={`${styles.swatch} ${styles.s_needs_you}`} />Needs you</li>
        <li className={styles.legendItem}><span className={`${styles.swatch} ${styles.s_working}`} />Working</li>
        <li className={styles.legendItem}><span className={`${styles.swatch} ${styles.swatchHatched}`} />Not measured</li>
      </ul>
    </div>
  )
}
