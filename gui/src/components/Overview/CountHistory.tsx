import { useEffect, useId, useMemo, useRef, useState } from 'react'
import type { KeyboardEvent } from 'react'
import { countSeries, type AgentFilter, type CountKey } from '@shared/overview/buckets'
import type { HistoryPoint } from '@shared/overview/types'
import { HatchPattern, formatHourMinute, plainNumber, safeId } from './chartParts'
import styles from './Charts.module.css'
import own from './CountHistory.module.css'

const COARSE_MS = 5000
const HEIGHT = 64

const RANGES = [
  { id: '15m', label: '15 m', ms: 15 * 60_000, buckets: 15 },
  { id: '1h', label: '1 h', ms: 60 * 60_000, buckets: 12 },
  { id: '6h', label: '6 h', ms: 6 * 60 * 60_000, buckets: 24 }
] as const

const METRIC_LABEL: Record<CountKey, string> = {
  needsYou: 'Needs you',
  working: 'Working',
  idle: 'Idle'
}

const METRIC_CLASS: Record<CountKey, string> = {
  needsYou: styles.barNeeds,
  working: styles.barWorking,
  idle: styles.s_idle
}

/** Bars of the average count per time bucket (hatched where nothing was measured). */
export function HistoryChart({
  points,
  starts,
  now,
  metric,
  windowMs,
  bucketCount,
  agentFilter
}: {
  points: HistoryPoint[]
  starts: number[]
  now: number
  metric: CountKey
  windowMs: number
  bucketCount: number
  agentFilter?: AgentFilter
}) {
  const hatchId = safeId(useId()) + 'c'
  const coarse = Math.floor(now / COARSE_MS) * COARSE_MS
  const series = useMemo(
    () => countSeries(points, starts, metric, coarse, windowMs, bucketCount, agentFilter),
    [points, starts, metric, coarse, windowMs, bucketCount, agentFilter]
  )
  const [active, setActive] = useState<number | null>(null)
  const scale = Math.max(1, ...series.map((s) => s.value ?? 0))
  const slot = 100 / series.length
  const shown = active === null ? null : series[active]

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const last = series.length - 1
    const move = (to: number) => {
      e.preventDefault()
      setActive(Math.min(last, Math.max(0, to)))
    }
    if (e.key === 'ArrowRight') move(active === null ? 0 : active + 1)
    else if (e.key === 'ArrowLeft') move(active === null ? last : active - 1)
  }

  return (
    <div>
      <div
        className={styles.chart}
        role="group"
        tabIndex={0}
        aria-label={`${METRIC_LABEL[metric]} over time. Use the arrow keys to read a bar.`}
        onKeyDown={onKeyDown}
        onBlur={() => setActive(null)}
        onMouseLeave={() => setActive(null)}
        onMouseMove={(e) => {
          const rect = e.currentTarget.getBoundingClientRect()
          if (rect.width <= 0) return
          const i = Math.floor(((e.clientX - rect.left) / rect.width) * series.length)
          setActive(Math.min(series.length - 1, Math.max(0, i)))
        }}
      >
        <svg className={styles.svg} height={HEIGHT} aria-hidden="true" focusable="false">
          <defs>
            <HatchPattern id={hatchId} />
          </defs>
          {series.map((s, i) => {
            const h = s.value === null ? 0 : (s.value / scale) * HEIGHT
            return (
              <g key={s.start} data-count-bucket={i}>
                {active === i && (
                  <rect className={styles.highlight} x={`${i * slot}%`} width={`${slot}%`} y={0} height={HEIGHT} />
                )}
                {s.value === null ? (
                  <rect x={`${i * slot + 0.3}%`} width={`${slot - 0.6}%`} y={0} height={HEIGHT} fill={`url(#${hatchId})`} />
                ) : (
                  <rect
                    className={METRIC_CLASS[metric]}
                    x={`${i * slot + 0.3}%`}
                    width={`${slot - 0.6}%`}
                    y={HEIGHT - h}
                    height={h}
                  />
                )}
              </g>
            )
          })}
          <line className={styles.baseline} x1="0" x2="100%" y1={HEIGHT} y2={HEIGHT} />
        </svg>
      </div>
      <p className={styles.readout} aria-live="polite" data-testid="count-readout">
        {shown === null
          ? `Highest average ${plainNumber(Math.max(0, ...series.map((s) => s.value ?? 0)))}`
          : shown.value === null
            ? `${formatHourMinute(shown.start)}-${formatHourMinute(shown.end)}: not measured`
            : `${formatHourMinute(shown.start)}-${formatHourMinute(shown.end)}: ${plainNumber(shown.value)} on average`}
      </p>
    </div>
  )
}

interface CountHistoryProps {
  metric: CountKey
  points: HistoryPoint[]
  starts: number[]
  now: number
  agentFilter?: AgentFilter
  onClose: () => void
}

/** Inline panel with the history of one count and a 15 m / 1 h / 6 h switch. Esc closes it. */
export function CountHistory({ metric, points, starts, now, agentFilter, onClose }: CountHistoryProps) {
  const [rangeId, setRangeId] = useState<(typeof RANGES)[number]['id']>('1h')
  const range = RANGES.find((r) => r.id === rangeId) ?? RANGES[1]
  const ref = useRef<HTMLElement>(null)

  useEffect(() => {
    ref.current?.focus()
  }, [])

  return (
    <section
      ref={ref}
      tabIndex={-1}
      className={own.panel}
      aria-label={`${METRIC_LABEL[metric]} history`}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          onClose()
        }
      }}
    >
      <header className={own.header}>
        <h4 className={own.title}>{METRIC_LABEL[metric]} over the last {range.label}</h4>
        <div className={own.range} role="group" aria-label="Time range">
          {RANGES.map((r) => (
            <button
              key={r.id}
              type="button"
              className={`${own.rangeBtn} ${r.id === rangeId ? own.rangeActive : ''}`}
              aria-pressed={r.id === rangeId}
              onClick={() => setRangeId(r.id)}
            >
              {r.label}
            </button>
          ))}
        </div>
        <button type="button" className={own.close} onClick={onClose} aria-label="Close chart">
          ×
        </button>
      </header>
      <HistoryChart
        points={points}
        starts={starts}
        now={now}
        metric={metric}
        windowMs={range.ms}
        bucketCount={range.buckets}
        agentFilter={agentFilter}
      />
    </section>
  )
}
