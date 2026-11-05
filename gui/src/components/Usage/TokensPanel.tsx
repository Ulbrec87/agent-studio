import { type UsagePeriod, type UsageState, type UsageSummary } from '@shared/usage/types'
import { formatChange, formatCompactTokens, formatExactTokens, formatUsd } from './format'
import { useUsageSettings } from './UsageSettingsContext'
import { ProviderLegend } from './ProviderLegend'
import { UsageBars } from './UsageBars'
import styles from './TokensPanel.module.css'

interface TokensPanelProps {
  period: UsagePeriod
  summary: UsageSummary | null
  indexing: UsageState['indexing'] | null
}

export function indexingPercent(indexing: UsageState['indexing'] | null): number | null {
  const p = indexing?.progress
  if (!indexing?.indexing || !p || p.bytesTotal <= 0) return null
  return Math.min(100, Math.round((p.bytesDone / p.bytesTotal) * 100))
}

export function TokensPanel({ period, summary, indexing }: TokensPanelProps) {
  const percent = indexingPercent(indexing)
  return (
    <div className={styles.panel}>
      {indexing?.indexing && <p className={styles.indexing}>Indexing usage…{percent !== null ? ` ${percent}%` : ''}</p>}
      {summary ? <Summary period={period} summary={summary} /> : <p className={styles.muted}>Loading…</p>}
    </div>
  )
}

function Summary({ period, summary }: { period: UsagePeriod; summary: UsageSummary }) {
  const { showCost } = useUsageSettings()
  const changeText = formatChange(summary.tokensChangePct, period, 'tokens')
  const share = (tokens: number): number => (summary.totalTokens > 0 ? (tokens / summary.totalTokens) * 100 : 0)

  return (
    <>
      <div className={styles.hero}>
        <span className={styles.heroNumber} title={formatExactTokens(summary.totalTokens)}>
          {formatCompactTokens(summary.totalTokens)}
        </span>
        <span className={styles.heroUnit}>
          tokens
          {showCost && (
            <>
              {' · '}
              {summary.costPartial ? '≥' : '~'}
              {formatUsd(summary.costUsd)} API-equivalent
            </>
          )}
        </span>
      </div>
      <p className={styles.muted}>
        {[changeText, `${formatExactTokens(summary.messages)} messages`].filter(Boolean).join(' · ')}
      </p>

      <div className={styles.split} aria-hidden="true">
        {summary.byProvider.map((p) => (
          <span key={p.provider} className={styles.splitSeg} data-provider={p.provider} style={{ width: `${share(p.totalTokens)}%` }} />
        ))}
      </div>
      <ProviderLegend summary={summary} />

      <UsageBars period={period} slots={summary.slots} />
    </>
  )
}
