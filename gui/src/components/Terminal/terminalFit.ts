export interface TermSize {
  cols: number
  rows: number
}

/** Smallest grid worth sending to a PTY; anything below means the container was mid-layout / hidden. */
export const MIN_COLS = 2
export const MIN_ROWS = 2

/** True when the container has a real box (not display:none / 0x0 / mid-layout). */
export function hasUsableBox(width: number, height: number): boolean {
  return width > 0 && height > 0
}

/** True when a proposed or fitted grid is plausible to send to the PTY. */
export function isUsableGrid(size: Partial<TermSize> | null | undefined): size is TermSize {
  return (
    !!size &&
    Number.isFinite(size.cols) &&
    Number.isFinite(size.rows) &&
    (size.cols as number) >= MIN_COLS &&
    (size.rows as number) >= MIN_ROWS
  )
}

/**
 * Whether the PTY must be told about `next`. `force` is used when a pane (re)mounts or becomes visible: another pane (grid tile,
 * chat view) may have resized the shared PTY meanwhile, so our own "last size" can no longer be trusted.
 */
export function shouldNotify(last: TermSize | null, next: TermSize, force: boolean): boolean {
  if (force || !last) return true
  return last.cols !== next.cols || last.rows !== next.rows
}

/**
 * Coalesces fit requests into one per animation frame. A forced request wins over non-forced ones queued in the same frame.
 * `run(force)` is invoked at most once per frame.
 */
export function createFitScheduler(
  run: (force: boolean) => void,
  raf: (cb: () => void) => number = (cb) => requestAnimationFrame(cb),
  caf: (id: number) => void = (id) => cancelAnimationFrame(id)
) {
  let pending = false
  let forceNext = false
  let id = 0
  let disposed = false
  return {
    schedule(force = false) {
      if (disposed) return
      if (force) forceNext = true
      if (pending) return
      pending = true
      id = raf(() => {
        pending = false
        const f = forceNext
        forceNext = false
        if (!disposed) run(f)
      })
    },
    dispose() {
      disposed = true
      if (pending) caf(id)
      pending = false
    }
  }
}
