import { limitAlertLevel, pickChipWindows } from '@shared/usage/limits'
import type { UsageChipProvider, UsageSettings } from '@shared/usage/settings'
import type { ClaudeProfileLimits, LimitWindow, UsageProvider, UsageState } from '@shared/usage/types'

/** Values older than this render dimmed. */
export const STALE_AFTER_MS = 15 * 60_000

/** A window with the time its value was read. */
export interface TimedWindow {
  window: LimitWindow
  /** Epoch ms of the fetch / log line, `null` when unknown. */
  at: number | null
}

export function isStale(at: number | null, now: number): boolean {
  return at === null || now - at > STALE_AFTER_MS
}

function allTimedWindows(state: UsageState): TimedWindow[] {
  const out: TimedWindow[] = []
  for (const profile of state.claude) {
    for (const window of profile.windows) out.push({ window, at: profile.fetchedAt })
  }
  if (state.codex) {
    for (const window of state.codex.windows) out.push({ window, at: state.codex.at })
  }
  return out
}

export interface ChipWindows {
  session: TimedWindow | null
  weekly: TimedWindow | null
}

/**
 * The most-used session and weekly window over every visible account, or of one provider
 * (`chipProvider`); a provider with no session and no weekly window falls back to all.
 */
export function chipWindows(state: UsageState | null, chipProvider: UsageChipProvider = 'auto'): ChipWindows {
  if (!state) return { session: null, weekly: null }
  let timed = allTimedWindows(state)
  if (chipProvider !== 'auto') {
    const own = timed.filter((t) => t.window.provider === chipProvider)
    if (own.some((t) => t.window.kind === 'session' || t.window.kind === 'weekly')) timed = own
  }
  const picked = pickChipWindows(timed.map((t) => t.window))
  const find = (w: LimitWindow | null): TimedWindow | null => (w ? (timed.find((t) => t.window === w) ?? null) : null)
  return { session: find(picked.session), weekly: find(picked.weekly) }
}

/** `Claude · alternative` / `Codex`: where a value comes from. */
export function sourceLabel(w: LimitWindow): string {
  return w.provider === 'codex' ? 'Codex' : `Claude ${w.account}`
}

export interface LimitGroup {
  key: string
  provider: UsageProvider
  name: string
  /** `Pro`, `Pro · alternative`, or `''`. */
  subtitle: string
  profile: ClaudeProfileLimits | null
  windows: TimedWindow[]
  /** Epoch ms of the Codex log line, for the source text. */
  codexAt: number | null
}

/** One group per signed-in Claude account (all of them when none is signed in), then Codex. */
export function buildLimitGroups(state: UsageState): LimitGroup[] {
  const signedIn = state.claude.filter((p) => p.status !== 'not-signed-in')
  const shown = signedIn.length > 0 ? signedIn : state.claude
  const groups: LimitGroup[] = shown.map((profile) => {
    const parts = [profile.plan, shown.length > 1 ? profile.account : null].filter((p): p is string => !!p)
    return {
      key: `claude:${profile.account}`,
      provider: 'claude',
      name: 'Claude',
      subtitle: parts.join(' · '),
      profile,
      windows: profile.windows.map((window) => ({ window, at: profile.fetchedAt })),
      codexAt: null
    }
  })
  if (state.codex) {
    groups.push({
      key: 'codex',
      provider: 'codex',
      name: 'Codex',
      subtitle: state.codex.plan ?? '',
      profile: null,
      windows: state.codex.windows.map((window) => ({ window, at: state.codex!.at })),
      codexAt: state.codex.at
    })
  }
  return groups
}

export const LOW_MAX_ROWS = 2

/** The windows at or above their alert level, most used first, at most `LOW_MAX_ROWS`. */
export function lowWindows(
  groups: LimitGroup[],
  settings?: Pick<UsageSettings, 'sessionAlertPercent' | 'weeklyAlertPercent'>
): Set<LimitWindow> {
  const near = groups
    .flatMap((g) => g.windows.map((t) => t.window))
    .filter((w) => w.usedPercent >= limitAlertLevel(w, settings))
    .sort((a, b) => b.usedPercent - a.usedPercent)
    .slice(0, LOW_MAX_ROWS)
  return new Set(near)
}

/** Newest time any value was read, for the "Updated" text. */
export function newestFetch(state: UsageState): number {
  const times = allTimedWindows(state)
    .map((t) => t.at)
    .filter((t): t is number => t !== null)
  return Math.max(state.updatedAt, ...times)
}

/** Why an account shows no (or only old) limits, `null` when it is fine. */
export function profileProblem(p: ClaudeProfileLimits): { title: string; hint?: string } | null {
  switch (p.status) {
    case 'ok':
      return null
    case 'not-signed-in':
      return { title: 'Not signed in.' }
    case 'expired':
      return { title: 'Sign-in expired', hint: 'Start a Claude agent in this profile to refresh the sign-in.' }
    case 'keychain-denied':
      return { title: 'Agent Studio could not read this sign-in from the Keychain.' }
    case 'unavailable':
      return { title: 'No plan limits for this sign-in. Token usage still shows below.' }
    case 'rate-limited':
    case 'error':
      return p.windows.length === 0 ? { title: 'Limits unavailable — will retry shortly.' } : null
  }
}

export interface LimitTile {
  key: string
  provider: UsageProvider
  /** `Claude · session (5h)`, `Claude alternative · weekly`, `Codex · weekly`. */
  label: string
  /** Set for a plan window. */
  timed: TimedWindow | null
  /** Set for an account that has a problem instead of (or besides) windows. */
  problem: { title: string; hint?: string } | null
  dimmed: boolean
  /** Codex values come from session logs, not from the provider. */
  estimated: boolean
  at: number | null
}

const KIND_LABELS: Record<string, string> = { session: 'session (5h)', weekly: 'weekly' }

function windowName(w: LimitWindow): string {
  const fixed = KIND_LABELS[w.kind]
  if (fixed) return fixed
  if (w.kind === 'weekly-model') return `weekly ${w.label.replace(/^Weekly\s*·\s*/i, '')}`.trim()
  return w.label
}

/**
 * Flat tiles for the Overview card: Claude accounts in order (session, weekly, ...), then
 * Codex. The account is named only when more than one Claude account is shown.
 */
export function buildLimitTiles(state: UsageState): LimitTile[] {
  const groups = buildLimitGroups(state)
  const claudeCount = groups.filter((g) => g.provider === 'claude').length
  const tiles: LimitTile[] = []
  for (const g of groups) {
    const source =
      g.provider === 'codex' ? 'Codex' : g.profile && claudeCount > 1 ? `Claude ${g.profile.account}` : 'Claude'
    const problem = g.profile ? profileProblem(g.profile) : null
    const dimmed = g.profile !== null && g.profile.status !== 'ok'
    if (problem) {
      tiles.push({
        key: `${g.key}:problem`,
        provider: g.provider,
        label: g.profile ? `Claude ${g.profile.account}` : source,
        timed: null,
        problem,
        dimmed: false,
        estimated: false,
        at: null
      })
    }
    for (const t of g.windows) {
      tiles.push({
        key: `${g.key}:${t.window.id}`,
        provider: g.provider,
        label: `${source} · ${windowName(t.window)}`,
        timed: t,
        problem: null,
        dimmed,
        estimated: g.provider === 'codex',
        at: t.at
      })
    }
  }
  return tiles
}
