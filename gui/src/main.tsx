import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import { AppErrorBoundary } from './components/Observability/AppErrorBoundary'
import { createTraceId, logEvent } from './lib/observability'
import './styles/global.css'
import { installHydra } from './bridge'

window.addEventListener('error', (event) => {
  const traceId = createTraceId('window-error')
  logEvent({
    level: 'error',
    event: 'renderer.window-error',
    traceId,
    message: event.message,
    meta: {
      filename: event.filename,
      lineno: event.lineno,
      colno: event.colno,
      stack: event.error?.stack
    }
  })
})

window.addEventListener('unhandledrejection', (event) => {
  const traceId = createTraceId('window-rejection')
  const reason = event.reason
  const message = reason instanceof Error ? reason.message : String(reason)
  const stack = reason instanceof Error ? reason.stack : undefined

  logEvent({
    level: 'error',
    event: 'renderer.unhandled-rejection',
    traceId,
    message,
    meta: { stack }
  })
})

// Prevent the webview from navigating to dropped files.
// Individual components opt-in to handle drops themselves.
document.addEventListener('dragover', (e) => e.preventDefault())
document.addEventListener('drop', (e) => e.preventDefault())

// `window.hydra` must exist before anything renders (components call it from effects and event handlers).
void installHydra().then(() => {
  logEvent({ level: 'info', event: 'renderer.app.start', traceId: createTraceId('app-start') })
  ReactDOM.createRoot(document.getElementById('root')!).render(
    <React.StrictMode>
      <AppErrorBoundary>
        <App />
      </AppErrorBoundary>
    </React.StrictMode>
  )
})
