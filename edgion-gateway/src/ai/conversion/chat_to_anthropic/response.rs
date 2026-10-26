//! Bounded Anthropic Messages to OpenAI Chat Completions response translation.

use std::borrow::Cow;
use std::io::{self, Write};
use std::sync::Arc;

use super::{
    budget,
    head::{failure, UnaryHead},
    limits::{append, UnaryLimits},
};
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::EdgionErrorCode;
use edgion_waf::seclang::{
    JsonDiscardedStringSummary, JsonStreamInput, JsonStreamLimits, JsonStreamSelective, JsonStreamSelectiveLimits,
    JsonStreamSelectiveSink, JsonStreamSinkError, JsonStreamToken,
};
use http::StatusCode;
use pingora_http::ResponseHeader;
use pingora_proxy::ResponseBodySink;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::ai::accounting::usage::{terminate_over_quota, AiUsageShared};
use crate::ai::protocols::openai::canonical_sse::{write_canonical_record, CanonicalRecordBatch};
use crate::ai::quota::QuotaAccumulator;
use crate::ctx::ResponseStreamKind;
use crate::plugins::runtime::traits::streaming_response_processor::StreamingResponseProcessor;
use crate::plugins::runtime::ResponseStreamSession;
use crate::routes::http::proxy_http::content_type::response_is_identity_encoded;
use crate::sse::{SseDecoder, SseEventRef};

const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 512;
const MAX_TOOL_NAME_BYTES: usize = 64;
const MAX_CONTENT_BLOCKS: usize = 128;

const STREAM_ERROR: &[u8] = b"data: {\"error\":{\"message\":\"The AI backend stream could not be processed.\",\"type\":\"api_error\",\"code\":\"response_translation_failed\"}}\n\n";

#[derive(Clone, Copy)]
struct FixedError {
    status: u16,
    body: &'static [u8],
    record_translation_failure: bool,
}

const PROVIDER_INVALID_REQUEST: FixedError = FixedError {
    status: 400,
    body: br#"{"error":{"code":"provider_invalid_request","message":"The AI backend rejected the request.","type":"invalid_request_error"}}"#,
    record_translation_failure: false,
};
const REQUEST_TOO_LARGE: FixedError = FixedError {
    status: 413,
    body: br#"{"error":{"code":"request_too_large","message":"The request is too large.","type":"invalid_request_error"}}"#,
    record_translation_failure: false,
};
const RATE_LIMIT_EXCEEDED: FixedError = FixedError {
    status: 429,
    body: br#"{"error":{"code":"rate_limit_exceeded","message":"The AI backend rate limit was exceeded.","type":"rate_limit_error"}}"#,
    record_translation_failure: false,
};
const SERVICE_UNAVAILABLE: FixedError = FixedError {
    status: 503,
    body: br#"{"error":{"code":"service_unavailable","message":"The AI backend is temporarily unavailable.","type":"api_error"}}"#,
    record_translation_failure: false,
};
const TRANSLATION_UNAVAILABLE: FixedError = FixedError {
    record_translation_failure: true,
    ..SERVICE_UNAVAILABLE
};
const RESPONSE_TOO_LARGE: FixedError = FixedError {
    status: 503,
    body: br#"{"error":{"code":"response_too_large","message":"The AI backend response is too large.","type":"api_error"}}"#,
    record_translation_failure: true,
};
const RESPONSE_TRANSLATION_FAILED: FixedError = FixedError {
    status: 200,
    body: br#"{"error":{"code":"response_translation_failed","message":"The AI backend response could not be processed.","type":"api_error"}}"#,
    record_translation_failure: true,
};

enum BodyMode {
    AwaitingHeader,
    NonStreaming,
    Streaming,
    Fixed(FixedError),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActiveBlock {
    Text { index: usize },
    Tool { index: usize, tool_index: usize },
}

#[derive(Default)]
struct TokenUsage {
    input: u64,
    cache_creation: u64,
    cache_read: u64,
    output: u64,
    input_seen: bool,
    output_seen: bool,
}

impl TokenUsage {
    fn normalized(&self) -> (u64, u64) {
        (
            self.input
                .saturating_add(self.cache_creation)
                .saturating_add(self.cache_read),
            self.output,
        )
    }

