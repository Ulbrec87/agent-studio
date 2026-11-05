// @vitest-environment jsdom

import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { OverviewCounts } from '@shared/overview/types'
import { CountsRow } from './CountsRow'

const counts: OverviewCounts = {
  working: 3,
  idle: 2,
  stale: 1,
  needsYou: 0,
  open: 6,
  finished: 2,
  failed: 1,
  notRunning: 0,
  total: 9,
  longestWorkingMs: 724_000,
  longestIdleMs: null
}

describe('CountsRow', () => {
  afterEach(cleanup)

  it('renders the four counts with durations', () => {
    render(<CountsRow counts={counts} />)
    expect(screen.getByText('Working').nextElementSibling?.textContent).toBe('3')
    expect(screen.getByText('longest 12m')).toBeTruthy()
    expect(screen.getByText('Idle').nextElementSibling?.textContent).toBe('2')
    expect(screen.getByText('Stale').nextElementSibling?.textContent).toBe('1')
    expect(screen.getByText('Agents').nextElementSibling?.textContent).toBe('9')
    expect(screen.getByText('6 open · 2 finished · 1 failed')).toBeTruthy()
  })

  it('shows the not running count only when above zero', () => {
    const { container } = render(<CountsRow counts={{ ...counts, notRunning: 4 }} />)
    expect(container.textContent).toContain('1 failed · 4 not running')
  })

  it('shows a dash when a duration is missing', () => {
    render(<CountsRow counts={counts} />)
    expect(screen.getByText('longest —')).toBeTruthy()
  })
})
