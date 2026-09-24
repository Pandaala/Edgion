//! Bounded canonical request span selection.
//!
//! Selection is explicit, never opportunistic: the bounded parser walks
//! `messages[]`, reads a unique `role`, and admits text only from `content`
//! (a string) or `content[j].text` where `content[j].type == "text"`. Every
//! selected span carries the byte range of its complete quoted JSON string
//! token, so a mask addresses whole tokens and can never split a code point.
//!
//! Strings are decoded on demand: the parser records offsets without decoding,
//! and only selected tokens are materialized, so peak allocation follows the
//! configured span bounds rather than the size of the request body.

use edgion_resources::resources::edgion_plugins::AiGuardRole;

use crate::ai::json_span::{decode_json_string, Field, Node, Parser, StringToken};

const MAX_RECOGNIZED_ROLE_BYTES: usize = 9;
const TEXT_PART_TYPE_BYTES: usize = 4;

/// Maximum quoted JSON token size that can decode to at most `limit` bytes.
///
/// Escaped ASCII (`\u00XX`) establishes the worst-case ratio: six encoded
/// bytes for one decoded byte. Saturation only disables this early rejection;
/// the decoded length remains authoritative.
fn max_encoded_json_string_token_bytes(limit: usize) -> usize {
    limit.saturating_mul(6).saturating_add(2)
}

fn token_can_decode_within(token: &StringToken, limit: usize) -> bool {
    token.end.saturating_sub(token.start) <= max_encoded_json_string_token_bytes(limit)
}

fn decode_fixed_value(body: &[u8], token: &StringToken, max_recognized_bytes: usize) -> Option<String> {
    if !token_can_decode_within(token, max_recognized_bytes) {
        return None;
    }
    decode_json_string(&body[token.start..token.end])
}

/// Which text position inside a message a span came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GuardPart {
    /// `messages[i].content` as a bare string.
    Content,
    /// `messages[i].content[j].text`.
    TextPart(u32),
}

/// Stable, sortable identity of one selected span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GuardSpanKey {
    pub message_index: u32,
    pub part: GuardPart,
}

/// One selected span: its decoded text plus the byte range of the complete
/// quoted JSON string token that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardSpan {
    pub key: GuardSpanKey,
    pub role: Option<AiGuardRole>,
    /// Prior context. Always empty on the request side; the response-side hook
    /// supplies the window overlap.
    pub context: String,
    pub current: String,
    pub token: std::ops::Range<u64>,
}

/// Why a canonical request could not be guarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanError {
    /// The body is not a parseable canonical chat-completions request.
    Unparseable,
    /// A configured span bound was exceeded.
    Bounds,
}

/// Inclusive selection bounds.
#[derive(Debug, Clone, Copy)]
pub struct SpanLimits {
    pub max_spans: usize,
    pub max_span_bytes: usize,
    pub max_total_span_bytes: usize,
}

fn parse_role(value: &str) -> Option<AiGuardRole> {
    match value {
        "system" => Some(AiGuardRole::System),
        "user" => Some(AiGuardRole::User),
        "assistant" => Some(AiGuardRole::Assistant),
        "tool" => Some(AiGuardRole::Tool),
        "developer" => Some(AiGuardRole::Developer),
        _ => None,
    }
}

