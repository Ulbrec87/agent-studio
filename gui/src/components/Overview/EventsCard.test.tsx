// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { HistoryResponse, OverviewEvent } from '@shared/overview/types'
import { EventsCard } from './EventsCard'
import { LAST_SEEN_KEY } from './newSince'

const NOW = new Date(2026, 9, 22, 12, 0, 0).getTime()

function ev(at: number, extra: Partial<OverviewEvent> = {}): OverviewEvent {
  return { id: `a@${at}:status`, at, kind: 'status', agentId: 'a', name: 'alpha', projectDir: '/w/one', ...extra }
}

function historyOf(events: OverviewEvent[], since = NOW - 3_600_000): HistoryResponse {
  return {
    since,
    startedAt: since,
    starts: [since],
    points: [],
    events,
    agents: { a: { name: 'alpha', projectDir: '/w/one' }, b: { name: 'beta', projectDir: '/w/two' } }
  }
}

describe('EventsCard', () => {
  beforeEach(() => localStorage.clear())
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('lists events newest first with sentences', () => {
    const history = historyOf([
      ev(NOW - 3000, { kind: 'appeared', to: 'working' }),
      ev(NOW - 2000, { from: 'working', to: 'needs-you', waitingReason: 'permission' }),
      ev(NOW - 1000, { from: 'needs-you', to: 'working', waitedMs: 252_000 })
    ])
    render(<EventsCard history={history} filter="" now={NOW} onOpenAgent={() => undefined} />)
    const items = screen.getAllByRole('listitem').map((li) => li.textContent ?? '')
    expect(items[0]).toContain('alpha is working after waiting 4m 12s')
    expect(items[1]).toContain('alpha is waiting for permission')
    expect(items[2]).toContain('alpha started')
  })

  it('filters by project, keeping resumed rows, and shows the resumed text', () => {
    const history = historyOf([
      ev(NOW - 3000, { kind: 'appeared' }),
      ev(NOW - 2000, { id: 'b@1', agentId: 'b', name: 'beta', projectDir: '/w/two', kind: 'appeared' }),
      { id: 'resumed@1', at: NOW - 1000, kind: 'resumed', agentId: '', name: '', projectDir: '', gapMs: 720_000 }
    ])
    render(<EventsCard history={history} filter="/w/two" now={NOW} onOpenAgent={() => undefined} />)
    expect(screen.queryByText('alpha')).toBeNull()
    expect(screen.getByText('beta')).toBeTruthy()
    expect(screen.getByText(/Watching resumed — 12m not measured/)).toBeTruthy()
  })

  it('opens the details from the name', () => {
    const onOpenAgent = vi.fn()
    render(<EventsCard history={historyOf([ev(NOW - 1000, { kind: 'appeared' })])} filter="" now={NOW} onOpenAgent={onOpenAgent} />)
    fireEvent.click(screen.getByRole('button', { name: 'alpha' }))
    expect(onOpenAgent).toHaveBeenCalledWith('a', expect.any(HTMLElement))
  })

  it('shows the footer, with the day when not today', () => {
    const time = (at: number) => new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
    const today = NOW - 3_600_000
    const { rerender } = render(
      <EventsCard history={historyOf([ev(NOW - 1000)], today)} filter="" now={NOW} onOpenAgent={() => undefined} />
    )
    expect(screen.getByText(`Started watching ${time(today)}`)).toBeTruthy()
    const old = NOW - 3 * 86_400_000
    rerender(<EventsCard history={historyOf([ev(NOW - 1000)], old)} filter="" now={NOW} onOpenAgent={() => undefined} />)
    const text = screen.getByText(/^Started watching/).textContent ?? ''
    expect(text).not.toBe(`Started watching ${time(old)}`)
    expect(text.endsWith(time(old))).toBe(true)
  })

  it('shows no marker on a first visit or when nothing is new', () => {
    const events = [ev(NOW - 1000), ev(NOW - 2000)]
    const { unmount } = render(<EventsCard history={historyOf(events)} filter="" now={NOW} onOpenAgent={() => undefined} />)
    expect(screen.queryByRole('separator')).toBeNull()
    expect(screen.queryByText(/new$/)).toBeNull()
    // Leaving stores the time; coming back with nothing newer shows nothing.
    unmount()
    expect(localStorage.getItem(LAST_SEEN_KEY)).not.toBeNull()
    localStorage.setItem(LAST_SEEN_KEY, String(NOW))
    render(<EventsCard history={historyOf(events)} filter="" now={NOW} onOpenAgent={() => undefined} />)
    expect(screen.queryByRole('separator')).toBeNull()
  })

  it('marks the new events and clears the marker after 10 s in view', () => {
    vi.useFakeTimers()
    let callback: IntersectionObserverCallback = () => undefined
    class FakeObserver {
      constructor(cb: IntersectionObserverCallback) {
        callback = cb
      }
      observe() {}
      disconnect() {}
      unobserve() {}
      takeRecords() {
        return []
      }
    }
    vi.stubGlobal('IntersectionObserver', FakeObserver)
    const lastSeen = NOW - 5000
    localStorage.setItem(LAST_SEEN_KEY, String(lastSeen))
    const events = [ev(NOW - 1000), ev(NOW - 2000), ev(NOW - 3000), ev(NOW - 9000)]
    render(<EventsCard history={historyOf(events)} filter="" now={NOW} onOpenAgent={() => undefined} />)

    expect(screen.getByText('3 new')).toBeTruthy()
    const separator = screen.getByRole('separator')
    expect(separator.textContent).toMatch(/^New since \d/)
    expect(Array.from(separator.parentElement?.children ?? []).indexOf(separator)).toBe(3)

    // Out of view: nothing happens.
    act(() => callback([{ isIntersecting: false } as IntersectionObserverEntry], {} as IntersectionObserver))
    act(() => {
      vi.advanceTimersByTime(20_000)
    })
    expect(screen.queryByRole('separator')).not.toBeNull()

    // In view for 9 s, then out, then in again: the 10 s start over.
    act(() => callback([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver))
    act(() => {
      vi.advanceTimersByTime(9000)
    })
    act(() => callback([{ isIntersecting: false } as IntersectionObserverEntry], {} as IntersectionObserver))
    act(() => {
      vi.advanceTimersByTime(5000)
    })
    expect(screen.queryByRole('separator')).not.toBeNull()
    act(() => callback([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver))
    act(() => {
      vi.advanceTimersByTime(10_000)
    })
    expect(screen.queryByRole('separator')).toBeNull()
    expect(screen.queryByText('3 new')).toBeNull()
  })
})
