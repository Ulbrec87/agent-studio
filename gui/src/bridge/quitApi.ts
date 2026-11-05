// The quit flow. Closing the desktop window while agents run makes the shell emit `confirm-quit` (with the number of running agents);
// the renderer's dialog answers with `confirmQuit()` (stop every agent and the daemon, then exit) or `quitBackground()` (exit the
// window only; the daemon and the agents keep running). With nothing running the shell closes the window without asking.
// A browser (dev bridge) has no window to close: the methods answer false and the event never fires.
import type { BridgeContext, MethodGroup } from './context'

export const QUIT_METHODS = ['confirmQuit', 'quitBackground'] as const
export const QUIT_CHANNELS = ['onConfirmQuit'] as const

export function createQuitMethods(ctx: BridgeContext): MethodGroup {
  const quit = async (stopAgents: boolean): Promise<boolean> => {
    const shell = ctx.shell
    if (!shell?.supports('quit_app')) return false
    try {
      await shell.invoke('quit_app', { stopAgents })
      return true
    } catch (err) {
      console.warn('[bridge] quit failed', err)
      return false
    }
  }
  let off: (() => void) | undefined
  if (ctx.shell) {
    off = ctx.shell.on('confirm-quit', (running) => {
      const count = Number(running)
      ctx.emit('onConfirmQuit', Number.isFinite(count) ? count : 0)
    })
  }
  return {
    methods: {
      confirmQuit: (() => quit(true)) as never,
      quitBackground: (() => quit(false)) as never
    },
    dispose() {
      off?.()
    }
  }
}
