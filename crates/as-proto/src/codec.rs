//! Framing: `LengthDelimitedCodec` (u32 big-endian length prefix) carrying one JSON value per frame.

use anyhow::{Context, Result};
use bytes::Bytes;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// Largest accepted frame. A subscribe reply carries up to 2 MB of PTY output (~2.7 MB base64).
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

pub fn codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME_LEN)
        .new_codec()
}

/// Wrap a byte stream into a framed `Stream`/`Sink` of [`Bytes`] frames.
pub fn framed<T: AsyncRead + AsyncWrite>(io: T) -> Framed<T, LengthDelimitedCodec> {
    Framed::new(io, codec())
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Bytes> {
    Ok(Bytes::from(
        serde_json::to_vec(msg).context("encoding message")?,
    ))
}

pub fn decode<T: DeserializeOwned>(frame: &[u8]) -> Result<T> {
    serde_json::from_slice(frame).context("decoding message")
}
