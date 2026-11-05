import { claudeProjectKeyLabel } from './claudeLine'
import { addTokens, emptyTokens, totalTokens, type TokenCounts, type UsageBucket, type UsageProvider } from './types'

export type UsageReportProvider = UsageProvider | 'all'

export interface UsageReportModel {
  model: string
  tokens: TokenCounts
  totalTokens: number
  costUsd: number
}

export interface UsageReportProject {
  projectKey: string
  /** Last path segment of the project key. */
  label: string
  totalTokens: number
  costUsd: number
}

export interface UsageReportDay {
  /** Local calendar day, `YYYY-MM-DD`. */
  date: string
  tokens: TokenCounts
  totalTokens: number
  messages: number
  costUsd: number
  /** Some messages had no price, so `costUsd` is a lower bound. */
  costPartial: boolean
  /** Sorted by `totalTokens`, largest first. */
  models: UsageReportModel[]
  /** Sorted by `totalTokens`, largest first. */
  projects: UsageReportProject[]
}

export interface UsageReport {
  provider: UsageReportProvider
  /** One entry per local calendar day, oldest first, empty days included. */
  daily: UsageReportDay[]
  /** Epoch ms. */
  generatedAt: number
}

export const REPORT_MIN_DAYS = 7
export const REPORT_MAX_DAYS = 90

export function dateKey(d: Date): string {
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  return `${d.getFullYear()}-${m}-${day}`
}

/** Daily report of the last `days` local calendar days ending with the day of `now`. */
export function buildUsageReport(
  buckets: readonly UsageBucket[],
  opts: { days: number; provider: UsageReportProvider; now: Date }
): UsageReport {
  const days = Math.max(1, Math.floor(opts.days))
  const { now, provider } = opts

  interface Acc {
    day: UsageReportDay
    models: Map<string, UsageReportModel>
    projects: Map<string, UsageReportProject>
  }
  const byDate = new Map<string, Acc>()
  const order: Acc[] = []
  for (let i = days - 1; i >= 0; i--) {
    const d = new Date(now.getFullYear(), now.getMonth(), now.getDate() - i)
    const acc: Acc = {
      day: {
        date: dateKey(d),
        tokens: emptyTokens(),
        totalTokens: 0,
        messages: 0,
        costUsd: 0,
        costPartial: false,
        models: [],
        projects: []
      },
      models: new Map(),
      projects: new Map()
    }
    byDate.set(acc.day.date, acc)
    order.push(acc)
  }

  for (const b of buckets) {
    if (provider !== 'all' && b.provider !== provider) continue
    const acc = byDate.get(dateKey(new Date(b.hour)))
    if (!acc) continue
    const total = totalTokens(b.tokens)
    const { day } = acc
    day.tokens = addTokens(day.tokens, b.tokens)
    day.totalTokens += total
    day.messages += b.messages
    day.costUsd += b.costUsd
    if (b.unpricedMessages > 0) day.costPartial = true

    const model = acc.models.get(b.model)
    if (model) {
      model.tokens = addTokens(model.tokens, b.tokens)
      model.totalTokens += total
      model.costUsd += b.costUsd
    } else {
      acc.models.set(b.model, { model: b.model, tokens: addTokens(emptyTokens(), b.tokens), totalTokens: total, costUsd: b.costUsd })
    }

    const project = acc.projects.get(b.projectKey)
    if (project) {
      project.totalTokens += total
      project.costUsd += b.costUsd
    } else {
      acc.projects.set(b.projectKey, {
        projectKey: b.projectKey,
        label: claudeProjectKeyLabel(b.projectKey),
        totalTokens: total,
        costUsd: b.costUsd
      })
    }
  }

  const bySize = <T extends { totalTokens: number }>(a: T, b: T) => b.totalTokens - a.totalTokens
  return {
    provider,
    daily: order.map(({ day, models, projects }) => ({
      ...day,
      models: [...models.values()].sort(bySize),
      projects: [...projects.values()].sort(bySize)
    })),
    generatedAt: now.getTime()
  }
}
