// @vitest-environment jsdom

import { cleanup, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { WaitBar } from '@shared/overview/types'
import { formatHourMinute } from './chartParts'
import { WaitedOnYou } from './WaitedOnYou'

const FROM = Date.parse('2026-10-22T11:00:00')

const bar = (agentId: string, ms: number, open = false): WaitBar => ({ agentId, name: `agent-${agentId}`, ms, count: 1, open })

describe('WaitedOnYou', () => {
  afterEach(cleanup)

  it('is left out when nobody waited', () => {
    const { container } = render(<WaitedOnYou bars={[]} from={FROM} unmeasuredMs={5 * 60_000} />)
    expect(container.firstChild).toBeNull()
  })

  it('shows a bar per agent, in the order given, and where the window begins', () => {
    render(<WaitedOnYou bars={[bar('b', 125_000), bar('a', 30_000)]} from={FROM} unmeasuredMs={0} />)
    expect(screen.getByRole('heading').textContent).toBe(`Waited on you since ${formatHourMinute(FROM)}`)
    const rows = screen.getAllByRole('listitem')
    expect(rows.map((r) => within(r).getByText(/^agent-/).textContent)).toEqual(['agent-b', 'agent-a'])
    expect(within(rows[0]).getByText('2m 05s')).toBeTruthy()
    expect(within(rows[1]).getByText('30s')).toBeTruthy()
  })

  it('tells an open wait from an answered one', () => {
    render(<WaitedOnYou bars={[bar('a', 90_000, true), bar('b', 60_000, false)]} from={FROM} unmeasuredMs={0} />)
    const [open, answered] = screen.getAllByRole('listitem')
    expect(open.getAttribute('data-state')).toBe('open')
    expect(answered.getAttribute('data-state')).toBe('answered')
    expect(within(open).getByRole('img').getAttribute('aria-label')).toContain('still waiting')
    expect(within(answered).getByRole('img').getAttribute('aria-label')).toContain('answered')
  })

  it('scales the bars to the longest wait', () => {
    render(<WaitedOnYou bars={[bar('a', 120_000), bar('b', 60_000)]} from={FROM} unmeasuredMs={0} />)
    const widths = screen
      .getAllByRole('img')
      .map((el) => (el.firstElementChild as HTMLElement).style.width)
    expect(widths).toEqual(['100%', '50%'])
  })

  it('says how much of the time was not measured, only when some was', () => {
    const { rerender } = render(<WaitedOnYou bars={[bar('a', 10_000)]} from={FROM} unmeasuredMs={12 * 60_000} />)
    expect(screen.getByText('Not measured in this time: 12m')).toBeTruthy()
    rerender(<WaitedOnYou bars={[bar('a', 10_000)]} from={FROM} unmeasuredMs={0} />)
    expect(screen.queryByText(/Not measured/)).toBeNull()
  })
})
