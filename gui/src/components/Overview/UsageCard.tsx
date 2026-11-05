import type { UsagePeriod, UsageState } from '@shared/usage/types'
import { limitSeverity } from '@shared/usage/limits'
import { useCoarseNow, usePersistedChoice, useUsageSummary, useUsageVisibility } from '@/hooks/useUsage'
import { formatAge, formatChange, formatCompactTokens, formatExactTokens, formatResets, formatUpdated, formatUsd } from '../Usage/format'
import { buildLimitTiles, newestFetch, type LimitTile } from '../Usage/model'
import { useUsageSettings } from '../Usage/UsageSettingsContext'
import { ProviderDot } from '../Usage/ProviderDot'
import { ProviderLegend } from '../Usage/ProviderLegend'
import { Segmented } from '../Usage/Segmented'
import { indexingPercent } from '../Usage/TokensPanel'
import { UsageBars } from '../Usage/UsageBars'
import { Card } from './Card'
import { projectName } from './format'
import styles from './UsageCard.module.css'

const PERIODS = ['today', 'week', 'month'] as const
const PERIOD_TITLES = { today: 'today', week: 'this week', month: 'this month' } as const
const MAX_TILES = 8

interface UsageCardProps {
  /** `null` until the first load: the card shows a skeleton so the grid does not jump. */
  state: UsageState | null
  /** The Overview project filter: a project directory, or `''` for all projects. */
  projectDir: string
  /** Opens the detailed report (the Usage dashboard). */
  onOpenReport?: () => void
}

/** The Overview "Usage & limits" card: plan limits (account-wide) and tokens of one period. */
export function UsageCard({ state, projectDir, onOpenReport }: UsageCardProps) {
  const now = useCoarseNow(30_000)
  const { showCost } = useUsageSettings()
  const [period, setPeriod] = usePersistedChoice<UsagePeriod>('agentStudio.usage.cardPeriod', 'week', PERIODS)
  // Only mounted while the Overview is shown, so this pings exactly while the card is visible.
  useUsageVisibility(true)
  // Claude's and Codex's `cwd` is the directory the agent ran in. An agent in a git worktree
  // records the worktree path, which differs from the project directory, so its tokens are
  // not counted under the project. Known limitation, not worth a path-prefix heuristic.
  const projectKey = projectDir || undefined
  const { summary } = useUsageSummary(period, state !== null, state?.dataVersion, projectKey)

  if (!state) {
    return (
      <Card title="Usage & limits" area="usage">
        <div className={styles.body} aria-busy="true" data-testid="usage-skeleton">
          <div className={styles.skeletonTiles}>
            {[0, 1, 2, 3].map((i) => (
              <div key={i} className={styles.skeleton} />
            ))}
          </div>
          <div className={styles.skeletonChart}>
            <div className={styles.skeletonBars}>
              {[40, 70, 55, 90, 30, 60, 45].map((h, i) => (
                <span key={i} style={{ height: `${h}%` }} />
              ))}
            </div>
          </div>
        </div>
      </Card>
    )
  }

  const tiles = buildLimitTiles(state)
  const shown = tiles.slice(0, MAX_TILES)
  const hidden = tiles.length - shown.length
  const percent = indexingPercent(state.indexing)
  const where = projectKey ? ` · ${projectName(projectKey)}` : ''

  return (
    <Card
      title="Usage & limits"
      area="usage"
      meta={`updated ${formatUpdated(newestFetch(state), now)}`}
      actions={
        <>
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
          {onOpenReport && (
            <button type="button" className={styles.link} onClick={onOpenReport}>
              Detailed report
            </button>
          )}
        </>
      }
    >
      <div className={styles.body}>
        <section className={styles.section} aria-label="Plan limits">
          <h4 className={styles.sectionTitle}>Plan limits</h4>
          {shown.length === 0 ? (
            <p className={styles.quiet}>No plan limits found.</p>
          ) : (
            <div className={styles.tiles}>
              {shown.map((t) => (
                <Tile key={t.key} tile={t} now={now} />
              ))}
            </div>
          )}
          {hidden > 0 && <p className={styles.note}>+{hidden} more in the header popover</p>}
          {projectKey && <p className={styles.note}>Limits are per account, not per project</p>}
        </section>

        <section className={styles.section} aria-label="Tokens">
          <h4 className={styles.sectionTitle}>
            Tokens · {PERIOD_TITLES[period]}
            {where}
          </h4>
          {state.indexing?.indexing && (
            <p className={styles.indexing}>Indexing usage…{percent !== null ? ` ${percent}%` : ''}</p>
          )}
          {summary ? (
            <>
              <div className={styles.hero}>
                <span className={styles.heroNumber} title={formatExactTokens(summary.totalTokens)}>
                  {formatCompactTokens(summary.totalTokens)}
                </span>
                <span className={styles.heroUnit}>
                  {[
                    'tokens',
                    showCost ? `${summary.costPartial ? '≥' : '~'}${formatUsd(summary.costUsd)} API-equivalent` : '',
                    formatChange(summary.tokensChangePct, period)
                  ]
                    .filter(Boolean)
                    .join(' · ')}
                </span>
              </div>
              <UsageBars period={period} slots={summary.slots} tall />
              <ProviderLegend summary={summary} />
            </>
          ) : (
            <p className={styles.quiet}>Loading…</p>
          )}
        </section>
      </div>
    </Card>
  )
}

function Tile({ tile, now }: { tile: LimitTile; now: number }) {
  if (tile.problem) {
    return (
      <div className={styles.tile} data-problem="true">
        <div className={styles.tileHead}>
          <ProviderDot provider={tile.provider} />
          <span className={styles.tileLabel}>{tile.label}</span>
        </div>
        <p className={styles.problemTitle}>{tile.problem.title}</p>
        {tile.problem.hint && <p className={styles.problemHint}>{tile.problem.hint}</p>}
      </div>
    )
  }
  const w = tile.timed!.window
  const used = Math.round(w.usedPercent)
  return (
    <div className={styles.tile} data-dimmed={tile.dimmed} data-severity={limitSeverity(w.usedPercent)}>
      <div className={styles.tileHead}>
        <ProviderDot provider={tile.provider} />
        <span className={styles.tileLabel}>{tile.label}</span>
        {tile.estimated && (
          <span className={styles.pill}>
            ESTIMATED{tile.at !== null ? ` · ${formatAge(now - tile.at)}` : ''}
          </span>
        )}
      </div>
      <div className={styles.tileValue}>
        <span className={styles.percent}>{used}%</span>
        <span className={styles.used}>used</span>
      </div>
      <div className={styles.bar} role="progressbar" aria-label={tile.label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={used}>
        <div className={styles.barFill} style={{ width: `${used}%` }} />
      </div>
      <span className={styles.reset}>{formatResets(w.resetsAt, now) || ' '}</span>
    </div>
  )
}
