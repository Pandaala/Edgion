//! Strict, bounded Guardrail unary decision wire format.

use std::borrow::Cow;
use std::io;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::sse::window::{LogicalBucketKey, LogicalField};

const MAX_CODE_BYTES: usize = 64;
const MAX_REASON_BYTES: usize = 256;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DecisionRequest<'a> {
    pub version: &'static str,
    pub windows: &'a [RequestWindow],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RequestWindow {
    pub choice_index: u32,
    pub field: WireField,
    pub context: String,
    pub current: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum WireField {
    Content,
    ReasoningContent,
    ToolArguments { tool_call_index: u32 },
}

impl From<LogicalBucketKey> for (u32, WireField) {
    fn from(key: LogicalBucketKey) -> Self {
        let field = match key.field {
            LogicalField::Content => WireField::Content,
            LogicalField::ReasoningContent => WireField::ReasoningContent,
            LogicalField::ToolArguments { tool_call_index } => WireField::ToolArguments { tool_call_index },
        };
        (key.choice_index, field)
    }
}

impl WireField {
    pub(super) fn key(&self, choice_index: u32) -> LogicalBucketKey {
        LogicalBucketKey {
            choice_index,
            field: match self {
                Self::Content => LogicalField::Content,
                Self::ReasoningContent => LogicalField::ReasoningContent,
                Self::ToolArguments { tool_call_index } => LogicalField::ToolArguments {
                    tool_call_index: *tool_call_index,
                },
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecisionEnvelope<'a> {
    #[serde(borrow)]
    version: Cow<'a, str>,
    #[serde(borrow)]
    action: Cow<'a, str>,
    #[serde(default)]
    replacements: Option<Vec<Replacement>>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Replacement {
    choice_index: u32,
    field: WireField,
    current: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ParsedDecision {
    Pass,
    Replace(Vec<(LogicalBucketKey, String)>),
    Reject,
}

pub(super) fn parse_decision(body: &[u8], max_replacement_bytes: usize) -> Result<ParsedDecision, &'static str> {
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let envelope = DecisionEnvelope::deserialize(&mut deserializer).map_err(|_| "invalid-json")?;
    deserializer.end().map_err(|_| "trailing-json")?;
    if envelope.version != "v1" {
        return Err("unsupported-version");
    }
    match envelope.action.as_ref() {
        "pass" if envelope.replacements.is_none() && envelope.code.is_none() && envelope.reason.is_none() => {
            Ok(ParsedDecision::Pass)
        }
        "reject" if envelope.replacements.is_none() => {
            let code = envelope.code;
            let reason = envelope.reason;
            if code.as_ref().is_some_and(|value| {
                value.len() > MAX_CODE_BYTES
                    || value.is_empty()
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
            }) {
                return Err("invalid-reject-code");
            }
            if reason.as_ref().is_some_and(|value| value.len() > MAX_REASON_BYTES) {
                return Err("invalid-reject-reason");
            }
            Ok(ParsedDecision::Reject)
        }
        "replace" if envelope.code.is_none() && envelope.reason.is_none() => {
            let replacements = envelope.replacements.ok_or("missing-replacements")?;
            if replacements.is_empty() {
                return Err("empty-replacements");
            }
            let mut total = 0usize;
            let mut parsed = Vec::with_capacity(replacements.len());
            for replacement in replacements {
                total = total
                    .checked_add(replacement.current.len())
                    .ok_or("replacement-overflow")?;
                if total > max_replacement_bytes {
                    return Err("replacement-too-large");
                }
                parsed.push((replacement.field.key(replacement.choice_index), replacement.current));
            }
            parsed.sort_by_key(|(key, _)| *key);
            if parsed.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                return Err("duplicate-replacement");
            }
            Ok(ParsedDecision::Replace(parsed))
        }
        "pass" | "replace" | "reject" => Err("invalid-action-fields"),
        _ => Err("unsupported-action"),
    }
}

struct CappedWriter {
    bytes: Vec<u8>,
    cap: usize,
}

impl io::Write for CappedWriter {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        let next = self
            .bytes
            .len()
            .checked_add(input.len())
            .ok_or_else(|| io::Error::other("Guardrail request length overflow"))?;
        if next > self.cap {
            return Err(io::Error::other("Guardrail request exceeds cap"));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn encode_request(windows: &[RequestWindow], cap: usize) -> Result<Bytes, &'static str> {
    let mut writer = CappedWriter {
        bytes: Vec::with_capacity(cap.min(4096)),
        cap,
    };
    DecisionRequest { version: "v1", windows }
        .serialize(&mut serde_json::Serializer::new(&mut writer))
        .map_err(|_| "request-too-large")?;
    Ok(Bytes::from(writer.bytes))
}

pub(super) fn is_json_response(status: u16, headers: &reqwest::header::HeaderMap) -> bool {
    if !(200..300).contains(&status) {
        return false;
    }
    let mut values = headers.get_all(reqwest::header::CONTENT_TYPE).iter();
    let Some(value) = values.next() else {
        return false;
    };
    if values.next().is_some() {
        return false;
    }
    value
        .to_str()
        .ok()
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
}

#[cfg(test)]
#[path = "../../../../tests/unit/ai/guards/semantic/protocol_test.rs"]
mod tests;