/// Select every guardable text span from a canonical request body.
pub fn select_spans(body: &[u8], limits: SpanLimits) -> Result<Vec<GuardSpan>, SpanError> {
    let root = Parser::new_without_decoding(body)
        .parse()
        .map_err(|_| SpanError::Unparseable)?;
    let messages = match crate::ai::json_span::field(&root, "messages") {
        Field::Unique(node) => node.array().ok_or(SpanError::Unparseable)?,
        // A duplicate `messages` key is ambiguous: the provider and the guard
        // could disagree on which one is authoritative, so refuse to guess.
        Field::Missing | Field::Ambiguous => return Err(SpanError::Unparseable),
    };

    let mut spans: Vec<GuardSpan> = Vec::new();
    let mut total = 0_usize;

    for (index, message) in messages.iter().enumerate() {
        let message_index = u32::try_from(index).map_err(|_| SpanError::Bounds)?;
        let role = match crate::ai::json_span::field(message, "role") {
            Field::Unique(node) => {
                // A wrong-typed role is as unguardable as a missing one: a
                // role-restricted rule would silently not apply.
                let token = node.string_token().ok_or(SpanError::Unparseable)?;
                decode_fixed_value(body, token, MAX_RECOGNIZED_ROLE_BYTES)
                    .as_deref()
                    .and_then(parse_role)
            }
            // An absent or ambiguous role leaves the message unguardable
            // rather than silently guarded under the wrong rule scope.
            Field::Missing | Field::Ambiguous => return Err(SpanError::Unparseable),
        };

        let content = match crate::ai::json_span::field(message, "content") {
            Field::Unique(node) => node,
            Field::Missing => continue,
            Field::Ambiguous => return Err(SpanError::Unparseable),
        };

        let push = |key: GuardSpanKey,
                    token: &StringToken,
                    spans: &mut Vec<GuardSpan>,
                    total: &mut usize|
         -> Result<(), SpanError> {
            // The count is already exhausted, so decoding another selected
            // value cannot change the result and would waste bounded work.
            if spans.len() >= limits.max_spans {
                return Err(SpanError::Bounds);
            }
            // A valid JSON string can consume at most six encoded bytes per
            // decoded byte. Reject only when even that worst case exceeds the
            // configured decoded limit.
            if !token_can_decode_within(token, limits.max_span_bytes) {
                return Err(SpanError::Bounds);
            }
            let text = decode_json_string(&body[token.start..token.end]).ok_or(SpanError::Unparseable)?;
            if text.len() > limits.max_span_bytes {
                return Err(SpanError::Bounds);
            }
            *total = total.checked_add(text.len()).ok_or(SpanError::Bounds)?;
            if *total > limits.max_total_span_bytes {
                return Err(SpanError::Bounds);
            }
            spans.push(GuardSpan {
                key,
                role,
                context: String::new(),
                current: text,
                token: token.start as u64..token.end as u64,
            });
            Ok(())
        };

        match content {
            Node::String(token) => {
                let key = GuardSpanKey {
                    message_index,
                    part: GuardPart::Content,
                };
                push(key, token, &mut spans, &mut total)?;
            }
            Node::Array(parts) => {
                for (part_index, part) in parts.iter().enumerate() {
                    let part_index = u32::try_from(part_index).map_err(|_| SpanError::Bounds)?;
                    if part.object_marker().is_none() {
                        continue;
                    }
                    let is_text = match crate::ai::json_span::field(part, "type") {
                        Field::Unique(node) => node.string_token().is_some_and(|token| {
                            decode_fixed_value(body, token, TEXT_PART_TYPE_BYTES).is_some_and(|value| value == "text")
                        }),
                        Field::Missing => false,
                        Field::Ambiguous => return Err(SpanError::Unparseable),
                    };
                    if !is_text {
                        // Non-text parts are rejected downstream by the provider
                        // translators, so they are not a live guarded path.
                        continue;
                    }
                    match crate::ai::json_span::field(part, "text") {
                        Field::Unique(node) => {
                            if let Some(token) = node.string_token() {
                                let key = GuardSpanKey {
                                    message_index,
                                    part: GuardPart::TextPart(part_index),
                                };
                                push(key, token, &mut spans, &mut total)?;
                            }
                        }
                        Field::Missing => {}
                        Field::Ambiguous => return Err(SpanError::Unparseable),
                    }
                }
            }
            _ => continue,
        }
    }

    spans.sort_by_key(|span| span.key);
    Ok(spans)
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/ai_guard/spans_test.rs"]
mod tests;
