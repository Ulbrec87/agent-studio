import { describe, expect, it } from 'vitest'
import { PRICING_VERSION, estimateCost, isSyntheticModel, normalizeModel, priceFor } from './pricing'
import { emptyTokens, type TokenCounts } from './types'

const tok = (p: Partial<TokenCounts>): TokenCounts => ({ ...emptyTokens(), ...p })
const M = 1_000_000

describe('normalizeModel', () => {
  it('lower-cases and trims', () => {
    expect(normalizeModel('  Claude-Opus-4-5 ')).toBe('claude-opus-4-5')
  })
  it('strips provider prefixes', () => {
    expect(normalizeModel('anthropic/claude-opus-4-5')).toBe('claude-opus-4-5')
    expect(normalizeModel('openai/gpt-5')).toBe('gpt-5')
  })
  it('strips context tags', () => {
    expect(normalizeModel('claude-sonnet-4-5[1m]')).toBe('claude-sonnet-4-5')
  })
  it('strips date suffixes', () => {
    expect(normalizeModel('claude-sonnet-4-5-20250929')).toBe('claude-sonnet-4-5')
    expect(normalizeModel('claude-sonnet-4-5-20250929[1m]')).toBe('claude-sonnet-4-5')
  })
  it('dashes Claude versions but leaves OpenAI ids alone', () => {
    expect(normalizeModel('claude-opus-4.5')).toBe('claude-opus-4-5')
    expect(normalizeModel('gpt-5.1')).toBe('gpt-5.1')
  })
})

describe('priceFor', () => {
  it('matches exactly after normalising', () => {
    expect(priceFor('claude-sonnet-4-5-20250929')).toMatchObject({ input: 3, output: 15 })
  })
  it('prefers the longest prefix', () => {
    expect(priceFor('claude-opus-4-1-foo')).toMatchObject({ input: 15 })
    expect(priceFor('claude-opus-4-5-foo')).toMatchObject({ input: 5 })
    expect(priceFor('claude-opus-4-9')).toMatchObject({ input: 15 }) // falls back to claude-opus-4
  })
  it('requires a dash after the prefix', () => {
    expect(priceFor('gpt-50')).toBeNull()
    expect(priceFor('gpt-5.1')).toBeNull()
    expect(priceFor('claude-opus-45')).toBeNull()
  })
  it('keeps gpt-5-mini apart from gpt-5', () => {
    expect(priceFor('gpt-5-mini')).toMatchObject({ input: 0.25 })
    expect(priceFor('gpt-5-codex-mini')).toBeNull() // OpenAI ids match exactly
  })
  it('returns null for unknown models', () => {
    expect(priceFor('mystery-1')).toBeNull()
    expect(priceFor('')).toBeNull()
  })
  it('has a version', () => {
    expect(PRICING_VERSION).toBe(1)
  })
})

describe('estimateCost', () => {
  it('prices input and output', () => {
    expect(estimateCost('claude-sonnet-4-5', tok({ input: M, output: M }))).toBeCloseTo(18)
  })
  it('prices 5m writes at 1.25x and 1h writes at 2x', () => {
    expect(estimateCost('claude-sonnet-4-5', tok({ cacheWrite5m: M }))).toBeCloseTo(3.75)
    expect(estimateCost('claude-sonnet-4-5', tok({ cacheWrite1h: M }))).toBeCloseTo(6)
  })
  it('prices cache reads at 0.1x by default', () => {
    expect(estimateCost('claude-opus-4-5', tok({ cacheRead: M }))).toBeCloseTo(0.5)
  })
  it('uses the cache read override of Opus 5.5', () => {
    expect(estimateCost('claude-opus-5-5', tok({ cacheRead: M }))).toBeCloseTo(0.2)
    expect(estimateCost('claude-fable-5', tok({ cacheRead: M }))).toBeCloseTo(1)
    expect(estimateCost('claude-fable-5-1', tok({ cacheRead: M }))).toBeCloseTo(0.25)
  })
  it('switches Haiku 5.5 tier above 100,000 prompt tokens', () => {
    expect(estimateCost('claude-haiku-5-5', tok({ input: 100_000 }))).toBeCloseTo(0.01)
    expect(estimateCost('claude-haiku-5-5', tok({ input: 100_001 }))).toBeCloseTo(0.0500005)
  })
  it('counts cache tokens toward the Haiku 5.5 prompt and scales them with the tier', () => {
    expect(estimateCost('claude-haiku-5-5', tok({ input: 1, cacheRead: 100_000 }))).toBeCloseTo(100_000 * 0.5 * 0.1 / M + 0.5 / M)
    expect(estimateCost('claude-haiku-5-5', tok({ cacheRead: 100_000 }))).toBeCloseTo(0.001)
    expect(estimateCost('claude-haiku-5-5', tok({ output: 200_000 }))).toBeCloseTo(0.1)
  })
  it('has no cache writes for OpenAI models', () => {
    expect(estimateCost('gpt-5', tok({ input: M, output: M, cacheRead: M }))).toBeCloseTo(1.25 + 10 + 0.125)
    expect(estimateCost('gpt-5', tok({ cacheWrite5m: M }))).toBe(0)
  })
  it('returns null when unpriced and never throws', () => {
    expect(estimateCost('mystery', tok({ input: 5 }))).toBeNull()
    expect(estimateCost('claude-opus-5', tok({ input: NaN }))).toBeNull()
  })
})

describe('isSyntheticModel', () => {
  it('flags <synthetic> only', () => {
    expect(isSyntheticModel('<synthetic>')).toBe(true)
    expect(isSyntheticModel('claude-opus-5')).toBe(false)
  })
})
