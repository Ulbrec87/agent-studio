//! Wire protocol between the daemon and its clients.
//!
//! * [`message`]: serde `Request` / `Response` / `Event` enums and the `ClientMsg` / `ServerMsg` envelopes.
//! * [`b64`]: serde helper that encodes `Vec<u8>` (PTY bytes) as base64 strings.
//! * [`codec`]: length-delimited framing (one JSON value per frame).
//! * [`transport`]: local-socket helpers (Unix socket / Windows named pipe).
//! * [`lock`]: the `daemon.lock` file and liveness probing.
//! * [`client`]: [`client::DaemonClient`], used by the TUI and the one-shot CLI commands.
//! * [`lifecycle`]: [`lifecycle::ensure_daemon`] (spawn-if-missing, version handshake, stop).

pub mod b64;
pub mod client;
pub mod codec;
pub mod lifecycle;
pub mod lock;
pub mod message;
pub mod transport;

pub use client::DaemonClient;
pub use message::*;
