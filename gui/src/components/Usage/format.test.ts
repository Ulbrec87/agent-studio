import { describe, expect, it } from 'vitest'
import { formatAge, formatCompactTokens, formatExactTokens, formatResets, formatUpdated, formatUsd } from './format'

describe('formatCompactTokens', () => {
  it('compacts by unit', () => {
    expect(formatCompactTokens(12)).toBe('12')
    expect(formatCompactTokens(950_000)).toBe('950k')
    expect(formatCompactTokens(48_200_000)).toBe('48.2M')
    expect(formatCompactTokens(3_000_000_000)).toBe('3B')
    expect(formatCompactTokens(1_500)).toBe('1.5k')
  })

  it('rolls over at the unit boundary', () => {
    expect(formatCompactTokens(999_960)).toBe('1M')
    expect(formatCompactTokens(0)).toBe('0')
  })
})

describe('formatUsd', () => {
  it('formats dollars and cents', () => {
    expect(formatUsd(31.4)).toBe('$31.40')
    expect(formatUsd(0.42)).toBe('$0.42')
    expect(formatUsd(1234.5)).toBe('$1,234.50')
    expect(formatUsd(0)).toBe('$0.00')
  })

  it('marks sub-cent amounts', () => {
    expect(formatUsd(0.001)).toBe('<$0.01')
  })
})

describe('formatExactTokens', () => {
  it('groups thousands', () => expect(formatExactTokens(48_213_456)).toBe('48,213,456'))
})

describe('age text', () => {
  it('formats spans', () => {
    expect(formatAge(10_000)).toBe('just now')
    expect(formatAge(6 * 60_000)).toBe('6 min')
    expect(formatAge(2 * 3_600_000)).toBe('2 h')
    expect(formatAge(3 * 86_400_000)).toBe('3 d')
  })

  it('adds ago except for just now', () => {
    expect(formatUpdated(0, 60_000)).toBe('1 min ago')
    expect(formatUpdated(0, 1000)).toBe('just now')
  })
})

describe('formatResets', () => {
  const now = new Date(2026, 9, 23, 12, 0).getTime()

  it('is relative under 24 hours', () => {
    expect(formatResets(now + 108 * 60_000, now)).toBe('resets in 1h 48m')
    expect(formatResets(now + 30_000, now)).toBe('resets in <1m')
  })

  it('uses weekday and time from 24 hours on', () => {
    const at = new Date(2026, 9, 26, 9, 0).getTime()
    const text = formatResets(at, now)
    expect(text.startsWith('resets ')).toBe(true)
    expect(text).toContain(at ? new Date(at).toLocaleDateString([], { weekday: 'short' }) : '')
    expect(text).not.toContain(' in ')
  })

  it('handles unknown and past times', () => {
    expect(formatResets(null, now)).toBe('')
    expect(formatResets(now - 1, now)).toBe('resetting now')
  })
})
