/**
 * Stops a middle click from pasting twice when the program in the terminal has
 * mouse tracking on.
 *
 * On Linux, Chromium pastes the primary selection on middle click, and xterm.js
 * sends that `paste` event to the PTY. When the running program has mouse
 * tracking on (Claude Code turns on `?1000h`/`?1006h`), xterm.js also forwards
 * the middle click to it, and Claude Code pastes the primary selection itself.
 * The text then arrives twice.
 *
 * Native terminals do not paste on middle click while mouse tracking is on; the
 * click belongs to the program. This guard does the same: it drops the browser
 * paste that follows such a click. Shift+middle-click keeps the browser paste,
 * because xterm.js does not forward Shift clicks to the program.
 */

/** The part of the xterm.js `Terminal` the guard reads. */
export interface MouseTrackingSource {
  readonly modes: { readonly mouseTrackingMode: string }
}

/** How long after the middle button is released a paste still counts as caused by it. */
export const MIDDLE_CLICK_PASTE_WINDOW_MS = 500

export function installMiddleClickPasteGuard(
  wrapper: HTMLElement,
  terminal: MouseTrackingSource,
  now: () => number = Date.now
): () => void {
  let pressed = false
  let blockUntil = 0

  const handleMouseDown = (e: MouseEvent) => {
    pressed = e.button === 1 && !e.shiftKey && terminal.modes.mouseTrackingMode !== 'none'
    blockUntil = 0
  }

  const handleMouseUp = (e: MouseEvent) => {
    if (e.button !== 1) return
    if (pressed) blockUntil = now() + MIDDLE_CLICK_PASTE_WINDOW_MS
    pressed = false
  }

  const handlePaste = (e: Event) => {
    if (blockUntil === 0) return
    const blocked = now() <= blockUntil
    blockUntil = 0
    if (!blocked) return
    e.preventDefault()
    e.stopImmediatePropagation()
  }

  // Capture phase: runs before xterm.js's own listeners on its textarea.
  wrapper.addEventListener('mousedown', handleMouseDown, true)
  wrapper.addEventListener('mouseup', handleMouseUp, true)
  wrapper.addEventListener('paste', handlePaste, true)

  return () => {
    wrapper.removeEventListener('mousedown', handleMouseDown, true)
    wrapper.removeEventListener('mouseup', handleMouseUp, true)
    wrapper.removeEventListener('paste', handlePaste, true)
  }
}
