import { useId, useMemo } from 'react'
import { HOUR_MS, timelineSegments, type AgentFilter, type Segment } from '@shared/overview/buckets'
import type { HistoryAgent, HistoryPoint, OverviewStatus } from '@shared/overview/types'
import { Card, EmptyState } from './Card'
import { HatchPattern, formatHourMinute, safeId } from './chartParts'
import { STATUS_LABEL } from './sessionFacts'
import styles from './Charts.module.css'
import detailStyles from './SessionDetails.module.css'
import own from './TimelineCard.module.css'

const COARSE_MS = 5000
const ROW_HEIGHT = 14
const LEGEND: OverviewStatus[] = ['needs-you', 'working', 'idle', 'starting', 'finished', 'not-running', 'failed']

function statusClass(status: Segment['status']): string {
  return status === 'unmeasured' ? '' : styles[`s_${status.replace('-', '_')}`]
}

/** Status segments of one agent over the last hour as an SVG strip. */
export function TimelineRow({
  segments,
  now,
  windowMs = HOUR_MS,
  label
}: {
  segments: Segment[]
  now: number
  windowMs?: number
  /** Accessible name, for example the agent's name. */
  label?: string
}) {
  const hatchId = safeId(useId()) + 't'
  const from = now - windowMs
  const pct = (at: number) => ((at - from) / windowMs) * 100
  return (
    <svg
      className={own.strip}
      height={ROW_HEIGHT}
      role="img"
      aria-label={`${label ? `${label}: ` : ''}${segments.length === 0 ? 'no status in the last hour' : 'status over the last hour'}`}
    >
      <defs>
        <HatchPattern id={hatchId} />
      </defs>
      <rect className={own.track} x="0" y="0" width="100%" height={ROW_HEIGHT} rx="3" />
      {segments.map((s) => {
        const text = `${s.status === 'unmeasured' ? 'Not measured' : STATUS_LABEL[s.status]}, ${formatHourMinute(s.start)}-${formatHourMinute(s.end)}`
        return (
          <rect
            key={`${s.start}-${s.status}`}
            data-segment={s.status}
            className={statusClass(s.status)}
            fill={s.status === 'unmeasured' ? `url(#${hatchId})` : undefined}
            x={`${pct(s.start)}%`}
            width={`${Math.max(0, pct(s.end) - pct(s.start))}%`}
            y="0"
            height={ROW_HEIGHT}
          >
            <title>{text}</title>
          </rect>
        )
      })}
    </svg>
  )
}

interface TimelineCardProps {
  points: HistoryPoint[]
  starts: number[]
  now: number
  agents: Record<string, HistoryAgent>
  agentFilter?: AgentFilter
}

/** Segments per agent for the agents shown (filtered, with at least one segment), by name. */
export function useTimelineRows(
  points: HistoryPoint[],
  starts: number[],
  now: number,
  agents: Record<string, HistoryAgent>,
  agentFilter?: AgentFilter
): { rows: { agentId: string; name: string; segments: Segment[] }[]; windowEnd: number } {
  const coarse = Math.floor(now / COARSE_MS) * COARSE_MS
  return useMemo(() => {
    const ids = new Set<string>()
    for (const p of points) {
      if (p.at < coarse - HOUR_MS) continue
      for (const id of Object.keys(p.statuses)) if (!agentFilter || agentFilter(id)) ids.add(id)
    }
    const segs = timelineSegments(points, starts, [...ids], coarse, HOUR_MS)
    const rows = [...ids]
      .filter((id) => segs[id].some((s) => s.status !== 'not-running' && s.status !== 'unmeasured'))
      .map((id) => ({ agentId: id, name: agents[id]?.name ?? id, segments: segs[id] }))
      .sort((a, b) => a.name.localeCompare(b.name))
    return { rows, windowEnd: coarse }
  }, [points, starts, coarse, agents, agentFilter])
}

export function TimelineCard({ points, starts, now, agents, agentFilter }: TimelineCardProps) {
  const { rows, windowEnd } = useTimelineRows(points, starts, now, agents, agentFilter)
  const from = windowEnd - HOUR_MS
  const ticks: number[] = []
  for (let t = Math.ceil(from / (15 * 60_000)) * 15 * 60_000; t <= windowEnd; t += 15 * 60_000) ticks.push(t)

  return (
    <Card title="Timeline" area="timeline" meta="last hour">
      {rows.length === 0 ? (
        <EmptyState>No history yet</EmptyState>
      ) : (
        <div>
          <ul className={own.rows} aria-label="Agents">
            {rows.map((r) => (
              <li key={r.agentId} className={own.row} data-agent={r.agentId}>
                <span className={own.name} title={r.name}>
                  {r.name}
                </span>
                <TimelineRow segments={r.segments} now={windowEnd} label={r.name} />
              </li>
            ))}
          </ul>
          <div className={own.axisWrap}>
            <div className={styles.axis} aria-hidden="true">
              {ticks.map((t) => (
                <span key={t} className={styles.axisLabel} style={{ left: `${((t - from) / HOUR_MS) * 100}%` }}>
                  {formatHourMinute(t)}
                </span>
              ))}
            </div>
          </div>
          <ul className={styles.legend} aria-label="Legend">
            {LEGEND.map((s) => (
              <li key={s} className={styles.legendItem}>
                <span className={`${styles.swatch} ${styles[`s_${s.replace('-', '_')}`]}`} />
                {STATUS_LABEL[s]}
              </li>
            ))}
            <li className={styles.legendItem}>
              <span className={`${styles.swatch} ${styles.swatchHatched}`} />
              Not measured
            </li>
          </ul>
        </div>
      )}
    </Card>
  )
}

/** This agent's timeline row for the details drawer. */
export function OwnTimeline({
  points,
  starts,
  now,
  agentId
}: {
  points: HistoryPoint[]
  starts: number[]
  now: number
  agentId: string
}) {
  const coarse = Math.floor(now / COARSE_MS) * COARSE_MS
  const segments = useMemo(
    () => timelineSegments(points, starts, [agentId], coarse, HOUR_MS)[agentId] ?? [],
    [points, starts, agentId, coarse]
  )
  return (
    <section className={detailStyles.section} aria-label="Timeline">
      <h4 className={detailStyles.sectionTitle}>Timeline</h4>
      {segments.length === 0 ? (
        <p className={detailStyles.muted}>No status recorded in the last hour</p>
      ) : (
        <TimelineRow segments={segments} now={coarse} />
      )}
    </section>
  )
}
