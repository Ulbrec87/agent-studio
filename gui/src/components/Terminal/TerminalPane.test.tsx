// @vitest-environment jsdom

import { render } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { TerminalPane } from './TerminalPane'

const { terminalState } = vi.hoisted(() => ({
  terminalState: {
    instances: [] as Array<{
      options: Record<string, unknown>
      write: ReturnType<typeof vi.fn>
      writeCallbacks: Array<() => void>
      reset: ReturnType<typeof vi.fn>
      dispose: ReturnType<typeof vi.fn>
      loadAddon: ReturnType<typeof vi.fn>
      open: ReturnType<typeof vi.fn>
      focus: ReturnType<typeof vi.fn>
      onDataHandler: ((data: string) => void) | null
      onData: (handler: (data: string) => void) => void
    }>
  }
}))

vi.mock('@xterm/xterm', () => ({
  Terminal: class MockTerminal {
    cols = 80
    rows = 24
    options: Record<string, unknown>
    writeCallbacks: Array<() => void> = []
    write = vi.fn((_: string, callback?: () => void) => {
      if (callback) this.writeCallbacks.push(callback)
    })
    reset = vi.fn()
    dispose = vi.fn()
    loadAddon = vi.fn()
    open = vi.fn()
    attachCustomKeyEventHandler = vi.fn()
    scrollToBottom = vi.fn()
    focus = vi.fn()
    buffer = { active: { baseY: 0, viewportY: 0, length: 24 } }
    onDataHandler: ((data: string) => void) | null = null

    constructor(options: Record<string, unknown> = {}) {
      this.options = options
      terminalState.instances.push(this)
    }

    onData(handler: (data: string) => void) {
      this.onDataHandler = handler
    }

    onScroll() {
      return { dispose: vi.fn() }
    }
  }
}))

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class MockFitAddon {
    fit = vi.fn()
  }
}))

vi.mock('@xterm/addon-image', () => ({
  ImageAddon: class MockImageAddon {}
}))

vi.mock('@xterm/addon-web-links', () => ({
  WebLinksAddon: class MockWebLinksAddon {}
}))

describe('TerminalPane', () => {
  beforeEach(() => {
    terminalState.instances = []
    vi.clearAllMocks()

    globalThis.ResizeObserver = class {
      observe() {}
      disconnect() {}
      unobserve() {}
    } as unknown as typeof ResizeObserver

    globalThis.requestAnimationFrame = (cb: FrameRequestCallback) => {
      cb(0)
      return 1
    }
    globalThis.cancelAnimationFrame = () => {}
    // jsdom has no layout: give every element a real box so the zero-size guard lets fits through.
    Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 800 })
    Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 600 })
  })

  it('writes output incrementally and resets when output shrinks', () => {
    const { rerender } = render(<TerminalPane rawOutput="hello" />)
    const term = terminalState.instances[0]

    expect(term.write).toHaveBeenCalledWith('hello', expect.any(Function))
    term.writeCallbacks.shift()?.()

    rerender(<TerminalPane rawOutput="hello world" />)
    expect(term.write).toHaveBeenLastCalledWith(' world', expect.any(Function))
    term.writeCallbacks.shift()?.()

    rerender(<TerminalPane rawOutput="x" />)
    expect(term.reset).toHaveBeenCalledTimes(1)
    expect(term.write).toHaveBeenLastCalledWith('x', expect.any(Function))
  })

  it('replays the output into the new xterm when a setting recreates it (it used to stay blank until the next chunk)', () => {
    const { rerender } = render(<TerminalPane rawOutput="hello world" fontSize={12} />)
    const first = terminalState.instances[0]
    first.writeCallbacks.shift()?.()
    expect(first.write).toHaveBeenCalledWith('hello world', expect.any(Function))

    rerender(<TerminalPane rawOutput="hello world" fontSize={16} />)
    expect(first.dispose).toHaveBeenCalled()
    const second = terminalState.instances[1]
    expect(second).toBeDefined()
    expect(second.options.fontSize).toBe(16)
    expect(second.write).toHaveBeenCalledWith('hello world', expect.any(Function))
  })

  it('replays saved output only after the first fit, so it is not laid out at the default 80x24', () => {
    // No box yet (hidden / mid view switch): fits are skipped, so nothing may be written.
    Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 0 })
    const { rerender } = render(<TerminalPane rawOutput="frame" isVisible={false} />)
    const term = terminalState.instances[0]
    expect(term.write).not.toHaveBeenCalled()

    // The pane gets its real size and becomes visible: the fit runs, then the output is replayed.
    Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 800 })
    rerender(<TerminalPane rawOutput="frame" isVisible />)
    expect(term.write).toHaveBeenCalledWith('frame', expect.any(Function))
  })

  it('queues later output until the active xterm write finishes', () => {
    const { rerender } = render(<TerminalPane rawOutput="hello" />)
    const term = terminalState.instances[0]

    expect(term.write).toHaveBeenCalledTimes(1)
    rerender(<TerminalPane rawOutput="hello world" />)
    expect(term.write).toHaveBeenCalledTimes(1)

    term.writeCallbacks.shift()?.()
    expect(term.write).toHaveBeenCalledTimes(2)
    expect(term.write).toHaveBeenLastCalledWith(' world', expect.any(Function))
  })

  it('forwards terminal keystrokes through onData', () => {
    const onData = vi.fn()
    render(<TerminalPane rawOutput="" onData={onData} />)
    const term = terminalState.instances[0]

    term.onDataHandler?.('\r')
    term.onDataHandler?.('a')

    expect(onData).toHaveBeenNthCalledWith(1, '\r')
    expect(onData).toHaveBeenNthCalledWith(2, 'a')
  })

  it('preserves raw line-ending semantics for TUI apps', () => {
    render(<TerminalPane rawOutput="" />)
    const term = terminalState.instances[0]

    expect(term.options.convertEol).toBe(false)
  })

  it('notifies terminal size changes through onResize', () => {
    const onResize = vi.fn()
    render(<TerminalPane rawOutput="" onResize={onResize} />)
    expect(onResize).toHaveBeenCalledWith(80, 24)
  })

  it('re-notifies the PTY size when the pane becomes visible again, even if unchanged (shared PTY may have been resized)', () => {
    const onResize = vi.fn()
    const { rerender } = render(<TerminalPane rawOutput="" onResize={onResize} isVisible={false} />)
    onResize.mockClear()
    rerender(<TerminalPane rawOutput="" onResize={onResize} isVisible />)
    expect(onResize).toHaveBeenCalledWith(80, 24)
  })

  it('never notifies the PTY while the container has no box', () => {
    Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 0 })
    const onResize = vi.fn()
    render(<TerminalPane rawOutput="" onResize={onResize} />)
    expect(onResize).not.toHaveBeenCalled()
  })

  it('does not autofocus when disabled', () => {
    render(<TerminalPane rawOutput="" autoFocus={false} />)
    const term = terminalState.instances[0]

    expect(term.focus).not.toHaveBeenCalled()
  })
})
