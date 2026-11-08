//! `u64` ⇄ 16 lowercase hex digits. A string, not a JSON number, so that no JSON tool with
//! 53-bit numbers can corrupt a fingerprint.

use serde::{Deserialize, Deserializer, Serializer, de::Error};

pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format!("{value:016x}"))
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let text = String::deserialize(deserializer)?;
    if text.len() != 16 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(D::Error::custom("expected 16 lowercase hexadecimal digits"));
    }
    u64::from_str_radix(&text, 16).map_err(D::Error::custom)
}
