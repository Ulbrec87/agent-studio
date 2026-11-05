// Headless runs: the provider CLI in non-interactive mode, one JSONL log per run (daemon `StartHeadlessRun`, ...). The daemon's run
// structs are Hydra's (`HeadlessRun`, `HeadlessRunLogPayload`, camelCase), so only the request envelope is mapped.
import type { HeadlessRun, HeadlessRunLogOptions, HeadlessRunLogPayload, ListHeadlessRunsOptions, StartHeadlessRunPayload } from '@shared/types'
import type { BridgeContext, MethodGroup } from './context'
import { compact } from './mappers'

export const HEADLESS_METHODS = ['startHeadlessRun', 'listHeadlessRuns', 'getHeadlessRun', 'cancelHeadlessRun', 'getHeadlessRunLog'] as const
export const HEADLESS_CHANNELS = ['onHeadlessRunEvent'] as const

export function createHeadlessMethods(ctx: BridgeContext): MethodGroup {
  return {
    methods: {
      startHeadlessRun: (async (payload: StartHeadlessRunPayload) => {
        const run = await ctx.call<HeadlessRun | null>('start_headless_run', compact({ ...payload }), 'headless_run')
        if (!run) throw new Error('The daemon did not start the run')
        return run
      }) as never,
      listHeadlessRuns: ((options?: ListHeadlessRunsOptions) =>
        ctx.call<HeadlessRun[]>('list_headless_runs', compact({ ...options }), 'headless_runs')) as never,
      getHeadlessRun: (async (runId: string) => (await ctx.call<HeadlessRun | null>('get_headless_run', { run_id: runId }, 'headless_run')) ?? null) as never,
      cancelHeadlessRun: ((runId: string) => ctx.call<boolean>('cancel_headless_run', { run_id: runId }, 'flag')) as never,
      getHeadlessRunLog: (async (runId: string, options?: HeadlessRunLogOptions) =>
        (await ctx.call<HeadlessRunLogPayload | null>('get_headless_run_log', { run_id: runId, options: compact({ ...options }) }, 'headless_run_log')) ?? null) as never
    },
    onEvent(type, payload) {
      if (type === 'headless_run_event') {
        ctx.emit('onHeadlessRunEvent', { runId: String(payload.run_id), data: String(payload.data ?? '') })
        return true
      }
      if (type === 'headless_run_changed') {
        // Hydra only announces output lines. A state change (started, session id captured, finished) is an event with no data, so
        // the panel refreshes its list and the selected run at once instead of at its next 4 s poll.
        const run = payload as unknown as Partial<HeadlessRun>
        if (typeof run.id === 'string') ctx.emit('onHeadlessRunEvent', { runId: run.id, data: '' })
        return true
      }
      return false
    }
  }
}
