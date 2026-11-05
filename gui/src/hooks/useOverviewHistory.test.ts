// @vitest-environment jsdom

import { act, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { HistoryResponse, OverviewEvent } from '@shared/overview/types'
import { HISTORY_REFRESH_MS, HISTORY_WINDOW_MS, mergeHistory, useOverviewHistory } from './useOverviewHistory'

const NOW = 1_800_000_000_000

function ev(at: number, extra: Partial<OverviewEvent> = {}): OverviewEvent {
  return { id: `a@${at}`, at, kind: 'status', agentId: 'a', name: 'alpha', projectDir: '/w', ...extra }
}

function response(extra: Partial<HistoryResponse> = {}): HistoryResponse {
  return { since: NOW - 1000, startedAt: NOW - 1000, starts: [NOW - 1000], points: [], events: [], agents: {}, ...extra }
}

function pt(at: number) {
  return { at, needsYou: 0, working: 1, idle: 0, total: 1, statuses: { a: 'working' as const } }
}

describe('mergeHistory', () => {
  it('adds new points, events and agents once, and drops points past the cutoff', () => {
    const held = response({ points: [pt(10), pt(20)], events: [ev(15)], agents: { a: { name: 'alpha', projectDir: '/w' } } })
    const merged = mergeHistory(
      held,
      response({ points: [pt(20), pt(30)], events: [ev(15), ev(25)], starts: [5], agents: { b: { name: 'b', projectDir: '/b' } } }),
      15
    )
    expect(merged.points.map((p) => p.at)).toEqual([20, 30])
    expect(merged.events.map((e) => e.at)).toEqual([15, 25])
    expect(Object.keys(merged.agents)).toEqual(['a', 'b'])
    expect(merged.starts).toEqual([NOW - 1000]) // the start at 5 is before the cutoff
  })
})

describe('useOverviewHistory', () => {
  let listener: ((events: OverviewEvent[]) => void) | null
  const unsubscribe = vi.fn()
  const getOverviewHistory = vi.fn()
  let ticks: (() => void)[]

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(NOW)
    ticks = []
    const realSetInterval = globalThis.setInterval
    // Only the hook's refresh timer is captured; testing-library polls with setInterval too.
    vi.spyOn(globalThis, 'setInterval').mockImplementation(((cb: () => void, ms?: number) => {
      if (ms !== HISTORY_REFRESH_MS) return realSetInterval(cb, ms)
      ticks.push(cb)
      return 0
    }) as unknown as typeof setInterval)
    listener = null
    unsubscribe.mockReset()
    getOverviewHistory.mockReset()
    window.hydra = {
      ...window.hydra,
      getOverviewHistory,
      onOverviewEvents: vi.fn((cb: (events: OverviewEvent[]) => void) => {
        listener = cb
        return unsubscribe
      })
    }
  })

  afterEach(() => {
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  it('loads the last 6 hours on mount', async () => {
    getOverviewHistory.mockResolvedValue(response({ points: [pt(NOW - 5000)] }))
    const { result } = renderHook(() => useOverviewHistory())
    expect(result.current.loading).toBe(true)
    await waitFor(() => expect(result.current.history?.points).toHaveLength(1))
    expect(getOverviewHistory).toHaveBeenCalledWith(NOW - HISTORY_WINDOW_MS)
    expect(result.current.loading).toBe(false)
    expect(result.current.error).toBeNull()
  })

  it('appends pushed events once and keeps those pushed before the first response', async () => {
    let resolve: (r: HistoryResponse) => void = () => undefined
    getOverviewHistory.mockReturnValue(new Promise<HistoryResponse>((r) => (resolve = r)))
    const { result } = renderHook(() => useOverviewHistory())
    act(() => listener?.([ev(NOW - 10)]))
    await act(async () => resolve(response({ events: [ev(NOW - 100)] })))
    expect(result.current.history?.events.map((e) => e.at)).toEqual([NOW - 100, NOW - 10])
    act(() => listener?.([ev(NOW - 10), ev(NOW - 5)]))
    expect(result.current.history?.events.map((e) => e.at)).toEqual([NOW - 100, NOW - 10, NOW - 5])
  })

  it('fetches only newer points on its refresh tick and unsubscribes on unmount', async () => {
    getOverviewHistory.mockResolvedValueOnce(response({ points: [pt(NOW - 5000)] }))
    const { result, unmount } = renderHook(() => useOverviewHistory())
    await waitFor(() => expect(result.current.history).not.toBeNull())
    getOverviewHistory.mockResolvedValueOnce(response({ points: [pt(NOW + 25_000)] }))
    expect(ticks).toHaveLength(1)
    await act(async () => {
      ticks[0]()
    })
    expect(getOverviewHistory).toHaveBeenLastCalledWith(NOW - 5000 + 1)
    await waitFor(() => expect(result.current.history?.points).toHaveLength(2))
    unmount()
    expect(unsubscribe).toHaveBeenCalledTimes(1)
  })

  it('reports an error', async () => {
    getOverviewHistory.mockRejectedValue(new Error('404 Not Found'))
    const { result } = renderHook(() => useOverviewHistory())
    await waitFor(() => expect(result.current.error).toBe('404 Not Found'))
    expect(result.current.loading).toBe(false)
    expect(result.current.history).toBeNull()
  })
})
