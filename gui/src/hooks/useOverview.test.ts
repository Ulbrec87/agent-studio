// @vitest-environment jsdom

import { act, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { OverviewSnapshot } from '@shared/overview/types'
import { computeCounts } from '@shared/overview/status'
import { useOverview } from './useOverview'

function snap(at: number): OverviewSnapshot {
  return { at, sessions: [], counts: computeCounts([], at) }
}

describe('useOverview', () => {
  let listener: ((s: OverviewSnapshot) => void) | null
  const unsubscribe = vi.fn()
  const getOverview = vi.fn()

  beforeEach(() => {
    listener = null
    unsubscribe.mockReset()
    getOverview.mockReset()
    window.hydra = {
      ...window.hydra,
      getOverview,
      onOverviewSnapshot: vi.fn((cb: (s: OverviewSnapshot) => void) => {
        listener = cb
        return unsubscribe
      })
    }
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('loads the initial snapshot', async () => {
    getOverview.mockResolvedValue(snap(100))
    const { result } = renderHook(() => useOverview())
    expect(result.current.loading).toBe(true)
    await waitFor(() => expect(result.current.snapshot?.at).toBe(100))
    expect(result.current.loading).toBe(false)
    expect(result.current.error).toBeNull()
  })

  it('applies pushed snapshots and unsubscribes on unmount', async () => {
    getOverview.mockResolvedValue(snap(100))
    const { result, unmount } = renderHook(() => useOverview())
    await waitFor(() => expect(result.current.snapshot).not.toBeNull())
    act(() => listener?.(snap(200)))
    expect(result.current.snapshot?.at).toBe(200)
    unmount()
    expect(unsubscribe).toHaveBeenCalledTimes(1)
  })

  it('reports an error when the daemon cannot serve the overview', async () => {
    getOverview.mockRejectedValue(new Error('404 Not Found'))
    const { result } = renderHook(() => useOverview())
    await waitFor(() => expect(result.current.error).toBe('404 Not Found'))
    expect(result.current.loading).toBe(false)
    expect(result.current.snapshot).toBeNull()
  })

  it('ticks now every second while mounted', async () => {
    vi.useFakeTimers()
    getOverview.mockResolvedValue(snap(1))
    const { result } = renderHook(() => useOverview())
    const first = result.current.now
    act(() => {
      vi.advanceTimersByTime(3000)
    })
    expect(result.current.now - first).toBeGreaterThanOrEqual(3000)
  })
})
