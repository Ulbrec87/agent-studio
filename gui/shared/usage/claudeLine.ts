import { isSyntheticModel, normalizeModel } from './pricing'
import type { TokenCounts, UsageRecord } from './types'

/** Lines longer than this are skipped without parsing. */
export const MAX_LINE_BYTES = 4 * 1024 * 1024

export interface ParsedClaudeLine {
  record: UsageRecord
  /**
   * `message.id:requestId` (or `message.id`). Claude Code writes one line per content block of the
   * same API response, and resumed/forked sessions copy earlier lines, so count a key once.
   */
  dedupeKey: string | null
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

/** Non-finite or negative → 0; otherwise floored. */
function count(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) && v > 0 ? Math.floor(v) : 0
}

function isNum(v: unknown): boolean {
  return typeof v === 'number' && Number.isFinite(v)
}

function nonEmpty(v: unknown): v is string {
  return typeof v === 'string' && v.length > 0
}

/** Parses one transcript line into a usage record; `null` for anything that is not a billable assistant call. */
export function parseClaudeUsageLine(line: string): ParsedClaudeLine | null {
  try {
    if (line.length > MAX_LINE_BYTES) return null
    if (!line.includes('"assistant"') || !line.includes('"usage"')) return null
    const obj: unknown = JSON.parse(line)
    if (!isObject(obj) || obj.type !== 'assistant') return null
    const message = obj.message
    if (!isObject(message) || !isObject(message.usage)) return null
    const usage = message.usage

    const rawModel = typeof message.model === 'string' ? message.model : ''
    if (isSyntheticModel(rawModel)) return null

    if (typeof obj.timestamp !== 'string') return null
    const ts = Date.parse(obj.timestamp)
    if (!Number.isFinite(ts)) return null

    let cacheWrite5m: number
    let cacheWrite1h = 0
    const split = usage.cache_creation
    if (isObject(split) && (isNum(split.ephemeral_5m_input_tokens) || isNum(split.ephemeral_1h_input_tokens))) {
      cacheWrite5m = count(split.ephemeral_5m_input_tokens)
      cacheWrite1h = count(split.ephemeral_1h_input_tokens)
    } else {
      cacheWrite5m = count(usage.cache_creation_input_tokens)
    }

    const tokens: TokenCounts = {
      input: count(usage.input_tokens),
      output: count(usage.output_tokens),
      cacheWrite5m,
      cacheWrite1h,
      cacheRead: count(usage.cache_read_input_tokens)
    }
    if (tokens.input + tokens.output + tokens.cacheWrite5m + tokens.cacheWrite1h + tokens.cacheRead === 0) return null

    const id = nonEmpty(message.id) ? message.id : null
    const dedupeKey = id ? (nonEmpty(obj.requestId) ? `${id}:${obj.requestId}` : id) : null

    return {
      record: {
        ts,
        provider: 'claude',
        model: rawModel.trim() ? normalizeModel(rawModel) : 'unknown',
        projectKey: typeof obj.cwd === 'string' ? obj.cwd : '',
        tokens
      },
      dedupeKey
    }
  } catch {
    return null
  }
}

/** Display name of a project: the last path segment of the cwd. */
export function claudeProjectKeyLabel(projectKey: string): string {
  const parts = projectKey.split(/[\\/]+/).filter(Boolean)
  return parts.length ? parts[parts.length - 1] : 'Unknown'
}
