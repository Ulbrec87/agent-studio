// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AgentDetails, OverviewSession, OverviewSnapshot } from '@shared/overview/types'
import { computeCounts } from '@shared/overview/status'
import { OverviewView } from './OverviewView'
import { SessionDetails } from './SessionDetails'

const NOW = 5_000_000

function session(extra: Partial<OverviewSession> = {}): OverviewSession {
  return {
    agentId: 'a1',
    name: 'builder',
    provider: 'claude',
    model: 'sonnet',
    projectDir: '/work/alpha',
    worktreeBranch: null,
    repoName: 'alpha',
    branch: 'main',
    status: 'working',
    statusSince: NOW - 90_000,
    stale: false,
    pid: 4242,
    startedAt: NOW - 3_600_000,
    lastOutputAt: NOW - 1000,
    ...extra
  } as OverviewSession
}

const DETAILS: AgentDetails = {
  lastMessage: { text: 'Line one\nLine two', truncated: false },
  tokens: { input: 182431, cached: 150000, output: 9120 }
}

describe('SessionDetails', () => {
  const getDetails = vi.fn()

  beforeEach(() => {
    getDetails.mockReset().mockResolvedValue(DETAILS)
    window.hydra = {
      ...window.hydra,
      getOverviewAgentDetails: getDetails,
      killAgent: vi.fn().mockResolvedValue(true),
      restartAgent: vi.fn().mockResolvedValue(null)
    }
  })
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it('tells how often the session waited on the user in the last hour', async () => {
    const { rerender } = render(
      <SessionDetails
        agentId="a1"
        session={session()}
        now={NOW}
        onClose={vi.fn()}
        onJumpToAgent={vi.fn()}
        waited={{ count: 3, ms: 252_000, from: NOW - 3_600_000 }}
      />
    )
    expect(await screen.findByText('Waited 3 times, total 4m 12s', { exact: false })).toBeTruthy()
    rerender(
      <SessionDetails
        agentId="a1"
        session={session()}
        now={NOW}
        onClose={vi.fn()}
        onJumpToAgent={vi.fn()}
        waited={{ count: 0, ms: 0, from: NOW - 3_600_000 }}
      />
    )
    expect(screen.queryByText(/Waited/)).toBeNull()
  })

  it('shows the facts, tokens and last message', async () => {
    render(
      <SessionDetails agentId="a1" session={session()} now={NOW} onClose={vi.fn()} onJumpToAgent={vi.fn()} />
    )
    expect(await screen.findByText(/182,431 in · 9,120 out/)).toBeTruthy()
    expect(screen.getByText('of which 150,000 cached')).toBeTruthy()
    expect(screen.getByText(/Line one/).textContent).toBe('Line one\nLine two')
    expect(screen.getByText('4242')).toBeTruthy()
    expect(screen.getByText('/work/alpha')).toBeTruthy()
    expect(screen.getByText('main')).toBeTruthy()
  })

  it('shows when an ended session ended, and its exit code', async () => {
    const at = NOW - 2 * 3_600_000
    const { rerender } = render(
      <SessionDetails
        agentId="a1"
        session={session({ status: 'failed', endedAt: at, statusSince: at, exitCode: 2 })}
        now={NOW}
        onClose={vi.fn()}
        onJumpToAgent={vi.fn()}
      />
    )
    expect(await screen.findByText(/^Ended at .*2h ago/)).toBeTruthy()
    expect(screen.getByText('Exit code')).toBeTruthy()
    expect(screen.getByText('2')).toBeTruthy()
    rerender(
      <SessionDetails
        agentId="a1"
        session={session({ status: 'not-running', endedAt: at, statusSince: at })}
        now={NOW}
        onClose={vi.fn()}
        onJumpToAgent={vi.fn()}
      />
    )
    expect(screen.getByText(/^Last active .*2h ago/)).toBeTruthy()
    expect(screen.queryByText('Exit code')).toBeNull()
  })

  it('closes with Esc', () => {
    const onClose = vi.fn()
    render(<SessionDetails agentId="a1" session={session()} now={NOW} onClose={onClose} onJumpToAgent={vi.fn()} />)
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('renders the placeholders only when given', () => {
    const { rerender } = render(
      <SessionDetails agentId="a1" session={session()} now={NOW} onClose={vi.fn()} onJumpToAgent={vi.fn()} />
    )
    expect(screen.queryByText('own events')).toBeNull()
    rerender(
      <SessionDetails
        agentId="a1"
        session={session()}
        now={NOW}
        onClose={vi.fn()}
        onJumpToAgent={vi.fn()}
        ownEvents={<p>own events</p>}
      />
    )
    expect(screen.getByText('own events')).toBeTruthy()
  })

  it('keeps the last known data and says so when the agent disappears', async () => {
    const { rerender } = render(
      <SessionDetails agentId="a1" session={session()} now={NOW} onClose={vi.fn()} onJumpToAgent={vi.fn()} />
    )
    await screen.findByText(/182,431 in/)
    rerender(
      <SessionDetails agentId="a1" session={undefined} now={NOW} onClose={vi.fn()} onJumpToAgent={vi.fn()} />
    )
    expect(screen.getByText('This agent is no longer running')).toBeTruthy()
    expect(screen.getByText('builder')).toBeTruthy()
    expect((screen.getByRole('button', { name: 'Jump' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('refetches every 2 s while visible', async () => {
    vi.useFakeTimers()
    render(<SessionDetails agentId="a1" session={session()} now={NOW} onClose={vi.fn()} onJumpToAgent={vi.fn()} />)
    expect(getDetails).toHaveBeenCalledTimes(1)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(4100)
    })
    expect(getDetails).toHaveBeenCalledTimes(3)
  })

  it('stops the agent through the existing action', async () => {
    render(<SessionDetails agentId="a1" session={session()} now={NOW} onClose={vi.fn()} onJumpToAgent={vi.fn()} />)
    fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
    expect(window.hydra.killAgent).toHaveBeenCalledWith('a1')
    await screen.findByRole('button', { name: 'Stop' })
  })
})

describe('OverviewView drawer', () => {
  beforeEach(() => {
    window.localStorage.clear()
    const sessions = [session(), session({ agentId: 'a2', name: 'waiter', status: 'needs-you', waitingReason: 'permission' })]
    const snapshot: OverviewSnapshot = { at: Date.now(), sessions, counts: computeCounts(sessions, Date.now()) }
    window.hydra = {
      ...window.hydra,
      getOverview: vi.fn().mockResolvedValue(snapshot),
      onOverviewSnapshot: vi.fn().mockReturnValue(() => undefined),
      getOverviewAgentDetails: vi.fn().mockResolvedValue(DETAILS)
    }
  })
  afterEach(cleanup)

  it('opens from a row, closes with Esc and returns focus to the row', async () => {
    render(<OverviewView />)
    const row = await screen.findByRole('button', { name: 'Details of builder' })
    row.focus()
    fireEvent.click(row)
    expect(screen.getByRole('dialog', { name: 'Details of builder' })).toBeTruthy()
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(document.activeElement).toBe(row)
  })

  it('opens from the agent name in the Needs you panel', async () => {
    render(<OverviewView />)
    const names = await screen.findAllByRole('button', { name: 'Details of waiter' })
    fireEvent.click(names[0])
    expect(screen.getByRole('dialog', { name: 'Details of waiter' })).toBeTruthy()
    expect(screen.getByText('Waiting for permission', { selector: 'dd' })).toBeTruthy()
  })

  it('does not open when Jump is clicked', async () => {
    const onJump = vi.fn()
    render(<OverviewView onJumpToAgent={onJump} />)
    fireEvent.click(await screen.findByRole('button', { name: 'Jump to builder' }))
    expect(onJump).toHaveBeenCalledWith('a1')
    expect(screen.queryByRole('dialog')).toBeNull()
  })
})
