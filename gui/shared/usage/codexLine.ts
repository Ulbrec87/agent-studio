// Adapted from Tokdash (MIT, © JingbiaoMei): src/tokdash/sources/coding_tools.py
// (codex_token_event_key, CodexCompactionTracker, codex_replay_key_session_id,
// CodexParser._parse_all), src/tokdash/sources/quota/codex.py and
// src/tokdash/codex_quota_windows.py.
import { normalizeModel } from './pricing'
import { emptyTokens, type LimitWindow, type TokenCounts, type UsageRecord } from './types'

/** Tokdash `CODEX_DEFAULT_MODEL`: used until the log names a model. */
export const CODEX_DEFAULT_MODEL = 'gpt-5.3-codex'

const MAX_TURN_MODELS = 256

/** A remote compaction no `token_count` has carried (yet). Plain JSON. */
interface PendingCompaction {
  /** Counters of the compaction request, in `COUNTER_FIELDS` order. */
  usage: number[]
  threadUsage: number[] | null
  model: string | null
  ts: number
  projectKey: string
  /** Session id the dedupe key is scoped to; null when unknown. */
  keyScope: string | null
  replayFallback: boolean
}

/** The parts of a `token_usage_record` a compaction needs. */
interface CompactionRecord {
  responseId: string | null
  turnId: string | null
  usage: number[] | null
  threadUsage: number[] | null
}

/** Parser state for one rollout file. Plain JSON, safe to persist and resume. */
export interface CodexFileState {
  /** The file's own session id (first `session_meta`). */
  sessionId: string | null
  model: string | null
  turnModels: Record<string, string>
  /** Latest cwd from `session_meta` / `turn_context`. */
  cwd: string
  /** Session id of the latest `session_meta` (a fork replays its parent's). */
  currentSessionId: string | null
  subagentParentId: string | null
  isForkFile: boolean
  sawTurnContext: boolean
  /** Latest `token_usage_record`. */
  lastRecord: CompactionRecord | null
  lastRecordCarried: boolean
  pendingCompactions: PendingCompaction[]
}

export function initialCodexFileState(): CodexFileState {
  return {
    sessionId: null,
    model: null,
    turnModels: {},
    cwd: '',
    currentSessionId: null,
    subagentParentId: null,
    isForkFile: false,
    sawTurnContext: false,
    lastRecord: null,
    lastRecordCarried: false,
    pendingCompactions: []
  }
}

export interface CodexParsedUsage {
  record: UsageRecord
  /** Stable identity across resumed/forked files, `null` when unknown (caller falls back to file:position). */
  dedupeKey: string | null
}

export interface CodexParsedLimits {
  /** Epoch ms of the line. */
  at: number
  account: string
  plan: string | null
  windows: LimitWindow[]
}

export interface CodexLineResult {
  usage?: CodexParsedUsage[]
  limits?: CodexParsedLimits
}

type Obj = Record<string, unknown>

