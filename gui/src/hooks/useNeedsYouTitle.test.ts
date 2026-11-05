// @vitest-environment jsdom

import { renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'
import { useNeedsYouTitle } from './useNeedsYouTitle'
import { needsYouCount } from './useOverview'
import type { OverviewSnapshot } from '@shared/overview/types'

describe('useNeedsYouTitle', () => {
  beforeEach(() => {
    document.title = 'Agent Studio'
  })

  it('prefixes the title while agents need the user and restores it', () => {
    const { rerender, unmount } = renderHook(({ n }) => useNeedsYouTitle(n), { initialProps: { n: 0 } })
    expect(document.title).toBe('Agent Studio')
    rerender({ n: 2 })
    expect(document.title).toBe('(2) Agent Studio')
    rerender({ n: 3 })
    expect(document.title).toBe('(3) Agent Studio')
    rerender({ n: 0 })
    expect(document.title).toBe('Agent Studio')
    rerender({ n: 1 })
    unmount()
    expect(document.title).toBe('Agent Studio')
  })
})

describe('needsYouCount', () => {
  it('counts needs-you sessions and is 0 without a snapshot', () => {
    expect(needsYouCount(null)).toBe(0)
    const snap = { sessions: [{ status: 'needs-you' }, { status: 'working' }, { status: 'needs-you' }] } as OverviewSnapshot
    expect(needsYouCount(snap)).toBe(2)
  })
})
