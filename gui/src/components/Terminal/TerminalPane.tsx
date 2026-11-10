import { useEffect, useRef, useState } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { ImageAddon } from '@xterm/addon-image'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { WebglAddon } from '@xterm/addon-webgl'
import '@xterm/xterm/css/xterm.css'
import { useTerminalConfig } from '../../hooks/useTerminalConfig'
import { installMiddleClickPasteGuard } from './middleClickPasteGuard'
import { createFitScheduler, hasUsableBox, isUsableGrid, shouldNotify } from './terminalFit'

interface TerminalPaneProps {
  rawOutput: string
  onData?: (data: string) => void
  onResize?: (cols: number, rows: number) => void
  className?: string
  fontSize?: number
  lineHeight?: number
  autoFocus?: boolean
  isVisible?: boolean
}

function getTerminalTheme(): Record<string, string> {
  const s = getComputedStyle(document.documentElement)
  const v = (name: string, fallback: string) => s.getPropertyValue(name).trim() || fallback
  const bg = v('--color-terminal-bg', '#262624')
  const fg = v('--color-terminal-text', '#E8E4E0')
  return {
    background: bg,
    foreground: fg,
    cursor: v('--color-terminal-cursor', '#D97757'),
    cursorAccent: bg,
    selectionBackground: v('--color-terminal-selection', 'rgba(217, 119, 87, 0.3)'),
    selectionForeground: fg,
    black: bg,
    red: '#E53935',
    green: '#66BB6A',
    yellow: '#FFA726',
    blue: '#42A5F5',
    magenta: '#AB47BC',
    cyan: '#26C6DA',
    white: fg,
    brightBlack: '#6B6760',
    brightRed: '#EF5350',
    brightGreen: '#81C784',
    brightYellow: '#FFB74D',
    brightBlue: '#64B5F6',
    brightMagenta: '#CE93D8',
    brightCyan: '#4DD0E1',
    brightWhite: '#FAF6F1'
  }
}

