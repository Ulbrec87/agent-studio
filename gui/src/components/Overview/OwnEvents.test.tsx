// @vitest-environment jsdom

import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import type { OverviewEvent } from '@shared/overview/types'
import { OwnEvents } from './OwnEvents'

const NOW = Date.parse('2026-10-22T12:00:00Z')

function ev(at: number, extra: Partial<OverviewEvent> = {}): OverviewEvent {
  return { id: `a@${at}`, at, kind: 'status', agentId: 'a', name: 'alpha', projectDir: '/w', ...extra }
}

describe('OwnEvents', () => {
  afterEach(cleanup)

  it('lists the events of the agent, newest first', () => {
    render(
      <OwnEvents
        agentId="a"
        now={NOW}
        events={[
          ev(NOW - 5000, { from: 'working', to: 'needs-you' }),
          ev(NOW - 1000, { from: 'needs-you', to: 'working', waitedMs: 4000 }),
          ev(NOW - 500, { agentId: 'b', from: 'idle', to: 'working' })
        ]}
      />
    )
    const items = screen.getAllByRole('listitem').map((li) => li.textContent)
    expect(items).toHaveLength(2)
    expect(items[0]).toContain('alpha is working after waiting 4s')
    expect(items[1]).toContain('alpha is waiting for you')
  })

  it('says so when there are none', () => {
    render(<OwnEvents agentId="a" now={NOW} events={[]} />)
    expect(screen.getByText('No events yet')).toBeTruthy()
  })
})
