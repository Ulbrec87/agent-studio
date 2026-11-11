import { useState } from 'react'
import styles from './BroadcastBar.module.css'

interface BroadcastBarProps {
  onBroadcast: (input: string) => void
  onNewAgent: () => void
  agentCount: number
}

export function BroadcastBar({ onBroadcast, onNewAgent, agentCount }: BroadcastBarProps) {
  const [value, setValue] = useState('')

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    const trimmed = value.trim()
    if (!trimmed) return
    onBroadcast(trimmed)
    setValue('')
  }

  return (
    <div className={styles.bar}>
      <button className={styles.addBtn} onClick={onNewAgent} title="New Agent">
        <span aria-hidden="true">＋</span> New Agent
      </button>
      <form className={styles.form} onSubmit={handleSubmit}>
        <label className={styles.label} htmlFor="grid-broadcast">Broadcast</label>
        <input
          id="grid-broadcast"
          aria-label={`Broadcast to ${agentCount} agents`}
          className={styles.input}
          type="text"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder={`Send to all ${agentCount} agents...`}
          disabled={agentCount === 0}
        />
        <button
          className={styles.sendBtn}
          type="submit"
          disabled={!value.trim() || agentCount === 0}
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="m21 3-7 18-4-7-7-4 18-7Z" /><path d="m10 14 11-11" /></svg>
          Send
        </button>
      </form>
    </div>
  )
}