export function TerminalPane({
  rawOutput,
  onData,
  onResize,
  className,
  fontSize,
  lineHeight = 1.35,
  autoFocus = true,
  isVisible = true
}: TerminalPaneProps) {
  const terminalConfig = useTerminalConfig()
  const effectiveFontSize = fontSize ?? terminalConfig.fontSize
  const effectiveFontFamily = terminalConfig.fontFamily
  const effectiveCursorStyle = terminalConfig.cursorStyle
  const effectiveCursorBlink = terminalConfig.cursorBlink
  const effectiveEnableWebgl = terminalConfig.enableWebgl
  const wrapperRef = useRef<HTMLDivElement>(null)
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalRef = useRef<Terminal | null>(null)
  const fitAddonRef = useRef<FitAddon | null>(null)
  const fitAndNotifyRef = useRef<((force?: boolean) => void) | null>(null)
  const onDataRef = useRef(onData)
  const onResizeRef = useRef(onResize)
  const writtenLengthRef = useRef(0)
  const targetOutputRef = useRef(rawOutput)
  const writeInFlightRef = useRef(false)
  // Writes whatever output the current xterm has not seen yet (set by the output effect below).
  const flushOutputRef = useRef<() => void>(() => undefined)
  // False until the new xterm has been fitted once. The saved output is only replayed at the real size: written into xterm's
  // default 80x24 it wraps, the fit then reflows it into garbage, and the agent never redraws because its PTY size did not
  // change (no SIGWINCH). That is the "terminal looks resized" bug after Overview -> Chat (WebKit lost the race more often).
  const outputReadyRef = useRef(false)
  const lastSizeRef = useRef<{ cols: number; rows: number } | null>(null)
  const [isDragOver, setIsDragOver] = useState(false)
  const dragCounterRef = useRef(0)
  const [isScrolledUp, setIsScrolledUp] = useState(false)

  useEffect(() => {
    onDataRef.current = onData
  }, [onData])

  useEffect(() => {
    onResizeRef.current = onResize
  }, [onResize])

  useEffect(() => {
    if (!containerRef.current) return

    const terminal = new Terminal({
      theme: getTerminalTheme(),
      fontFamily: effectiveFontFamily,
      fontSize: effectiveFontSize,
      lineHeight,
      cursorBlink: effectiveCursorBlink,
      cursorStyle: effectiveCursorStyle,
      disableStdin: !onData,
      scrollback: 5000,
      allowProposedApi: true,
      // Preserve raw terminal newline/cursor semantics for TUI apps like Codex.
      convertEol: false
    })

    const fitAddon = new FitAddon()
    fitAddonRef.current = fitAddon
    const imageAddon = new ImageAddon()
    const webLinksAddon = new WebLinksAddon((event, uri) => {
      event.preventDefault()
      window.hydra.openExternal(uri).catch(() => {})
    })
    const unicode11Addon = new Unicode11Addon()
    terminal.loadAddon(fitAddon)
    terminal.loadAddon(imageAddon)
    terminal.loadAddon(webLinksAddon)
    terminal.loadAddon(unicode11Addon)
    if (terminal.unicode) {
      terminal.unicode.activeVersion = '11'
    }
    terminal.open(containerRef.current)

    let webglAddon: WebglAddon | null = null
    if (effectiveEnableWebgl) {
      try {
        webglAddon = new WebglAddon()
        webglAddon.onContextLoss(() => {
          webglAddon?.dispose()
          webglAddon = null
        })
        terminal.loadAddon(webglAddon)
      } catch (err) {
        console.warn('[TerminalPane] WebGL renderer unavailable, falling back to canvas:', err)
        webglAddon = null
      }
    }

    // A new xterm (mount, font, cursor or renderer setting changed) starts empty: the output we already have is replayed by the
    // first successful fit (see `outputReadyRef`), so it is laid out at the real size. Set before any fit can run.
    terminalRef.current = terminal
    writtenLengthRef.current = 0
    writeInFlightRef.current = false
    outputReadyRef.current = false

    // `force` = pane just (re)mounted / became visible: tell the PTY our size even if it equals our last one, because another pane
    // (grid tile vs chat) may have resized the shared PTY in between.
    const runFit = (force: boolean) => {
      const container = containerRef.current
      // Hidden / mid-layout / 0x0: never fit or send a tiny size to the PTY. A later trigger will retry.
      if (!container || !hasUsableBox(container.clientWidth, container.clientHeight)) return
      const proposed = (fitAddon as { proposeDimensions?: () => { cols: number; rows: number } | undefined }).proposeDimensions?.()
      if (proposed !== undefined && !isUsableGrid(proposed)) return
      try {
        fitAddon.fit()
      } catch {
        return
      }

      const next = { cols: terminal.cols, rows: terminal.rows }
      if (!isUsableGrid(next)) return
      if (shouldNotify(lastSizeRef.current, next, force)) {
        lastSizeRef.current = next
        onResizeRef.current?.(next.cols, next.rows)
      }
      if (!outputReadyRef.current) {
        outputReadyRef.current = true
        flushOutputRef.current()
      }
    }
    const scheduler = createFitScheduler(runFit)
    const fitAndNotify = (force = false) => scheduler.schedule(force)
    fitAndNotifyRef.current = fitAndNotify

    // Fit once layout has settled (double rAF), then again after late layout/font changes. The first one is forced.
    let settleRaf = 0
    settleRaf = requestAnimationFrame(() => {
      settleRaf = requestAnimationFrame(() => {
        fitAndNotify(true)
        if (autoFocus) {
          terminal.focus()
        }
      })
    })
    const fitTimers = [80, 180, 500].map((delay) => setTimeout(() => fitAndNotify(false), delay))

    // Fonts finishing to load change the cell size without changing the container size: remeasure and refit.
    const onFontsDone = () => {
      terminal.clearTextureAtlas?.()
      fitAndNotify(false)
    }
    const fontSet = typeof document !== 'undefined' ? document.fonts : undefined
    fontSet?.ready?.then(onFontsDone, () => {})
    fontSet?.addEventListener?.('loadingdone', onFontsDone)
    const onWindowChange = () => fitAndNotify(false)
    const onVisibilityChange = () => {
      if (document.visibilityState === 'visible') fitAndNotify(true)
    }
    window.addEventListener('resize', onWindowChange)
    document.addEventListener('visibilitychange', onVisibilityChange)

    // Shift+Enter → send newline instead of carriage return
    terminal.attachCustomKeyEventHandler((e) => {
      if (e.type === 'keydown' && e.key === 'Enter' && e.shiftKey) {
        onDataRef.current?.('\n')
        return false // prevent xterm from also sending \r
      }
      return true
    })

    if (onDataRef.current) {
      terminal.onData((data) => {
        onDataRef.current?.(data)
      })
    }

    // Track whether user has scrolled up from the bottom.
    // viewportY is the top line of the visible viewport;
    // baseY is the top line of the last (bottom) page.
    // When viewportY < baseY, the user has scrolled up.
    const checkScrollPosition = () => {
      const buf = terminal.buffer.active
      setIsScrolledUp(buf.viewportY < buf.baseY)
    }
    const scrollDisposable = terminal.onScroll(checkScrollPosition)


    // Handle resize
    const observer = new ResizeObserver(() => {
      fitAndNotify(false)
    })
    observer.observe(containerRef.current)
    if (wrapperRef.current) observer.observe(wrapperRef.current)

    // Re-apply terminal theme when data-theme attribute changes
    const themeObserver = new MutationObserver(() => {
      terminal.options.theme = getTerminalTheme()
    })
    themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-theme']
    })

    // ── Native DOM listeners for Shift+Enter (capture phase fallback) ──
    // xterm creates an internal textarea that receives key events.
    // Use capture phase on the wrapper to intercept before xterm does.
    const wrapper = wrapperRef.current!
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Enter' && e.shiftKey) {
        e.preventDefault()
        e.stopPropagation()
        onDataRef.current?.('\n')
        return
      }
      // Cmd/Ctrl+Backspace → clear input line (readline kill-line, Ctrl+U).
      // macOS sends nothing for Cmd+Backspace by default; emit ^U so the
      // shell wipes from cursor to start-of-line, matching native input fields.
      if (e.key === 'Backspace' && (e.metaKey || (e.ctrlKey && !e.shiftKey && !e.altKey)) && !e.altKey) {
        e.preventDefault()
        e.stopPropagation()
        onDataRef.current?.('\u0015')
        return
      }
      // Cmd+Left / Cmd+Right → start / end of line (readline ^A / ^E).
      if (e.metaKey && !e.ctrlKey && !e.altKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
        e.preventDefault()
        e.stopPropagation()
        onDataRef.current?.(e.key === 'ArrowLeft' ? '' : '')
        return
      }
      // Option+Left / Option+Right → previous / next word (ESC b / ESC f).
      if (e.altKey && !e.metaKey && !e.ctrlKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
        e.preventDefault()
        e.stopPropagation()
        onDataRef.current?.(e.key === 'ArrowLeft' ? 'b' : 'f')
        return
      }
      // Option+Backspace → delete previous word (readline ^W).
      if (e.key === 'Backspace' && e.altKey && !e.metaKey && !e.ctrlKey) {
        e.preventDefault()
        e.stopPropagation()
        onDataRef.current?.('')
        return
      }
    }
    wrapper.addEventListener('keydown', handleKeyDown, true) // capture phase

    // Linux middle click: when the program has mouse tracking on (Claude Code),
    // it pastes the selection itself, so drop the browser's paste.
    const disposeMiddleClickPasteGuard = installMiddleClickPasteGuard(wrapper, terminal)

    // ── Native DOM listeners for drag-and-drop ──
    const handleDragEnter = (e: DragEvent) => {
      e.preventDefault()
      dragCounterRef.current++
      if (e.dataTransfer?.types.includes('Files') || e.dataTransfer?.types.includes('text/plain')) {
        setIsDragOver(true)
      }
    }
    const handleDragLeave = (e: DragEvent) => {
      e.preventDefault()
      dragCounterRef.current--
      if (dragCounterRef.current <= 0) {
        dragCounterRef.current = 0
        setIsDragOver(false)
      }
    }
    const handleDragOver = (e: DragEvent) => {
      e.preventDefault()
      if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy'
    }
    const handleDrop = (e: DragEvent) => {
      e.preventDefault()
      e.stopPropagation()
      dragCounterRef.current = 0
      setIsDragOver(false)

      // OS file drops
      const files = Array.from(e.dataTransfer?.files ?? [])
      const paths = files.map((f) => f.path).filter(Boolean)
      if (paths.length > 0) {
        onDataRef.current?.(paths.join(' '))
        return
      }
      // Internal editor drag (text/plain with file path)
      const textData = e.dataTransfer?.getData('text/plain')
      if (textData) {
        onDataRef.current?.(textData)
      }
    }

    wrapper.addEventListener('dragenter', handleDragEnter, true)
    wrapper.addEventListener('dragleave', handleDragLeave, true)
    wrapper.addEventListener('dragover', handleDragOver, true)
    wrapper.addEventListener('drop', handleDrop, true)

    return () => {
      for (const timer of fitTimers) {
        clearTimeout(timer)
      }
      cancelAnimationFrame(settleRaf)
      scheduler.dispose()
      fontSet?.removeEventListener?.('loadingdone', onFontsDone)
      window.removeEventListener('resize', onWindowChange)
      document.removeEventListener('visibilitychange', onVisibilityChange)
      scrollDisposable.dispose()
      observer.disconnect()
      themeObserver.disconnect()
      wrapper.removeEventListener('keydown', handleKeyDown, true)
      disposeMiddleClickPasteGuard()
      wrapper.removeEventListener('dragenter', handleDragEnter, true)
      wrapper.removeEventListener('dragleave', handleDragLeave, true)
      wrapper.removeEventListener('dragover', handleDragOver, true)
      wrapper.removeEventListener('drop', handleDrop, true)
      webglAddon?.dispose()
      terminal.dispose()
      terminalRef.current = null
      fitAddonRef.current = null
      fitAndNotifyRef.current = null
    }
  }, [
    autoFocus,
    effectiveFontSize,
    effectiveFontFamily,
    effectiveCursorStyle,
    effectiveCursorBlink,
    effectiveEnableWebgl,
    lineHeight
  ])

  useEffect(() => {
    if (!isVisible) return
    // Double rAF: let the layout of the newly shown view settle before measuring.
    let raf2 = 0
    const raf1 = requestAnimationFrame(() => {
      raf2 = requestAnimationFrame(() => {
        fitAndNotifyRef.current?.(true)
        if (autoFocus) {
          terminalRef.current?.focus()
        }
      })
    })
    return () => {
      cancelAnimationFrame(raf1)
      cancelAnimationFrame(raf2)
    }
  }, [autoFocus, isVisible])

  // Write new output incrementally
  useEffect(() => {
    targetOutputRef.current = rawOutput

    const flushPendingOutput = () => {
      const terminal = terminalRef.current
      if (!terminal || !outputReadyRef.current || writeInFlightRef.current) return

      const targetOutput = targetOutputRef.current
      if (targetOutput.length < writtenLengthRef.current) {
        terminal.reset()
        writtenLengthRef.current = 0
      }

      if (targetOutput.length <= writtenLengthRef.current) return

      const newData = targetOutput.slice(writtenLengthRef.current)
      writeInFlightRef.current = true
      terminal.write(newData, () => {
        writtenLengthRef.current += newData.length
        writeInFlightRef.current = false

        // After xterm finishes writing, re-check scroll position.
        // New output may have pushed past viewport if user scrolled up.
        const buf = terminal.buffer.active
        setIsScrolledUp(buf.viewportY < buf.baseY)

        flushPendingOutput()
      })
    }

    flushOutputRef.current = flushPendingOutput
    flushPendingOutput()
  }, [rawOutput])

  return (
    <div
      ref={wrapperRef}
      className={className}
      onMouseDown={() => terminalRef.current?.focus()}
      // Wheel scrolling used to be what "repaired" a stale fit: keep a cheap deduped refit on it as a safety net.
      onWheel={() => fitAndNotifyRef.current?.(false)}
      style={{
        width: '100%',
        height: '100%',
        background: 'var(--color-terminal-bg)',
        borderRadius: 'inherit',
        overflow: 'hidden',
        position: 'relative',
        minWidth: 0,
        minHeight: 0
      }}
    >
      <div
        ref={containerRef}
        style={{ width: '100%', height: '100%', minWidth: 0, minHeight: 0 }}
      />
      {isScrolledUp && (
        <button
          onClick={() => {
            terminalRef.current?.scrollToBottom()
            setIsScrolledUp(false)
          }}
          style={{
            position: 'absolute',
            bottom: 8,
            right: 16,
            zIndex: 10,
            display: 'flex',
            alignItems: 'center',
            gap: 4,
            padding: '4px 10px',
            fontSize: '11px',
            fontWeight: 500,
            color: 'var(--color-text-primary)',
            background: 'var(--color-surface)',
            border: '1px solid var(--color-border)',
            borderRadius: '12px',
            cursor: 'pointer',
            backdropFilter: 'blur(6px)',
            opacity: 0.92,
            transition: 'opacity 150ms'
          }}
          onMouseEnter={(e) => { e.currentTarget.style.opacity = '1' }}
          onMouseLeave={(e) => { e.currentTarget.style.opacity = '0.92' }}
        >
          <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
            <path d="M8 3v10M4 9l4 4 4-4" />
          </svg>
          Bottom
        </button>
      )}
      {isDragOver && (
        <div
          style={{
            position: 'absolute',
            inset: 0,
            background: 'var(--color-accent-subtle)',
            border: '2px dashed var(--color-accent-hover)',
            borderRadius: 'inherit',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            zIndex: 10,
            pointerEvents: 'none'
          }}
        >
          <span
            style={{
              color: 'var(--color-accent)',
              fontSize: '13px',
              fontWeight: 500,
              background: 'var(--color-surface)',
              padding: '6px 14px',
              borderRadius: '6px'
            }}
          >
            Drop to insert file path
          </span>
        </div>
      )}
    </div>
  )
}
