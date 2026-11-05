// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { OverviewSession } from '@shared/overview/types'
import { LAYOUT_STORAGE_KEY, SessionsCard } from './SessionsCard'

const NOW = 100_000_000

function s(id: string, status: OverviewSession['status'], extra: Partial<OverviewSession> = {}): OverviewSession {
  return {
    agentId: id,
    name: `agent-${id}`,
    provider: 'claude',
    model: 'sonnet',
    projectDir: `/work/${id}`,
    worktreeBranch: null,
    status,
    statusSince: NOW - 60_000,
    stale: false,
    pid: null,
    startedAt: 0,
    lastOutputAt: 0,
    ...extra
  } as OverviewSession
}

function renderCard(sessions: OverviewSession[], props: { onOpen?: () => void; onJump?: () => void } = {}) {
  return render(
    <SessionsCard
      sessions={sessions}
      now={NOW}
      showProvider={false}
      selectedId={null}
      onOpenDetails={props.onOpen ?? vi.fn()}
      onJumpToAgent={props.onJump ?? vi.fn()}
    />
  )
}

describe('SessionsCard', () => {
  beforeEach(() => window.localStorage.clear())
  afterEach(cleanup)

  it('lists groups in order without the needs-you sessions and counts all in the title', () => {
    renderCard([s('a', 'finished'), s('b', 'working'), s('c', 'needs-you'), s('d', 'idle', { stale: true })])
    expect(screen.getByRole('region', { name: 'Sessions' }).textContent).toContain('4')
    const headings = screen.getAllByRole('heading', { level: 4 }).map((h) => h.textContent)
    expect(headings).toEqual(['Working1', 'Idle1', 'Finished / Failed1'])
    expect(screen.queryByText('agent-c')).toBeNull()
    expect(screen.getByText('Idle (stale)')).toBeTruthy()
  })

  it('shows folder, branch, the commit when detached, and quiet for', () => {
    renderCard([
      s('a', 'working', { branch: 'feat/x', quietForMs: 700_000, lastOutputAt: NOW - 720_000 }),
      s('b', 'idle', { commit: 'abc1234' })
    ])
    expect(screen.getByText('feat/x')).toBeTruthy()
    expect(screen.getByText('abc1234')).toBeTruthy()
    expect(screen.getByText('quiet for 12m')).toBeTruthy()
  })

  it('switches layout and remembers it', () => {
    renderCard([s('a', 'working', { repoName: 'shop' }), s('b', 'idle', { repoName: 'shop' }), s('c', 'idle')])
    fireEvent.click(screen.getByRole('button', { name: 'Repos' }))
    const headings = screen.getAllByRole('heading', { level: 4 }).map((h) => h.textContent)
    expect(headings).toEqual(['shop2', 'No repository1'])
    expect(window.localStorage.getItem(LAYOUT_STORAGE_KEY)).toBe('repos')

    cleanup()
    renderCard([s('a', 'working')])
    expect(screen.getByRole('button', { name: 'Repos' }).getAttribute('aria-pressed')).toBe('true')
    fireEvent.click(screen.getByRole('button', { name: 'Board' }))
    expect(window.localStorage.getItem(LAYOUT_STORAGE_KEY)).toBe('board')
  })

  it('names the worktree folder in Repos only when it differs from the repository', () => {
    window.localStorage.setItem(LAYOUT_STORAGE_KEY, 'repos')
    renderCard([
      s('a', 'working', { repoName: 'a', projectDir: '/work/a' }),
      s('b', 'working', { repoName: 'a', projectDir: '/work/a-wt' })
    ])
    expect(screen.getAllByText('a-wt')).toHaveLength(1)
    // Only the group heading says "a": the folder of the main checkout is not repeated.
    expect(screen.getAllByText('a')).toHaveLength(1)
  })

  it('caps board columns at five cards with a note for the rest', () => {
    window.localStorage.setItem(LAYOUT_STORAGE_KEY, 'board')
    const many = Array.from({ length: 7 }, (_, i) => s(`w${i}`, 'working'))
    renderCard([...many, s('n', 'needs-you')])
    const working = screen.getByRole('region', { name: 'Working' })
    expect(within(working).getAllByRole('listitem')).toHaveLength(5)
    expect(within(working).getByText('and 2 more in the list')).toBeTruthy()
    expect(within(screen.getByRole('region', { name: 'Needs you' })).getByText('agent-n')).toBeTruthy()
    expect(within(screen.getByRole('region', { name: 'Idle' })).getByText('Nothing idle')).toBeTruthy()
  })

  it('shows 10 finished rows, then "Show 2 more" expands and "Show less" collapses', () => {
    const many = Array.from({ length: 12 }, (_, i) => s(`f${i}`, 'finished', { statusSince: NOW - i * 1000 }))
    renderCard(many)
    const group = screen.getByRole('region', { name: 'Finished / Failed' })
    expect(within(group).getAllByRole('listitem')).toHaveLength(10)
    expect(within(group).getByRole('heading', { level: 4 }).textContent).toBe('Finished / Failed12')
    const more = within(group).getByRole('button', { name: 'Show 2 more' })
    expect(more.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(more)
    expect(within(group).getAllByRole('listitem')).toHaveLength(12)
    const less = within(group).getByRole('button', { name: 'Show less' })
    expect(less.getAttribute('aria-expanded')).toBe('true')
    fireEvent.click(less)
    expect(within(group).getAllByRole('listitem')).toHaveLength(10)
  })

  it('does not cap the Repos layout', () => {
    window.localStorage.setItem(LAYOUT_STORAGE_KEY, 'repos')
    renderCard(Array.from({ length: 12 }, (_, i) => s(`f${i}`, 'finished', { projectDir: '/work/same' })))
    expect(screen.queryByRole('button', { name: /Show/ })).toBeNull()
  })

  it('notes not-running sessions under the board ended column', () => {
    window.localStorage.setItem(LAYOUT_STORAGE_KEY, 'board')
    renderCard([s('a', 'finished'), s('b', 'not-running'), s('c', 'not-running')])
    const ended = screen.getByRole('region', { name: 'Finished / Failed' })
    expect(within(ended).getByText('2 not running, in the list')).toBeTruthy()
    expect(within(ended).queryByText('agent-b')).toBeNull()
  })

  it('shows when ended rows ended instead of a running timer, and the exit code', () => {
    renderCard([
      s('a', 'failed', { endedAt: NOW - 3 * 3_600_000, statusSince: NOW - 3 * 3_600_000, exitCode: 1 }),
      s('b', 'not-running', { endedAt: NOW - 20 * 60_000, statusSince: NOW - 20 * 60_000 })
    ])
    expect(screen.getByText('ended 3h ago')).toBeTruthy()
    expect(screen.getByText('exit 1')).toBeTruthy()
    expect(screen.getByText('last active 20m ago')).toBeTruthy()
  })

  it('opens details on a row click but not when Jump is clicked', () => {
    const onOpen = vi.fn()
    const onJump = vi.fn()
    renderCard([s('a', 'working')], { onOpen, onJump })
    fireEvent.click(screen.getByRole('button', { name: 'Jump to agent-a' }))
    expect(onJump).toHaveBeenCalledWith('a')
    expect(onOpen).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Details of agent-a' }))
    expect(onOpen).toHaveBeenCalledWith('a', expect.any(HTMLElement))
  })
})
