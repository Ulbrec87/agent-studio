// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { OverviewSession } from '@shared/overview/types'
import { NeedsYouPanel } from './NeedsYouPanel'

function waiting(id: string, statusSince: number, extra: Partial<OverviewSession> = {}): OverviewSession {
  return {
    agentId: id,
    name: `agent-${id}`,
    provider: 'claude',
    model: 'sonnet',
    projectDir: '/work/alpha',
    worktreeBranch: null,
    status: 'needs-you',
    statusSince,
    stale: false,
    pid: null,
    startedAt: 0,
    lastOutputAt: 0,
    ...extra
  } as OverviewSession
}

const NOW = 10_000_000

describe('NeedsYouPanel', () => {
  afterEach(cleanup)

  it('lists the longest wait first', () => {
    render(
      <NeedsYouPanel
        sessions={[waiting('new', NOW - 1000), waiting('old', NOW - 90_000), waiting('mid', NOW - 30_000)]}
        now={NOW}
        showProvider={false}
        onJumpToAgent={vi.fn()}
      />
    )
    const items = screen.getAllByRole('listitem')
    expect(items.map((li) => within(li).getByText(/^agent-/).textContent)).toEqual([
      'agent-old',
      'agent-mid',
      'agent-new'
    ])
  })

  it('labels the reason', () => {
    render(
      <NeedsYouPanel
        sessions={[
          waiting('a', 1, { waitingReason: 'permission' }),
          waiting('b', 2, { waitingReason: 'question' }),
          waiting('c', 3, { waitingReason: 'other' }),
          waiting('d', 4)
        ]}
        now={NOW}
        showProvider={false}
        onJumpToAgent={vi.fn()}
      />
    )
    expect(screen.getByText('Waiting for permission')).toBeTruthy()
    expect(screen.getByText('Asked you a question')).toBeTruthy()
    expect(screen.getAllByText('Waiting for you')).toHaveLength(2)
  })

  it('shows branch, and the provider only when asked to', () => {
    const sessions = [waiting('a', 1, { worktreeBranch: 'feat/x', provider: 'codex' })]
    const { rerender } = render(
      <NeedsYouPanel sessions={sessions} now={NOW} showProvider={false} onJumpToAgent={vi.fn()} />
    )
    expect(screen.getByText('alpha on feat/x').getAttribute('title')).toBe('/work/alpha')
    rerender(<NeedsYouPanel sessions={sessions} now={NOW} showProvider onJumpToAgent={vi.fn()} />)
    expect(screen.getByText('alpha on feat/x · Codex')).toBeTruthy()
  })

  it('renders the waiting text and the live timer', () => {
    const sessions = [waiting('a', NOW - 252_000, { waitingText: 'Run: npm test' })]
    const { rerender } = render(
      <NeedsYouPanel sessions={sessions} now={NOW} showProvider={false} onJumpToAgent={vi.fn()} />
    )
    expect(screen.getByText('Run: npm test').getAttribute('title')).toBe('Run: npm test')
    expect(screen.getByText('4m 12s')).toBeTruthy()
    rerender(<NeedsYouPanel sessions={sessions} now={NOW + 1000} showProvider={false} onJumpToAgent={vi.fn()} />)
    expect(screen.getByText('4m 13s')).toBeTruthy()
  })

  it('calls onJumpToAgent with the agent id', () => {
    const onJump = vi.fn()
    render(
      <NeedsYouPanel
        sessions={[waiting('a', 1), waiting('b', 2)]}
        now={NOW}
        showProvider={false}
        onJumpToAgent={onJump}
      />
    )
    fireEvent.click(screen.getByRole('button', { name: 'Jump to agent-b' }))
    expect(onJump).toHaveBeenCalledWith('b')
  })

  it('shows the empty state, and the last wait when provided', () => {
    const { rerender } = render(
      <NeedsYouPanel sessions={[]} now={NOW} showProvider={false} onJumpToAgent={vi.fn()} />
    )
    expect(screen.getByRole('heading', { name: 'Nothing needs you' })).toBeTruthy()
    expect(screen.getByText(/it will show here/)).toBeTruthy()
    expect(screen.queryByText('Last wait')).toBeNull()
    rerender(
      <NeedsYouPanel
        sessions={[]}
        now={NOW}
        showProvider={false}
        onJumpToAgent={vi.fn()}
        lastWait={{ name: 'agent-z', waitedMs: 65_000, endedAt: NOW - 120_000 }}
      />
    )
    expect(screen.getByText('Last wait')).toBeTruthy()
    expect(screen.getByText('1m 05s')).toBeTruthy()
    expect(screen.getByText('agent-z')).toBeTruthy()
    expect(screen.getByText(/answered at/)).toBeTruthy()
  })
})
