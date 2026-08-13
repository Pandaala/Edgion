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

use crate::ai::json_span::{decode_json_string, Field, Node, Parser};

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
                decode_json_string(&body[token.start..token.end])
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
                    token: &crate::ai::json_span::StringToken,
                    spans: &mut Vec<GuardSpan>,
                    total: &mut usize|
         -> Result<(), SpanError> {
            // Check the raw token length first: it is an upper bound on the
            // decoded length, so an oversized span is rejected without ever
            // materializing it. Decoding first would make peak allocation
            // follow the body size rather than the configured span bound.
            if token.end.saturating_sub(token.start) > limits.max_span_bytes.saturating_add(2) {
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
            if spans.len() >= limits.max_spans {
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
                    let is_text = match crate::ai::json_span::field(part, "type") {
                        Field::Unique(node) => node
                            .string_token()
                            .and_then(|token| decode_json_string(&body[token.start..token.end]))
                            .is_some_and(|value| value == "text"),
                        Field::Missing | Field::Ambiguous => false,
                    };
                    if !is_text {
                        // Non-text parts are rejected downstream by the provider
                        // translators, so they are not a live guarded path.
                        continue;
                    }
                    if let Field::Unique(node) = crate::ai::json_span::field(part, "text") {
                        if let Some(token) = node.string_token() {
                            let key = GuardSpanKey {
                                message_index,
                                part: GuardPart::TextPart(part_index),
                            };
                            push(key, token, &mut spans, &mut total)?;
                        }
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
mod tests {
    use super::*;

    fn limits() -> SpanLimits {
        SpanLimits {
            max_spans: 64,
            max_span_bytes: 4096,
            max_total_span_bytes: 65536,
        }
    }

    #[test]
    fn selects_string_content_and_text_parts_by_role() {
        let body = br#"{"model":"m","messages":[
            {"role":"system","content":"be nice"},
            {"role":"user","content":[{"type":"text","text":"hello"},{"type":"image_url","image_url":{"url":"x"}},{"type":"text","text":"world"}]}
        ]}"#;
        let spans = select_spans(body, limits()).expect("selects");
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].role, Some(AiGuardRole::System));
        assert_eq!(spans[0].current, "be nice");
        assert_eq!(spans[1].key.part, GuardPart::TextPart(0));
        assert_eq!(spans[1].current, "hello");
        assert_eq!(spans[2].current, "world");
        // Spans address the complete quoted token, quotes included.
        assert_eq!(
            &body[spans[0].token.start as usize..spans[0].token.end as usize],
            b"\"be nice\""
        );
    }

    #[test]
    fn spans_are_sorted_and_disjoint() {
        let body = br#"{"messages":[{"role":"user","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]},{"role":"user","content":"c"}]}"#;
        let spans = select_spans(body, limits()).expect("selects");
        assert!(spans.windows(2).all(|pair| pair[0].key < pair[1].key));
        assert!(spans.windows(2).all(|pair| pair[0].token.end <= pair[1].token.start));
    }

    #[test]
    fn decodes_escapes_and_multibyte_text() {
        let body = r#"{"messages":[{"role":"user","content":"café \"q\" €"}]}"#.as_bytes();
        let spans = select_spans(body, limits()).expect("selects");
        assert_eq!(spans[0].current, "café \"q\" €");
    }

    #[test]
    fn ambiguous_role_or_content_is_unparseable() {
        let duplicate_role = br#"{"messages":[{"role":"user","role":"system","content":"x"}]}"#;
        assert_eq!(select_spans(duplicate_role, limits()), Err(SpanError::Unparseable));

        let duplicate_messages = br#"{"messages":[],"messages":[]}"#;
        assert_eq!(select_spans(duplicate_messages, limits()), Err(SpanError::Unparseable));

        let missing_messages = br#"{"model":"m"}"#;
        assert_eq!(select_spans(missing_messages, limits()), Err(SpanError::Unparseable));
    }

    #[test]
    fn bounds_are_inclusive_and_reject_plus_one() {
        let body = br#"{"messages":[{"role":"user","content":"abcde"}]}"#;
        let exact = SpanLimits {
            max_spans: 1,
            max_span_bytes: 5,
            max_total_span_bytes: 5,
        };
        assert!(select_spans(body, exact).is_ok());

        let short = SpanLimits {
            max_span_bytes: 4,
            ..exact
        };
        assert_eq!(select_spans(body, short), Err(SpanError::Bounds));

        let two = br#"{"messages":[{"role":"user","content":"ab"},{"role":"user","content":"cd"}]}"#;
        let one_span = SpanLimits {
            max_spans: 1,
            max_span_bytes: 8,
            max_total_span_bytes: 64,
        };
        assert_eq!(select_spans(two, one_span), Err(SpanError::Bounds));
    }

    #[test]
    fn unknown_role_selects_without_a_role_scope() {
        let body = br#"{"messages":[{"role":"function","content":"x"}]}"#;
        let spans = select_spans(body, limits()).expect("selects");
        assert_eq!(spans[0].role, None);
    }
}
