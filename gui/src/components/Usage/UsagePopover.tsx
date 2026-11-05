import { useEffect, useRef, useState, type RefObject } from 'react'
import type { UsagePeriod, UsageState } from '@shared/usage/types'
import { useCoarseNow, usePersistedChoice, useUsageSummary, useUsageVisibility } from '../../hooks/useUsage'
import { formatCompactTokens, formatUpdated } from './format'
import { LimitRows, type LimitsFilter } from './LimitRows'
import { newestFetch } from './model'
import { Segmented } from './Segmented'
import { TokensPanel } from './TokensPanel'
import styles from './UsagePopover.module.css'

export type CloseReason = 'escape' | 'outside' | 'action'

interface UsagePopoverProps {
  state: UsageState | null
  /** The chip: clicks on it are not "outside" (it toggles the popover itself). */
  anchorRef: RefObject<HTMLElement | null>
  onClose: (reason: CloseReason) => void
  /** Opens the detailed report (the old Usage dashboard). */
  onOpenReport: () => void
}

const FILTERS = ['low', 'all'] as const
const PERIODS = ['today', 'week', 'month'] as const

export function UsagePopover({ state, anchorRef, onClose, onOpenReport }: UsagePopoverProps) {
  const ref = useRef<HTMLDivElement>(null)
  const now = useCoarseNow(30_000)
  const [filter, setFilter] = usePersistedChoice<LimitsFilter>('agentStudio.usage.limitsFilter', 'all', FILTERS)
  const [period, setPeriod] = usePersistedChoice<UsagePeriod>('agentStudio.usage.period', 'today', PERIODS)
  const { summary } = useUsageSummary(period, true, state?.dataVersion)
  const [refreshing, setRefreshing] = useState(false)

  useUsageVisibility(true)

  useEffect(() => {
    ref.current?.focus()
  }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') onClose('escape')
    }
    const onDown = (e: MouseEvent): void => {
      const target = e.target as Node
      if (ref.current?.contains(target) || anchorRef.current?.contains(target)) return
      onClose('outside')
    }
    document.addEventListener('keydown', onKey)
    document.addEventListener('mousedown', onDown)
    return () => {
      document.removeEventListener('keydown', onKey)
      document.removeEventListener('mousedown', onDown)
    }
  }, [onClose, anchorRef])

  const refresh = async (): Promise<void> => {
    if (refreshing) return
    setRefreshing(true)
    try {
      await window.hydra.refreshUsage()
    } catch {
      /* the next push or retry brings the state */
    } finally {
      setRefreshing(false)
    }
  }

  const topModel = summary?.byModel[0]

  return (
    <div ref={ref} className={styles.popover} role="dialog" aria-label="Usage and limits" tabIndex={-1}>
      <header className={styles.header}>
        <h2 className={styles.title}>Usage</h2>
        {state && <span className={styles.updated}>Updated {formatUpdated(newestFetch(state), now)}</span>}
        <button
          type="button"
          className={styles.iconBtn}
          aria-label="Refresh usage"
          data-spinning={refreshing}
          disabled={refreshing}
          onClick={() => void refresh()}
        >
          <RefreshIcon />
        </button>
      </header>

      <div className={styles.body}>
        <section className={styles.section} aria-label="Plan limits">
          <div className={styles.sectionHead}>
            <h3 className={styles.sectionTitle}>Plan limits</h3>
            <Segmented
              label="Show plan limits"
              value={filter}
              options={[
                { value: 'low', label: 'Low' },
                { value: 'all', label: 'All' }
              ]}
              onChange={setFilter}
            />
          </div>
          {state ? <LimitRows state={state} filter={filter} now={now} /> : <p className={styles.muted}>Loading…</p>}
        </section>

        <section className={styles.section} aria-label="Tokens">
          <div className={styles.sectionHead}>
            <h3 className={styles.sectionTitle}>Tokens</h3>
            <Segmented
              label="Token period"
              value={period}
              options={[
                { value: 'today', label: 'Today' },
                { value: 'week', label: 'Week' },
                { value: 'month', label: 'Month' }
              ]}
              onChange={setPeriod}
            />
          </div>
          <TokensPanel period={period} summary={summary} indexing={state?.indexing ?? null} />
        </section>
      </div>

      <footer className={styles.footer}>
        <span className={styles.topModel}>
          {topModel ? `Top model: ${topModel.model.replace(/^claude-/, '')} · ${formatCompactTokens(topModel.totalTokens)}` : ''}
        </span>
        <button
          type="button"
          className={styles.link}
          onClick={() => {
            onClose('action')
            onOpenReport()
          }}
        >
          Detailed report
        </button>
      </footer>
    </div>
  )
}

function RefreshIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M21 12a9 9 0 1 1-2.64-6.36" />
      <polyline points="21 3 21 9 15 9" />
    </svg>
  )
}
