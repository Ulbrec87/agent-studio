// @vitest-environment jsdom

import { act, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { OverviewEvent, WaitsResponse } from '@shared/overview/types'
import { WAITS_REFRESH_MS, useOverviewWaits } from './useOverviewWaits'

function ev(extra: Partial<OverviewEvent>): OverviewEvent {
  return { id: 'a@1:status', at: 1, kind: 'status', agentId: 'a', name: 'alpha', projectDir: '/w', ...extra }
}

const response = (n: number): WaitsResponse => ({
  ledger: { transitions: [], spans: [{ start: 0, end: n }], starts: [] }
})

describe('useOverviewWaits', () => {
  let listener: ((events: OverviewEvent[]) => void) | null
  const unsubscribe = vi.fn()
  const getOverviewWaits = vi.fn()
  let ticks: (() => void)[]

  beforeEach(() => {
    ticks = []
    listener = null
    const realSetInterval = globalThis.setInterval
    vi.spyOn(globalThis, 'setInterval').mockImplementation(((cb: () => void, ms?: number) => {
      if (ms !== WAITS_REFRESH_MS) return realSetInterval(cb, ms)
      ticks.push(cb)
      return 0 as unknown as ReturnType<typeof setInterval>
    }) as typeof setInterval)
    getOverviewWaits.mockReset().mockResolvedValue(response(1))
    window.hydra = {
      ...window.hydra,
      getOverviewWaits,
      onOverviewEvents: (cb: (events: OverviewEvent[]) => void) => {
        listener = cb
        return unsubscribe
      }
    } as typeof window.hydra
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('fetches on mount, every 30 s and when an event moves an agent into or out of needs-you', async () => {
    const { result, unmount } = renderHook(() => useOverviewWaits())
    await waitFor(() => expect(result.current.ledger?.spans[0].end).toBe(1))
    expect(getOverviewWaits).toHaveBeenCalledTimes(1)

    act(() => ticks[0]())
    expect(getOverviewWaits).toHaveBeenCalledTimes(2)

    act(() => listener?.([ev({ from: 'working', to: 'idle' })]))
    expect(getOverviewWaits).toHaveBeenCalledTimes(2)

    getOverviewWaits.mockResolvedValue(response(2))
    act(() => listener?.([ev({ from: 'working', to: 'needs-you' })]))
    expect(getOverviewWaits).toHaveBeenCalledTimes(3)
    await waitFor(() => expect(result.current.ledger?.spans[0].end).toBe(2))

    act(() => listener?.([ev({ from: 'needs-you', to: 'working' })]))
    expect(getOverviewWaits).toHaveBeenCalledTimes(4)

    unmount()
    expect(unsubscribe).toHaveBeenCalled()
  })

  it('reports an error and keeps the last ledger', async () => {
    const { result } = renderHook(() => useOverviewWaits())
    await waitFor(() => expect(result.current.ledger).not.toBeNull())
    getOverviewWaits.mockRejectedValue(new Error('down'))
    act(() => ticks[0]())
    await waitFor(() => expect(result.current.error).toBe('down'))
    expect(result.current.ledger).not.toBeNull()
  })
})
