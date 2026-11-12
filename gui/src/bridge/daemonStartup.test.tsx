// @vitest-environment jsdom
import { act, cleanup, render } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { DaemonBanner } from '../components/Daemon/DaemonBanner'
import { createBridge } from './bridge'
import { createDaemonTransport, type DaemonTransport } from './daemonApi'
import { createHydraApi } from './hydraApi'
import { createMockTransport } from './mockTransport'
import type { DaemonStatus } from './protocol'
import { FakeRaw } from './testKit'

let transport: DaemonTransport | undefined

afterEach(() => {
  cleanup()
  transport?.dispose()
})

function mount() {
  const raw = new FakeRaw()
  let resolve!: (status: DaemonStatus) => void
  let reject!: (error: Error) => void
  const promise = new Promise<DaemonStatus>((yes, no) => { resolve = yes; reject = no })
  const initial = { promise, resolve, reject }
  raw.status = () => initial.promise
  transport = createDaemonTransport({ raw, fallback: createMockTransport(), storage: null, keybindingsPollMs: 0, fsPollMs: 0 })
  window.hydra = createHydraApi(createBridge(transport))
  const reload = vi.fn()
  Object.defineProperty(window, 'location', { configurable: true, value: { ...window.location, reload } })
  const view = render(<DaemonBanner />)
  return { raw, initial, reload, view }
}

describe('daemon startup with delayed native IPC', () => {
  it('never reloads an already-connected daemon, including after a page reload', async () => {
    for (let page = 0; page < 3; page++) {
      const { initial, reload, view } = mount()
      await act(async () => undefined)
      // Unknown status must not be presented as evidence that the daemon is disconnected.
      expect(view.queryByTestId('daemon-banner')).toBeNull()
      await act(async () => initial.resolve({ connected: true }))
      expect(reload).not.toHaveBeenCalled()
      view.unmount()
      transport?.dispose()
    }
  })

  it('reloads only once when the daemon really starts after the page', async () => {
    const { raw, initial, reload, view } = mount()
    await act(async () => initial.resolve({ connected: false, error: 'connecting to the daemon' }))
    expect(view.getByTestId('daemon-banner')).toBeTruthy()
    await act(async () => raw.setStatus({ connected: true }))
    await act(async () => raw.setStatus({ connected: true, warning: 'version mismatch' }))
    expect(reload).toHaveBeenCalledTimes(1)
  })

  it('uses a live event immediately and ignores an older delayed status response', async () => {
    const { raw, initial, reload, view } = mount()
    await act(async () => raw.setStatus({ connected: true, warning: 'version mismatch' }))
    expect(await window.hydra.getDaemonStatus()).toEqual({ connected: true, warning: 'version mismatch' })
    await act(async () => initial.resolve({ connected: false, error: 'connecting to the daemon' }))
    expect(await window.hydra.getDaemonStatus()).toEqual({ connected: true, warning: 'version mismatch' })
    expect(view.getByTestId('daemon-banner').textContent).toContain('version mismatch')
    expect(reload).not.toHaveBeenCalled()
  })

  it('reports an initial status failure and recovers when a real connection arrives', async () => {
    const { raw, initial, reload, view } = mount()
    await act(async () => initial.reject(new Error('status IPC failed')))
    expect(view.getByTestId('daemon-banner').textContent).toContain('status IPC failed')
    await act(async () => raw.setStatus({ connected: true }))
    expect(reload).toHaveBeenCalledTimes(1)
  })
})
