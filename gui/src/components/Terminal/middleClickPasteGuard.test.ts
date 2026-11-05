// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installMiddleClickPasteGuard, MIDDLE_CLICK_PASTE_WINDOW_MS } from './middleClickPasteGuard'

describe('installMiddleClickPasteGuard', () => {
  let wrapper: HTMLDivElement
  let textarea: HTMLTextAreaElement
  let terminal: { modes: { mouseTrackingMode: string } }
  let clock: number
  let xtermPaste: ReturnType<typeof vi.fn>
  let dispose: () => void

  const press = (type: 'mousedown' | 'mouseup', button: number, shiftKey = false) =>
    textarea.dispatchEvent(new MouseEvent(type, { button, shiftKey, bubbles: true }))
  const middleClick = (shiftKey = false) => {
    press('mousedown', 1, shiftKey)
    press('mouseup', 1, shiftKey)
  }
  const paste = () => {
    const event = new Event('paste', { bubbles: true, cancelable: true })
    textarea.dispatchEvent(event)
    return event
  }

  beforeEach(() => {
    wrapper = document.createElement('div')
    textarea = document.createElement('textarea')
    wrapper.appendChild(textarea)
    document.body.appendChild(wrapper)
    // Stands in for xterm.js's own paste listener on its textarea.
    xtermPaste = vi.fn()
    textarea.addEventListener('paste', xtermPaste)
    terminal = { modes: { mouseTrackingMode: 'vt200' } }
    clock = 1_000
    dispose = installMiddleClickPasteGuard(wrapper, terminal, () => clock)
  })

  afterEach(() => {
    dispose()
    wrapper.remove()
  })

  it('drops the browser paste after a middle click when mouse tracking is on', () => {
    middleClick()
    const event = paste()

    expect(xtermPaste).not.toHaveBeenCalled()
    expect(event.defaultPrevented).toBe(true)
  })

  it('drops only one paste per middle click', () => {
    middleClick()
    paste()
    paste()

    expect(xtermPaste).toHaveBeenCalledTimes(1)
  })

  it('keeps the browser paste when mouse tracking is off', () => {
    terminal.modes.mouseTrackingMode = 'none'
    middleClick()
    paste()

    expect(xtermPaste).toHaveBeenCalledTimes(1)
  })

  it('keeps the browser paste on Shift+middle-click', () => {
    middleClick(true)
    paste()

    expect(xtermPaste).toHaveBeenCalledTimes(1)
  })

  it('keeps keyboard pastes and pastes after other buttons', () => {
    paste()
    press('mousedown', 0)
    press('mouseup', 0)
    paste()

    expect(xtermPaste).toHaveBeenCalledTimes(2)
  })

  it('keeps a paste that comes long after the middle click', () => {
    middleClick()
    clock += MIDDLE_CLICK_PASTE_WINDOW_MS + 1
    paste()

    expect(xtermPaste).toHaveBeenCalledTimes(1)
  })

  it('stops guarding once disposed', () => {
    dispose()
    middleClick()
    paste()

    expect(xtermPaste).toHaveBeenCalledTimes(1)
  })
})