    fn update_from(&mut self, usage: &Map<String, Value>) -> Result<(), ()> {
        update_u64_seen(usage, "input_tokens", &mut self.input, &mut self.input_seen)?;
        update_u64(usage, "cache_creation_input_tokens", &mut self.cache_creation)?;
        update_u64(usage, "cache_read_input_tokens", &mut self.cache_read)?;
        update_u64_seen(usage, "output_tokens", &mut self.output, &mut self.output_seen)
    }
}

/// Per-request Anthropic response translator.
pub(crate) struct AnthropicResponseProcessor {
    unary: Option<Arc<UnaryHead>>,
    canonical_model: String,
    requested_streaming: bool,
    effective_usage_chunk: bool,
    created: u64,
    usage_shared: Arc<AiUsageShared>,
    quota: Option<Arc<QuotaAccumulator>>,
    /// Resolved from the route's `AiTokenQuotaConfig.onExceeded`: `true`
    /// only for `Reject`. See `UsageExtractionProcessor::terminate_on_over_limit`
    /// for the full rationale; this processor gates its own mid-stream
    /// termination on the same flag so Anthropic-backed traffic honors the
    /// same operator-declared policy as OpenAI-shaped traffic.
    terminate_on_over_limit: bool,
    mode: BodyMode,
    non_streaming_input: Vec<u8>,
    decoder: SseDecoder,
    message_started: bool,
    message_stopped: bool,
    message_id: String,
    active_block: Option<ActiveBlock>,
    next_block_index: usize,
    next_tool_index: usize,
    message_delta_started: bool,
    finish_emitted: bool,
    finish_reason: Option<&'static str>,
    usage: TokenUsage,
    sealed: bool,
    error_recorded: bool,
    fixed_emitted: bool,
    pending_output_chars: u64,
    semantic_records: Option<CanonicalRecordBatch>,
}

impl AnthropicResponseProcessor {
    #[cfg(test)]
    pub(crate) fn new(
        canonical_model: String,
        requested_streaming: bool,
        effective_usage_chunk: bool,
        created: u64,
        usage_shared: Arc<AiUsageShared>,
    ) -> Self {
        Self::new_with_quota(
            canonical_model,
            requested_streaming,
            effective_usage_chunk,
            created,
            usage_shared,
            None,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_quota_and_unary(
        canonical_model: String,
        requested_streaming: bool,
        effective_usage_chunk: bool,
        created: u64,
        usage_shared: Arc<AiUsageShared>,
        quota: Option<Arc<QuotaAccumulator>>,
        terminate_on_over_limit: bool,
        unary: Option<Arc<UnaryHead>>,
    ) -> Self {
        Self {
            unary,
            canonical_model,
            requested_streaming,
            effective_usage_chunk,
            created,
            usage_shared,
            quota,
            terminate_on_over_limit,
            mode: BodyMode::AwaitingHeader,
            non_streaming_input: Vec::new(),
            decoder: SseDecoder::with_default_limit(),
            message_started: false,
            message_stopped: false,
            message_id: String::new(),
            active_block: None,
            next_block_index: 0,
            next_tool_index: 0,
            message_delta_started: false,
            finish_emitted: false,
            finish_reason: None,
            usage: TokenUsage::default(),
            sealed: false,
            error_recorded: false,
            fixed_emitted: false,
            pending_output_chars: 0,
            semantic_records: None,
        }
    }

    fn normalize_header(&mut self, response: &mut ResponseHeader) {
        if !self.requested_streaming && self.unary.is_none() {
            if let Ok(mut fresh) = ResponseHeader::build_no_case(503, Some(0)) {
                let _ = fresh.insert_header("content-type", "application/json");
                let _ = fresh.insert_header("cache-control", "no-store");
                *response = fresh;
            }
            self.mode = BodyMode::Fixed(TRANSLATION_UNAVAILABLE);
            self.usage_shared.mark_observation_unavailable();
            return;
        }
        let original_status = response.status.as_u16();
        let error = match original_status {
            400 => Some(PROVIDER_INVALID_REQUEST),
            413 => Some(REQUEST_TOO_LARGE),
            429 => Some(RATE_LIMIT_EXCEEDED),
            200 => self.validate_success_header(response).err(),
            _ => Some(SERVICE_UNAVAILABLE),
        };
        if let Some(owner) = self.unary.as_ref() {
            owner.observe_header(original_status == 200, error.is_none());
            self.usage_shared.mark_json_started();
            if original_status != 200 {
                self.usage_shared.mark_observation_unavailable();
            }
        }
        let retry_after = if original_status == 429 {
            response
                .headers
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .filter(|value| (self.unary.is_none() || value.len() <= 64) && valid_retry_after(value))
                .map(str::to_owned)
        } else {
            None
        };

        if self.unary.is_some() {
            if let Ok(fresh) = ResponseHeader::build_no_case(response.status, Some(0)) {
                *response = fresh;
            }
        } else {
            clear_provider_headers(response);
        }
        let _ = response.insert_header("cache-control", "no-store");
        if let Some(error) = error {
            let _ = response.set_status(StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE));
            let _ = response.insert_header("content-type", "application/json");
            if error.status == 429 {
                if let Some(retry_after) = retry_after {
                    let _ = response.insert_header("retry-after", retry_after);
                }
            }
            self.mode = BodyMode::Fixed(error);
        } else if self.requested_streaming {
            let _ = response.insert_header("content-type", "text/event-stream");
            self.mode = BodyMode::Streaming;
        } else {
            let _ = response.insert_header("content-type", "application/json");
            self.mode = BodyMode::NonStreaming;
        }
    }

    fn validate_success_header(&self, response: &ResponseHeader) -> Result<(), FixedError> {
        if !identity_encoding(response) {
            return Err(TRANSLATION_UNAVAILABLE);
        }
        let expected = if self.requested_streaming {
            ("text", "event-stream")
        } else {
            ("application", "json")
        };
        if !matches_media_type(response, expected.0, expected.1) {
            return Err(TRANSLATION_UNAVAILABLE);
        }
        if !self.requested_streaming {
            let mut lengths = response.headers.get_all("content-length").iter();
            if let Some(value) = lengths.next() {
                if lengths.next().is_some() {
                    return Err(TRANSLATION_UNAVAILABLE);
                }
                let length = value
                    .to_str()
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
                    .ok_or(TRANSLATION_UNAVAILABLE)?;
                if length > self.unary.as_ref().map_or(MAX_BODY_BYTES, |owner| owner.limits.input) {
                    return Err(RESPONSE_TOO_LARGE);
                }
            }
        }
        Ok(())
    }

    fn process_fixed(&mut self, chunk: &mut Option<Bytes>, session: &mut dyn ResponseStreamSession, error: FixedError) {
        if !self.fixed_emitted {
            *chunk = Some(render_fixed_error(error));
            self.fixed_emitted = true;
            if error.record_translation_failure {
                self.record_translation_failure(session);
            }
        } else {
            *chunk = None;
        }
    }

    fn process_non_streaming(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        sink: &mut ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
    ) {
        let Some(limits) = self.unary.as_ref().map(|owner| owner.limits) else {
            self.sealed = true;
            self.non_streaming_input = Vec::new();
            self.usage_shared.mark_json_unavailable();
            *chunk = None;
            let _ = sink.fail_response_head(failure(false));
            return;
        };
        let limit = limits.input;
        if self.unary.as_ref().is_some_and(|owner| owner.expired()) {
            self.fail_unary(chunk, sink, session, true);
            return;
        }
        if let Some(bytes) = chunk.take() {
            if append(&mut self.non_streaming_input, &bytes, limit).is_err() {
                self.fail_unary(chunk, sink, session, false);
                return;
            }
        }
        if !end_of_stream {
            return;
        }
        let input = std::mem::take(&mut self.non_streaming_input);
        let translated = translate_unary(
            &input,
            &self.canonical_model,
            self.created,
            limits,
            Some(&self.usage_shared),
        );
        drop(input);
        if self.unary.as_ref().is_some_and(|owner| owner.expired()) {
            self.fail_unary(chunk, sink, session, true);
            return;
        }
        match translated {
            Ok((body, input_tokens, output_tokens, cache_read, cache_write)) => {
                self.usage_shared
                    .observe_reported_usage(input_tokens, output_tokens, cache_read, cache_write);
                self.usage_shared.mark_json_complete();
                let Some(owner) = self.unary.as_ref() else {
                    self.fail_unary(chunk, sink, session, false);
                    return;
                };
                *chunk = Some(budget::output(body, owner.reservation.clone()));
                if !sink.release_response_head() {
                    *chunk = None;
                    self.record_translation_failure(session);
                }
                self.sealed = true;
            }
            Err(_) => self.fail_unary(chunk, sink, session, false),
        }
    }
    fn fail_unary(
        &mut self,
        chunk: &mut Option<Bytes>,
        sink: &mut ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
        timeout: bool,
    ) {
        self.sealed = true;
        self.non_streaming_input = Vec::new();
        self.record_translation_failure(session);
        self.usage_shared.mark_json_unavailable();
        *chunk = None;
        let error = if timeout {
            self.unary
                .as_ref()
                .map_or_else(|| failure(true), |owner| owner.timeout_failure())
        } else {
            failure(false)
        };
        let _ = sink.fail_response_head(error);
    }
    fn fail_non_streaming(
        &mut self,
        chunk: &mut Option<Bytes>,
        session: &mut dyn ResponseStreamSession,
        error: FixedError,
    ) {
        self.sealed = true;
        self.non_streaming_input = Vec::new();
        self.record_translation_failure(session);
        *chunk = Some(render_fixed_error(error));
    }

    fn process_streaming(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        session: &mut dyn ResponseStreamSession,
    ) {
        let input = chunk.take().unwrap_or_default();
        if self.message_stopped && !input.is_empty() {
            self.fail_streaming(chunk, session);
            return;
        }
        let mut output = Vec::new();
        let mut failed = false;
        let mut decoder = std::mem::replace(&mut self.decoder, SseDecoder::with_default_limit());
        decoder.feed_each_bytes(input, |event| {
            if failed {
                return;
            }
            if self.process_sse_event(event, &mut output).is_err() {
                failed = true;
            }
        });
        if decoder.degraded() || decoder.malformed() {
            failed = true;
        }
        self.decoder = decoder;
        if end_of_stream && (!self.message_stopped || !self.decoder.is_idle()) {
            failed = true;
        }
        if failed {
            self.fail_streaming(chunk, session);
        } else if !output.is_empty() {
            *chunk = Some(Bytes::from(output));
        }
    }

    fn process_sse_event(&mut self, event: SseEventRef<'_>, output: &mut Vec<u8>) -> Result<(), ()> {
        if self.message_stopped {
            return Err(());
        }
        let data = event.semantic_data()?;
        let value: Value = serde_json::from_str(&data).map_err(|_| ())?;
        let object = value.as_object().ok_or(())?;
        let type_ = required_string(object, "type")?;
        if !event.event_is(type_) {
            return Err(());
        }
        match type_ {
            "ping" => Ok(()),
            "error" => Err(()),
            "message_start" => self.handle_message_start(object, output),
            "content_block_start" => self.handle_block_start(object, output),
            "content_block_delta" => self.handle_block_delta(object, output),
            "content_block_stop" => self.handle_block_stop(object),
            "message_delta" => self.handle_message_delta(object, output),
            "message_stop" => self.handle_message_stop(output),
            _ => Ok(()),
        }
    }

    fn handle_message_start(&mut self, event: &Map<String, Value>, output: &mut Vec<u8>) -> Result<(), ()> {
        if self.message_started || self.message_stopped {
            return Err(());
        }
        let message = event.get("message").and_then(Value::as_object).ok_or(())?;
        if required_string(message, "type")? != "message" || required_string(message, "role")? != "assistant" {
            return Err(());
        }
        let id = bounded_string(message, "id", MAX_IDENTIFIER_BYTES)?;
        let _provider_model = bounded_string(message, "model", MAX_IDENTIFIER_BYTES)?;
        if !message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            return Err(());
        }
        let usage = message.get("usage").and_then(Value::as_object).ok_or(())?;
        self.usage.update_from(usage)?;
        if !self.usage.input_seen {
            return Err(());
        }
        // Anthropic reports the final cumulative output count in a later
        // message_delta. A message_start placeholder must not be mistaken for
        // terminal exact usage.
        self.usage.output_seen = false;
        self.message_id = id.to_owned();
        self.message_started = true;
        self.push_chunk(
            output,
            json!({"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}),
        )
    }

    fn handle_block_start(&mut self, event: &Map<String, Value>, output: &mut Vec<u8>) -> Result<(), ()> {
        self.require_active_message()?;
        if self.active_block.is_some()
            || self.message_delta_started
            || self.finish_reason.is_some()
            || self.next_block_index >= MAX_CONTENT_BLOCKS
        {
            return Err(());
        }
        let index = required_index(event, "index")?;
        if index != self.next_block_index {
            return Err(());
        }
        let block = event.get("content_block").and_then(Value::as_object).ok_or(())?;
        match required_string(block, "type")? {
            "text" => {
                if !required_string(block, "text")?.is_empty() {
                    return Err(());
                }
                self.active_block = Some(ActiveBlock::Text { index });
            }
            "tool_use" => {
                let id = bounded_string(block, "id", MAX_IDENTIFIER_BYTES)?;
                let name = bounded_string(block, "name", MAX_TOOL_NAME_BYTES)?;
                if !valid_tool_name(name) || !block.get("input").and_then(Value::as_object).is_some_and(Map::is_empty) {
                    return Err(());
                }
                let tool_index = self.next_tool_index;
                self.next_tool_index = self.next_tool_index.checked_add(1).ok_or(())?;
                self.active_block = Some(ActiveBlock::Tool { index, tool_index });
                self.push_chunk(
                    output,
                    json!({"index":0,"delta":{"tool_calls":[{"index":tool_index,"id":id,"type":"function","function":{"name":name,"arguments":""}}]},"finish_reason":null}),
                )?;
            }
            _ => return Err(()),
        }
        self.next_block_index = self.next_block_index.checked_add(1).ok_or(())?;
        Ok(())
    }

    fn handle_block_delta(&mut self, event: &Map<String, Value>, output: &mut Vec<u8>) -> Result<(), ()> {
        self.require_active_message()?;
        let index = required_index(event, "index")?;
        let delta = event.get("delta").and_then(Value::as_object).ok_or(())?;
        match self.active_block {
            Some(ActiveBlock::Text { index: active }) if active == index => {
                if required_string(delta, "type")? != "text_delta" {
                    return Err(());
                }
                let text = required_string(delta, "text")?;
                if !text.is_empty() {
                    if let Some(quota) = &self.quota {
                        quota.add_output_units(1);
                    }
                }
                self.push_chunk(output, json!({"index":0,"delta":{"content":text},"finish_reason":null}))
            }
            Some(ActiveBlock::Tool {
                index: active,
                tool_index,
            }) if active == index => {
                if required_string(delta, "type")? != "input_json_delta" {
                    return Err(());
                }
                let partial_json = required_string(delta, "partial_json")?;
                if !partial_json.is_empty() {
                    if let Some(quota) = &self.quota {
                        quota.add_output_units(1);
                    }
                }
                self.push_chunk(
                    output,
                    json!({"index":0,"delta":{"tool_calls":[{"index":tool_index,"function":{"arguments":partial_json}}]},"finish_reason":null}),
                )
            }
            _ => Err(()),
        }
    }

    fn handle_block_stop(&mut self, event: &Map<String, Value>) -> Result<(), ()> {
        self.require_active_message()?;
        let index = required_index(event, "index")?;
        match self.active_block {
            Some(ActiveBlock::Text { index: active }) | Some(ActiveBlock::Tool { index: active, .. })
                if active == index =>
            {
                self.active_block = None;
                Ok(())
            }
            _ => Err(()),
        }
    }

    fn handle_message_delta(&mut self, event: &Map<String, Value>, output: &mut Vec<u8>) -> Result<(), ()> {
        self.require_active_message()?;
        if self.active_block.is_some() || self.next_block_index == 0 {
            return Err(());
        }
        self.message_delta_started = true;
        let delta = event.get("delta").and_then(Value::as_object).ok_or(())?;
        if let Some(usage) = event.get("usage") {
            self.usage.update_from(usage.as_object().ok_or(())?)?;
            if let Some(quota) = &self.quota {
                quota.observe_output_total(self.usage.output);
            }
        }
        match delta.get("stop_reason") {
            None | Some(Value::Null) => {}
            Some(Value::String(stop_reason)) => {
                let mapped = map_stop_reason(stop_reason).ok_or(())?;
                if let Some(existing) = self.finish_reason {
                    if existing != mapped {
                        return Err(());
                    }
                } else {
                    self.finish_reason = Some(mapped);
                }
                if !self.finish_emitted {
                    self.push_chunk(output, json!({"index":0,"delta":{},"finish_reason":mapped}))?;
                    self.finish_emitted = true;
                }
            }
            Some(_) => return Err(()),
        }
        Ok(())
    }

    fn handle_message_stop(&mut self, output: &mut Vec<u8>) -> Result<(), ()> {
        self.require_active_message()?;
        if self.active_block.is_some() || !self.finish_emitted || self.message_stopped || !self.usage.output_seen {
            return Err(());
        }
        self.message_stopped = true;
        let (input, output_tokens) = self.usage.normalized();
        self.usage_shared.observe_reported_usage(
            input,
            output_tokens,
            self.usage.cache_read,
            self.usage.cache_creation,
        );
        if self.effective_usage_chunk {
            self.push_json_event(
                output,
                &json!({
                    "id":self.message_id,
                    "object":"chat.completion.chunk",
                    "created":self.created,
                    "model":self.canonical_model,
                    "choices":[],
                    "usage":{"prompt_tokens":input,"completion_tokens":output_tokens,"total_tokens":input.saturating_add(output_tokens)}
                }),
            )?;
        }
        append_capped(output, b"data: [DONE]\n\n", MAX_BODY_BYTES)?;
        if let Some(batch) = &mut self.semantic_records {
            batch
                .push(crate::sse::window::WindowRecord::raw(vec![Bytes::from_static(
                    b"data: [DONE]\n\n",
                )]))
                .map_err(|_| ())?;
        }
        self.pending_output_chars = self.pending_output_chars.saturating_add(6);
        Ok(())
    }

    fn require_active_message(&self) -> Result<(), ()> {
        if self.message_started && !self.message_stopped {
            Ok(())
        } else {
            Err(())
        }
    }

    fn push_chunk(&mut self, output: &mut Vec<u8>, choice: Value) -> Result<(), ()> {
        let mut chunk = Map::new();
        chunk.insert("id".to_string(), Value::String(self.message_id.clone()));
        chunk.insert("object".to_string(), Value::String("chat.completion.chunk".to_string()));
        chunk.insert("created".to_string(), Value::from(self.created));
        chunk.insert("model".to_string(), Value::String(self.canonical_model.clone()));
        chunk.insert("choices".to_string(), Value::Array(vec![choice]));
        self.push_json_event(output, &Value::Object(chunk))
    }

    fn push_json_event(&mut self, output: &mut Vec<u8>, value: &Value) -> Result<(), ()> {
        let (encoded_record, record) = write_canonical_record(value, MAX_BODY_BYTES, self.semantic_records.is_some())?;
        append_capped(output, &encoded_record, MAX_BODY_BYTES)?;
        self.pending_output_chars = self.pending_output_chars.saturating_add(
            u64::try_from(
                String::from_utf8_lossy(&encoded_record[6..encoded_record.len() - 2])
                    .chars()
                    .count(),
            )
            .unwrap_or(u64::MAX),
        );
        if let (Some(batch), Some(record)) = (&mut self.semantic_records, record) {
            batch.push(record).map_err(|_| ())?;
        }
        Ok(())
    }

    fn fail_streaming(&mut self, chunk: &mut Option<Bytes>, session: &mut dyn ResponseStreamSession) {
        if !session.try_claim_response_terminal(crate::response::terminal::ResponseTerminalCause::TranslationFailure) {
            self.sealed = true;
            self.semantic_records = None;
            *chunk = None;
            return;
        }
        self.sealed = true;
        // A translation failure wins in the AI slot. Do not hand its fixed
        // client error to the preempted semantic guard as provider content.
        self.semantic_records = None;
        self.pending_output_chars = stream_error_output_chars();
        self.record_translation_failure(session);
        *chunk = Some(Bytes::from_static(STREAM_ERROR));
        session.mark_response_terminal_emission(crate::response::terminal::TerminalEmission::Rendered);
    }

    fn record_translation_failure(&mut self, session: &mut dyn ResponseStreamSession) {
        if !self.error_recorded {
            session.record_error(EdgionErrorCode::AiResponseTranslationFailed);
            self.error_recorded = true;
        }
    }
}

#[async_trait]
impl StreamingResponseProcessor for AnthropicResponseProcessor {
    fn on_response_header(&mut self, response: &mut ResponseHeader) {
        self.normalize_header(response);
    }

    fn changes_body_length(&self) -> bool {
        true
    }
    fn finalize_incomplete(&mut self, _outcome: &'static str, _reason: &'static str) {
        if self.unary.is_some() {
            self.usage_shared.mark_json_unavailable();
        }
        self.non_streaming_input = Vec::new();
        self.unary = None;
        self.sealed = true;
    }
    fn release_inflight(&mut self) {
        if self.unary.is_some() {
            self.usage_shared.mark_json_unavailable();
        }
        self.non_streaming_input = Vec::new();
        self.unary = None;
        self.sealed = true;
    }
    fn on_terminal_preempted(&mut self, _winner: crate::response::terminal::ResponseTerminalCause) {
        self.release_inflight();
    }

    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn ResponseStreamSession,
    ) {
        if self.sealed {
            *chunk = None;
            if matches!(self.mode, BodyMode::Streaming) {
                if session
                    .try_claim_response_terminal(crate::response::terminal::ResponseTerminalCause::ProviderCompleted)
                {
                    session.mark_response_terminal_emission(crate::response::terminal::TerminalEmission::NoEvent);
                }
                sink.terminate();
            }
            return;
        }
        self.pending_output_chars = 0;
        self.semantic_records = (session.semantic_guard_active() && matches!(self.mode, BodyMode::Streaming))
            .then(CanonicalRecordBatch::new);
        match self.mode {
            BodyMode::AwaitingHeader => self.fail_non_streaming(chunk, session, RESPONSE_TRANSLATION_FAILED),
            BodyMode::Fixed(error) => self.process_fixed(chunk, session, error),
            BodyMode::NonStreaming => self.process_non_streaming(chunk, end_of_stream, sink, session),
            BodyMode::Streaming => self.process_streaming(chunk, end_of_stream, session),
        }
        // `BodyMode::Streaming` is reachable only when `requested_streaming`
        // was true and the upstream header validated as a real event-stream
        // (see `normalize_header`), so it already gates out non-streaming
        // completions the same way `UsageExtractionProcessor` gates on
        // `requested_streaming`. `!self.sealed` skips a chunk that just
        // failed translation (it already carries `STREAM_ERROR`); `quota`
        // accounting for Anthropic traffic lives entirely in this processor
        // (see `handle_block_delta`/`handle_message_delta`), so this is the
        // only place that can observe `over_limit()` for it. Also gated on
        // `terminate_on_over_limit`: `Throttle`/`Downgrade` are the operator
        // declaring "do not hard-fail this user", already honored at
        // admission, so under those actions the stream runs to completion
        // even once over limit -- see `UsageExtractionProcessor`'s identical
        // gate for the full rationale.
        if !end_of_stream
            && !self.sealed
            && matches!(self.mode, BodyMode::Streaming)
            && self.terminate_on_over_limit
            && self.quota.as_ref().is_some_and(|quota| quota.over_limit())
        {
            // Streaming mode always emits the OpenAI-shaped SSE wire format
            // to the client (that is the point of this translator), so the
            // injected event is always safe here.
            terminate_over_quota(&self.usage_shared, sink, session, true);
        }
        if self.sealed && matches!(self.mode, BodyMode::Streaming) {
            if session.try_claim_response_terminal(crate::response::terminal::ResponseTerminalCause::ProviderCompleted)
            {
                session.mark_response_terminal_emission(crate::response::terminal::TerminalEmission::Rendered);
            }
            sink.terminate();
        }
        if let Some(mut batch) = self.semantic_records.take() {
            if self.sealed {
                batch.mark_unavailable();
                if let Some(bytes) = chunk.take() {
                    let _ = batch.push(crate::sse::window::WindowRecord::raw(vec![bytes]).semantic_unavailable());
                }
            } else {
                *chunk = None;
            }
            if sink.is_terminated() {
                // The AI slot won, so the semantic guard will be preempted.
                // Reclaim only translator-produced client bytes from this
                // callback; the batch never contains undecoded provider tail.
                *chunk = batch.into_raw_bytes().ok();
                return;
            }
            // Publish an empty active batch at EOS as an explicit semantic
            // source boundary for a window retained by an earlier callback.
            if (!batch.is_empty() || end_of_stream)
                && session.publish_canonical_records(batch).is_err()
                && session
                    .try_claim_response_terminal(crate::response::terminal::ResponseTerminalCause::ProcessorFailure)
            {
                session.mark_response_terminal_emission(crate::response::terminal::TerminalEmission::NoEvent);
                sink.terminate();
            }
        }
        if !sink.is_terminated() && self.unary.is_none() {
            let chars = if matches!(self.mode, BodyMode::Streaming) {
                self.pending_output_chars
            } else {
                chunk
                    .as_ref()
                    .map(|bytes| u64::try_from(String::from_utf8_lossy(bytes).chars().count()).unwrap_or(u64::MAX))
                    .unwrap_or(0)
            };
            self.usage_shared.add_output_chars(chars);
        }
    }
}

fn stream_error_output_chars() -> u64 {
    u64::try_from(STREAM_ERROR.len().saturating_sub(b"data: \n\n".len())).unwrap_or(u64::MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonStreamingError {
    Invalid,
    OutputTooLarge,
}

struct Preflight {
    nodes: usize,
    limit: usize,
}
impl Preflight {
    fn node(&mut self) -> Result<(), JsonStreamSinkError> {
        self.nodes = self
            .nodes
            .checked_add(1)
            .filter(|n| *n <= self.limit)
            .ok_or(JsonStreamSinkError)?;
        Ok(())
    }
}
impl JsonStreamSelectiveSink for Preflight {
    fn on_token(&mut self, token: JsonStreamToken<'_>) -> Result<(), JsonStreamSinkError> {
        match token {
            JsonStreamToken::ObjectStart
            | JsonStreamToken::ArrayStart
            | JsonStreamToken::Boolean(_)
            | JsonStreamToken::Null => self.node(),
            _ => Ok(()),
        }
    }
    fn on_number(&mut self, _raw: &str) -> Result<(), JsonStreamSinkError> {
        self.node()
    }
    fn on_discarded_string(&mut self, _summary: JsonDiscardedStringSummary) -> Result<(), JsonStreamSinkError> {
        self.node()
    }
}
fn preflight(input: &[u8], limits: UnaryLimits) -> Result<(), NonStreamingError> {
    let mut parser = JsonStreamSelective::new(JsonStreamSelectiveLimits {
        lexical: JsonStreamLimits {
            max_source_bytes: limits.input,
            max_raw_token_bytes: 1538,
            max_decoded_bytes: limits.input,
            max_events: limits.lexical_events,
            max_depth: limits.depth,
            max_retained_bytes: 8192,
            max_work: limits.lexical_work,
        },
        max_key_bytes: limits.key,
        max_number_bytes: 64,
    });
    let mut sink = Preflight {
        nodes: 0,
        limit: limits.nodes,
    };
    parser
        .feed(JsonStreamInput::Data(input), &mut sink)
        .map_err(|_| NonStreamingError::Invalid)?;
    parser
        .feed(JsonStreamInput::Complete, &mut sink)
        .map_err(|_| NonStreamingError::Invalid)
}
#[derive(Serialize)]
struct UnaryCompletion<'a> {
    id: &'a str,
    object: &'static str,
    created: u64,
    model: &'a str,
    choices: [UnaryChoice<'a>; 1],
    usage: UnaryUsage,
}
#[derive(Serialize)]
struct UnaryChoice<'a> {
    index: usize,
    message: UnaryMessage<'a>,
    finish_reason: &'static str,
}
#[derive(Serialize)]
struct UnaryMessage<'a> {
    role: &'static str,
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<UnaryToolCall<'a>>,
}
#[derive(Serialize)]
struct UnaryToolCall<'a> {
    index: usize,
    id: &'a str,
    #[serde(rename = "type")]
    type_: &'static str,
    function: UnaryFunction<'a>,
}
#[derive(Serialize)]
struct UnaryFunction<'a> {
    name: &'a str,
    arguments: String,
}
#[derive(Serialize)]
struct UnaryUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}
const _: () = {
    assert!(std::mem::size_of::<UnaryToolCall<'static>>() <= 128);
    assert!(std::mem::size_of::<AnthropicResponseProcessor>() <= 64 * 1024);
};
fn translate_unary(
    input: &[u8],
    canonical_model: &str,
    created: u64,
    limits: UnaryLimits,
    shared: Option<&AiUsageShared>,
) -> Result<(Vec<u8>, u64, u64, u64, u64), NonStreamingError> {
    let invalid = NonStreamingError::Invalid;
    if canonical_model.is_empty() || canonical_model.len() > MAX_IDENTIFIER_BYTES {
        return Err(invalid);
    }
    preflight(input, limits)?; // Parser scratch is destroyed before the DOM exists.
    let value: Value = serde_json::from_slice(input).map_err(|_| invalid)?;
    let message = value.as_object().ok_or(invalid)?;
    if required_string(message, "type").map_err(|_| invalid)? != "message"
        || required_string(message, "role").map_err(|_| invalid)? != "assistant"
    {
        return Err(invalid);
    }
    let id = bounded_string(message, "id", MAX_IDENTIFIER_BYTES).map_err(|_| invalid)?;
    bounded_string(message, "model", MAX_IDENTIFIER_BYTES).map_err(|_| invalid)?;
    let blocks = message
        .get("content")
        .and_then(Value::as_array)
        .filter(|b| b.len() <= MAX_CONTENT_BLOCKS)
        .ok_or(invalid)?;
    // Validate the supported shape completely before usage or output publication.
    for block in blocks {
        let block = block.as_object().ok_or(invalid)?;
        match required_string(block, "type").map_err(|_| invalid)? {
            "text" => {
                required_string(block, "text").map_err(|_| invalid)?;
            }
            "tool_use" => {
                bounded_string(block, "id", MAX_IDENTIFIER_BYTES).map_err(|_| invalid)?;
                let name = bounded_string(block, "name", MAX_TOOL_NAME_BYTES).map_err(|_| invalid)?;
                if !valid_tool_name(name) || !block.get("input").is_some_and(Value::is_object) {
                    return Err(invalid);
                }
            }
            _ => return Err(invalid),
        }
    }
    let finish_reason =
        map_stop_reason(required_string(message, "stop_reason").map_err(|_| invalid)?).ok_or(invalid)?;
    let mut usage = TokenUsage::default();
    usage
        .update_from(message.get("usage").and_then(Value::as_object).ok_or(invalid)?)
        .map_err(|_| invalid)?;
    if !usage.input_seen || !usage.output_seen {
        return Err(invalid);
    }
    let input_tokens = usage
        .input
        .checked_add(usage.cache_creation)
        .and_then(|n| n.checked_add(usage.cache_read))
        .ok_or(invalid)?;
    let total = input_tokens.checked_add(usage.output).ok_or(invalid)?;
    if let Some(shared) = shared {
        shared.observe_reported_usage(input_tokens, usage.output, usage.cache_read, usage.cache_creation);
        shared.mark_json_complete();
    }
    let mut text = Cow::Borrowed(&[][..]);
    let mut tools = Vec::new();
    let mut arguments_bytes = 0usize;
    for block in blocks {
        let block = block.as_object().ok_or(invalid)?;
        if required_string(block, "type").map_err(|_| invalid)? == "text" {
            append_unary_text(
                &mut text,
                required_string(block, "text").map_err(|_| invalid)?.as_bytes(),
                limits.input,
            )
            .map_err(|_| NonStreamingError::OutputTooLarge)?;
        } else {
            let mut writer = LimitedWriter::new(
                limits
                    .output
                    .checked_sub(arguments_bytes)
                    .ok_or(NonStreamingError::OutputTooLarge)?,
            );
            serde_json::to_writer(&mut writer, block.get("input").ok_or(invalid)?)
                .map_err(|_| NonStreamingError::OutputTooLarge)?;
            arguments_bytes = arguments_bytes
                .checked_add(writer.bytes.len())
                .ok_or(NonStreamingError::OutputTooLarge)?;
            let arguments = String::from_utf8(writer.bytes).map_err(|_| invalid)?;
            if tools.len() == tools.capacity() {
                tools.try_reserve(1).map_err(|_| NonStreamingError::OutputTooLarge)?;
            }
            tools.push(UnaryToolCall {
                index: tools.len(),
                id: required_string(block, "id").map_err(|_| invalid)?,
                type_: "function",
                function: UnaryFunction {
                    name: required_string(block, "name").map_err(|_| invalid)?,
                    arguments,
                },
            });
        }
    }
    let text = std::str::from_utf8(text.as_ref()).map_err(|_| invalid)?;
    let content = if text.is_empty() && !tools.is_empty() {
        None
    } else {
        Some(text)
    };
    let response = UnaryCompletion {
        id,
        object: "chat.completion",
        created,
        model: canonical_model,
        choices: [UnaryChoice {
            index: 0,
            message: UnaryMessage {
                role: "assistant",
                content,
                tool_calls: tools,
            },
            finish_reason,
        }],
        usage: UnaryUsage {
            prompt_tokens: input_tokens,
            completion_tokens: usage.output,
            total_tokens: total,
        },
    };
    let mut writer = LimitedWriter::new(limits.output);
    serde_json::to_writer(&mut writer, &response).map_err(|_| NonStreamingError::OutputTooLarge)?;
    Ok((
        writer.bytes,
        input_tokens,
        usage.output,
        usage.cache_read,
        usage.cache_creation,
    ))
}

fn append_unary_text<'a>(text: &mut Cow<'a, [u8]>, fragment: &'a [u8], limit: usize) -> Result<(), ()> {
    text.len()
        .checked_add(fragment.len())
        .filter(|len| *len <= limit)
        .ok_or(())?;
    if fragment.is_empty() {
        return Ok(());
    }
    match text {
        Cow::Borrowed([]) => *text = Cow::Borrowed(fragment),
        Cow::Borrowed(current) => {
            let mut joined = Vec::new();
            append(&mut joined, current, limit)?;
            append(&mut joined, fragment, limit)?;
            *text = Cow::Owned(joined);
        }
        Cow::Owned(current) => append(current, fragment, limit)?,
    }
    Ok(())
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl LimitedWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
}

