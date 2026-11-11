// @vitest-environment jsdom
import { act, cleanup, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { APPEARANCE_STORAGE_KEY, useAppearance } from './useAppearance'

describe('renderer appearance', () => {
  beforeEach(() => localStorage.clear())
  afterEach(() => { cleanup(); vi.restoreAllMocks() })

  it('follows daemon themes when there is no local override', () => {
    const save = vi.fn()
    const { result, rerender } = renderHook(({ theme }) => useAppearance(theme, save), {
      initialProps: { theme: 'midnight' as 'midnight' | 'light' }
    })
    expect(result.current.theme).toBe('midnight')
    rerender({ theme: 'light' })
    expect(document.documentElement.dataset.theme).toBe('light')
    expect(save).not.toHaveBeenCalled()
  })

  it('persists Soft across reloads without sending an unknown theme to IPC', async () => {
    const save = vi.fn()
    const first = renderHook(() => useAppearance('midnight', save))
    await act(() => first.result.current.selectTheme('modern-workspace-soft'))
    expect(save).not.toHaveBeenCalled()
    expect(localStorage.getItem(APPEARANCE_STORAGE_KEY)).toBe('modern-workspace-soft')
    first.unmount()
    const next = renderHook(() => useAppearance('dark', save))
    expect(next.result.current.theme).toBe('modern-workspace-soft')
    expect(document.documentElement.dataset.theme).toBe('modern-workspace-soft')
  })

  it('keeps Soft during unrelated config changes, then clears it after a successful legacy save', async () => {
    localStorage.setItem(APPEARANCE_STORAGE_KEY, 'modern-workspace-soft')
    const save = vi.fn().mockResolvedValue(undefined)
    const { result, rerender } = renderHook(({ theme }) => useAppearance(theme, save), {
      initialProps: { theme: 'midnight' as 'midnight' | 'light' }
    })
    rerender({ theme: 'light' })
    expect(result.current.theme).toBe('modern-workspace-soft')
    await act(() => result.current.selectTheme('light'))
    expect(save).toHaveBeenCalledTimes(1)
    expect(save).toHaveBeenCalledWith({ theme: 'light' })
    expect(localStorage.getItem(APPEARANCE_STORAGE_KEY)).toBeNull()
    expect(result.current.theme).toBe('light')
  })

  it('retains the selection and reports a failed config save', async () => {
    localStorage.setItem(APPEARANCE_STORAGE_KEY, 'modern-workspace-soft')
    const save = vi.fn().mockRejectedValue(new Error('Disconnected'))
    const { result } = renderHook(() => useAppearance('midnight', save))
    await act(() => result.current.selectTheme('light'))
    expect(result.current.theme).toBe('modern-workspace-soft')
    expect(result.current.error).toBe('Disconnected')
    expect(localStorage.getItem(APPEARANCE_STORAGE_KEY)).toBe('modern-workspace-soft')
  })

  it('does not let a delayed old save undo a newer Soft selection', async () => {
    let finish!: () => void
    const save = vi.fn(() => new Promise<void>((resolve) => { finish = resolve }))
    const { result } = renderHook(() => useAppearance('midnight', save))
    let pending!: Promise<void>
    act(() => { pending = result.current.selectTheme('light') })
    await act(() => result.current.selectTheme('modern-workspace-soft'))
    await act(async () => { finish(); await pending })
    expect(result.current.theme).toBe('modern-workspace-soft')
    expect(localStorage.getItem(APPEARANCE_STORAGE_KEY)).toBe('modern-workspace-soft')
  })

  it('ignores corrupt storage and still allows selection when storage is blocked', async () => {
    localStorage.setItem(APPEARANCE_STORAGE_KEY, '{broken')
    const { result } = renderHook(() => useAppearance('midnight', vi.fn()))
    expect(result.current.theme).toBe('midnight')
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('blocked') })
    await act(() => result.current.selectTheme('modern-workspace-soft'))
    expect(result.current.theme).toBe('modern-workspace-soft')
  })
})
