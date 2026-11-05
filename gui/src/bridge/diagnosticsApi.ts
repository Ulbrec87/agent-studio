// Diagnostics export and the renderer's structured log.
//
// `exportDiagnostics` = daemon `ExportDiagnostics` (a folder with the newest logs and a redacted snapshot, secrets removed by the daemon);
// the desktop app then shows the folder in the file manager (a browser has no way to). `logEvent` goes to the GUI log file
// (`<data>/logs/agent-studio-gui.<date>`, JSON lines) through the shell's `log_event`; the browser dev bridge forwards it to the same
// file, a plain browser (no shell, no bridge) only writes the console. The shell redacts credentials and caps sizes (`gui_log.rs`).
import type { ExportDiagnosticsResult, ObservabilityLogEventPayload } from '@shared/types'
import type { BridgeContext, MethodGroup } from './context'
import { toError } from './protocol'

export const DIAGNOSTICS_METHODS = ['exportDiagnostics', 'logEvent'] as const

export interface DiagnosticsDeps {
  ctx: BridgeContext
  /** Show a folder to the user (the native group's `openPath`); only used in the desktop app. */
  reveal?: (path: string) => Promise<unknown>
}

export function createDiagnosticsMethods({ ctx, reveal }: DiagnosticsDeps): MethodGroup {
  return {
    methods: {
      exportDiagnostics: (async (): Promise<ExportDiagnosticsResult> => {
        try {
          const path = await ctx.call<string>('export_diagnostics', undefined, 'path')
          // Only the desktop app (shell with its own "open in") reveals the folder; never from a browser tab.
          if (reveal && ctx.shell?.supports('open_in')) void reveal(path).catch(() => undefined)
          return { path, error: null }
        } catch (err) {
          return { path: null, error: toError(err).message }
        }
      }) as never,
      logEvent: ((event: ObservabilityLogEventPayload) => {
        const shell = ctx.shell
        if (shell?.supports('log_event')) {
          return shell.invoke('log_event', { event }).then(
            () => undefined,
            () => undefined // best effort: a lost log line must never reach the UI
          )
        }
        const level = event.level === 'error' ? 'error' : event.level === 'warn' ? 'warn' : 'debug'
        console[level]('[renderer]', event.event, event.message ?? '')
        return undefined
      }) as never
    }
  }
}
