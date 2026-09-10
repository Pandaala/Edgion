//! One bounded external evaluator over the canonical outbound stack.
//!
//! Everything transport-shaped is reused: the managed outbound client (which
//! wires the canonical builder, `EgressResolver`, and `resolve_outbound_tls`),
//! `outbound_call_with_body`, and the permit-inside-one-absolute-deadline idiom
//! proven by the response-side Guardrail. Nothing here is a second HTTP client,
//! egress resolver, TLS resolver, or failure-policy vocabulary.
//!
//! The wire uses the same `v1` grammar and `pass|replace|reject` vocabulary as
//! the response-side decision, with request-shaped window keys. The envelope
//! types are deliberately not shared: the key is baked into serde field names,
//! and genericizing would require `#[serde(flatten)]`, which serde cannot
//! combine with `deny_unknown_fields` — silently losing the strict parsing that
//! is a security boundary.

use std::sync::Arc;

use bytes::Bytes;
use edgion_resources::resources::common::MethodSpec;
use edgion_resources::resources::edgion_plugins::AiGuardEvaluator;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use super::rules::GuardDecision;
use super::spans::{GuardPart, GuardSpanKey};
use crate::plugins::http::common::outbound::call::outbound_call_with_body;
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;
use crate::plugins::http::common::CappedWriter;
use crate::plugins::runtime::PluginSession;

/// Absolute ceiling on a decision response, independent of operator config.
const MAX_DECISION_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_CODE_BYTES: usize = 64;
const MAX_REASON_BYTES: usize = 256;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DecisionRequest<'a> {
    version: &'static str,
    spans: &'a [RequestSpan<'a>],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestSpan<'a> {
    message_index: u32,
    part: WirePart,
    current: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum WirePart {
    Content,
    TextPart {
        // `rename_all` renames variants, not their fields, so the wire name is
        // set explicitly to keep the payload camelCase.
        #[serde(rename = "partIndex")]
        part_index: u32,
    },
}

impl From<GuardPart> for WirePart {
    fn from(part: GuardPart) -> Self {
        match part {
            GuardPart::Content => Self::Content,
            GuardPart::TextPart(part_index) => Self::TextPart { part_index },
        }
    }
}

