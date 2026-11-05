// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { useRef, useState } from 'react'
import { afterEach, describe, expect, it } from 'vitest'
import type { HistoryAgent, HistoryPoint, OverviewCounts, OverviewStatus } from '@shared/overview/types'
import type { CountKey } from '@shared/overview/buckets'
import { CountHistory } from './CountHistory'
import { CountsRow } from './CountsRow'
import { LastHourChart } from './LastHourChart'
import { TimelineCard } from './TimelineCard'

const MIN = 60_000
const NOW = Date.UTC(2026, 0, 1, 12, 7, 0)

function point(at: number, statuses: Record<string, OverviewStatus>): HistoryPoint {
  const v = Object.values(statuses)
  return {
    at,
    needsYou: v.filter((s) => s === 'needs-you').length,
    working: v.filter((s) => s === 'working').length,
    idle: v.filter((s) => s === 'idle').length,
    total: v.length,
    statuses
  }
}

function run(from: number, to: number, statuses: Record<string, OverviewStatus>): HistoryPoint[] {
  const out: HistoryPoint[] = []
  for (let t = from; t <= to; t += 5000) out.push(point(t, statuses))
  return out
}

describe('LastHourChart', () => {
  afterEach(cleanup)

  it('draws a bar for each measured bucket and hatches the rest', () => {
    // last 10 minutes measured: the 12:00 and 12:05 buckets
    const points = run(NOW - 10 * MIN, NOW, { a: 'needs-you', b: 'working' })
    const { container } = render(<LastHourChart points={points} starts={[]} now={NOW} />)
    expect(container.querySelectorAll('[data-bucket]')).toHaveLength(12)
    expect(container.querySelectorAll('[data-bar]')).toHaveLength(3)
    expect(container.querySelectorAll('[data-unmeasured]').length).toBeGreaterThanOrEqual(9)
    expect(screen.getByText(/Agents waited on you 10m in the last hour/)).toBeTruthy()
  })

  it('says so when nothing waited', () => {
    const points = run(NOW - 10 * MIN, NOW, { a: 'working' })
    render(<LastHourChart points={points} starts={[]} now={NOW} />)
    expect(screen.getByText('Nothing waited on you in the last hour')).toBeTruthy()
  })

  it('shows an unmeasured state without history', () => {
    const { container } = render(<LastHourChart points={[]} starts={[]} now={NOW} />)
    expect(container.querySelectorAll('[data-bar]')).toHaveLength(0)
    expect(container.querySelectorAll('[data-unmeasured]')).toHaveLength(12)
    expect(screen.getByText('Nothing measured in the last hour yet')).toBeTruthy()
  })

  it('reads a bar with the arrow keys', () => {
    const points = run(NOW - 60 * MIN, NOW, { a: 'working', b: 'idle' })
    render(<LastHourChart points={points} starts={[]} now={NOW} />)
    const chart = screen.getByRole('group', { name: /Last hour/ })
    expect(chart.getAttribute('tabindex')).toBe('0')
    const readout = screen.getByTestId('last-hour-readout')
    expect(readout.getAttribute('aria-live')).toBe('polite')
    fireEvent.keyDown(chart, { key: 'ArrowLeft' })
    expect(readout.textContent).toMatch(/working 1, idle 1 on average/)
    fireEvent.keyDown(chart, { key: 'Home' })
    expect(readout.textContent).toMatch(/needs you 0/)
    fireEvent.keyDown(chart, { key: 'Escape' })
    expect(readout.textContent).toMatch(/Point at a bar/)
  })

  it('reports an unmeasured bucket', () => {
    render(<LastHourChart points={run(NOW - 2 * MIN, NOW, { a: 'idle' })} starts={[]} now={NOW} />)
    fireEvent.keyDown(screen.getByRole('group', { name: /Last hour/ }), { key: 'Home' })
    expect(screen.getByTestId('last-hour-readout').textContent).toMatch(/not measured/)
  })
})

