// The daemon protocol as the shell forwards it (`as-proto` serde JSON): adjacently tagged `{ type, payload }`, snake_case
// names for requests and events, camelCase inside the domain structs, binary data as base64.

/** A request (`as_proto::Request`). Fields of `payload` are snake_case for the message variants, camelCase inside domain structs. */
export interface ProtoRequest {
  type: string
  payload?: unknown
}

/** A response (`as_proto::Response`). */
export interface ProtoResponse {
  type: string
  payload?: unknown
}

/** An event pushed by the daemon (`as_proto::Event`). */
export interface ProtoEvent {
  type: string
  payload?: unknown
}

/** Connection state announced by the shell (`daemon-status`). */
export interface DaemonStatus {
  connected: boolean
  error?: string
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = ''
  const CHUNK = 0x8000
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK))
  }
  return btoa(binary)
}

export function base64ToBytes(text: string): Uint8Array {
  const binary = atob(text)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
  return bytes
}

const encoder = new TextEncoder()

/** Typed input (strings from xterm) -> the UTF-8 bytes `SendRaw` expects. */
export function utf8Bytes(text: string): Uint8Array {
  return encoder.encode(text)
}

/** Make whatever a transport rejected with (Tauri rejects with a bare string) a proper `Error`. */
export function toError(err: unknown): Error {
  if (err instanceof Error) return err
  if (typeof err === 'string') return new Error(err)
  if (err && typeof err === 'object' && 'message' in err) return new Error(String((err as { message: unknown }).message))
  return new Error(String(err))
}