impl WirePart {
    fn key(&self, message_index: u32) -> GuardSpanKey {
        GuardSpanKey {
            message_index,
            part: match self {
                Self::Content => GuardPart::Content,
                Self::TextPart { part_index } => GuardPart::TextPart(*part_index),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecisionEnvelope {
    version: String,
    action: String,
    #[serde(default)]
    replacements: Option<Vec<WireReplacement>>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireReplacement {
    message_index: u32,
    part: WirePart,
    current: String,
}

/// The compiled evaluator owned by one plugin snapshot.
pub struct CompiledEvaluator {
    client: Arc<ManagedOutboundClient>,
    permits: Arc<Semaphore>,
    max_request_bytes: usize,
    max_replacement_bytes: usize,
}

impl CompiledEvaluator {
    pub fn build(config: &AiGuardEvaluator, max_replacement_bytes: usize) -> Result<Self, String> {
        let mut request = config.request.clone();
        // The operator cannot choose the method: the compiled call is always
        // the non-idempotent decision POST.
        request.method = Some(MethodSpec {
            template: "POST".to_string(),
        });
        let client = ManagedOutboundClient::from_parts(
            &config.conn,
            &request,
            config.resolved_secrets.as_ref(),
            "AiGuard decision service",
            false,
        )?;
        Ok(Self {
            client: Arc::new(client),
            permits: Arc::new(Semaphore::new(config.max_concurrent)),
            max_request_bytes: config.max_request_bytes,
            // The operator's aggregate replacement bound, not the request
            // encode cap: conflating them let an evaluator return orders of
            // magnitude more replacement text than the operator allowed.
            max_replacement_bytes,
        })
    }

    /// Run one bounded decision over the post-local-mask span text.
    ///
    /// Errors are `&'static str` classification labels only; no endpoint,
    /// payload, evaluator code/reason, or raw dependency error is ever
    /// surfaced.
    pub async fn decide(
        &self,
        session: &dyn PluginSession,
        spans: &[(GuardSpanKey, &str)],
    ) -> Result<GuardDecision<GuardSpanKey>, &'static str> {
        if spans.is_empty() || spans.iter().all(|(_, text)| text.is_empty()) {
            return Ok(GuardDecision::Pass);
        }
        // Data minimization: only the selected bounded span text and its
        // structural key leave the gateway. No raw body, headers, model,
        // provider identity, or backend address.
        let payload: Vec<RequestSpan<'_>> = spans
            .iter()
            .map(|(key, current)| RequestSpan {
                message_index: key.message_index,
                part: key.part.into(),
                current: *current,
            })
            .collect();
        let request = encode_request(&payload, self.max_request_bytes)?;

        let timeout = self.client.timeout;
        let call = async {
            // The permit is acquired inside the deadline, so queueing counts
            // against the same absolute budget as the exchange, and is
            // released by RAII if the future is dropped.
            let _permit = Arc::clone(&self.permits)
                .acquire_owned()
                .await
                .map_err(|_| "evaluator closed")?;
            outbound_call_with_body(
                &self.client,
                session,
                None,
                self.client.max_response_bytes.min(MAX_DECISION_RESPONSE_BYTES),
                &[("content-type", "application/json")],
                Some(request),
                // Evaluator validation forbids all origin fields, so no
                // presence-policy diagnostics can be produced here.
                &mut Default::default(),
            )
            .await
            .map_err(|_| "evaluator dependency")
        };
        let response = tokio::time::timeout(timeout, call)
            .await
            .map_err(|_| "evaluator timeout")??;

        if !is_json_response(response.status, &response.headers) {
            return Err("evaluator bad response");
        }
        parse_decision(&response.body, self.max_replacement_bytes)
    }
}

fn encode_request(spans: &[RequestSpan<'_>], cap: usize) -> Result<Bytes, &'static str> {
    let mut writer = CappedWriter::new(cap);
    DecisionRequest { version: "v1", spans }
        .serialize(&mut serde_json::Serializer::new(&mut writer))
        .map_err(|_| "evaluator request too large")?;
    Ok(Bytes::from(writer.into_inner()))
}

fn is_json_response(status: u16, headers: &reqwest::header::HeaderMap) -> bool {
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

fn parse_decision(body: &[u8], max_replacement_bytes: usize) -> Result<GuardDecision<GuardSpanKey>, &'static str> {
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let envelope = DecisionEnvelope::deserialize(&mut deserializer).map_err(|_| "evaluator invalid json")?;
    deserializer.end().map_err(|_| "evaluator trailing json")?;
    if envelope.version != "v1" {
        return Err("evaluator bad version");
    }
    match envelope.action.as_str() {
        "pass" if envelope.replacements.is_none() && envelope.code.is_none() && envelope.reason.is_none() => {
            Ok(GuardDecision::Pass)
        }
        "reject" if envelope.replacements.is_none() => {
            if envelope.code.as_ref().is_some_and(|value| {
                value.is_empty()
                    || value.len() > MAX_CODE_BYTES
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
            }) {
                return Err("evaluator bad code");
            }
            if envelope
                .reason
                .as_ref()
                .is_some_and(|value| value.len() > MAX_REASON_BYTES)
            {
                return Err("evaluator bad reason");
            }
            Ok(GuardDecision::Reject)
        }
        "replace" if envelope.code.is_none() && envelope.reason.is_none() => {
            let replacements = envelope.replacements.ok_or("evaluator missing replacements")?;
            if replacements.is_empty() {
                return Err("evaluator empty replacements");
            }
            let mut total = 0_usize;
            let mut parsed = Vec::with_capacity(replacements.len());
            for replacement in replacements {
                total = total
                    .checked_add(replacement.current.len())
                    .ok_or("evaluator replacement overflow")?;
                if total > max_replacement_bytes {
                    return Err("evaluator replacement too large");
                }
                parsed.push((replacement.part.key(replacement.message_index), replacement.current));
            }
            parsed.sort_by_key(|(key, _)| *key);
            if parsed.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                return Err("evaluator duplicate replacement");
            }
            Ok(GuardDecision::Replace(parsed))
        }
        "pass" | "replace" | "reject" => Err("evaluator bad fields"),
        _ => Err("evaluator bad action"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_envelope_rejects_malformed_decisions() {
        assert_eq!(
            parse_decision(br#"{"version":"v1","action":"pass"}"#, 64),
            Ok(GuardDecision::Pass)
        );
        assert!(parse_decision(br#"{"version":"v2","action":"pass"}"#, 64).is_err());
        assert!(parse_decision(br#"{"version":"v1","action":"pass","extra":1}"#, 64).is_err());
        assert!(parse_decision(br#"{"version":"v1","action":"pass"} {}"#, 64).is_err());
        assert!(parse_decision(br#"{"version":"v1","action":"pass","code":"x"}"#, 64).is_err());
        assert!(parse_decision(br#"{"version":"v1","action":"replace","replacements":[]}"#, 64).is_err());
        assert!(parse_decision(br#"{"version":"v1","action":"nope"}"#, 64).is_err());
    }

    #[test]
    fn replacement_cap_is_inclusive_and_rejects_plus_one() {
        let body = br#"{"version":"v1","action":"replace","replacements":[{"messageIndex":0,"part":{"type":"content"},"current":"abc"}]}"#;
        assert!(parse_decision(body, 3).is_ok());
        assert!(parse_decision(body, 2).is_err());
    }

    #[test]
    fn duplicate_replacement_keys_are_rejected() {
        let body = br#"{"version":"v1","action":"replace","replacements":[{"messageIndex":0,"part":{"type":"content"},"current":"a"},{"messageIndex":0,"part":{"type":"content"},"current":"b"}]}"#;
        assert!(parse_decision(body, 64).is_err());
    }

    #[test]
    fn request_encoding_is_capped_inclusively() {
        let spans = vec![RequestSpan {
            message_index: 0,
            part: WirePart::Content,
            current: "hello",
        }];
        let encoded = encode_request(&spans, 1024).expect("encodes");
        assert_eq!(encode_request(&spans, encoded.len()).expect("exact cap"), encoded);
        assert!(encode_request(&spans, encoded.len() - 1).is_err());
    }

    #[test]
    fn only_single_json_success_responses_are_accepted() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            reqwest::header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        assert!(is_json_response(200, &headers));
        assert!(!is_json_response(500, &headers));
        headers.append(reqwest::header::CONTENT_TYPE, "application/json".parse().unwrap());
        assert!(!is_json_response(200, &headers));
    }

    #[test]
    fn request_payload_carries_only_span_text_and_keys() {
        let spans = vec![RequestSpan {
            message_index: 2,
            part: WirePart::TextPart { part_index: 1 },
            current: "prompt",
        }];
        let encoded = encode_request(&spans, 4096).expect("encodes");
        let text = String::from_utf8(encoded.to_vec()).expect("utf8");
        assert!(text.contains("\"version\":\"v1\""));
        assert!(text.contains("\"messageIndex\":2"));
        assert!(text.contains("\"partIndex\":1"));
        assert!(text.contains("prompt"));
        // No model, provider, backend, header, or credential material.
        assert!(!text.contains("model"));
        assert!(!text.contains("authorization"));
    }
}