describe('TimelineCard', () => {
  afterEach(cleanup)

  const agents: Record<string, HistoryAgent> = {
    a: { name: 'alpha', projectDir: '/p/one' },
    b: { name: 'beta', projectDir: '/p/two' },
    old: { name: 'old-one', projectDir: '/p/one' }
  }

  it('renders a row per agent present in the hour and hides the others', () => {
    const points = [
      ...run(NOW - 30 * MIN, NOW - 20 * MIN, { a: 'working', old: 'idle' }),
      ...run(NOW - 19 * MIN, NOW, { a: 'idle', b: 'working' })
    ]
    // `old` last seen 70 minutes ago would be hidden: put it outside the window
    const outside = run(NOW - 90 * MIN, NOW - 80 * MIN, { old: 'idle' })
    render(<TimelineCard points={[...outside, ...points]} starts={[]} now={NOW} agents={agents} />)
    const rows = screen.getAllByRole('listitem').filter((li) => li.hasAttribute('data-agent'))
    expect(rows.map((r) => r.getAttribute('data-agent'))).toEqual(['a', 'b', 'old'])
    expect(within(rows[0]).getByText('alpha')).toBeTruthy()
    expect(screen.getByText('Not measured')).toBeTruthy()
  })

  it('hides agents that have no segment in the hour', () => {
    const points = [...run(NOW - 90 * MIN, NOW - 80 * MIN, { old: 'idle' }), ...run(NOW - 10 * MIN, NOW, { a: 'idle' })]
    render(<TimelineCard points={points} starts={[]} now={NOW} agents={agents} />)
    expect(screen.queryByText('old-one')).toBeNull()
    expect(screen.getByText('alpha')).toBeTruthy()
  })

  it('applies the project filter', () => {
    const points = run(NOW - 10 * MIN, NOW, { a: 'idle', b: 'working' })
    render(
      <TimelineCard points={points} starts={[]} now={NOW} agents={agents} agentFilter={(id) => agents[id].projectDir === '/p/two'} />
    )
    expect(screen.queryByText('alpha')).toBeNull()
    expect(screen.getByText('beta')).toBeTruthy()
  })

  it('shows an empty state without history', () => {
    render(<TimelineCard points={[]} starts={[]} now={NOW} agents={agents} />)
    expect(screen.getByText('No history yet')).toBeTruthy()
  })
})

const counts: OverviewCounts = {
  working: 1, idle: 1, stale: 0, needsYou: 0, open: 2, finished: 0, failed: 0, notRunning: 0, total: 2,
  longestWorkingMs: null, longestIdleMs: null
}

function Harness({ points }: { points: HistoryPoint[] }) {
  const [key, setKey] = useState<CountKey | null>(null)
  const trigger = useRef<HTMLElement | null>(null)
  return (
    <div>
      <CountsRow
        counts={counts}
        openChart={key}
        onOpenChart={(k, el) => {
          trigger.current = el
          setKey(k)
        }}
      />
      {key && (
        <CountHistory
          metric={key}
          points={points}
          starts={[]}
          now={NOW}
          onClose={() => {
            setKey(null)
            trigger.current?.focus()
          }}
        />
      )}
    </div>
  )
}

describe('counts chart', () => {
  afterEach(cleanup)
  const points = run(NOW - 60 * MIN, NOW, { a: 'working', b: 'idle' })

  it('opens from Working, switches range, and closes with the button, returning focus', () => {
    const { container } = render(<Harness points={points} />)
    const trigger = screen.getByRole('button', { name: 'Working history' })
    expect(trigger.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(trigger)
    const panel = screen.getByRole('region', { name: 'Working history' })
    expect(trigger.getAttribute('aria-expanded')).toBe('true')
    expect(container.querySelectorAll('[data-count-bucket]')).toHaveLength(12)
    fireEvent.click(screen.getByRole('button', { name: '15 m' }))
    expect(container.querySelectorAll('[data-count-bucket]')).toHaveLength(15)
    fireEvent.click(screen.getByRole('button', { name: '6 h' }))
    expect(container.querySelectorAll('[data-count-bucket]')).toHaveLength(24)
    expect(panel.isConnected).toBe(true)
    fireEvent.click(screen.getByRole('button', { name: 'Close chart' }))
    expect(screen.queryByRole('region', { name: 'Working history' })).toBeNull()
    expect(document.activeElement).toBe(trigger)
  })

  it('closes on Escape', () => {
    render(<Harness points={points} />)
    const trigger = screen.getByRole('button', { name: 'Idle history' })
    fireEvent.click(trigger)
    fireEvent.keyDown(screen.getByRole('region', { name: 'Idle history' }), { key: 'Escape' })
    expect(screen.queryByRole('region', { name: 'Idle history' })).toBeNull()
    expect(document.activeElement).toBe(trigger)
  })

  it('reads a bar with the keyboard', () => {
    render(<Harness points={points} />)
    fireEvent.click(screen.getByRole('button', { name: 'Working history' }))
    fireEvent.keyDown(screen.getByRole('group', { name: /Working over time/ }), { key: 'ArrowLeft' })
    expect(screen.getByTestId('count-readout').textContent).toMatch(/1 on average/)
  })
})
