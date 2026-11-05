// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { OverviewSession, OverviewSnapshot } from '@shared/overview/types'
import { computeCounts } from '@shared/overview/status'
import { OverviewView } from './OverviewView'

function session(id: string, projectDir: string, status: OverviewSession['status'] = 'working'): OverviewSession {
  return {
    agentId: id,
    name: `agent-${id}`,
    provider: 'claude',
    model: 'sonnet',
    projectDir,
    worktreeBranch: null,
    status,
    statusSince: 0,
    stale: false,
    pid: null,
    startedAt: 0,
    lastOutputAt: 0
  } as OverviewSession
}

function snapshotOf(sessions: OverviewSession[]): OverviewSnapshot {
  return { at: Date.now(), sessions, counts: computeCounts(sessions, Date.now()) }
}

const mockSnapshot = (sessions: OverviewSession[]) => {
  window.hydra = {
    ...window.hydra,
    getOverview: vi.fn().mockResolvedValue(snapshotOf(sessions)),
    onOverviewSnapshot: vi.fn().mockReturnValue(() => undefined)
  }
}

describe('OverviewView', () => {
  afterEach(cleanup)

  beforeEach(() => {
    window.localStorage.clear()
  })

  it('filters sessions by project and remembers the choice', async () => {
    mockSnapshot([
      session('1', '/work/alpha'),
      session('2', '/work/beta'),
      session('3', '/work/beta', 'needs-you')
    ])
    render(<OverviewView />)

    const select = (await screen.findByLabelText('Project filter')) as HTMLSelectElement
    expect(within(select).getAllByRole('option').map((o) => o.textContent)).toEqual([
      'All projects',
      'alpha',
      'beta'
    ])
    expect(screen.getByText('3 agents · 1 need you · 2 working')).toBeTruthy()

    fireEvent.change(select, { target: { value: '/work/beta' } })

    expect(screen.queryByText('agent-1')).toBeNull()
    expect(screen.getAllByText('agent-2').length).toBeGreaterThan(0)
    expect(screen.getByText('2 agents · 1 need you · 1 working')).toBeTruthy()
    expect(window.localStorage.getItem('agentStudio.overview.projectFilter')).toBe('/work/beta')
  })

  it('restores the stored filter and falls back to All when the project is gone', async () => {
    window.localStorage.setItem('agentStudio.overview.projectFilter', '/work/gone')
    mockSnapshot([session('1', '/work/alpha')])
    render(<OverviewView />)
    const select = (await screen.findByLabelText('Project filter')) as HTMLSelectElement
    expect(select.value).toBe('')
    expect(screen.getByText('agent-1')).toBeTruthy()
  })

  it('shows an empty state with a New Agent button when there are no agents', async () => {
    mockSnapshot([])
    const onOpenNewAgent = vi.fn()
    render(<OverviewView onOpenNewAgent={onOpenNewAgent} />)
    fireEvent.click(await screen.findByText('New Agent'))
    expect(onOpenNewAgent).toHaveBeenCalledTimes(1)
  })

  it('shows an error state when the overview is unavailable', async () => {
    window.hydra = {
      ...window.hydra,
      getOverview: vi.fn().mockRejectedValue(new Error('404')),
      onOverviewSnapshot: vi.fn().mockReturnValue(() => undefined)
    }
    render(<OverviewView />)
    expect(await screen.findByRole('alert')).toBeTruthy()
  })
})
