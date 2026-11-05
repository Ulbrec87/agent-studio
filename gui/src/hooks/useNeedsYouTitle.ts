import { useEffect } from 'react'

/** Prefixes the window title with the number of agents that need the user, e.g. `(2) Agent Studio`. */
export function useNeedsYouTitle(count: number): void {
  useEffect(() => {
    if (count <= 0) return
    const original = document.title
    document.title = `(${count}) ${original}`
    return () => {
      document.title = original
    }
  }, [count])
}
