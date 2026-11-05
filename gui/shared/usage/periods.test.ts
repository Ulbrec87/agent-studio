import { describe, expect, it } from 'vitest'
import { periodRange, periodSlots, previousRange } from './periods'

// Local time throughout: dates are built with the local constructor, whatever the zone.
const d = (y: number, m: number, day: number, h = 0, min = 0) => new Date(y, m - 1, day, h, min).getTime()

describe('periodRange', () => {
  const now = new Date(2026, 9, 22, 15, 30) // Thursday
  it('today starts at local midnight', () => {
    expect(periodRange('today', now)).toEqual({ start: d(2026, 10, 22), end: now.getTime() })
  })
  it('week starts on Monday', () => {
    expect(periodRange('week', now)).toEqual({ start: d(2026, 10, 19), end: now.getTime() })
  })
  it('a Sunday belongs to the week that began the previous Monday', () => {
    const sunday = new Date(2026, 9, 25, 10)
    expect(periodRange('week', sunday).start).toBe(d(2026, 10, 19))
  })
  it('a Monday starts its own week', () => {
    const monday = new Date(2026, 9, 19, 0, 0)
    expect(periodRange('week', monday).start).toBe(d(2026, 10, 19))
  })
  it('week can start in the previous month', () => {
    expect(periodRange('week', new Date(2026, 10, 1, 9)).start).toBe(d(2026, 10, 26))
  })
  it('month starts on the 1st', () => {
    expect(periodRange('month', now)).toEqual({ start: d(2026, 10, 1), end: now.getTime() })
  })
})

describe('previousRange', () => {
  it('today maps to yesterday up to the same clock time', () => {
    const now = new Date(2026, 9, 22, 15, 30)
    expect(previousRange('today', now)).toEqual({ start: d(2026, 10, 21), end: d(2026, 10, 21, 15, 30) })
  })
  it('today crosses a month boundary', () => {
    expect(previousRange('today', new Date(2026, 10, 1, 8))).toEqual({ start: d(2026, 10, 31), end: d(2026, 10, 31, 8) })
  })
  it('week maps to the previous Monday with the same offset', () => {
    const now = new Date(2026, 9, 22, 15, 30) // Thursday
    expect(previousRange('week', now)).toEqual({ start: d(2026, 10, 12), end: d(2026, 10, 15, 15, 30) })
  })
  it('week works on a Sunday', () => {
    expect(previousRange('week', new Date(2026, 9, 25, 10))).toEqual({ start: d(2026, 10, 12), end: d(2026, 10, 18, 10) })
  })
  it('month maps to the previous month, same day and time', () => {
    expect(previousRange('month', new Date(2026, 9, 22, 15, 30))).toEqual({
      start: d(2026, 9, 1),
      end: d(2026, 9, 22, 15, 30)
    })
  })
  it('month wraps over the year', () => {
    expect(previousRange('month', new Date(2026, 0, 15, 12))).toEqual({ start: d(2025, 12, 1), end: d(2025, 12, 15, 12) })
  })
  it('month clamps to the end of a shorter previous month', () => {
    expect(previousRange('month', new Date(2026, 2, 31, 10))).toEqual({ start: d(2026, 2, 1), end: d(2026, 3, 1) })
    expect(previousRange('month', new Date(2024, 2, 31, 10))).toEqual({ start: d(2024, 2, 1), end: d(2024, 3, 1) })
    expect(previousRange('month', new Date(2026, 2, 28, 10)).end).toBe(d(2026, 2, 28, 10))
  })
  it('month clamps 31 to 30', () => {
    expect(previousRange('month', new Date(2026, 9, 31, 10)).end).toBe(d(2026, 10, 1))
  })
})

describe('periodSlots', () => {
  const now = new Date(2026, 9, 22, 15, 30)
  it('today has 24 hourly slots', () => {
    const slots = periodSlots('today', now)
    expect(slots).toHaveLength(24)
    expect(slots[0]).toMatchObject({ label: '00', start: d(2026, 10, 22) })
    expect(slots[23]).toMatchObject({ label: '23', end: d(2026, 10, 23) })
  })
  it('week has 7 daily slots Mon..Sun', () => {
    const slots = periodSlots('week', now)
    expect(slots.map((s) => s.label)).toEqual(['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'])
    expect(slots[0].start).toBe(d(2026, 10, 19))
    expect(slots[6].end).toBe(d(2026, 10, 26))
  })
  it('month has one slot per day', () => {
    expect(periodSlots('month', now)).toHaveLength(31)
    expect(periodSlots('month', new Date(2026, 1, 10))).toHaveLength(28)
    expect(periodSlots('month', new Date(2024, 1, 10))).toHaveLength(29)
    const slots = periodSlots('month', now)
    expect(slots[0].label).toBe('1')
    expect(slots[30].label).toBe('31')
  })
  it('slots are contiguous and include the future', () => {
    const slots = periodSlots('week', now)
    for (let i = 1; i < slots.length; i++) expect(slots[i].start).toBe(slots[i - 1].end)
    expect(slots[6].start).toBeGreaterThan(now.getTime())
  })
})
