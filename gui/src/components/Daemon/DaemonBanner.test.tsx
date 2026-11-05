// @vitest-environment jsdom
import { act, cleanup, render } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { DaemonBanner } from './DaemonBanner'

type Status = { connected: boolean; error?: string }

function setup(initial: Status) {
  let listener: ((s: Status) => void) | null = null
  const reload = vi.fn()
  Object.defineProperty(window, 'location', { configurable: true, value: { ...window.location, reload } })
  window.hydra = {
    getDaemonStatus: () => Promise.resolve(initial),
    onDaemonStatus: (cb: (s: Status) => void) => {
      listener = cb
      return () => undefined
    }
  } as unknown as typeof window.hydra
  const view = render(<DaemonBanner />)
  return { view, reload, push: (s: Status) => act(async () => listener?.(s)) }
}

afterEach(cleanup)

describe('DaemonBanner', () => {
  it('reloads once when the page started without a daemon and it comes up', async () => {
    const { reload, push } = setup({ connected: false, error: 'connecting to the daemon' })
    await act(async () => undefined)
    await push({ connected: true })
    expect(reload).toHaveBeenCalledTimes(1)
  })

  it('a drop after the page was connected never reloads it (open panels and terminals survive a daemon restart)', async () => {
    const { reload, push, view } = setup({ connected: true })
    await act(async () => undefined)
    await push({ connected: false, error: 'connection lost' })
    expect(view.getByTestId('daemon-banner').textContent).toContain('Daemon not reachable')
    await push({ connected: true })
    expect(reload).not.toHaveBeenCalled()
    expect(view.queryByTestId('daemon-banner')).toBeNull()
  })
})
