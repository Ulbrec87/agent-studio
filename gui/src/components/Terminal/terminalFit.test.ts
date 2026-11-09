import { describe, expect, it, vi } from 'vitest'
import { createFitScheduler, hasUsableBox, isUsableGrid, shouldNotify } from './terminalFit'

describe('terminalFit', () => {
  it('rejects zero-size boxes and tiny grids', () => {
    expect(hasUsableBox(0, 100)).toBe(false)
    expect(hasUsableBox(100, 0)).toBe(false)
    expect(hasUsableBox(300, 200)).toBe(true)
    expect(isUsableGrid(undefined)).toBe(false)
    expect(isUsableGrid({ cols: 1, rows: 24 })).toBe(false)
    expect(isUsableGrid({ cols: 80, rows: NaN })).toBe(false)
    expect(isUsableGrid({ cols: 80, rows: 24 })).toBe(true)
  })

  it('notifies on first size, on change, and when forced; dedupes otherwise', () => {
    const s = { cols: 80, rows: 24 }
    expect(shouldNotify(null, s, false)).toBe(true)
    expect(shouldNotify(s, { ...s }, false)).toBe(false)
    expect(shouldNotify(s, { cols: 81, rows: 24 }, false)).toBe(true)
    expect(shouldNotify(s, { ...s }, true)).toBe(true)
  })

  it('coalesces requests per frame and lets force win', () => {
    const frames: Array<() => void> = []
    const run = vi.fn()
    const s = createFitScheduler(run, (cb) => frames.push(cb), vi.fn())
    s.schedule()
    s.schedule(true)
    s.schedule()
    expect(frames).toHaveLength(1)
    frames.shift()?.()
    expect(run).toHaveBeenCalledTimes(1)
    expect(run).toHaveBeenLastCalledWith(true)
    s.schedule()
    frames.shift()?.()
    expect(run).toHaveBeenLastCalledWith(false)
  })

  it('does nothing after dispose', () => {
    const frames: Array<() => void> = []
    const run = vi.fn()
    const caf = vi.fn()
    const s = createFitScheduler(run, (cb) => frames.push(cb), caf)
    s.schedule()
    s.dispose()
    expect(caf).toHaveBeenCalled()
    frames.shift()?.()
    s.schedule()
    expect(run).not.toHaveBeenCalled()
    expect(frames).toHaveLength(0)
  })
})
