import { useCallback, useMemo, useRef, useState } from 'react'
import type { UsageState } from '@shared/usage/types'
import type { HistoryAgent, WaitingNow } from '@shared/overview/types'
import { lastHourWaits } from '@shared/overview/waits'
import type { CountKey } from '@shared/overview/buckets'
import { computeCounts } from '@shared/overview/status'
import { useNow, useOverviewSnapshot, type OverviewData } from '@/hooks/useOverview'
import { useOverviewHistory } from '@/hooks/useOverviewHistory'
import { useOverviewWaits } from '@/hooks/useOverviewWaits'
import { Card, EmptyState } from './Card'
import { CountHistory } from './CountHistory'
import { CountsRow } from './CountsRow'
import { projectName } from './format'
import { EventsCard } from './EventsCard'
import { lastWaitFrom } from './historyFacts'
import { LastHourChart } from './LastHourChart'
import { NeedsYouPanel } from './NeedsYouPanel'
import { OwnEvents } from './OwnEvents'
import { SessionDetails } from './SessionDetails'
import { SessionsCard } from './SessionsCard'
import { UsageCard } from './UsageCard'
import { WaitedOnYou } from './WaitedOnYou'
import { WaitsCard } from './WaitsCard'
import { OwnTimeline, TimelineCard } from './TimelineCard'
import styles from './OverviewView.module.css'

const FILTER_STORAGE_KEY = 'agentStudio.overview.projectFilter'
const ALL_PROJECTS = ''
const NO_AGENTS: Record<string, HistoryAgent> = {}

export { projectName }

interface OverviewViewProps {
  /** Selects the agent (and its project) and switches to the Chat view. */
  onJumpToAgent?: (agentId: string) => void
  onOpenNewAgent?: () => void
  /** Snapshot state owned by the parent (shared with the tab badge); subscribes itself when absent. */
  overview?: OverviewData
  /** Usage state owned by `App`; `null` while loading. Leave undefined to hide the usage card. */
  usage?: UsageState | null
  /** Opens the detailed usage report. */
  onOpenUsageReport?: () => void
}

function readStoredFilter(): string {
  try {
    return localStorage.getItem(FILTER_STORAGE_KEY) ?? ALL_PROJECTS
  } catch {
    return ALL_PROJECTS
  }
}