impl Write for LimitedWriter {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        let new_len = self
            .bytes
            .len()
            .checked_add(input.len())
            .ok_or_else(|| io::Error::other("bounded response output overflow"))?;
        if new_len > self.limit {
            return Err(io::Error::other("bounded response output overflow"));
        }
        append(&mut self.bytes, input, self.limit)
            .map_err(|_| io::Error::other("cannot grow bounded response output"))?;
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn render_fixed_error(error: FixedError) -> Bytes {
    Bytes::from_static(error.body)
}

fn clear_provider_headers(response: &mut ResponseHeader) {
    // The translated body has a gateway-owned representation, so none of the
    // provider's metadata or representation headers remain authoritative.
    // Normalization restores the small public contract below, while Edgion's
    // own response headers and response-plugin mutations run after this hook.
    // The dense key iterator exposes its last entry directly. Removing that
    // entry also avoids relocating another header while keeping CaseMap in sync.
    while let Some(name) = response.headers.keys().last().cloned() {
        response.remove_header(&name);
    }
}

fn identity_encoding(response: &ResponseHeader) -> bool {
    response_is_identity_encoded(&response.headers)
}

pub(super) fn matches_media_type(response: &ResponseHeader, type_: &str, subtype: &str) -> bool {
    let mut values = response.headers.get_all("content-type").iter();
    let Some(value) = values.next() else {
        return false;
    };
    if values.next().is_some() {
        return false;
    }
    let Ok(value) = value.to_str() else {
        return false;
    };
    let value = value.trim();
    if match (type_, subtype) {
        ("application", "json") => value.eq_ignore_ascii_case("application/json"),
        ("text", "event-stream") => value.eq_ignore_ascii_case("text/event-stream"),
        _ => false,
    } {
        return true;
    }
    value
        .parse::<mime::Mime>()
        .ok()
        .is_some_and(|media| media.type_().as_str() == type_ && media.subtype().as_str() == subtype)
}

fn valid_retry_after(value: &str) -> bool {
    let value = value.trim();
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())) || httpdate::parse_http_date(value).is_ok()
}

