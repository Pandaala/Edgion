//! Bounded model metadata and byte-span mutation shared by native APIs.

use crate::ai::AiProxyError;
use bytes::{Bytes, BytesMut};
use edgion_resources::resources::edgion_backend::AI_MODEL_NAME_MAX_BYTES;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use std::ops::Range;

/// Validate the complete native body without copying it before selective parsing.
/// Protocol parsers retain ownership of JSON syntax and field semantics.
pub(crate) fn native_json_utf8(body: &[u8]) -> Result<&str, AiProxyError> {
    std::str::from_utf8(body).map_err(|_| AiProxyError::MalformedBody)
}

pub(crate) fn raw_field<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<&'de RawValue>, D::Error> {
    <&RawValue>::deserialize(deserializer).map(Some)
}

pub(crate) fn decode_model(raw: Option<&RawValue>) -> Result<String, AiProxyError> {
    let raw = raw.ok_or(AiProxyError::ModelNotFound)?;
    if !raw.get().starts_with('"') {
        return Err(AiProxyError::MalformedBody);
    }
    // Bound decoding allocation before materializing the one metadata string.
    if raw.get().len() > AI_MODEL_NAME_MAX_BYTES * 6 + 2 {
        return Err(AiProxyError::ModelNotFound);
    }
    let model: String = serde_json::from_str(raw.get()).map_err(|_| AiProxyError::MalformedBody)?;
    if model.is_empty() || model.len() > AI_MODEL_NAME_MAX_BYTES {
        return Err(AiProxyError::ModelNotFound);
    }
    Ok(model)
}

pub(crate) fn span(body: &[u8], raw: &RawValue) -> Result<Range<usize>, AiProxyError> {
    let start = (raw.get().as_ptr() as usize)
        .checked_sub(body.as_ptr() as usize)
        .ok_or(AiProxyError::BackendUnavailable)?;
    let end = start
        .checked_add(raw.get().len())
        .ok_or(AiProxyError::BackendUnavailable)?;
    if end > body.len() {
        return Err(AiProxyError::BackendUnavailable);
    }
    Ok(start..end)
}

pub(crate) fn patch(body: &Bytes, range: Range<usize>, replacement: &[u8]) -> Result<Bytes, AiProxyError> {
    let len = body
        .len()
        .checked_sub(range.len())
        .and_then(|len| len.checked_add(replacement.len()))
        .ok_or(AiProxyError::RequestTooLarge)?;
    let mut output = BytesMut::with_capacity(len);
    output.extend_from_slice(&body[..range.start]);
    output.extend_from_slice(replacement);
    output.extend_from_slice(&body[range.end..]);
    Ok(output.freeze())
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/protocols/request_test.rs"]
mod tests;
