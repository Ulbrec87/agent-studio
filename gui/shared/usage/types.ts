import type { CodexParsedLimits } from './codexLine'

export type UsageProvider = 'claude' | 'codex'
export const USAGE_PROVIDERS: readonly UsageProvider[] = ['claude', 'codex']
export type UsagePeriod = 'today' | 'week' | 'month'

/** Token counts of one or more model calls. `input` is fresh (uncached) input only. */
export interface TokenCounts {
  input: number
  /** Includes reasoning/thinking tokens. */
  output: number
  cacheWrite5m: number
  cacheWrite1h: number
  cacheRead: number
}

/** One usage record as parsed from a log line (Steps 2-3 produce these). */
export interface UsageRecord {
  /** Epoch ms. */
  ts: number
  provider: UsageProvider
  /** Normalised model id (see `pricing.normalizeModel`). */
  model: string
  /** Folder/cwd the call was made in, `''` when unknown. */
  projectKey: string
  tokens: TokenCounts
}

/** Aggregate of records in one UTC hour for one (provider, model, projectKey). */
export interface UsageBucket {
  /** Epoch ms at the start of the UTC hour. */
  hour: number
  provider: UsageProvider
  model: string
  projectKey: string
  tokens: TokenCounts
  messages: number
  /** Estimated cost of the priced records only. */
  costUsd: number
  /** Records whose model has no price. */
  unpricedMessages: number
}

export interface ProviderUsage {
  provider: UsageProvider
  tokens: TokenCounts
  totalTokens: number
  messages: number
  costUsd: number
  costPartial: boolean
}

export interface ModelUsage {
  model: string
  provider: UsageProvider
  totalTokens: number
  costUsd: number
}

/** One chart slot (an hour, a day) with its tokens per provider. */
export interface UsageSlot {
  start: number
  label: string
  tokensByProvider: Record<UsageProvider, number>
}

export interface UsageSummary {
  period: UsagePeriod
  range: { start: number; end: number }
  totalTokens: number
  messages: number
  costUsd: number
  /** Some records had no price, so `costUsd` is a lower bound. */
  costPartial: boolean
  /** One entry per `USAGE_PROVIDERS`, in that order (zeros when unused). */
  byProvider: ProviderUsage[]
  /** Sorted by `totalTokens`, largest first. */
  byModel: ModelUsage[]
  slots: UsageSlot[]
  /** `null` when the previous range has no data. */
  previous: { totalTokens: number; costUsd: number } | null
  /** `(current - previous) / previous * 100`, `null` when `previous` is `null` or 0. */
  tokensChangePct: number | null
}

export type LimitWindowKind = 'session' | 'weekly' | 'weekly-model' | 'other'

/** One plan-limit window (5-hour session, weekly, weekly for one model). */
export interface LimitWindow {
  provider: UsageProvider
  /** Claude profile name or Codex account id; 'default' when unknown. */
  account: string
  /** Stable within (provider, account): 'session' | 'weekly' | 'weekly:<model-slug>' | other kind. */
  id: string
  kind: LimitWindowKind
  /** Display label, e.g. '5-hour window', 'Weekly'. */
  label: string
  /** 0-100, clamped. */
  usedPercent: number
  /** Epoch ms. */
  resetsAt: number | null
}

export function emptyTokens(): TokenCounts {
  return { input: 0, output: 0, cacheWrite5m: 0, cacheWrite1h: 0, cacheRead: 0 }
}

export function addTokens(a: TokenCounts, b: TokenCounts): TokenCounts {
  return {
    input: a.input + b.input,
    output: a.output + b.output,
    cacheWrite5m: a.cacheWrite5m + b.cacheWrite5m,
    cacheWrite1h: a.cacheWrite1h + b.cacheWrite1h,
    cacheRead: a.cacheRead + b.cacheRead
  }
}

export function totalTokens(t: TokenCounts): number {
  return t.input + t.output + t.cacheWrite5m + t.cacheWrite1h + t.cacheRead
}

/** Plan limits of one Claude profile. Only percentages, times and labels: never a credential. */
export interface ClaudeProfileLimits {
  /** Profile label ('default', 'work', ...). */
  account: string
  configDir: string
  plan: string | null
  status: 'ok' | 'not-signed-in' | 'expired' | 'unavailable' | 'rate-limited' | 'error' | 'keychain-denied'
  /** Last good values when `status` is not 'ok' (may be empty). */
  windows: LimitWindow[]
  /** When `windows` were fetched. */
  fetchedAt: number | null
  /** Last attempt. */
  checkedAt: number
}

export interface UsageScanProgress {
  filesDone: number
  filesTotal: number
  bytesDone: number
  bytesTotal: number
}

/** What the renderer shows of the usage service; pushed on change. */
export interface UsageState {
  claude: ClaudeProfileLimits[]
  /** Passive limits from the newest Codex rollout line, `null` when none was seen. */
  codex: CodexParsedLimits | null
  indexing: { indexing: boolean; progress: UsageScanProgress | null; lastScanAt: number | null }
  /** Bumps whenever the store's data changed; the renderer refetches summaries when it changes. */
  dataVersion: number
  updatedAt: number
}
