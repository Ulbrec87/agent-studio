import { describe, expect, it } from 'vitest'
import { formatAgo, formatCount, formatDuration } from './format'

describe('formatDuration', () => {
  it('formats seconds, minutes and hours', () => {
    expect(formatDuration(0)).toBe('0s')
    expect(formatDuration(45_000)).toBe('45s')
    expect(formatDuration(252_000)).toBe('4m 12s')
    expect(formatDuration(65_000)).toBe('1m 05s')
    expect(formatDuration(3_780_000)).toBe('1h 03m')
    expect(formatDuration(36_000_000)).toBe('10h 00m')
  })

  it('clamps negative values to zero', () => {
    expect(formatDuration(-5000)).toBe('0s')
  })

  it('drops seconds in compact mode', () => {
    expect(formatDuration(45_000, { compact: true })).toBe('45s')
    expect(formatDuration(724_000, { compact: true })).toBe('12m')
    expect(formatDuration(3_780_000, { compact: true })).toBe('1h 03m')
  })
})

describe('formatCount', () => {
  it('groups thousands', () => {
    expect(formatCount(182431)).toBe('182,431')
  })
})

describe('formatAgo', () => {
  const now = new Date(2026, 9, 23, 15, 0).getTime()

  it('says just now under a minute and for future times', () => {
    expect(formatAgo(now - 30_000, now)).toBe('just now')
    expect(formatAgo(now + 5_000, now)).toBe('just now')
  })

  it('uses minutes then hours under a day', () => {
    expect(formatAgo(now - 12 * 60_000, now)).toBe('12m ago')
    expect(formatAgo(now - 3.5 * 3_600_000, now)).toBe('3h ago')
  })

  it('says yesterday with the time', () => {
    const at = new Date(2026, 9, 22, 14, 2).getTime()
    expect(formatAgo(at, now)).toBe(`yesterday ${new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`)
  })

  it('falls back to the date, with the year when it differs', () => {
    const same = new Date(2026, 9, 21, 14, 2).getTime()
    expect(formatAgo(same, now)).toBe(new Date(same).toLocaleDateString([], { day: 'numeric', month: 'short' }))
    const other = new Date(2025, 9, 21, 14, 2).getTime()
    expect(formatAgo(other, now)).toContain('2025')
  })
})
