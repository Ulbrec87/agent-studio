// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { WaitLedgerData, WaitTransition } from '@shared/overview/types'
import { WaitsCard } from './WaitsCard'

// Local noon: the card works in the local time zone.
const NOW = new Date(2026, 9, 22, 12, 0, 0).getTime()
const local = (day: number, h: number, m = 0): number => new Date(2026, 9, day, h, m).getTime()

function tr(agentId: string, direction: 'in' | 'out', at: number): WaitTransition {
  return { agentId, name: `agent-${agentId}`, projectDir: '/w', at, direction }
}

function ledgerOf(extra: Partial<WaitLedgerData> = {}): WaitLedgerData {
  return {
    // Yesterday and today measured; the five days before are not.
    spans: [{ start: local(21, 0), end: NOW }],
    starts: [],
    transitions: [
      tr('a', 'in', local(21, 10)),
      tr('a', 'out', local(21, 10, 20)),
      tr('a', 'in', local(22, 9)),
      tr('a', 'out', local(22, 9, 5)),
      tr('b', 'in', local(22, 10)),
      tr('b', 'out', local(22, 10, 2))
    ],
    ...extra
  }
}

describe('WaitsCard', () => {
  afterEach(cleanup)

  it('says so when nothing is known yet', () => {
    render(<WaitsCard ledger={null} waitingNow={[]} now={NOW} />)
    expect(screen.getByText('No waits recorded yet')).toBeTruthy()
  })

  it('shows today and the 7 days with their counts, and what was measured', () => {
    render(<WaitsCard ledger={ledgerOf()} waitingNow={[]} now={NOW} />)
    const figures = screen.getAllByText(/^\d+ waits?$/)
    // Today: a 5 min and a 2 min wait; the 7 days add the 20 min one. The first two figures come first.
    expect(figures[0].textContent).toBe('2 waits')
    expect(figures[1].textContent).toBe('3 waits')
    expect(screen.getByText('7m 00s')).toBeTruthy()
    expect(screen.getByText('27m 00s')).toBeTruthy()
    expect(screen.getByText('Measured 12h 00m of today, 36h 00m of the 7 days')).toBeTruthy()
  })

  it('hatches the days that were not measured and gives them a dash', () => {
    const { container } = render(<WaitsCard ledger={ledgerOf()} waitingNow={[]} now={NOW} />)
    const days = container.querySelectorAll('[data-day]')
    expect(days).toHaveLength(7)
    const hatched = Array.from(days).filter((g) => g.querySelector('[data-unmeasured]'))
    expect(hatched).toHaveLength(5)
    expect(within(screen.getByLabelText('By day')).getAllByText('—')).toHaveLength(5)
  })

  it('shows an open wait in amber at the end of today', () => {
    const ledger = ledgerOf({ transitions: [tr('a', 'in', local(22, 11, 30))] })
    const { container } = render(
      <WaitsCard
        ledger={ledger}
        waitingNow={[{ agentId: 'a', name: 'agent-a', since: local(22, 11, 30) }]}
        now={NOW}
      />
    )
    const today = container.querySelector('[data-day="2026-10-22"]')
    expect(today?.querySelector('[data-open]')).not.toBeNull()
    expect(container.querySelectorAll('[data-open]')).toHaveLength(1)
    expect(screen.getByText('waiting now')).toBeTruthy()
  })

  it('switches the longest waits between today and the 7 days', () => {
    render(<WaitsCard ledger={ledgerOf()} waitingNow={[]} now={NOW} />)
    const longest = screen.getByLabelText('Longest waits')
    expect(within(longest).getAllByRole('listitem').map((li) => li.getAttribute('data-agent'))).toEqual(['a', 'b'])
    expect(within(longest).getAllByText('1 wait')).toHaveLength(2)

    fireEvent.click(within(longest).getByRole('button', { name: '7 days' }))
    const rows = within(longest).getAllByRole('listitem')
    expect(within(rows[0]).getByText('2 waits')).toBeTruthy()
    expect(within(rows[0]).getByText('25m 00s')).toBeTruthy()
    expect(within(longest).getByRole('button', { name: '7 days' }).getAttribute('aria-pressed')).toBe('true')
  })

  it('leaves out agents the filter rejects', () => {
    render(<WaitsCard ledger={ledgerOf()} waitingNow={[]} now={NOW} agentFilter={(id) => id === 'b'} />)
    const longest = screen.getByLabelText('Longest waits')
    expect(within(longest).getAllByRole('listitem').map((li) => li.getAttribute('data-agent'))).toEqual(['b'])
  })

  it('says when there was nothing to measure', () => {
    render(<WaitsCard ledger={ledgerOf({ spans: [], transitions: [] })} waitingNow={[]} now={NOW} />)
    expect(screen.getByText('Nothing measured yet')).toBeTruthy()
  })
})
