//! Bounded Anthropic Messages to OpenAI Chat Completions response translation.

use std::io::{self, Write};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::EdgionErrorCode;
use http::StatusCode;
use pingora_http::ResponseHeader;
use pingora_proxy::ResponseBodySink;
use serde_json::{json, Map, Value};

use crate::ai::accounting::usage::{terminate_over_quota, AiUsageShared};
use crate::ai::protocols::openai::canonical_sse::{write_canonical_record, CanonicalRecordBatch};
use crate::ai::quota::QuotaAccumulator;
use crate::ctx::ResponseStreamKind;
use crate::plugins::runtime::traits::streaming_response_processor::StreamingResponseProcessor;
use crate::plugins::runtime::PluginSession;
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
    message: &'static str,
    type_: &'static str,
    code: &'static str,
    record_translation_failure: bool,
}

const PROVIDER_INVALID_REQUEST: FixedError = FixedError {
    status: 400,
    message: "The AI backend rejected the request.",
    type_: "invalid_request_error",
    code: "provider_invalid_request",
    record_translation_failure: false,
};
const REQUEST_TOO_LARGE: FixedError = FixedError {
    status: 413,
    message: "The request is too large.",
    type_: "invalid_request_error",
    code: "request_too_large",
    record_translation_failure: false,
};
const RATE_LIMIT_EXCEEDED: FixedError = FixedError {
    status: 429,
    message: "The AI backend rate limit was exceeded.",
    type_: "rate_limit_error",
    code: "rate_limit_exceeded",
    record_translation_failure: false,
};
const SERVICE_UNAVAILABLE: FixedError = FixedError {
    status: 503,
    message: "The AI backend is temporarily unavailable.",
    type_: "api_error",
    code: "service_unavailable",
    record_translation_failure: false,
};
const TRANSLATION_UNAVAILABLE: FixedError = FixedError {
    record_translation_failure: true,
    ..SERVICE_UNAVAILABLE
};
const RESPONSE_TOO_LARGE: FixedError = FixedError {
    status: 503,
    message: "The AI backend response is too large.",
    type_: "api_error",
    code: "response_too_large",
    record_translation_failure: true,
};
const RESPONSE_TRANSLATION_FAILED: FixedError = FixedError {
    status: 200,
    message: "The AI backend response could not be processed.",
    type_: "api_error",
    code: "response_translation_failed",
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

    pub(crate) fn new_with_quota(
        canonical_model: String,
        requested_streaming: bool,
        effective_usage_chunk: bool,
        created: u64,
        usage_shared: Arc<AiUsageShared>,
        quota: Option<Arc<QuotaAccumulator>>,
        terminate_on_over_limit: bool,
    ) -> Self {
        Self {
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
        let original_status = response.status.as_u16();
        let retry_after = response
            .headers
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .filter(|value| valid_retry_after(value))
            .map(str::to_owned);

        let error = match original_status {
            400 => Some(PROVIDER_INVALID_REQUEST),
            413 => Some(REQUEST_TOO_LARGE),
            429 => Some(RATE_LIMIT_EXCEEDED),
            200 => self.validate_success_header(response).err(),
            _ => Some(SERVICE_UNAVAILABLE),
        };

        clear_provider_headers(response);
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
            match response
                .headers
                .get_all("content-length")
                .iter()
                .collect::<Vec<_>>()
                .as_slice()
            {
                [] => {}
                [value] => {
                    let length = value
                        .to_str()
                        .ok()
                        .and_then(|value| value.parse::<usize>().ok())
                        .ok_or(TRANSLATION_UNAVAILABLE)?;
                    if length > MAX_BODY_BYTES {
                        return Err(RESPONSE_TOO_LARGE);
                    }
                }
                _ => return Err(TRANSLATION_UNAVAILABLE),
            }
        }
        Ok(())
    }

    fn process_fixed(&mut self, chunk: &mut Option<Bytes>, session: &mut dyn PluginSession, error: FixedError) {
        if !self.fixed_emitted {
            *chunk = Some(Bytes::from(render_fixed_error(error)));
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
        session: &mut dyn PluginSession,
    ) {
        if let Some(bytes) = chunk.take() {
            let Some(new_len) = self.non_streaming_input.len().checked_add(bytes.len()) else {
                self.fail_non_streaming(chunk, session, RESPONSE_TOO_LARGE);
                return;
            };
            if new_len > MAX_BODY_BYTES {
                self.fail_non_streaming(chunk, session, RESPONSE_TOO_LARGE);
                return;
            }
            self.non_streaming_input.extend_from_slice(&bytes);
        }
        if !end_of_stream {
            return;
        }
        match translate_non_streaming(
            &self.non_streaming_input,
            &self.canonical_model,
            self.created,
            MAX_BODY_BYTES,
        ) {
            Ok((body, input_tokens, output_tokens, cache_read_input_tokens, cache_write_input_tokens)) => {
                self.usage_shared.observe_reported_usage(
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_write_input_tokens,
                );
                *chunk = Some(Bytes::from(body));
            }
            Err(NonStreamingError::OutputTooLarge) => {
                self.fail_non_streaming(chunk, session, RESPONSE_TOO_LARGE);
            }
            Err(NonStreamingError::Invalid) => {
                self.fail_non_streaming(chunk, session, RESPONSE_TRANSLATION_FAILED);
            }
        }
        self.non_streaming_input.clear();
    }

    fn fail_non_streaming(&mut self, chunk: &mut Option<Bytes>, session: &mut dyn PluginSession, error: FixedError) {
        self.sealed = true;
        self.non_streaming_input.clear();
        self.record_translation_failure(session);
        *chunk = Some(Bytes::from(render_fixed_error(error)));
    }

    fn process_streaming(&mut self, chunk: &mut Option<Bytes>, end_of_stream: bool, session: &mut dyn PluginSession) {
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
        if event.event().as_deref() != Some(type_) {
            return Err(());
        }
        if !is_known_event_type(type_) {
            return Ok(());
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
        self.push_json_event(
            output,
            &json!({
                "id":self.message_id,
                "object":"chat.completion.chunk",
                "created":self.created,
                "model":self.canonical_model,
                "choices":[choice]
            }),
        )
    }

    fn push_json_event(&mut self, output: &mut Vec<u8>, value: &Value) -> Result<(), ()> {
        let (encoded_record, record) = write_canonical_record(value, MAX_BODY_BYTES)?;
        append_capped(output, &encoded_record, MAX_BODY_BYTES)?;
        self.pending_output_chars = self.pending_output_chars.saturating_add(
            u64::try_from(
                String::from_utf8_lossy(&encoded_record[6..encoded_record.len() - 2])
                    .chars()
                    .count(),
            )
            .unwrap_or(u64::MAX),
        );
        if let Some(batch) = &mut self.semantic_records {
            batch.push(record).map_err(|_| ())?;
        }
        Ok(())
    }

    fn fail_streaming(&mut self, chunk: &mut Option<Bytes>, session: &mut dyn PluginSession) {
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

    fn record_translation_failure(&mut self, session: &mut dyn PluginSession) {
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

    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
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
            BodyMode::NonStreaming => self.process_non_streaming(chunk, end_of_stream, session),
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
        if !sink.is_terminated() {
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

fn translate_non_streaming(
    input: &[u8],
    canonical_model: &str,
    created: u64,
    output_limit: usize,
) -> Result<(Vec<u8>, u64, u64, u64, u64), NonStreamingError> {
    let value: Value = serde_json::from_slice(input).map_err(|_| NonStreamingError::Invalid)?;
    let message = value.as_object().ok_or(NonStreamingError::Invalid)?;
    if required_string(message, "type").map_err(|_| NonStreamingError::Invalid)? != "message"
        || required_string(message, "role").map_err(|_| NonStreamingError::Invalid)? != "assistant"
    {
        return Err(NonStreamingError::Invalid);
    }
    let id = bounded_string(message, "id", MAX_IDENTIFIER_BYTES).map_err(|_| NonStreamingError::Invalid)?;
    let _provider_model =
        bounded_string(message, "model", MAX_IDENTIFIER_BYTES).map_err(|_| NonStreamingError::Invalid)?;
    let blocks = message
        .get("content")
        .and_then(Value::as_array)
        .filter(|blocks| blocks.len() <= MAX_CONTENT_BLOCKS)
        .ok_or(NonStreamingError::Invalid)?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in blocks {
        let block = block.as_object().ok_or(NonStreamingError::Invalid)?;
        match required_string(block, "type").map_err(|_| NonStreamingError::Invalid)? {
            "text" => text.push_str(required_string(block, "text").map_err(|_| NonStreamingError::Invalid)?),
            "tool_use" => {
                let tool_id =
                    bounded_string(block, "id", MAX_IDENTIFIER_BYTES).map_err(|_| NonStreamingError::Invalid)?;
                let name =
                    bounded_string(block, "name", MAX_TOOL_NAME_BYTES).map_err(|_| NonStreamingError::Invalid)?;
                if !valid_tool_name(name) {
                    return Err(NonStreamingError::Invalid);
                }
                let arguments = block
                    .get("input")
                    .filter(|input| input.is_object())
                    .ok_or(NonStreamingError::Invalid)
                    .and_then(|input| serde_json::to_string(input).map_err(|_| NonStreamingError::Invalid))?;
                let index = tool_calls.len();
                tool_calls.push(json!({
                    "index":index,
                    "id":tool_id,
                    "type":"function",
                    "function":{"name":name,"arguments":arguments}
                }));
            }
            _ => return Err(NonStreamingError::Invalid),
        }
    }
    let stop_reason = required_string(message, "stop_reason").map_err(|_| NonStreamingError::Invalid)?;
    let finish_reason = map_stop_reason(stop_reason).ok_or(NonStreamingError::Invalid)?;
    let usage_object = message
        .get("usage")
        .and_then(Value::as_object)
        .ok_or(NonStreamingError::Invalid)?;
    let mut usage = TokenUsage::default();
    usage
        .update_from(usage_object)
        .map_err(|_| NonStreamingError::Invalid)?;
    if !usage.input_seen || !usage.output_seen {
        return Err(NonStreamingError::Invalid);
    }
    let (input_tokens, output_tokens) = usage.normalized();
    let content = if text.is_empty() && !tool_calls.is_empty() {
        Value::Null
    } else {
        Value::String(text)
    };
    let mut response_message = Map::new();
    response_message.insert("role".to_string(), Value::String("assistant".to_string()));
    response_message.insert("content".to_string(), content);
    if !tool_calls.is_empty() {
        response_message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    let response = json!({
        "id":id,
        "object":"chat.completion",
        "created":created,
        "model":canonical_model,
        "choices":[{"index":0,"message":response_message,"finish_reason":finish_reason}],
        "usage":{"prompt_tokens":input_tokens,"completion_tokens":output_tokens,"total_tokens":input_tokens.saturating_add(output_tokens)}
    });
    let mut writer = LimitedWriter::new(output_limit);
    serde_json::to_writer(&mut writer, &response).map_err(|error| {
        if writer.overflowed || error.is_io() {
            NonStreamingError::OutputTooLarge
        } else {
            NonStreamingError::Invalid
        }
    })?;
    Ok((
        writer.bytes,
        input_tokens,
        output_tokens,
        usage.cache_read,
        usage.cache_creation,
    ))
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
    overflowed: bool,
}

impl LimitedWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            overflowed: false,
        }
    }
}

impl Write for LimitedWriter {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        let new_len = self.bytes.len().checked_add(input.len()).ok_or_else(|| {
            self.overflowed = true;
            io::Error::other("bounded response output overflow")
        })?;
        if new_len > self.limit {
            self.overflowed = true;
            return Err(io::Error::other("bounded response output overflow"));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn render_fixed_error(error: FixedError) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "error":{"message":error.message,"type":error.type_,"code":error.code}
    }))
    .unwrap_or_else(|_| b"{\"error\":{\"message\":\"The request could not be processed.\",\"type\":\"api_error\",\"code\":\"internal_error\"}}".to_vec())
}

fn clear_provider_headers(response: &mut ResponseHeader) {
    // The translated body has a gateway-owned representation, so none of the
    // provider's metadata or representation headers remain authoritative.
    // Normalization restores the small public contract below, while Edgion's
    // own response headers and response-plugin mutations run after this hook.
    let names: Vec<_> = response.headers.keys().cloned().collect();
    for name in names {
        response.remove_header(&name);
    }
}

fn identity_encoding(response: &ResponseHeader) -> bool {
    response_is_identity_encoded(&response.headers)
}

fn matches_media_type(response: &ResponseHeader, type_: &str, subtype: &str) -> bool {
    let mut values = response.headers.get_all("content-type").iter();
    let Some(value) = values.next() else {
        return false;
    };
    if values.next().is_some() {
        return false;
    }
    value
        .to_str()
        .ok()
        .and_then(|value| value.trim().parse::<mime::Mime>().ok())
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

fn is_known_event_type(type_: &str) -> bool {
    matches!(
        type_,
        "ping"
            | "error"
            | "message_start"
            | "content_block_start"
            | "content_block_delta"
            | "content_block_stop"
            | "message_delta"
            | "message_stop"
    )
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
