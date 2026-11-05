import type { ClaudeProfileLimits, LimitWindow, UsageState, UsageSummary } from '@shared/usage/types'

export const NOW = new Date(2026, 9, 23, 12, 0, 0).getTime()

export function win(account: string, kind: 'session' | 'weekly', usedPercent: number, resetsAt: number | null): LimitWindow {
  return {
    provider: 'claude',
    account,
    id: kind,
    kind,
    label: kind === 'session' ? 'Current session (5h)' : 'Weekly · all models',
    usedPercent,
    resetsAt
  }
}

export function profile(account: string, over: Partial<ClaudeProfileLimits> = {}): ClaudeProfileLimits {
  return {
    account,
    configDir: `/home/u/.claude-${account}`,
    plan: 'Pro',
    status: 'ok',
    windows: [
      win(account, 'session', 29, NOW + 108 * 60_000),
      win(account, 'weekly', 4, NOW + 4 * 86_400_000)
    ],
    fetchedAt: NOW - 60_000,
    checkedAt: NOW - 60_000,
    ...over
  }
}

export function usageState(over: Partial<UsageState> = {}): UsageState {
  return {
    claude: [
      profile('default'),
      profile('alternative', {
        windows: [win('alternative', 'session', 20, NOW + 3_600_000), win('alternative', 'weekly', 26, NOW + 3 * 86_400_000)]
      }),
      profile('work', { plan: null, status: 'expired', windows: [], fetchedAt: null })
    ],
    codex: null,
    indexing: { indexing: false, progress: null, lastScanAt: NOW - 60_000 },
    dataVersion: 1,
    updatedAt: NOW - 60_000,
    ...over
  }
}

export function summaryOf(period: 'today' | 'week' | 'month' = 'today', over: Partial<UsageSummary> = {}): UsageSummary {
  const slots = Array.from({ length: 24 }, (_, h) => ({
    start: NOW + h * 3_600_000,
    label: String(h).padStart(2, '0'),
    tokensByProvider: { claude: h === 15 ? 1000 : h % 5, codex: 0 }
  }))
  return {
    period,
    range: { start: NOW - 12 * 3_600_000, end: NOW },
    totalTokens: 48_200_000,
    messages: 1284,
    costUsd: 31.4,
    costPartial: false,
    byProvider: [
      { provider: 'claude', tokens: { input: 0, output: 0, cacheWrite5m: 0, cacheWrite1h: 0, cacheRead: 0 }, totalTokens: 48_200_000, messages: 1284, costUsd: 31.4, costPartial: false },
      { provider: 'codex', tokens: { input: 0, output: 0, cacheWrite5m: 0, cacheWrite1h: 0, cacheRead: 0 }, totalTokens: 0, messages: 0, costUsd: 0, costPartial: false }
    ],
    byModel: [{ model: 'claude-opus-5-5', provider: 'claude', totalTokens: 31_200_000, costUsd: 20 }],
    slots,
    previous: { totalTokens: 43_000_000, costUsd: 28 },
    tokensChangePct: 12,
    ...over
  }
}
