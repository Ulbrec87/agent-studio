// Adapted from agent-lookout (MIT, © 2026 Idris Olubisi): src/dashboard/components/waits/WaitsCard.tsx
import { useId, useMemo, useState } from 'react'
import type { AgentFilter } from '@shared/overview/buckets'
import type { LongestWait, WaitFigure, WaitLedgerData, WaitingNow } from '@shared/overview/types'
import { waitTotals } from '@shared/overview/waits'
import { Card, EmptyState } from './Card'
import { HatchPattern, safeId } from './chartParts'
import { formatDuration } from './format'
import styles from './Charts.module.css'
import own from './WaitsCard.module.css'
import switchStyles from './Sessions.module.css'

const HEIGHT = 72
const BAR_GAP_PCT = 2
/** The math is rerun at most this often although `now` ticks every second. */
const COARSE_MS = 5000

type Period = 'today' | 'week'

interface WaitsCardProps {
  /** `null` until the daemon answered. */
  ledger: WaitLedgerData | null
  waitingNow: WaitingNow[]
  now: number
  agentFilter?: AgentFilter
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`
}

function Figure({ label, figure }: { label: string; figure: WaitFigure }) {
  return (
    <div className={own.figure}>
      <span className={own.figureLabel}>{label}</span>
      <span className={own.figureValue}>{formatDuration(figure.ms)}</span>
      <span className={own.figureSub}>{plural(figure.count, 'wait', 'waits')}</span>
    </div>
  )
}

/** `2026-10-22` as a short weekday in the local calendar. */
function weekday(day: string): string {
  const [y, m, d] = day.split('-').map(Number)
  return new Date(y, m - 1, d).toLocaleDateString([], { weekday: 'short' })
}

function Longest({ entries }: { entries: LongestWait[] }) {
  if (entries.length === 0) return <p className={own.none}>Nobody waited in this time.</p>
  return (
    <ol className={own.longest}>
      {entries.map((e) => (
        <li key={e.agentId} className={own.longestRow} data-agent={e.agentId}>
          <span className={own.longestName} title={e.name}>
            {e.name}
          </span>
          {e.waitingNow && <span className={own.nowMark}>waiting now</span>}
          <span className={own.longestCount}>{plural(e.count, 'wait', 'waits')}</span>
          <span className={own.longestValue}>{formatDuration(e.ms)}</span>
        </li>
      ))}
    </ol>
  )
}

export function WaitsCard({ ledger, waitingNow, now, agentFilter }: WaitsCardProps) {
  const hatchId = safeId(useId()) + 'w'
  const [period, setPeriod] = useState<Period>('today')
  const coarse = Math.floor(now / COARSE_MS) * COARSE_MS
  const totals = useMemo(
    () =>
      ledger
        ? waitTotals({
            transitions: ledger.transitions,
            spans: ledger.spans,
            starts: ledger.starts,
            waitingNow,
            now: coarse,
            agentFilter
          })
        : null,
    [ledger, waitingNow, coarse, agentFilter]
  )

  if (!totals || totals.week.measuredMs <= 0) {
    return (
      <Card title="Waits" area="waits">
        <EmptyState>{ledger ? 'Nothing measured yet' : 'No waits recorded yet'}</EmptyState>
      </Card>
    )
  }

  const slot = 100 / totals.byDay.length
  const scale = Math.max(1, ...totals.byDay.map((d) => d.ms))
  const px = (ms: number) => (ms > 0 ? Math.max(2, (ms / scale) * HEIGHT) : 0)

  const control = (
    <div className={switchStyles.segmented} role="group" aria-label="Longest waits period">
      {(
        [
          ['today', 'Today'],
          ['week', '7 days']
        ] as const
      ).map(([id, label]) => (
        <button
          key={id}
          type="button"
          className={`${switchStyles.segment} ${period === id ? switchStyles.segmentActive : ''}`}
          aria-pressed={period === id}
          onClick={() => setPeriod(id)}
        >
          {label}
        </button>
      ))}
    </div>
  )

  return (
    <Card title="Waits" area="waits">
      <div className={own.body}>
        <div className={own.figures}>
          <Figure label="Today" figure={totals.today} />
          <Figure label="7 days" figure={totals.week} />
        </div>

        <div className={own.columns}>
          <section aria-label="By day" className={own.byDay}>
            <h4 className={own.subheading}>By day</h4>
            <svg className={styles.svg} height={HEIGHT} aria-hidden="true" focusable="false">
              <defs>
                <HatchPattern id={hatchId} />
              </defs>
              {totals.byDay.map((d, i) => {
                const x = i * slot + BAR_GAP_PCT / 2
                const w = slot - BAR_GAP_PCT
                const closedH = px(d.ms - d.openMs)
                const openH = px(d.openMs)
                return (
                  <g key={d.day} data-day={d.day}>
                    {!d.measured && (
                      <rect x={`${x}%`} width={`${w}%`} y={0} height={HEIGHT} fill={`url(#${hatchId})`} data-unmeasured="" />
                    )}
                    {closedH > 0 && (
                      <rect className={own.barClosed} x={`${x}%`} width={`${w}%`} y={HEIGHT - closedH - openH} height={closedH} />
                    )}
                    {openH > 0 && (
                      <rect className={styles.barNeeds} x={`${x}%`} width={`${w}%`} y={HEIGHT - openH} height={openH} data-open="" />
                    )}
                  </g>
                )
              })}
              <line className={styles.baseline} x1="0" x2="100%" y1={HEIGHT} y2={HEIGHT} />
            </svg>
            <ol className={own.days}>
              {totals.byDay.map((d) => (
                <li
                  key={d.day}
                  className={own.day}
                  title={`${d.day}: waited ${formatDuration(d.ms)}, measured ${formatDuration(d.measuredMs, { compact: true })}`}
                >
                  <span className={own.dayValue}>{d.measured ? formatDuration(d.ms, { compact: true }) : '—'}</span>
                  <span className={own.dayName}>{weekday(d.day)}</span>
                </li>
              ))}
            </ol>
          </section>

          <section aria-label="Longest waits" className={own.longestWrap}>
            <div className={own.longestHead}>
              <h4 className={own.subheading}>Longest waits</h4>
              {control}
            </div>
            <Longest entries={totals.longest[period]} />
          </section>
        </div>

        <p className={own.measured}>
          Measured {formatDuration(totals.today.measuredMs, { compact: true })} of today,{' '}
          {formatDuration(totals.week.measuredMs, { compact: true })} of the 7 days
        </p>
      </div>
    </Card>
  )
}
