import type { TokenCounts } from './types'

/** Bump when the table changes, so stored costs get recomputed. */
export const PRICING_VERSION = 1

/** USD per million tokens. */
export interface ModelPrice {
  input: number
  output: number
  cacheWrite5m: number
  cacheWrite1h: number
  cacheRead: number
  /** Prices that replace these for calls with a prompt larger than `above` tokens. */
  longPrompt?: { above: number; price: Omit<ModelPrice, 'longPrompt'> }
}

interface Spec {
  input: number
  output: number
  /** Absolute cache read price; defaults to 0.1x input. */
  cacheRead?: number
  /** OpenAI models have no cache writes. */
  noCacheWrite?: boolean
  longPrompt?: { above: number; input: number; output: number }
}

const SPECS: Record<string, Spec> = {
  'claude-fable-5-1': { input: 10, output: 50, cacheRead: 0.25 },
  'claude-mythos-5-1': { input: 10, output: 50, cacheRead: 0.25 },
  'claude-fable-5': { input: 10, output: 50, cacheRead: 1 },
  'claude-mythos-5': { input: 10, output: 50, cacheRead: 1 },
  'claude-opus-5-5': { input: 4, output: 20, cacheRead: 0.2 },
  'claude-opus-5': { input: 5, output: 25 },
  'claude-opus-4-8': { input: 5, output: 25 },
  'claude-opus-4-7': { input: 5, output: 25 },
  'claude-opus-4-6': { input: 5, output: 25 },
  'claude-opus-4-5': { input: 5, output: 25 },
  'claude-opus-4-1': { input: 15, output: 75 },
  'claude-opus-4': { input: 15, output: 75 },
  'claude-sonnet-5-5': { input: 2, output: 10, cacheRead: 0.2 },
  'claude-sonnet-5': { input: 2, output: 10, cacheRead: 0.2 },
  'claude-sonnet-4-6': { input: 3, output: 15 },
  'claude-sonnet-4-5': { input: 3, output: 15 },
  'claude-sonnet-4': { input: 3, output: 15 },
  'claude-3-7-sonnet': { input: 3, output: 15 },
  'claude-haiku-5-5': { input: 0.1, output: 0.5, longPrompt: { above: 100_000, input: 0.5, output: 2.5 } },
  'claude-haiku-4-5': { input: 1, output: 5 },
  'claude-3-5-haiku': { input: 0.8, output: 4 },
  'gpt-5': { input: 1.25, output: 10, noCacheWrite: true },
  'gpt-5-codex': { input: 1.25, output: 10, noCacheWrite: true },
  'gpt-5-mini': { input: 0.25, output: 2, noCacheWrite: true }
}

function flatPrice(spec: Spec, input: number, output: number): Omit<ModelPrice, 'longPrompt'> {
  return {
    input,
    output,
    cacheWrite5m: spec.noCacheWrite ? 0 : input * 1.25,
    cacheWrite1h: spec.noCacheWrite ? 0 : input * 2,
    cacheRead: spec.cacheRead ?? input * 0.1
  }
}

function priceOf(spec: Spec): ModelPrice {
  const tier = spec.longPrompt
  return {
    ...flatPrice(spec, spec.input, spec.output),
    longPrompt: tier ? { above: tier.above, price: flatPrice(spec, tier.input, tier.output) } : undefined
  }
}

const PRICES: Record<string, ModelPrice> = Object.fromEntries(
  Object.entries(SPECS).map(([id, spec]) => [id, priceOf(spec)])
)

// Longest keys first, so the first prefix hit is the most specific one.
const KEYS_LONGEST_FIRST = Object.keys(PRICES).sort((a, b) => b.length - a.length)

/**
 * Canonical model id: lower case, no provider prefix (`anthropic/`), no context
 * tag (`[1m]`), no date suffix (`-20250929`). Claude's dotted versions become
 * dashed; OpenAI ids such as `gpt-5.1` keep their dot.
 */
export function normalizeModel(raw: string): string {
  let id = raw.trim().toLowerCase()
  id = id.replace(/^[a-z0-9_-]+\//, '')
  id = id.replace(/\[[^\]]*\]$/, '')
  id = id.replace(/-\d{8}$/, '')
  if (id.startsWith('claude')) id = id.replace(/\./g, '-')
  return id
}

/**
 * Price of a model, `null` when unknown. Exact match first, then (Claude ids
 * only) the longest table key followed by `-`. OpenAI ids match exactly:
 * `gpt-5-codex-mini` is a cheaper model, not a `gpt-5-codex` variant.
 */
export function priceFor(model: string): ModelPrice | null {
  const id = normalizeModel(model)
  const exact = PRICES[id]
  if (exact) return exact
  if (!id.startsWith('claude')) return null
  for (const key of KEYS_LONGEST_FIRST) {
    if (id.startsWith(`${key}-`)) return PRICES[key]
  }
  return null
}

/** `<synthetic>` marks records Claude Code writes itself, not model calls. */
export function isSyntheticModel(model: string): boolean {
  return model.trim().toLowerCase() === '<synthetic>'
}

/**
 * Estimated API-equivalent cost of a single call, `null` when the model has no
 * price. Per call because some tiers depend on that call's prompt size.
 */
export function estimateCost(model: string, tokens: TokenCounts): number | null {
  try {
    const base = priceFor(model)
    if (!base) return null
    const prompt = tokens.input + tokens.cacheWrite5m + tokens.cacheWrite1h + tokens.cacheRead
    const price = base.longPrompt && prompt > base.longPrompt.above ? base.longPrompt.price : base
    const usd =
      (tokens.input * price.input +
        tokens.output * price.output +
        tokens.cacheWrite5m * price.cacheWrite5m +
        tokens.cacheWrite1h * price.cacheWrite1h +
        tokens.cacheRead * price.cacheRead) /
      1_000_000
    return Number.isFinite(usd) ? usd : null
  } catch {
    return null
  }
}