function isObj(v: unknown): v is Obj {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

function str(v: unknown): string {
  return typeof v === 'string' ? v.trim() : typeof v === 'number' ? String(v) : ''
}

const COUNTER_FIELDS = [
  'input_tokens',
  'cached_input_tokens',
  'cache_write_input_tokens',
  'output_tokens',
  'reasoning_output_tokens'
] as const
const IN = 0
const CACHED = 1
const OUT = 3
const REASONING = 4

/** Counters in `COUNTER_FIELDS` order; missing = 0; `null` when a field is not a number. */
function counters(snapshot: Obj): number[] | null {
  const out: number[] = []
  for (const f of COUNTER_FIELDS) {
    const v = snapshot[f]
    if (v === undefined || v === null) out.push(0)
    else if (typeof v === 'number' && Number.isFinite(v)) out.push(Math.trunc(v))
    else if (typeof v === 'string' && /^-?\d+$/.test(v.trim())) out.push(parseInt(v, 10))
    else return null
  }
  return out
}

function nonEmptyObj(v: unknown): Obj | null {
  return isObj(v) && Object.keys(v).length > 0 ? v : null
}

/** Port of `codex_token_event_key`. Ignores timestamps so replays collapse. */
function tokenEventKey(sessionId: string | null, info: Obj): string | null {
  if (!sessionId) return null
  const total = nonEmptyObj(info.total_token_usage)
  const last = nonEmptyObj(info.last_token_usage)
  if (!total || !last) return null
  const t = counters(total)
  const l = counters(last)
  if (!t || !l) return null
  return `codex-token-v1:${sessionId}|${t.join(',')}|${l.join(',')}`
}

/** Port of `codex_compaction_event_key`. */
function compactionEventKey(sessionId: string | null, usage: number[], thread: number[] | null): string | null {
  if (!sessionId || !thread) return null
  return `codex-compaction-v1:${sessionId}|${thread.join(',')}|${usage.join(',')}`
}

/** `null` for a missing or all-zero snapshot (Tokdash `_signature`). */
function signature(usage: unknown): number[] | null {
  if (!isObj(usage)) return null
  const c = counters(usage)
  return c && c.some((n) => n !== 0) ? c : null
}

const sameCounters = (a: number[], b: number[]): boolean => a.length === b.length && a.every((n, i) => n === b[i])

/** Port of `codex_fork_ancestry` (only the declared parent id is needed here). */
function forkAncestry(p: Obj): { isFork: boolean; parentId: string | null } {
  const ownId = str(p.id)
  const source = isObj(p.source) ? p.source : {}
  const subagent = isObj(source.subagent) ? source.subagent : null
  const spawn = subagent && isObj(subagent.thread_spawn) ? subagent.thread_spawn : null
  let pid = spawn ? str(spawn.parent_thread_id) : ''
  if (!pid) {
    for (const f of ['forked_from_id', 'parent_thread_id']) {
      const c = str(p[f])
      if (c && c !== ownId) {
        pid = c
        break
      }
    }
  }
  return { isFork: spawn !== null || pid !== '', parentId: pid || null }
}

/**
 * Port of `codex_replay_key_session_id`: the session a replayed event is keyed
 * to, so a fork's copied parent history collapses against the parent file.
 */
function replayKeyScope(state: CodexFileState): { scope: string | null; replayFallback: boolean } {
  const own = state.sessionId
  const current = state.currentSessionId
  if (!state.isForkFile || own === null || current === null) return { scope: current, replayFallback: false }
  const parent = state.subagentParentId
  if (parent !== null) {
    if (current === parent) return { scope: current, replayFallback: true }
    if (current === own && !state.sawTurnContext) return { scope: parent, replayFallback: false }
  } else if (current !== own) {
    return { scope: current, replayFallback: true }
  }
  return { scope: current, replayFallback: false }
}

/** ISO string or epoch (seconds, or ms when > 1e10) to epoch ms. */
function parseTime(v: unknown): number | null {
  if (v === null || v === undefined) return null
  if (typeof v === 'number') {
    if (!Number.isFinite(v)) return null
    return Math.round(v > 1e10 ? v : v * 1000)
  }
  if (typeof v !== 'string') return null
  const t = v.trim()
  if (!t) return null
  if (/^\d+$/.test(t)) return parseTime(Number(t))
  const ms = Date.parse(t)
  return Number.isNaN(ms) ? null : ms
}

function buildUsage(
  usage: number[],
  model: string | null,
  ts: number,
  projectKey: string,
  dedupeKey: string | null
): CodexParsedUsage | null {
  // Codex input_tokens INCLUDES cached tokens; reasoning is already part of
  // output_tokens and is not billed separately (as Tokdash).
  const cacheRead = Math.max(0, usage[CACHED])
  const input = Math.max(0, usage[IN] - cacheRead)
  const output = Math.max(0, usage[OUT])
  if (input === 0 && output === 0 && cacheRead === 0) return null
  const tokens: TokenCounts = { ...emptyTokens(), input, output, cacheRead }
  return {
    record: { ts, provider: 'codex', model: normalizeModel(model || CODEX_DEFAULT_MODEL), projectKey, tokens },
    dedupeKey
  }
}

const FIVE_HOURS = 300
const SEVEN_DAYS = 10080

function windowMinutes(w: Obj): number | null {
  const num = (v: unknown): number | null => {
    const n = typeof v === 'string' && v.trim() ? Number(v) : v
    return typeof n === 'number' && Number.isFinite(n) ? n : null
  }
  const secs = num(w.limit_window_seconds) ?? num(w.window_seconds)
  if (secs !== null) return secs / 60
  return num(w.window_minutes)
}

function otherLabel(minutes: number): string {
  if (minutes % 1440 === 0) return `${minutes / 1440}-day window`
  if (minutes % 60 === 0) return `${minutes / 60}-hour window`
  return `${Math.round(minutes)}-minute window`
}

type Slot = 'session' | 'weekly'

/** Port of `classify_codex_api_windows` for windows with no unrecognised explicit duration. */
function classify(primary: Obj | null, secondary: Obj | null): Array<[Slot, Obj]> {
  const bucket = (w: Obj): Slot | null => {
    const m = windowMinutes(w)
    return m === FIVE_HOURS ? 'session' : m === SEVEN_DAYS ? 'weekly' : null
  }
  const other = (s: Slot): Slot => (s === 'session' ? 'weekly' : 'session')
  if (primary && !secondary) return [[bucket(primary) ?? 'weekly', primary]]
  if (!primary && secondary) return [[bucket(secondary) ?? 'weekly', secondary]]
  if (!primary || !secondary) return []
  const pb = bucket(primary)
  const sb = bucket(secondary)
  if (pb && sb && pb !== sb) return [[pb, primary], [sb, secondary]]
  if (pb && !sb) return [[pb, primary], [other(pb), secondary]]
  if (sb && !pb) return [[other(sb), primary], [sb, secondary]]
  return [['session', primary], ['weekly', secondary]]
}

function parseLimits(p: Obj, info: Obj, at: number): CodexParsedLimits | null {
  const rl = nonEmptyObj(p.rate_limits) ?? nonEmptyObj(info.rate_limits)
  if (!rl) return null
  const account = str(rl.account_id) || 'default'
  const plan = str(rl.plan_type) || null

  const usable = (v: unknown): Obj | null => (isObj(v) ? v : null)
  const others: Array<{ w: Obj; id: string; label: string }> = []
  const pending: { primary: Obj | null; secondary: Obj | null } = {
    primary: usable(rl.primary),
    secondary: usable(rl.secondary)
  }
  // A window with an explicit unrecognised duration is its own `other` window.
  for (const pos of ['primary', 'secondary'] as const) {
    const w = pending[pos]
    if (!w) continue
    const m = windowMinutes(w)
    if (m !== null && m > 0 && m !== FIVE_HOURS && m !== SEVEN_DAYS) {
      others.push({ w, id: `window:${m}`, label: otherLabel(m) })
      pending[pos] = null
    }
  }
  const classified = classify(pending.primary, pending.secondary)

  const windows: LimitWindow[] = []
  const push = (w: Obj, id: string, kind: LimitWindow['kind'], label: string): void => {
    const raw = w.used_percent ?? w.usage_percent ?? w.usedPercent
    const n = typeof raw === 'string' && raw.trim() ? Number(raw) : raw
    // Codex reports 0-100 already; unlike other sources a value <= 1 is NOT a fraction.
    if (typeof n !== 'number' || !Number.isFinite(n) || n < 0) return
    const usedPercent = Math.min(100, Math.round(n * 10000) / 10000)
    let resetsAt = parseTime(w.resets_at ?? w.reset_at ?? w.resetAt)
    if (resetsAt === null) {
      const rel = typeof w.resets_in_seconds === 'number' ? w.resets_in_seconds : Number(w.resets_in_seconds)
      if (w.resets_in_seconds !== undefined && w.resets_in_seconds !== null && Number.isFinite(rel) && rel >= 0) {
        resetsAt = at + Math.round(rel * 1000)
      }
    }
    // An idle window reports a phantom reset (now + window length); show none (Tokdash).
    if (usedPercent === 0) resetsAt = null
    windows.push({ provider: 'codex', account, id, kind, label, usedPercent, resetsAt })
  }
  for (const [slot, w] of classified) {
    if (slot === 'session') push(w, 'session', 'session', '5-hour window')
    else push(w, 'weekly', 'weekly', 'Weekly')
  }
  for (const e of others) push(e.w, e.id, 'other', e.label)
  if (windows.length === 0) return null
  return { at, account, plan, windows }
}

function toRecord(v: unknown): CompactionRecord | null {
  if (!isObj(v)) return null
  return {
    responseId: str(v.response_id) || null,
    turnId: str(v.turn_id) || null,
    usage: signature(v.usage),
    threadUsage: nonEmptyObj(v.thread_token_usage) ? counters(v.thread_token_usage as Obj) : null
  }
}

/** Port of `CodexCompactionTracker.observe`; uncarried compactions queue on `state`. */
function observeCompaction(msg: Obj, kind: string, p: Obj, state: CodexFileState): void {
  if (kind === 'token_usage_record') {
    state.lastRecord = toRecord(p)
    state.lastRecordCarried = false
  } else if (kind === 'compacted') {
    const rid = str(p.compaction_response_id)
    let record: CompactionRecord | null = null
    for (const c of [toRecord(p.latest_token_usage_record), state.lastRecord]) {
      if (rid && c && c.responseId === rid) {
        record = c
        break
      }
    }
    if (!record || !record.usage) return
    if (typeof msg.timestamp !== 'string') return
    const ts = parseTime(msg.timestamp)
    if (ts === null) return
    // A custom provider's token_count can land between the record and its `compacted` event.
    if (state.lastRecordCarried && state.lastRecord?.responseId === rid) return
    const { scope, replayFallback } = replayKeyScope(state)
    state.pendingCompactions.push({
      usage: record.usage,
      threadUsage: record.threadUsage,
      model: (record.turnId && state.turnModels[record.turnId]) || state.model,
      ts,
      projectKey: state.cwd,
      keyScope: scope,
      replayFallback
    })
  } else if (kind === 'event_msg' && p.type === 'token_count') {
    const info = isObj(p.info) ? p.info : {}
    const sig = signature(info.last_token_usage)
    if (!sig) return
    if (state.lastRecord?.usage && sameCounters(state.lastRecord.usage, sig)) state.lastRecordCarried = true
    const i = state.pendingCompactions.findIndex((c) => sameCounters(c.usage, sig))
    if (i >= 0) state.pendingCompactions.splice(i, 1) // carried: the token_count bills it
  }
}

function rememberTurnModel(state: CodexFileState, turnId: string, model: string): void {
  delete state.turnModels[turnId]
  state.turnModels[turnId] = model
  const keys = Object.keys(state.turnModels)
  for (let i = 0; i < keys.length - MAX_TURN_MODELS; i++) delete state.turnModels[keys[i]]
}

/** Feed one rollout line; mutates `state`. Never throws. */
export function parseCodexLine(line: string, state: CodexFileState): CodexLineResult | null {
  try {
    return parseLine(line, state)
  } catch {
    return null
  }
}

function parseLine(line: string, state: CodexFileState): CodexLineResult | null {
  let msg: unknown
  try {
    msg = JSON.parse(line)
  } catch {
    return null
  }
  if (!isObj(msg)) return null
  const kind = typeof msg.type === 'string' ? msg.type : ''
  const p = isObj(msg.payload) ? msg.payload : {}

  if (kind === 'turn_context') {
    state.sawTurnContext = true
    const model = str(p.model)
    if (model) {
      state.model = model
      const turnId = str(p.turn_id)
      if (turnId) rememberTurnModel(state, turnId, model)
    }
    if (typeof p.cwd === 'string' && p.cwd) state.cwd = p.cwd
  } else if (kind === 'event_msg' && p.type === 'thread_settings_applied' && !state.sawTurnContext) {
    // Newer Codex applies thread settings before the first turn_context.
    const settings = isObj(p.thread_settings) ? p.thread_settings : {}
    const model = str(settings.model)
    if (model) state.model = model
  } else if (kind === 'session_meta') {
    const sid = str(p.id)
    if (sid) {
      if (state.sessionId === null) {
        state.sessionId = sid
        const { isFork, parentId } = forkAncestry(p)
        state.isForkFile = isFork
        state.subagentParentId = parentId
      }
      state.currentSessionId = sid
    }
    if (typeof p.cwd === 'string' && p.cwd) state.cwd = p.cwd
    if (!state.sawTurnContext) {
      const base = isObj(p.base_instructions) ? p.base_instructions : {}
      const prov = isObj(base.provenance) ? base.provenance : {}
      const model = str(prov.model)
      if (model) state.model = model
    }
  }

  observeCompaction(msg, kind, p, state)

  if (kind !== 'event_msg' || p.type !== 'token_count') return null
  const ts = typeof msg.timestamp === 'string' ? parseTime(msg.timestamp) : null
  if (ts === null) return null

  const result: CodexLineResult = {}
  const info = isObj(p.info) ? p.info : {}

  const limits = parseLimits(p, info, ts)
  if (limits) result.limits = limits

  const last = nonEmptyObj(info.last_token_usage)
  const lastCounters = last ? counters(last) : null
  if (lastCounters) {
    const { scope, replayFallback } = replayKeyScope(state)
    const key = tokenEventKey(scope, info)
    // A fork's replayed history without a stable key is dropped, not double counted.
    if (!(replayFallback && key === null)) {
      const parsed = buildUsage(lastCounters, state.model, ts, state.cwd, key)
      if (parsed) result.usage = [parsed]
    }
  }
  return result.usage || result.limits ? result : null
}

/**
 * Call at the end of the available bytes: compaction requests that no
 * `token_count` carried (Tokdash issue #222). Each is returned once; it is
 * removed from `state`, so calling again after more lines never repeats it.
 * `dedupeKey` is null when the log lacks the thread total the key needs.
 */
export function flushCodexCompactions(state: CodexFileState): CodexParsedUsage[] {
  const out: CodexParsedUsage[] = []
  try {
    const pending = state.pendingCompactions
    state.pendingCompactions = []
    for (const c of pending) {
      const key = compactionEventKey(c.keyScope, c.usage, c.threadUsage)
      if (c.replayFallback && key === null) continue
      const parsed = buildUsage(c.usage, c.model, c.ts, c.projectKey, key)
      if (parsed) out.push(parsed)
    }
  } catch {
    /* never throws */
  }
  return out
}