fn required_string<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a str, ()> {
    object.get(field).and_then(Value::as_str).ok_or(())
}

fn bounded_string<'a>(object: &'a Map<String, Value>, field: &str, max_bytes: usize) -> Result<&'a str, ()> {
    required_string(object, field).and_then(|value| {
        if !value.is_empty() && value.len() <= max_bytes {
            Ok(value)
        } else {
            Err(())
        }
    })
}

fn required_index(object: &Map<String, Value>, field: &str) -> Result<usize, ()> {
    let value = object.get(field).and_then(Value::as_u64).ok_or(())?;
    usize::try_from(value).map_err(|_| ())
}

fn update_u64(object: &Map<String, Value>, field: &str, target: &mut u64) -> Result<(), ()> {
    if let Some(value) = object.get(field) {
        *target = (*target).max(value.as_u64().ok_or(())?);
    }
    Ok(())
}

fn update_u64_seen(object: &Map<String, Value>, field: &str, target: &mut u64, seen: &mut bool) -> Result<(), ()> {
    if let Some(value) = object.get(field) {
        *target = (*target).max(value.as_u64().ok_or(())?);
        *seen = true;
    }
    Ok(())
}

fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TOOL_NAME_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn map_stop_reason(reason: &str) -> Option<&'static str> {
    match reason {
        "end_turn" | "stop_sequence" | "pause_turn" => Some("stop"),
        "max_tokens" | "model_context_window_exceeded" => Some("length"),
        "tool_use" => Some("tool_calls"),
        "refusal" => Some("content_filter"),
        _ => None,
    }
}

fn append_capped(output: &mut Vec<u8>, bytes: &[u8], limit: usize) -> Result<(), ()> {
    let new_len = output.len().checked_add(bytes.len()).ok_or(())?;
    if new_len > limit {
        return Err(());
    }
    output.extend_from_slice(bytes);
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/ai/conversion/chat_to_anthropic/response_test.rs"]
mod tests;
