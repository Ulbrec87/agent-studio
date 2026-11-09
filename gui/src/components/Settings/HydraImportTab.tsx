import { useState } from 'react'
import type { ImportPreview, ImportReport } from '../../bridge/hydraImportApi'
import styles from './HydraImportTab.module.css'

type Phase =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'preview'; preview: ImportPreview }
  | { kind: 'applying'; preview: ImportPreview }
  | { kind: 'done'; report: ImportReport }
  | { kind: 'error'; message: string }

const errorText = (err: unknown): string => (err instanceof Error ? err.message : String(err))
const plural = (n: number, word: string): string => `${n} ${word}${n === 1 ? '' : 's'}`

/** Settings > Import from Hydra: pick (optionally) Hydra's data folder, look at what would change, confirm, apply. */
export function HydraImportTab() {
  const [dir, setDir] = useState('')
  const [phase, setPhase] = useState<Phase>({ kind: 'idle' })

  const browse = async () => {
    const picked = await window.hydra.selectDirectory().catch(() => null)
    if (picked) {
      setDir(picked)
      setPhase({ kind: 'idle' })
    }
  }

  const preview = async () => {
    setPhase({ kind: 'loading' })
    try {
      setPhase({ kind: 'preview', preview: await window.hydra.previewHydraImport(dir) })
    } catch (err) {
      setPhase({ kind: 'error', message: errorText(err) })
    }
  }

  const apply = async (shown: ImportPreview) => {
    setPhase({ kind: 'applying', preview: shown })
    try {
      // The daemon publishes the new config and the added agents to every client; the UI follows its own events.
      setPhase({ kind: 'done', report: await window.hydra.applyHydraImport(dir) })
    } catch (err) {
      setPhase({ kind: 'error', message: errorText(err) })
    }
  }

  const shown = phase.kind === 'preview' || phase.kind === 'applying' ? phase.preview : null
  const nothingToImport = shown !== null && shown.configChanges.length === 0 && shown.agentsNew === 0

  return (
    <div className={styles.container}>
      <p className={styles.intro}>
        Bring your settings and agents over from Hydra. Hydra&apos;s own files are only read, never changed. Settings that
        differ are replaced by Hydra&apos;s values; agents you already have are skipped.
      </p>

      <div className={styles.field}>
        <label className={styles.label} htmlFor="hydra-import-dir">Hydra data folder</label>
        <div className={styles.dirField}>
          <input
            id="hydra-import-dir"
            className={styles.dirInput}
            type="text"
            value={dir}
            placeholder="Default location"
            onChange={(e) => { setDir(e.target.value); setPhase({ kind: 'idle' }) }}
          />
          <button className={styles.browseBtn} onClick={() => void browse()}>Browse</button>
        </div>
      </div>

      <div className={styles.actions}>
        <button
          className={styles.secondaryBtn}
          onClick={() => void preview()}
          disabled={phase.kind === 'loading' || phase.kind === 'applying'}
        >
          {phase.kind === 'loading' ? 'Looking...' : 'Preview import'}
        </button>
      </div>

      {phase.kind === 'error' && <div className={styles.error} role="alert">{phase.message}</div>}

      {shown && (
        <div className={styles.result}>
          <div className={styles.resultTitle}>Found in <span className={styles.path}>{shown.dir}</span></div>
          <div className={styles.summary}>
            {plural(shown.agentsNew, 'new agent')} ({shown.agentsTotal} in Hydra) and {plural(shown.configChanges.length, 'setting')} to update.
          </div>
          {shown.configChanges.length > 0 && (
            <table className={styles.changes}>
              <thead>
                <tr><th>Setting</th><th>Now</th><th>From Hydra</th></tr>
              </thead>
              <tbody>
                {shown.configChanges.map((c) => (
                  <tr key={c.key}>
                    <td>{c.key}</td>
                    <td className={styles.current}>{c.current}</td>
                    <td className={styles.incoming}>{c.incoming}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {shown.warnings.map((w) => <div key={w} className={styles.warning}>{w}</div>)}
          {nothingToImport ? (
            <div className={styles.summary}>Nothing to import: everything is already here.</div>
          ) : (
            <div className={styles.actions}>
              <button className={styles.primaryBtn} onClick={() => void apply(shown)} disabled={phase.kind === 'applying'}>
                {phase.kind === 'applying' ? 'Importing...' : 'Import now'}
              </button>
              <span className={styles.hint}>This changes your settings and adds the agents above.</span>
            </div>
          )}
        </div>
      )}

      {phase.kind === 'done' && (
        <div className={styles.result} role="status">
          <div className={styles.resultTitle}>Import finished</div>
          <div className={styles.summary}>
            {plural(phase.report.agentsAdded.length, 'agent')} added, {plural(phase.report.configKeysApplied, 'setting')} updated.
          </div>
          {phase.report.warnings.map((w) => <div key={w} className={styles.warning}>{w}</div>)}
        </div>
      )}
    </div>
  )
}
