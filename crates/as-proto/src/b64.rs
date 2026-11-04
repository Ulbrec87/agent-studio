//! `#[serde(with = "as_proto::b64")]` for `Vec<u8>` fields: base64 (standard, padded) strings in JSON.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&STANDARD.encode(data))
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    let text = String::deserialize(deserializer)?;
    STANDARD.decode(text).map_err(serde::de::Error::custom)
}