function writeStoredFilter(value: string): void {
  try {
    if (value === ALL_PROJECTS) localStorage.removeItem(FILTER_STORAGE_KEY)
    else localStorage.setItem(FILTER_STORAGE_KEY, value)
  } catch {
    /* storage unavailable */
  }
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`
}

export function OverviewView({ onOpenNewAgent, onJumpToAgent, overview, usage, onOpenUsageReport }: OverviewViewProps) {
  const own = useOverviewSnapshot(overview !== undefined)
  const { snapshot, loading, error } = overview ?? own
  const now = useNow()
  const { history } = useOverviewHistory()
  const { ledger } = useOverviewWaits()
  const [storedFilter, setStoredFilter] = useState<string>(readStoredFilter)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [chartKey, setChartKey] = useState<CountKey | null>(null)
  const rootRef = useRef<HTMLDivElement>(null)
  const chartTriggerRef = useRef<HTMLElement | null>(null)
  const triggerRef = useRef<HTMLElement | null>(null)

  const openDetails = useCallback((agentId: string, trigger: HTMLElement | null) => {
    triggerRef.current = trigger
    setSelectedId(agentId)
  }, [])

  // Esc or the close button: back to the row (or card) that opened the drawer.
  const closeDetails = useCallback(() => {
    const agentId = selectedId
    setSelectedId(null)
    const trigger =
      triggerRef.current?.isConnected === true
        ? triggerRef.current
        : rootRef.current?.querySelector<HTMLElement>(`[data-session-trigger="${agentId}"]`)
    trigger?.focus()
  }, [selectedId])

  const allSessions = useMemo(() => snapshot?.sessions ?? [], [snapshot])

  const projectDirs = useMemo(
    () => Array.from(new Set(allSessions.map((s) => s.projectDir))).sort((a, b) =>
      projectName(a).localeCompare(projectName(b))
    ),
    [allSessions]
  )

  // A stored project that no longer has agents falls back to "All" without forgetting the choice.
  const filter = projectDirs.includes(storedFilter) ? storedFilter : ALL_PROJECTS

  const sessions = useMemo(
    () => (filter === ALL_PROJECTS ? allSessions : allSessions.filter((s) => s.projectDir === filter)),
    [allSessions, filter]
  )
  const counts = useMemo(() => computeCounts(sessions, now), [sessions, now])
  const historyEvents = history?.events
  const lastWait = useMemo(() => {
    if (!historyEvents) return undefined
    const ids = new Set(sessions.map((s) => s.agentId))
    return lastWaitFrom(historyEvents.filter((e) => ids.has(e.agentId)), now)
  }, [historyEvents, sessions, now])

  const historyAgents = history?.agents
  // Same identity until the filter or the known agents change, so the chart math can be memoized.
  const agentFilter = useMemo(
    () =>
      filter === ALL_PROJECTS
        ? undefined
        : (agentId: string) => historyAgents?.[agentId]?.projectDir === filter,
    [filter, historyAgents]
  )
  const waitingNow = useMemo<WaitingNow[]>(
    () =>
      allSessions
        .filter((s) => s.status === 'needs-you')
        .map((s) => ({ agentId: s.agentId, name: s.name, since: s.statusSince })),
    [allSessions]
  )
  // The ledger names the project of every agent that waited, also those that are gone.
  const waitsFilter = useMemo(() => {
    if (filter === ALL_PROJECTS) return undefined
    const dirs = new Map<string, string>()
    for (const t of ledger?.transitions ?? []) dirs.set(t.agentId, t.projectDir)
    for (const s of allSessions) dirs.set(s.agentId, s.projectDir)
    return (agentId: string) => dirs.get(agentId) === filter
  }, [filter, ledger, allSessions])
  const lastStart = ledger?.starts[ledger.starts.length - 1]
  // Once a second: an open wait grows with the clock.
  const waited = useMemo(
    () =>
      ledger
        ? lastHourWaits(ledger.transitions, waitingNow, ledger.spans, now, {
            starts: ledger.starts,
            agentFilter: waitsFilter,
            from: lastStart
          })
        : null,
    [ledger, waitingNow, now, waitsFilter, lastStart]
  )
  const historyPoints = history?.points
  const historyStarts = history?.starts
  const noPoints = useMemo<never[]>(() => [], [])

  const toggleChart = useCallback((key: CountKey, trigger: HTMLElement) => {
    chartTriggerRef.current = trigger
    setChartKey((current) => (current === key ? null : key))
  }, [])

  // Esc or the close button: back to the count that opened the chart.
  const closeChart = useCallback(() => {
    setChartKey(null)
    const trigger = chartTriggerRef.current
    if (trigger?.isConnected) trigger.focus()
  }, [])

  const handleFilterChange = (value: string) => {
    setStoredFilter(value)
    writeStoredFilter(value)
  }

  if (loading && !snapshot) {
    return (
      <div className={styles.overview}>
        <div className={styles.centered}>Loading overview…</div>
      </div>
    )
  }

  if (!snapshot) {
    return (
      <div className={styles.overview}>
        <div className={styles.centered} role="alert">
          <p className={styles.errorTitle}>The overview is not available</p>
          <p className={styles.errorText}>
            {error ?? 'No data received.'} If Agent Studio was updated, restart the background service to enable it.
          </p>
        </div>
      </div>
    )
  }

  if (allSessions.length === 0) {
    return (
      <div className={styles.overview}>
        <div className={styles.centered}>
          <p className={styles.emptyTitle}>No agents yet</p>
          <p className={styles.errorText}>Start an agent and its status shows up here.</p>
          {onOpenNewAgent && (
            <button className={styles.primaryBtn} onClick={onOpenNewAgent}>
              New Agent
            </button>
          )}
        </div>
      </div>
    )
  }

  const needsYou = sessions.filter((s) => s.status === 'needs-you')
  const multipleProviders = new Set(allSessions.map((s) => s.provider)).size > 1
  const jump = (agentId: string) => onJumpToAgent?.(agentId)
  const selectedBar = waited?.bars.find((bar) => bar.agentId === selectedId)
  const selectedWaits = selectedBar && waited ? { count: selectedBar.count, ms: selectedBar.ms, from: waited.from } : undefined

  return (
    <div className={styles.overview} ref={rootRef}>
      <div className={styles.topBar}>
        <h2 className={styles.title}>Overview</h2>
        <span className={styles.summary}>
          {plural(sessions.length, 'agent', 'agents')}
          {counts.needsYou > 0 && ` · ${counts.needsYou} need you`}
          {counts.working > 0 && ` · ${counts.working} working`}
        </span>
        <div className={styles.spacer} />
        <label className={styles.filter}>
          <span className={styles.filterLabel}>Project</span>
          <select
            className={styles.select}
            value={filter}
            onChange={(e) => handleFilterChange(e.target.value)}
            aria-label="Project filter"
            title={filter === ALL_PROJECTS ? 'All projects' : filter}
          >
            <option value={ALL_PROJECTS}>All projects</option>
            {projectDirs.map((dir) => (
              <option key={dir} value={dir} title={dir}>
                {projectName(dir)}
              </option>
            ))}
          </select>
        </label>
      </div>

      <section className={styles.summaryBlock} aria-label="Agent statistics">
        <CountsRow counts={counts} onOpenChart={toggleChart} openChart={chartKey} />
        {chartKey && (
          <CountHistory
            key={chartKey}
            metric={chartKey}
            points={historyPoints ?? noPoints}
            starts={historyStarts ?? noPoints}
            now={now}
            agentFilter={agentFilter}
            onClose={closeChart}
          />
        )}
      </section>

      <div className={styles.grid}>
        {usage !== undefined && <UsageCard state={usage} projectDir={filter} onOpenReport={onOpenUsageReport} />}

        <Card
          title="Needs you"
          area="needs"
          meta={counts.needsYou > 0 ? counts.needsYou : undefined}
          onTitleClick={(el) => toggleChart('needsYou', el)}
          titleExpanded={chartKey === 'needsYou'}
          titleLabel="Needs you history"
        >
          <div className={styles.needsBody}>
            <NeedsYouPanel
              sessions={needsYou}
              now={now}
              showProvider={multipleProviders}
              onJumpToAgent={jump}
              onOpenDetails={openDetails}
              lastWait={lastWait}
            />
            {waited && <WaitedOnYou bars={waited.bars} from={waited.from} unmeasuredMs={waited.unmeasuredMs} />}

          </div>
        </Card>

        <Card title="Last hour" area="lastHour">
          {historyPoints && historyStarts ? (
            <LastHourChart points={historyPoints} starts={historyStarts} now={now} agentFilter={agentFilter} />
          ) : (
            <EmptyState>No history yet</EmptyState>
          )}
        </Card>

        <SessionsCard
          sessions={sessions}
          now={now}
          showProvider={multipleProviders}
          selectedId={selectedId}
          onOpenDetails={openDetails}
          onJumpToAgent={jump}
        />

        <EventsCard history={history} filter={filter} now={now} onOpenAgent={openDetails} />

        <TimelineCard
          points={historyPoints ?? noPoints}
          starts={historyStarts ?? noPoints}
          now={now}
          agents={historyAgents ?? NO_AGENTS}
          agentFilter={agentFilter}
        />

        <WaitsCard ledger={ledger} waitingNow={waitingNow} now={now} agentFilter={waitsFilter} />
      </div>

      {selectedId && (
        <SessionDetails
          key={selectedId}
          agentId={selectedId}
          session={allSessions.find((x) => x.agentId === selectedId)}
          now={now}
          onClose={closeDetails}
          onJumpToAgent={jump}
          waited={selectedWaits}
          ownEvents={<OwnEvents events={history?.events ?? []} agentId={selectedId} now={now} />}
          timelineRow={
            <OwnTimeline
              points={historyPoints ?? noPoints}
              starts={historyStarts ?? noPoints}
              now={now}
              agentId={selectedId}
            />
          }
        />
      )}
    </div>
  )
}
