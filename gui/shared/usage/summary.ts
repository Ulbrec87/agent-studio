import { periodRange, periodSlots, previousRange } from './periods'
import {
  USAGE_PROVIDERS,
  addTokens,
  emptyTokens,
  totalTokens,
  type ModelUsage,
  type ProviderUsage,
  type UsageBucket,
  type UsagePeriod,
  type UsageProvider,
  type UsageSlot,
  type UsageSummary
} from './types'

export type { ModelUsage, ProviderUsage, UsageSlot, UsageSummary } from './types'

/** Buckets are one hour wide, so a bucket is in a range when its hour start is (hour granularity is enough). */
function within(buckets: readonly UsageBucket[], start: number, end: number, projectKey?: string): UsageBucket[] {
  return buckets.filter(
    (b) => b.hour >= start && b.hour < end && (projectKey === undefined || b.projectKey === projectKey)
  )
}

export function summarize(
  buckets: readonly UsageBucket[],
  period: UsagePeriod,
  now: Date,
  opts?: { projectKey?: string }
): UsageSummary {
  const range = periodRange(period, now)
  const projectKey = opts?.projectKey
  const current = within(buckets, range.start, range.end, projectKey)

  const providers = new Map<UsageProvider, ProviderUsage>(
    USAGE_PROVIDERS.map((provider) => [
      provider,
      { provider, tokens: emptyTokens(), totalTokens: 0, messages: 0, costUsd: 0, costPartial: false }
    ])
  )
  const models = new Map<string, ModelUsage>()
  const periodSlotRanges = periodSlots(period, now)
  const slots: UsageSlot[] = periodSlotRanges.map((s) => ({
    start: s.start,
    label: s.label,
    tokensByProvider: Object.fromEntries(USAGE_PROVIDERS.map((p) => [p, 0])) as Record<UsageProvider, number>
  }))

  let messages = 0
  let costUsd = 0
  let costPartial = false
  for (const b of current) {
    const tokens = totalTokens(b.tokens)
    messages += b.messages
    costUsd += b.costUsd
    if (b.unpricedMessages > 0) costPartial = true

    const p = providers.get(b.provider)
    if (p) {
      p.tokens = addTokens(p.tokens, b.tokens)
      p.totalTokens += tokens
      p.messages += b.messages
      p.costUsd += b.costUsd
      if (b.unpricedMessages > 0) p.costPartial = true
    }

    const key = `${b.provider}\u0000${b.model}`
    const m = models.get(key) ?? { model: b.model, provider: b.provider, totalTokens: 0, costUsd: 0 }
    m.totalTokens += tokens
    m.costUsd += b.costUsd
    models.set(key, m)

    const i = slots.findIndex((s, idx) => b.hour >= s.start && b.hour < periodSlotRanges[idx].end)
    if (i >= 0) slots[i].tokensByProvider[b.provider] += tokens
  }

  const total = [...providers.values()].reduce((sum, p) => sum + p.totalTokens, 0)

  const prevRange = previousRange(period, now)
  const prev = within(buckets, prevRange.start, prevRange.end, projectKey)
  const previous =
    prev.length === 0
      ? null
      : {
          totalTokens: prev.reduce((sum, b) => sum + totalTokens(b.tokens), 0),
          costUsd: prev.reduce((sum, b) => sum + b.costUsd, 0)
        }

  return {
    period,
    range,
    totalTokens: total,
    messages,
    costUsd,
    costPartial,
    byProvider: [...providers.values()],
    byModel: [...models.values()].sort((a, b) => b.totalTokens - a.totalTokens),
    slots,
    previous,
    tokensChangePct: previous && previous.totalTokens > 0 ? ((total - previous.totalTokens) / previous.totalTokens) * 100 : null
  }
}
