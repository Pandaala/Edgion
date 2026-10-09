//! Selective Chat metadata and byte-preserving, explicitly authorized mutations.

use crate::ai::protocols::request::{decode_model, patch, raw_field, span};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::value::RawValue;

use super::error::AiProxyError;

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
    raw.filter(|raw| raw.get() != "null").map_or(Ok(false), |raw| {
        serde_json::from_str(raw.get()).map_err(|_| AiProxyError::MalformedBody)
    })
}

pub(crate) fn responses_metadata(body: &[u8]) -> Result<ResponsesMetadata, AiProxyError> {
    std::str::from_utf8(body).map_err(|_| AiProxyError::MalformedBody)?;
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
        .filter(|raw| raw.get() != "null")
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
