import { useEffect, useRef, useState } from 'react'
import styles from './DaemonBanner.module.css'

interface DaemonStatus {
  connected: boolean
  error?: string
  warning?: string
}

/**
 * Connection state of the daemon. Hidden while connected. While the shell is still starting/connecting it says so; once a
 * connect attempt failed it shows the error ("Daemon not reachable"). When the page was rendered without a daemon and the
 * daemon comes up, the page reloads once so every view loads real data (reconnects after a drop need no reload: the bridge
 * re-subscribes and the lists refresh).
 */
export function DaemonBanner() {
  const [status, setStatus] = useState<DaemonStatus | null>(null)
  const renderedWithoutDaemon = useRef(false)
  const everConnected = useRef(false)

  useEffect(() => {
    let cancelled = false
    const apply = (next: DaemonStatus): void => {
      if (cancelled) return
      setStatus(next)
      // Only a page that *started* without a daemon reloads; a drop after it was connected must keep the UI (open panels,
      // terminals): the bridge re-subscribes and the views refresh by themselves.
      if (!next.connected) {
        if (!everConnected.current) renderedWithoutDaemon.current = true
      } else {
        everConnected.current = true
        if (renderedWithoutDaemon.current) window.location.reload()
      }
    }
    // Both are Agent Studio extensions of the Hydra API: absent in partial `window.hydra` test doubles.
    const api = window.hydra
    if (!api.getDaemonStatus || !api.onDaemonStatus) return
    void api.getDaemonStatus().then(apply).catch(() => undefined)
    const off = api.onDaemonStatus(apply)
    return () => {
      cancelled = true
      off()
    }
  }, [])

  if (status?.connected && status.warning) {
    return (
      <div className={`${styles.banner} ${styles.error}`} role="status" aria-live="polite" data-testid="daemon-banner">
        <div>
          <strong>Daemon version mismatch</strong>
          <div className={styles.detail}>{status.warning}</div>
        </div>
      </div>
    )
  }
  if (!status || status.connected) return null
  const connecting = !status.error || /^connecting/i.test(status.error)
  return (
    <div className={`${styles.banner} ${connecting ? '' : styles.error}`} role="status" aria-live="polite" data-testid="daemon-banner">
      {connecting && <div className={styles.spinner} aria-hidden="true" />}
      <div>
        <strong>{connecting ? 'Connecting to the daemon…' : 'Daemon not reachable'}</strong>
        {!connecting && <div className={styles.detail}>{status.error} — retrying…</div>}
      </div>
    </div>
  )
}
