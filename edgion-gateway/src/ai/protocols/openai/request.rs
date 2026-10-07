//! Selective Chat metadata and byte-preserving, explicitly authorized mutations.

use std::ops::Range;

use bytes::{Bytes, BytesMut};
use edgion_resources::resources::edgion_backend::AI_MODEL_NAME_MAX_BYTES;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

use super::error::AiProxyError;

fn raw_field<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<&'de RawValue>, D::Error> {
    <&RawValue>::deserialize(deserializer).map(Some)
}

/// Unknown provider fields are validated and skipped, never represented as a conversation.
#[derive(Deserialize)]
struct Fields<'a> {
    #[serde(borrow, default, deserialize_with = "raw_field")]
    model: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    stream: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    stream_options: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    max_tokens: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    max_completion_tokens: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct StreamOptions<'a> {
    #[serde(borrow, default, deserialize_with = "raw_field")]
    include_usage: Option<&'a RawValue>,
}

pub(crate) struct ChatMetadata {
    pub model: String,
    pub streaming: bool,
    pub output_bound: Option<u64>,
}

fn fields(body: &[u8]) -> Result<Fields<'_>, AiProxyError> {
    std::str::from_utf8(body).map_err(|_| AiProxyError::MalformedBody)?;
    if body.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return Err(AiProxyError::MalformedBody);
    }
    serde_json::from_slice(body).map_err(|_| AiProxyError::MalformedBody)
}

fn model(fields: &Fields<'_>) -> Result<String, AiProxyError> {
    decode_model(fields.model)
}

fn decode_model(raw: Option<&RawValue>) -> Result<String, AiProxyError> {
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

fn output_bound(fields: &Fields<'_>) -> Result<Option<u64>, AiProxyError> {
    let tokens = fields
        .max_tokens
        .and_then(|raw| serde_json::from_str::<u64>(raw.get()).ok());
    let completion = fields
        .max_completion_tokens
        .and_then(|raw| serde_json::from_str::<u64>(raw.get()).ok());
    if tokens.is_some() && completion.is_some() && tokens != completion {
        return Err(AiProxyError::MalformedBody);
    }
    Ok(completion.or(tokens))
}

pub(crate) fn metadata(body: &[u8]) -> Result<ChatMetadata, AiProxyError> {
    let fields = fields(body)?;
    Ok(ChatMetadata {
        model: model(&fields)?,
        streaming: fields.stream.is_some_and(|raw| raw.get() == "true"),
        output_bound: output_bound(&fields)?,
    })
}

fn span(body: &[u8], raw: &RawValue) -> Result<Range<usize>, AiProxyError> {
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

fn patch(body: &Bytes, range: Range<usize>, replacement: &[u8]) -> Result<Bytes, AiProxyError> {
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

/// None means the original Bytes and BodyStorage generation can be retained.
pub(crate) fn rewrite_model(body: &Bytes, selected_model: &str) -> Result<Option<Bytes>, AiProxyError> {
    let fields = fields(body)?;
    if model(&fields)? == selected_model {
        return Ok(None);
    }
    let raw = fields.model.ok_or(AiProxyError::ModelNotFound)?;
    let replacement = serde_json::to_vec(selected_model).map_err(|_| AiProxyError::BackendUnavailable)?;
    patch(body, span(body, raw)?, &replacement).map(Some)
}

pub(crate) fn inject_usage(body: &Bytes) -> Result<Option<Bytes>, AiProxyError> {
    let fields = fields(body)?;
    if fields.stream.is_none_or(|raw| raw.get() != "true") {
        return Ok(None);
    }
    if let Some(options) = fields.stream_options {
        if !options.get().starts_with('{') {
            return Err(AiProxyError::MalformedBody);
        }
        let parsed: StreamOptions<'_> = serde_json::from_str(options.get()).map_err(|_| AiProxyError::MalformedBody)?;
        if let Some(raw) = parsed.include_usage {
            if raw.get() == "true" {
                return Ok(None);
            }
            return patch(body, span(body, raw)?, b"true").map(Some);
        }
        let object = span(body, options)?;
        let insertion = object.start + 1;
        let has_fields = !options.get()[1..options.get().len() - 1].trim().is_empty();
        return patch(
            body,
            insertion..insertion,
            if has_fields {
                b"\"include_usage\":true,"
            } else {
                b"\"include_usage\":true"
            },
        )
        .map(Some);
    }
    // A validated model makes the root object non-empty.
    let insertion = body
        .iter()
        .position(|byte| *byte == b'{')
        .ok_or(AiProxyError::MalformedBody)?
        + 1;
    patch(
        body,
        insertion..insertion,
        b"\"stream_options\":{\"include_usage\":true},",
    )
    .map(Some)
}

#[derive(Deserialize)]
struct ResponsesFields<'a> {
    #[serde(borrow, default, deserialize_with = "raw_field")]
    model: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    stream: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    max_output_tokens: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    background: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    previous_response_id: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    conversation: Option<&'a RawValue>,
}

pub(crate) struct ResponsesMetadata {
    pub model: String,
    pub streaming: bool,
    pub output_bound: Option<u64>,
    pub continuation: bool,
}

fn optional_bool(raw: Option<&RawValue>) -> Result<bool, AiProxyError> {
    raw.map_or(Ok(false), |raw| {
        serde_json::from_str(raw.get()).map_err(|_| AiProxyError::MalformedBody)
    })
}

pub(crate) fn responses_metadata(body: &[u8]) -> Result<ResponsesMetadata, AiProxyError> {
    if body.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return Err(AiProxyError::MalformedBody);
    }
    let fields: ResponsesFields<'_> = serde_json::from_slice(body).map_err(|_| AiProxyError::MalformedBody)?;
    let model = decode_model(fields.model)?;
    if optional_bool(fields.background)? {
        return Err(AiProxyError::UnsupportedParameter);
    }
    let previous = fields.previous_response_id.filter(|raw| raw.get() != "null");
    let conversation = fields.conversation.filter(|raw| raw.get() != "null");
    if previous.is_some() && conversation.is_some() {
        return Err(AiProxyError::MalformedBody);
    }
    if previous.is_some_and(|raw| !raw.get().starts_with('"'))
        || conversation.is_some_and(|raw| !matches!(raw.get().as_bytes().first(), Some(b'"' | b'{')))
    {
        return Err(AiProxyError::MalformedBody);
    }
    let output_bound = fields
        .max_output_tokens
        .map(|raw| serde_json::from_str::<u64>(raw.get()).map_err(|_| AiProxyError::MalformedBody))
        .transpose()?;
    Ok(ResponsesMetadata {
        model,
        streaming: optional_bool(fields.stream)?,
        output_bound,
        continuation: previous.is_some() || conversation.is_some(),
    })
}

#[cfg(test)]
#[path = "../../../../tests/unit/ai/protocols/openai/request_test.rs"]
mod tests;
