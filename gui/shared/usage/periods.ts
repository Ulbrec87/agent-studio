import type { UsagePeriod } from './types'

export interface Range {
  start: number
  end: number
}

export interface PeriodSlot extends Range {
  label: string
}

const DAY_LABELS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']

// Everything here is calendar arithmetic in the local zone (Date local getters and
// constructors), never fixed-millisecond subtraction, so DST changes stay correct.

/** Monday 00:00 of the week containing `d`. Sunday belongs to the week that began the previous Monday. */
function weekStart(d: Date): Date {
  const sinceMonday = (d.getDay() + 6) % 7
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() - sinceMonday)
}

function daysInMonth(year: number, month: number): number {
  return new Date(year, month + 1, 0).getDate()
}

/** From the start of the period (local time) up to `now`. */
export function periodRange(period: UsagePeriod, now: Date): Range {
  const end = now.getTime()
  if (period === 'today') return { start: new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime(), end }
  if (period === 'week') return { start: weekStart(now).getTime(), end }
  return { start: new Date(now.getFullYear(), now.getMonth(), 1).getTime(), end }
}

/** The same elapsed span one period earlier. A month that is too short ends at its last day's end. */
export function previousRange(period: UsagePeriod, now: Date): Range {
  const clock = [now.getHours(), now.getMinutes(), now.getSeconds(), now.getMilliseconds()] as const
  const y = now.getFullYear()
  const m = now.getMonth()
  const d = now.getDate()
  if (period === 'today') {
    return {
      start: new Date(y, m, d - 1).getTime(),
      end: new Date(y, m, d - 1, ...clock).getTime()
    }
  }
  if (period === 'week') {
    const offset = (now.getDay() + 6) % 7
    return {
      start: new Date(y, m, d - offset - 7).getTime(),
      end: new Date(y, m, d - 7, ...clock).getTime()
    }
  }
  const prevYear = m === 0 ? y - 1 : y
  const prevMonth = (m + 11) % 12
  const length = daysInMonth(prevYear, prevMonth)
  return {
    start: new Date(prevYear, prevMonth, 1).getTime(),
    end: d > length ? new Date(y, m, 1).getTime() : new Date(prevYear, prevMonth, d, ...clock).getTime()
  }
}

/**
 * Chart slots of the whole period, including those after `now` (they stay empty).
 * On a DST day an hourly slot can be empty or doubled, which is what the wall clock does.
 */
export function periodSlots(period: UsagePeriod, now: Date): PeriodSlot[] {
  const y = now.getFullYear()
  const m = now.getMonth()
  const d = now.getDate()
  const slots: PeriodSlot[] = []
  if (period === 'today') {
    for (let h = 0; h < 24; h++) {
      slots.push({
        start: new Date(y, m, d, h).getTime(),
        end: new Date(y, m, d, h + 1).getTime(),
        label: String(h).padStart(2, '0')
      })
    }
  } else if (period === 'week') {
    const first = weekStart(now)
    for (let i = 0; i < 7; i++) {
      slots.push({
        start: new Date(first.getFullYear(), first.getMonth(), first.getDate() + i).getTime(),
        end: new Date(first.getFullYear(), first.getMonth(), first.getDate() + i + 1).getTime(),
        label: DAY_LABELS[i]
      })
    }
  } else {
    const days = daysInMonth(y, m)
    for (let day = 1; day <= days; day++) {
      slots.push({
        start: new Date(y, m, day).getTime(),
        end: new Date(y, m, day + 1).getTime(),
        label: String(day)
      })
    }
  }
  return slots
}
