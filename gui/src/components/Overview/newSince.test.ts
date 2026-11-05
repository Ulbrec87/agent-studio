// @vitest-environment jsdom

import { beforeEach, describe, expect, it } from 'vitest'
import type { OverviewEvent } from '@shared/overview/types'
import { LAST_SEEN_KEY, newSinceMarker, readLastSeen, writeLastSeen } from './newSince'

function ev(at: number): OverviewEvent {
  return { id: `a@${at}`, at, kind: 'status', agentId: 'a', name: 'alpha', projectDir: '/w' }
}

describe('newSinceMarker', () => {
  const events = [ev(50), ev(40), ev(30), ev(20)]

  it('has no marker on a first visit', () => {
    expect(newSinceMarker(events, null)).toBeNull()
  })

  it('has no marker when nothing is newer', () => {
    expect(newSinceMarker(events, 50)).toBeNull()
    expect(newSinceMarker([], 10)).toBeNull()
  })

  it('counts the events newer than the last visit', () => {
    expect(newSinceMarker(events, 35)).toEqual({ at: 35, count: 2, index: 2 })
  })

  it('counts all when everything is newer', () => {
    expect(newSinceMarker(events, 5)).toEqual({ at: 5, count: 4, index: 4 })
  })
})

describe('last seen storage', () => {
  beforeEach(() => localStorage.clear())

  it('round-trips and ignores garbage', () => {
    expect(readLastSeen()).toBeNull()
    writeLastSeen(1234)
    expect(readLastSeen()).toBe(1234)
    localStorage.setItem(LAST_SEEN_KEY, 'nope')
    expect(readLastSeen()).toBeNull()
  })
})
