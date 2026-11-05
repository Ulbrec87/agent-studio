import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { UsageReport, UsageReportDay, UsageReportProvider } from '@shared/usage/report'
import { formatCompactTokens, formatUsd } from '../Usage/format'
import { useUsageSettings } from '../Usage/UsageSettingsContext'
import styles from './UsageDashboard.module.css'

interface UsageDashboardProps {
  onClose: () => void
}

const DAY_LIMIT = 30
const TREND_WINDOWS = [7, 30] as const
type TrendWindow = (typeof TREND_WINDOWS)[number]
const PROVIDER_OPTIONS: Array<{ id: UsageReportProvider; label: string }> = [
  { id: 'all', label: 'All' },
  { id: 'claude', label: 'Claude' },
  { id: 'codex', label: 'Codex' }
]

interface TrendPoint {
  date: string
  label: string
  value: number
  partial: boolean
}

export function UsageDashboard({ onClose }: UsageDashboardProps) {
  const { showCost } = useUsageSettings()
  const [report, setReport] = useState<UsageReport | null>(null)
  const [isLoading, setIsLoading] = useState(true)
  const [selectedDate, setSelectedDate] = useState<string | null>(null)
  const [trendWindow, setTrendWindow] = useState<TrendWindow>(7)
  const [selectedProvider, setSelectedProvider] = useState<UsageReportProvider>('all')

  const requestId = useRef(0)

  const load = useCallback(async (opts?: { rescan?: boolean }) => {
    const id = ++requestId.current
    setIsLoading(true)
    try {
      if (opts?.rescan) await window.hydra.refreshUsage().catch(() => undefined)
      const next = await window.hydra.getUsageReport(DAY_LIMIT, selectedProvider)
      if (id !== requestId.current) return
      setReport(next)
      setSelectedDate((prev) => {
        if (prev && next.daily.some((d) => d.date === prev)) return prev
        return next.daily[next.daily.length - 1]?.date ?? null
      })
    } catch {
      if (id === requestId.current) setReport(null)
    } finally {
      if (id === requestId.current) setIsLoading(false)
    }
  }, [selectedProvider])

  useEffect(() => {
    void load()
  }, [load])

  const daily = useMemo(() => report?.daily ?? [], [report])
  const hasData = daily.some((d) => d.totalTokens > 0 || d.messages > 0)

  const selectedDay = useMemo<UsageReportDay | null>(() => {
    if (selectedDate) return daily.find((d) => d.date === selectedDate) ?? null
    return daily[daily.length - 1] ?? null
  }, [daily, selectedDate])

  const trendSeries = useMemo<TrendPoint[]>(
    () =>
      daily.slice(-trendWindow).map((d) => ({
        date: d.date,
        label: formatDayShort(d.date),
        value: showCost ? d.costUsd : d.totalTokens,
        partial: d.costPartial
      })),
    [daily, trendWindow, showCost]
  )
  const trendTotal = trendSeries.reduce((sum, p) => sum + p.value, 0)
  const trendActiveDays = trendSeries.filter((p) => p.value > 0).length
  const trendAvgPerDay = trendTotal / trendWindow
  const trendProjected30 = trendAvgPerDay * 30
  const trendPartial = showCost && trendSeries.some((p) => p.partial)

  const projectEntries = selectedDay?.projects.filter((p) => p.totalTokens > 0) ?? []
  const subtitle = 'Token usage from local Claude Code and Codex logs. Costs are API-equivalent estimates.'
  const cacheWrite = selectedDay ? selectedDay.tokens.cacheWrite5m + selectedDay.tokens.cacheWrite1h : 0
  const trendNoun = showCost ? 'Cost' : 'Token'

  return (
    <div className={styles.overlay} onClick={onClose}>
      <div className={styles.panel} onClick={(e) => e.stopPropagation()}>
        <div className={styles.header}>
          <div>
            <h2>Usage Dashboard</h2>
            <p className={styles.subtitle}>{subtitle}</p>
          </div>
          <button className={styles.closeBtn} type="button" onClick={onClose}>
            &#x2715;
          </button>
        </div>

        <div className={styles.controls}>
          <div className={styles.controlGroup}>
            <label className={styles.label}>Provider</label>
            <div className={styles.segmented}>
              {PROVIDER_OPTIONS.map((option) => (
                <button
                  key={option.id}
                  className={`${styles.segmentBtn} ${selectedProvider === option.id ? styles.segmentBtnActive : ''}`}
                  type="button"
                  aria-pressed={selectedProvider === option.id}
                  onClick={() => setSelectedProvider(option.id)}
                >
                  {option.label}
                </button>
              ))}
            </div>
          </div>

          {hasData && (
            <div className={styles.controlGroup}>
              <label className={styles.label} htmlFor="usage-day">Day</label>
              <select
                id="usage-day"
                className={styles.select}
                value={selectedDate ?? ''}
                onChange={(e) => setSelectedDate(e.target.value || null)}
              >
                {daily.map((day) => (
                  <option key={day.date} value={day.date}>
                    {formatDay(day.date)}
                  </option>
                ))}
              </select>
            </div>
          )}

          <div className={styles.actionRow}>
            <button className={styles.refreshBtn} type="button" disabled={isLoading} onClick={() => void load({ rescan: true })}>
              Refresh
            </button>
          </div>
        </div>

        <div className={styles.content}>
          {isLoading ? (
            <div className={styles.loading}>
              <div className={styles.spinner} />
              <span>Loading usage data...</span>
            </div>
          ) : selectedDay && hasData ? (
            <>
              <div className={styles.summaryGrid}>
                {showCost && (
                  <SummaryCard
                    label="Cost"
                    value={`${selectedDay.costPartial ? '≥' : ''}${formatUsd(selectedDay.costUsd)}`}
                  />
                )}
                <SummaryCard label="Total Tokens" value={formatCompactTokens(selectedDay.totalTokens)} />
                <SummaryCard label="Input" value={formatCompactTokens(selectedDay.tokens.input)} />
                <SummaryCard label="Output" value={formatCompactTokens(selectedDay.tokens.output)} />
                <SummaryCard label="Cache Write" value={formatCompactTokens(cacheWrite)} />
                <SummaryCard label="Cache Read" value={formatCompactTokens(selectedDay.tokens.cacheRead)} />
              </div>

              <section className={styles.trendSection}>
                <div className={styles.trendHeader}>
                  <div>
                    <h3>{trendNoun} Trend</h3>
                    <p>Daily {trendNoun.toLowerCase()} over the last {trendWindow} days</p>
                  </div>

                  <div className={styles.trendControls}>
                    <div className={styles.segmented}>
                      {TREND_WINDOWS.map((windowSize) => (
                        <button
                          key={windowSize}
                          className={`${styles.segmentBtn} ${trendWindow === windowSize ? styles.segmentBtnActive : ''}`}
                          type="button"
                          onClick={() => setTrendWindow(windowSize)}
                        >
                          {windowSize}d
                        </button>
                      ))}
                    </div>
                  </div>
                </div>

                <TrendChart
                  series={trendSeries}
                  total={trendTotal}
                  avgPerDay={trendAvgPerDay}
                  projected30={trendProjected30}
                  activeDays={trendActiveDays}
                  windowDays={trendWindow}
                  format={showCost ? formatUsd : formatCompactTokens}
                  unitLabel={showCost ? 'cost' : 'token'}
                  partial={trendPartial}
                />
              </section>

              <div className={styles.tables}>
                <section className={styles.tableSection}>
                  <div className={styles.tableHeader}>
                    <h3>Model Breakdown</h3>
                    <span>{selectedDay.models.length} models</span>
                  </div>
                  <div className={styles.tableWrap}>
                    <table className={styles.table}>
                      <thead>
                        <tr>
                          <th>Model</th>
                          <th>Input</th>
                          <th>Output</th>
                          <th>Cache Write</th>
                          <th>Cache Read</th>
                          {showCost && <th>Cost</th>}
                        </tr>
                      </thead>
                      <tbody>
                        {selectedDay.models.map((model) => (
                          <tr key={model.model}>
                            <td>{prettifyModelName(model.model)}</td>
                            <td>{formatCompactTokens(model.tokens.input)}</td>
                            <td>{formatCompactTokens(model.tokens.output)}</td>
                            <td>{formatCompactTokens(model.tokens.cacheWrite5m + model.tokens.cacheWrite1h)}</td>
                            <td>{formatCompactTokens(model.tokens.cacheRead)}</td>
                            {showCost && <td>{formatUsd(model.costUsd)}</td>}
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </section>

                {projectEntries.length > 0 && (
                  <section className={styles.tableSection}>
                    <div className={styles.tableHeader}>
                      <h3>Projects</h3>
                      <span>{projectEntries.length} projects</span>
                    </div>
                    <div className={styles.tableWrap}>
                      <table className={styles.table}>
                        <thead>
                          <tr>
                            <th>Project</th>
                            <th>Tokens</th>
                            {showCost && <th>Cost</th>}
                          </tr>
                        </thead>
                        <tbody>
                          {projectEntries.map((project) => (
                            <tr key={project.projectKey}>
                              <td title={project.projectKey || undefined}>{project.label}</td>
                              <td>{formatCompactTokens(project.totalTokens)}</td>
                              {showCost && <td>{formatUsd(project.costUsd)}</td>}
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  </section>
                )}
              </div>
            </>
          ) : (
            <div className={styles.empty}>No usage recorded in the last {DAY_LIMIT} days.</div>
          )}
        </div>
      </div>
    </div>
  )
}

function SummaryCard({ label, value }: { label: string; value: string }) {
  return (
    <div className={styles.summaryCard}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  )
}

function TrendChart({
  series,
  total,
  avgPerDay,
  projected30,
  activeDays,
  windowDays,
  format,
  unitLabel,
  partial
}: {
  series: TrendPoint[]
  total: number
  avgPerDay: number
  projected30: number
  activeDays: number
  windowDays: TrendWindow
  format: (n: number) => string
  unitLabel: string
  partial: boolean
}) {
  const lower = partial ? '≥' : ''
  const width = 920
  const height = 220
  const paddingX = 20
  const paddingTop = 16
  const paddingBottom = 30
  const plotWidth = width - paddingX * 2
  const plotHeight = height - paddingTop - paddingBottom
  const baselineY = paddingTop + plotHeight
  const maxValue = Math.max(...series.map((p) => p.value), 0)
  const safeMax = maxValue > 0 ? maxValue : 1
  const tickStep = series.length > 14 ? 4 : series.length > 8 ? 2 : 1

  const points = series.map((point, index) => {
    const ratio = point.value / safeMax
    const x = paddingX + (series.length <= 1 ? 0 : (index * plotWidth) / (series.length - 1))
    const y = baselineY - ratio * plotHeight
    return { x, y, ...point }
  })

  const linePath = points
    .map((p, i) => `${i === 0 ? 'M' : 'L'} ${p.x.toFixed(2)} ${p.y.toFixed(2)}`)
    .join(' ')
  const areaPath = points.length
    ? `${linePath} L ${points[points.length - 1].x.toFixed(2)} ${baselineY.toFixed(2)} L ${points[0].x.toFixed(2)} ${baselineY.toFixed(2)} Z`
    : ''

  return (
    <div className={styles.trendBody}>
      <div className={styles.chartWrap}>
        <svg
          className={styles.chartSvg}
          viewBox={`0 0 ${width} ${height}`}
          role="img"
          aria-label={`Usage ${unitLabel} trend chart`}
        >
          {[0.25, 0.5, 0.75, 1].map((ratio) => {
            const y = baselineY - plotHeight * ratio
            return (
              <line
                key={ratio}
                x1={paddingX}
                y1={y}
                x2={width - paddingX}
                y2={y}
                className={styles.chartGridLine}
              />
            )
          })}

          {areaPath && <path d={areaPath} className={styles.chartArea} />}
          {linePath && <path d={linePath} className={styles.chartLine} />}

          {points.length <= 14 &&
            points.map((point) => (
              <circle
                key={point.date}
                cx={point.x}
                cy={point.y}
                r={2.8}
                className={styles.chartPoint}
              />
            ))}

          {points.map((point, index) => {
            if (index !== 0 && index !== points.length - 1 && index % tickStep !== 0) return null
            return (
              <text
                key={`${point.date}-label`}
                x={point.x}
                y={height - 8}
                className={styles.chartLabel}
              >
                {point.label}
              </text>
            )
          })}
        </svg>
      </div>

      <div className={styles.trendStats}>
        <div className={styles.trendStatCard}>
          <span>Total ({windowDays}d)</span>
          <strong>{lower}{format(total)}</strong>
        </div>
        <div className={styles.trendStatCard}>
          <span>Avg / day</span>
          <strong>{lower}{format(avgPerDay)}</strong>
        </div>
        <div className={styles.trendStatCard}>
          <span>Projected 30d</span>
          <strong>{lower}{format(projected30)}</strong>
        </div>
        <div className={styles.trendStatCard}>
          <span>Active days</span>
          <strong>
            {activeDays} / {windowDays}
          </strong>
        </div>
      </div>
    </div>
  )
}

function formatDay(date: string): string {
  const parsed = parseUsageDate(date)
  if (!parsed) return date
  return parsed.toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric'
  })
}

function formatDayShort(dateKey: string): string {
  const date = parseUsageDate(dateKey)
  if (!date) return dateKey
  return date.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric'
  })
}

function parseUsageDate(value: string): Date | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return null
  const date = new Date(`${value}T00:00:00`)
  return Number.isNaN(date.getTime()) ? null : date
}

function prettifyModelName(name: string): string {
  // e.g. "claude-opus-4-6" -> "Opus 4.6", "claude-haiku-4-5-20251001" -> "Haiku 4.5"
  if (name.startsWith('claude-')) {
    const match = name.slice(7).match(/^(opus|sonnet|haiku)-(\d+)-(\d+)/)
    if (match) {
      const family = match[1].charAt(0).toUpperCase() + match[1].slice(1)
      return `${family} ${match[2]}.${match[3]}`
    }
  }
  return name
}
