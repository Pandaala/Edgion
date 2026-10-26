//! Borrowed routing metadata and bounded native capability inspection.
use crate::ai::protocols::json_span::decode_json_string_borrowed;
use crate::ai::protocols::request::{decode_model, native_json_utf8, patch, raw_field, span};
use crate::ai::AiProxyError;
use bytes::Bytes;
use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use std::borrow::Cow;

#[derive(Deserialize)]
struct Fields<'a> {
    #[serde(borrow, default, deserialize_with = "raw_field")]
    model: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    stream: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    max_tokens: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    messages: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    tools: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    container: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    fallbacks: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    compaction: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    context_management: Option<&'a RawValue>,
}
#[derive(Deserialize, Default)]
struct Item<'a> {
    #[serde(rename = "type", borrow, default, deserialize_with = "raw_field")]
    kind: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    content: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    source: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    file_id: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    id: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    tool: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    definition: Option<&'a RawValue>,
    #[serde(borrow, default, deserialize_with = "raw_field")]
    edits: Option<&'a RawValue>,
}
pub(crate) struct MessagesMetadata {
    pub model: String,
    pub streaming: bool,
    pub output_bound: u64,
    pub binding: bool,
    pub composite: bool,
}
fn fields(body: &[u8]) -> Result<Fields<'_>, AiProxyError> {
    let text = native_json_utf8(body)?;
    if body.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
        return Err(AiProxyError::MalformedBody);
    }
    serde_json::from_str(text).map_err(|_| AiProxyError::MalformedBody)
}
fn item(raw: &RawValue) -> Result<Item<'_>, AiProxyError> {
    serde_json::from_str(raw.get()).map_err(|_| AiProxyError::MalformedBody)
}
fn kind(raw: Option<&RawValue>) -> Result<Option<Cow<'_, str>>, AiProxyError> {
    raw.filter(|v| v.get() != "null")
        .map(|v| {
            if v.get().len() > 386 {
                return Err(AiProxyError::UnsupportedParameter);
            }
            decode_json_string_borrowed(v.get().as_bytes()).ok_or(AiProxyError::MalformedBody)
        })
        .transpose()
}
/// A sequence visitor retains one borrowed item at a time, never an array/AST.
fn each<'de, T: Deserialize<'de>>(
    raw: &'de RawValue,
    f: &mut impl FnMut(T) -> Result<(), AiProxyError>,
) -> Result<(), AiProxyError> {
    struct Items<'a, F, T> {
        f: &'a mut F,
        marker: std::marker::PhantomData<T>,
    }
    impl<'de, F, T> Visitor<'de> for Items<'_, F, T>
    where
        F: FnMut(T) -> Result<(), AiProxyError>,
        T: Deserialize<'de>,
    {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a bounded native array")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            let mut count = 0usize;
            while let Some(value) = seq.next_element::<T>()? {
                count += 1;
                if count > 4096 {
                    return Err(serde::de::Error::custom("native metadata inspection limit"));
                }
                (self.f)(value).map_err(|_| serde::de::Error::custom("invalid native metadata"))?;
            }
            Ok(())
        }
    }
    let mut de = serde_json::Deserializer::from_str(raw.get());
    de.deserialize_seq(Items {
        f,
        marker: std::marker::PhantomData,
    })
    .map_err(|_| AiProxyError::MalformedBody)?;
    de.end().map_err(|_| AiProxyError::MalformedBody)
}
fn advisor(tool: &Item<'_>) -> Result<bool, AiProxyError> {
    Ok(kind(tool.kind)?.is_some_and(|v| v.starts_with("advisor_")))
}
fn inspect_block(
    block: Item<'_>,
    result: &mut MessagesMetadata,
    depth: usize,
    remaining: &mut usize,
) -> Result<(), AiProxyError> {
    if depth > 16 || *remaining == 0 {
        return Err(AiProxyError::UnsupportedParameter);
    }
    *remaining -= 1;
    let ty = kind(block.kind)?;
    result.binding |= matches!(
        ty.as_deref(),
        Some("thinking" | "redacted_thinking" | "container_upload")
    );
    if let Some(source) = block.source.filter(|v| v.get().starts_with('{')) {
        let source = item(source)?;
        result.binding |= kind(source.kind)?.as_deref() == Some("file") || source.file_id.is_some();
    }
    if ty.as_deref() == Some("tool_addition") {
        if let Some(tool) = block.tool {
            let tool = item(tool)?;
            if kind(tool.kind)?.as_deref() == Some("tool_definition") {
                if let Some(definition) = tool.definition {
                    result.composite |= advisor(&item(definition)?)?;
                }
            }
        }
    }
    if ty.as_deref() == Some("tool_result") {
        if let Some(content) = block.content.filter(|v| v.get().starts_with('[')) {
            each::<Item<'_>>(content, &mut |nested| {
                inspect_block(nested, result, depth + 1, remaining)
            })?;
        }
    }
    Ok(())
}
pub(crate) fn metadata(body: &[u8]) -> Result<MessagesMetadata, AiProxyError> {
    let fields = fields(body)?;
    let mut result = MessagesMetadata {
        model: decode_model(fields.model)?.into_owned(),
        streaming: fields
            .stream
            .filter(|v| v.get() != "null")
            .map(|v| serde_json::from_str::<bool>(v.get()).map_err(|_| AiProxyError::MalformedBody))
            .transpose()?
            .unwrap_or(false),
        output_bound: serde_json::from_str::<u64>(
            fields.max_tokens.ok_or(AiProxyError::MissingRequiredParameter)?.get(),
        )
        .map_err(|_| AiProxyError::MalformedBody)?,
        binding: false,
        composite: false,
    };
    if let Some(container) = fields.container.filter(|v| v.get() != "null") {
        result.binding = container.get().starts_with('"') || item(container)?.id.is_some_and(|v| v.get() != "null");
    }
    if let Some(fallback) = fields.fallbacks.filter(|v| v.get() != "null") {
        if fallback.get().starts_with('[') {
            let mut count = 0;
            each::<&RawValue>(fallback, &mut |_| {
                count += 1;
                Ok(())
            })?;
            result.composite |= count != 0;
        } else {
            result.composite |= kind(Some(fallback))?.as_deref() == Some("default");
        }
    }
    result.composite |= fields.compaction.is_some_and(|v| {
        v.get() != "null"
            && !v
                .get()
                .bytes()
                .filter(|b| !b.is_ascii_whitespace())
                .eq(b"{}".iter().copied())
    });
    if let Some(tools) = fields.tools {
        each::<Item<'_>>(tools, &mut |tool| {
            result.composite |= advisor(&tool)?;
            Ok(())
        })?;
    }
    if let Some(context) = fields.context_management.filter(|v| v.get() != "null") {
        if let Some(edits) = item(context)?.edits {
            each::<Item<'_>>(edits, &mut |edit| {
                result.composite |= kind(edit.kind)?.is_some_and(|v| v.starts_with("compact_"));
                Ok(())
            })?;
        }
    }
    let mut remaining = 4096;
    if let Some(messages) = fields.messages {
        each::<Item<'_>>(messages, &mut |message| {
            if let Some(content) = message.content.filter(|v| v.get().starts_with('[')) {
                each::<Item<'_>>(content, &mut |block| {
                    inspect_block(block, &mut result, 0, &mut remaining)?;
                    Ok(())
                })?;
            }
            Ok(())
        })?;
    }
    Ok(result)
}
pub(crate) fn rewrite_model(body: &Bytes, model: &str) -> Result<Option<Bytes>, AiProxyError> {
    let fields = fields(body)?;
    if decode_model(fields.model)? == model {
        return Ok(None);
    }
    let raw = fields.model.ok_or(AiProxyError::ModelNotFound)?;
    let replacement = serde_json::to_vec(model).map_err(|_| AiProxyError::BackendUnavailable)?;
    patch(body, span(body, raw)?, &replacement).map(Some)
}
#[cfg(test)]
#[path = "../../../../tests/unit/ai/protocols/anthropic/request_test.rs"]
mod tests;
