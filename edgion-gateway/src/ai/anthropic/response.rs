//! Bounded Anthropic Messages to OpenAI Chat Completions response translation.

use std::io::{self, Write};
use std::sync::Arc;

use bytes::Bytes;
use edgion_resources::EdgionStatus;
use http::StatusCode;
use pingora_http::ResponseHeader;
use serde_json::{json, Map, Value};

use crate::ai::quota::QuotaAccumulator;
use crate::ai::usage::AiUsageShared;
use crate::ctx::ResponseStreamKind;
use crate::plugins::runtime::traits::streaming_response_processor::StreamingResponseProcessor;
use crate::plugins::runtime::PluginSession;
use crate::sse::{SseDecoder, SseEvent};

const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 512;
const MAX_TOOL_NAME_BYTES: usize = 64;
const MAX_CONTENT_BLOCKS: usize = 128;

const STREAM_ERROR: &[u8] = b"data: {\"error\":{\"message\":\"The AI service stream could not be processed.\",\"type\":\"api_error\",\"code\":\"response_translation_failed\"}}\n\n";

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
    message: "The AI service rejected the request.",
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
    message: "The AI service rate limit was exceeded.",
    type_: "rate_limit_error",
    code: "rate_limit_exceeded",
    record_translation_failure: false,
};
const SERVICE_UNAVAILABLE: FixedError = FixedError {
    status: 503,
    message: "The AI service is temporarily unavailable.",
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
    message: "The AI service response is too large.",
    type_: "api_error",
    code: "response_too_large",
    record_translation_failure: true,
};
const RESPONSE_TRANSLATION_FAILED: FixedError = FixedError {
    status: 200,
    message: "The AI service response could not be processed.",
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
        )
    }

    pub(crate) fn new_with_quota(
        canonical_model: String,
        requested_streaming: bool,
        effective_usage_chunk: bool,
        created: u64,
        usage_shared: Arc<AiUsageShared>,
        quota: Option<Arc<QuotaAccumulator>>,
    ) -> Self {
        Self {
            canonical_model,
            requested_streaming,
            effective_usage_chunk,
            created,
            usage_shared,
            quota,
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

        strip_provider_headers(response);
        if let Some(error) = error {
            response.status = StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
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
        decoder.feed_each(&input, |event| {
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

    fn process_sse_event(&mut self, event: SseEvent, output: &mut Vec<u8>) -> Result<(), ()> {
        if self.message_stopped {
            return Err(());
        }
        let value: Value = serde_json::from_str(&event.data).map_err(|_| ())?;
        let object = value.as_object().ok_or(())?;
        let type_ = required_string(object, "type")?;
        if event.event.as_deref() != Some(type_) {
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
        append_capped(output, b"data: [DONE]\n\n", MAX_BODY_BYTES)
    }

    fn require_active_message(&self) -> Result<(), ()> {
        if self.message_started && !self.message_stopped {
            Ok(())
        } else {
            Err(())
        }
    }

    fn push_chunk(&self, output: &mut Vec<u8>, choice: Value) -> Result<(), ()> {
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

    fn push_json_event(&self, output: &mut Vec<u8>, value: &Value) -> Result<(), ()> {
        let encoded = serde_json::to_vec(value).map_err(|_| ())?;
        append_capped(output, b"data: ", MAX_BODY_BYTES)?;
        append_capped(output, &encoded, MAX_BODY_BYTES)?;
        append_capped(output, b"\n\n", MAX_BODY_BYTES)
    }

    fn fail_streaming(&mut self, chunk: &mut Option<Bytes>, session: &mut dyn PluginSession) {
        self.sealed = true;
        self.record_translation_failure(session);
        *chunk = Some(Bytes::from_static(STREAM_ERROR));
    }

    fn record_translation_failure(&mut self, session: &mut dyn PluginSession) {
        if !self.error_recorded {
            session.add_error(EdgionStatus::AiResponseTranslationFailed);
            self.error_recorded = true;
        }
    }
}

impl StreamingResponseProcessor for AnthropicResponseProcessor {
    fn name(&self) -> &str {
        "anthropic_response_translation"
    }

    fn on_response_header(&mut self, response: &mut ResponseHeader) {
        self.normalize_header(response);
    }

    fn changes_body_length(&self) -> bool {
        true
    }

    fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        _stream_kind: ResponseStreamKind,
        session: &mut dyn PluginSession,
    ) {
        if self.sealed {
            *chunk = None;
            return;
        }
        match self.mode {
            BodyMode::AwaitingHeader => self.fail_non_streaming(chunk, session, RESPONSE_TRANSLATION_FAILED),
            BodyMode::Fixed(error) => self.process_fixed(chunk, session, error),
            BodyMode::NonStreaming => self.process_non_streaming(chunk, end_of_stream, session),
            BodyMode::Streaming => self.process_streaming(chunk, end_of_stream, session),
        }
    }
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

fn strip_provider_headers(response: &mut ResponseHeader) {
    for name in [
        "request-id",
        "x-request-id",
        "anthropic-request-id",
        "location",
        "set-cookie",
        "content-encoding",
        "etag",
        "last-modified",
        "content-md5",
        "digest",
        "content-range",
        "accept-ranges",
        "content-length",
        "transfer-encoding",
        "retry-after",
    ] {
        let _ = response.remove_header(name);
    }
    let provider_headers: Vec<_> = response
        .headers
        .keys()
        .filter(|name| {
            let name = name.as_str();
            name.starts_with("anthropic-")
                || name.starts_with("x-anthropic-")
                || matches!(
                    name,
                    "x-amzn-requestid" | "x-amzn-request-id" | "x-amz-request-id" | "x-amzn-trace-id"
                )
        })
        .cloned()
        .collect();
    for name in provider_headers {
        response.headers.remove(name);
    }
}

fn identity_encoding(response: &ResponseHeader) -> bool {
    response.headers.get_all("content-encoding").iter().all(|value| {
        value.to_str().ok().is_some_and(|value| {
            value
                .split(',')
                .all(|part| part.trim().eq_ignore_ascii_case("identity"))
        })
    })
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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;

    fn processor(streaming: bool, usage_chunk: bool) -> (AnthropicResponseProcessor, Arc<AiUsageShared>) {
        let usage = Arc::new(AiUsageShared::default());
        (
            AnthropicResponseProcessor::new(
                "claude-canonical".to_string(),
                streaming,
                usage_chunk,
                1_785_772_800,
                Arc::clone(&usage),
            ),
            usage,
        )
    }

    fn header(status: u16, content_type: &str) -> ResponseHeader {
        let mut response = ResponseHeader::build(status, None).expect("valid test status");
        response
            .insert_header("content-type", content_type)
            .expect("valid test content type");
        response
    }

    fn run_chunk(
        processor: &mut AnthropicResponseProcessor,
        session: &mut MockPluginSession,
        bytes: Option<&[u8]>,
        eos: bool,
    ) -> Option<Bytes> {
        let mut chunk = bytes.map(Bytes::copy_from_slice);
        processor.on_chunk(&mut chunk, eos, ResponseStreamKind::None, session);
        chunk
    }

    fn event(name: &str, value: Value) -> String {
        format!(
            "event: {name}\ndata: {}\n\n",
            serde_json::to_string(&value).expect("test event serializes")
        )
    }

    fn streaming_fixture() -> String {
        [
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"claude-provider","role":"assistant","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":3,"cache_creation_input_tokens":5,"cache_read_input_tokens":7,"output_tokens":0}}}),
            ),
            event(
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            ),
            event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}),
            ),
            event(
                "content_block_stop",
                json!({"type":"content_block_stop","index":0}),
            ),
            event(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":5}}),
            ),
            event(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":11}}),
            ),
            event("message_stop", json!({"type":"message_stop"})),
        ]
        .concat()
    }

    #[test]
    fn header_matrix_normalizes_and_strips_provider_metadata() {
        let cases = [
            (400, 400, "invalid_request_error", "provider_invalid_request"),
            (413, 413, "invalid_request_error", "request_too_large"),
            (429, 429, "rate_limit_error", "rate_limit_exceeded"),
            (201, 503, "api_error", "service_unavailable"),
            (500, 503, "api_error", "service_unavailable"),
        ];
        for (upstream, expected, type_, code) in cases {
            let (mut processor, _) = processor(false, false);
            let mut response = header(upstream, "application/json");
            response
                .insert_header("x-request-id", "secret-id")
                .expect("valid header");
            response
                .insert_header("set-cookie", "private=true")
                .expect("valid header");
            response
                .insert_header("etag", "provider-validator")
                .expect("valid header");
            response
                .insert_header("anthropic-ratelimit-tokens-limit", "1000")
                .expect("valid header");
            response
                .insert_header("x-amzn-requestid", "provider-request")
                .expect("valid header");
            response.insert_header("retry-after", "12").expect("valid header");
            processor.on_response_header(&mut response);
            assert_eq!(response.status.as_u16(), expected);
            assert!(response.headers.get("x-request-id").is_none());
            assert!(response.headers.get("set-cookie").is_none());
            assert!(response.headers.get("etag").is_none());
            assert!(response.headers.get("anthropic-ratelimit-tokens-limit").is_none());
            assert!(response.headers.get("x-amzn-requestid").is_none());
            assert_eq!(response.headers.get("retry-after").is_some(), expected == 429);

            let mut session = MockPluginSession::new();
            let body =
                run_chunk(&mut processor, &mut session, Some(b"provider secret body"), false).expect("fixed body");
            let body: Value = serde_json::from_slice(&body).expect("fixed JSON body");
            assert_eq!(body["error"]["type"], type_);
            assert_eq!(body["error"]["code"], code);
            assert!(!body.to_string().contains("provider secret body"));
        }
    }

    #[test]
    fn success_header_requires_media_identity_and_bounded_length() {
        let (mut streaming, _) = processor(true, false);
        let mut good = header(200, "Text/Event-Stream; charset=utf-8");
        good.insert_header("content-encoding", "identity")
            .expect("valid header");
        good.insert_header("content-length", "42").expect("valid header");
        streaming.on_response_header(&mut good);
        assert_eq!(good.status.as_u16(), 200);
        assert_eq!(good.headers["content-type"], "text/event-stream");
        assert!(good.headers.get("content-length").is_none());
        assert!(good.headers.get("content-encoding").is_none());

        for (content_type, encoding, length, expected_code) in [
            ("text/plain", None, None, "service_unavailable"),
            ("application/json", Some("gzip"), None, "service_unavailable"),
            (
                "application/json",
                None,
                Some((MAX_BODY_BYTES + 1).to_string()),
                "response_too_large",
            ),
        ] {
            let (mut p, _) = processor(false, false);
            let mut response = header(200, content_type);
            if let Some(encoding) = encoding {
                response
                    .insert_header("content-encoding", encoding)
                    .expect("valid encoding");
            }
            if let Some(length) = length {
                response.insert_header("content-length", length).expect("valid length");
            }
            p.on_response_header(&mut response);
            assert_eq!(response.status.as_u16(), 503);
            let mut session = MockPluginSession::new();
            session
                .expect_add_error()
                .withf(|status| *status == EdgionStatus::AiResponseTranslationFailed)
                .times(1)
                .return_const(());
            let body = run_chunk(&mut p, &mut session, None, true).expect("fixed body");
            let body: Value = serde_json::from_slice(&body).expect("fixed JSON body");
            assert_eq!(body["error"]["code"], expected_code);
        }
    }

    #[test]
    fn non_streaming_text_tools_stop_and_cache_usage_are_canonical() {
        let (mut processor, usage) = processor(false, false);
        let mut response = header(200, "application/json; charset=utf-8");
        processor.on_response_header(&mut response);
        let provider = serde_json::to_vec(&json!({
            "id":"msg_1","type":"message","role":"assistant","model":"provider-alias",
            "content":[
                {"type":"text","text":"Hello "},
                {"type":"tool_use","id":"toolu_1","name":"weather","input":{"city":"Paris"}},
                {"type":"text","text":"world"}
            ],
            "stop_reason":"tool_use","stop_sequence":null,
            "usage":{"input_tokens":3,"cache_creation_input_tokens":5,"cache_read_input_tokens":7,"output_tokens":11}
        }))
        .expect("fixture serializes");
        let split = provider.len() / 2;
        let mut session = MockPluginSession::new();
        assert!(run_chunk(&mut processor, &mut session, Some(&provider[..split]), false).is_none());
        let translated =
            run_chunk(&mut processor, &mut session, Some(&provider[split..]), true).expect("translated response");
        let translated: Value = serde_json::from_slice(&translated).expect("OpenAI response JSON");
        assert_eq!(translated["model"], "claude-canonical");
        assert_eq!(translated["choices"][0]["message"]["content"], "Hello world");
        assert_eq!(translated["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(translated["choices"][0]["message"]["tool_calls"][0]["index"], 0);
        assert_eq!(
            translated["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"city\":\"Paris\"}"
        );
        assert_eq!(translated["usage"]["prompt_tokens"], 15);
        assert_eq!(translated["usage"]["completion_tokens"], 11);
        assert_eq!(usage.reported_tokens(), Some((15, 11)));
        let detailed = usage.reported_usage().unwrap();
        assert_eq!(detailed.cache_read_input_tokens, 7);
        assert_eq!(detailed.cache_write_input_tokens, 5);
    }

    #[test]
    fn malformed_non_streaming_response_is_fixed_redacted_and_sealed() {
        let (mut processor, _) = processor(false, false);
        processor.on_response_header(&mut header(200, "application/json"));
        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiResponseTranslationFailed)
            .times(1)
            .return_const(());
        let body = run_chunk(
            &mut processor,
            &mut session,
            Some(b"{\"provider_secret\":\"must-not-leak\"}"),
            true,
        )
        .expect("fixed failure");
        let rendered = String::from_utf8(body.to_vec()).expect("fixed UTF-8");
        assert!(rendered.contains("response_translation_failed"));
        assert!(!rendered.contains("must-not-leak"));
        assert!(run_chunk(&mut processor, &mut session, Some(b"later raw bytes"), true).is_none());
    }

    #[test]
    fn streaming_translation_handles_every_byte_split_and_exact_usage() {
        let fixture = streaming_fixture();
        for split in 0..=fixture.len() {
            let (mut processor, usage) = processor(true, true);
            processor.on_response_header(&mut header(200, "text/event-stream"));
            let mut session = MockPluginSession::new();
            let first = run_chunk(&mut processor, &mut session, Some(&fixture.as_bytes()[..split]), false);
            let second = run_chunk(&mut processor, &mut session, Some(&fixture.as_bytes()[split..]), true);
            let mut output = Vec::new();
            if let Some(first) = first {
                output.extend_from_slice(&first);
            }
            if let Some(second) = second {
                output.extend_from_slice(&second);
            }
            let output = String::from_utf8(output).expect("OpenAI SSE is UTF-8");
            assert!(output.contains("\"role\":\"assistant\""), "split={split}");
            assert!(output.contains("\"content\":\"Hello\""), "split={split}");
            assert!(output.contains("\"finish_reason\":\"stop\""), "split={split}");
            assert!(output.contains("\"prompt_tokens\":15"), "split={split}");
            assert!(output.ends_with("data: [DONE]\n\n"), "split={split}");
            assert!(!output.contains("claude-provider"));
            assert_eq!(usage.reported_tokens(), Some((15, 11)));
        }
    }

    #[test]
    fn streaming_tools_forward_argument_fragments_without_accumulating() {
        let fixture = [
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_tools","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":1}}}),
            ),
            event(
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"weather","input":{}}}),
            ),
            event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"city\":"}}),
            ),
            event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"Paris\"}"}}),
            ),
            event("content_block_stop", json!({"type":"content_block_stop","index":0})),
            event(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
            ),
            event("message_stop", json!({"type":"message_stop"})),
        ]
        .concat();
        let (mut processor, usage) = processor(true, false);
        processor.on_response_header(&mut header(200, "text/event-stream"));
        let mut session = MockPluginSession::new();
        let output =
            run_chunk(&mut processor, &mut session, Some(fixture.as_bytes()), true).expect("translated stream");
        let output = String::from_utf8(output.to_vec()).expect("UTF-8 output");
        assert!(output.contains("\"name\":\"weather\""));
        assert!(output.contains("\"arguments\":\"\""));
        assert!(output.contains("\"arguments\":\"{\\\"city\\\":\""));
        assert!(output.contains("\"arguments\":\"\\\"Paris\\\"}\""));
        assert!(output.contains("\"finish_reason\":\"tool_calls\""));
        assert!(!output.contains("\"usage\":"));
        assert_eq!(usage.reported_tokens(), Some((1, 2)));
    }

    #[test]
    fn streaming_quota_tracks_semantic_deltas_and_cumulative_usage_monotonically() {
        let usage = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new_for_test(100));
        let mut processor = AnthropicResponseProcessor::new_with_quota(
            "claude-canonical".to_string(),
            true,
            false,
            1_785_772_800,
            usage.clone(),
            Some(quota.clone()),
        );
        processor.on_response_header(&mut header(200, "text/event-stream"));
        let mut session = MockPluginSession::new();
        let mut send = |type_: &str, value: Value, expected: u64| {
            let framed = event(type_, value);
            run_chunk(&mut processor, &mut session, Some(framed.as_bytes()), false);
            assert_eq!(quota.observed_output_for_test(), expected);
        };

        send(
            "message_start",
            json!({"type":"message_start","message":{"type":"message","id":"msg_quota","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":10}}}),
            0,
        );
        send(
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            0,
        );
        send(
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":""}}),
            0,
        );
        send(
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}),
            1,
        );
        send("content_block_stop", json!({"type":"content_block_stop","index":0}), 1);
        send(
            "content_block_start",
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup","input":{}}}),
            1,
        );
        send(
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":""}}),
            1,
        );
        send(
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
            2,
        );
        send("content_block_stop", json!({"type":"content_block_stop","index":1}), 2);
        send(
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":5}}),
            5,
        );
        send(
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":3}}),
            5,
        );
        send(
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":11}}),
            11,
        );
        let stop = event("message_stop", json!({"type":"message_stop"}));
        run_chunk(&mut processor, &mut session, Some(stop.as_bytes()), true);
        assert_eq!(usage.reported_tokens(), Some((10, 11)));
    }

    #[test]
    fn malformed_order_clears_same_callback_output_and_emits_one_error() {
        let fixture = [
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":1}}}),
            ),
            event(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"must-not-leak"}}),
            ),
        ]
        .concat();
        let (mut processor, _) = self::processor(true, false);
        processor.on_response_header(&mut header(200, "text/event-stream"));
        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiResponseTranslationFailed)
            .times(1)
            .return_const(());
        let output =
            run_chunk(&mut processor, &mut session, Some(fixture.as_bytes()), false).expect("fixed stream error");
        assert_eq!(output.as_ref(), STREAM_ERROR);
        assert!(!String::from_utf8_lossy(&output).contains("must-not-leak"));
        assert!(run_chunk(&mut processor, &mut session, Some(b"provider secret"), true).is_none());
    }

    #[test]
    fn unknown_events_and_ping_are_ignored_but_known_name_mismatch_fails() {
        let unknown = event("future_event", json!({"type":"future_event","secret":"ignored"}));
        let ping = event("ping", json!({"type":"ping"}));
        let (mut processor, _) = self::processor(true, false);
        processor.on_response_header(&mut header(200, "text/event-stream"));
        let mut session = MockPluginSession::new();
        assert!(run_chunk(
            &mut processor,
            &mut session,
            Some(format!("{unknown}{ping}").as_bytes()),
            false
        )
        .is_none());

        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiResponseTranslationFailed)
            .times(1)
            .return_const(());
        let mismatch = event("ping", json!({"type":"message_start"}));
        let output =
            run_chunk(&mut processor, &mut session, Some(mismatch.as_bytes()), false).expect("fixed stream error");
        assert_eq!(output.as_ref(), STREAM_ERROR);

        let (mut second_processor, _) = self::processor(true, false);
        second_processor.on_response_header(&mut header(200, "text/event-stream"));
        let mut session = MockPluginSession::new();
        session.expect_add_error().times(1).return_const(());
        let mismatch = event("error", json!({"type":"future_event"}));
        let output = run_chunk(&mut second_processor, &mut session, Some(mismatch.as_bytes()), false)
            .expect("known event-name mismatch fails");
        assert_eq!(output.as_ref(), STREAM_ERROR);
    }

    #[test]
    fn missing_required_usage_fields_fail_instead_of_publishing_zero() {
        let provider = serde_json::to_vec(&json!({
            "id":"msg_1","type":"message","role":"assistant","model":"provider",
            "content":[{"type":"text","text":"Hello"}],"stop_reason":"end_turn",
            "usage":{"input_tokens":3}
        }))
        .expect("fixture serializes");
        let (mut processor, usage) = processor(false, false);
        processor.on_response_header(&mut header(200, "application/json"));
        let mut session = MockPluginSession::new();
        session.expect_add_error().times(1).return_const(());
        let output = run_chunk(&mut processor, &mut session, Some(&provider), true).expect("fixed error");
        assert!(String::from_utf8_lossy(&output).contains("response_translation_failed"));
        assert_eq!(usage.reported_tokens(), None);

        let stream = [
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":3,"output_tokens":0}}}),
            ),
            event(
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            ),
            event("content_block_stop", json!({"type":"content_block_stop","index":0})),
            event(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{}}),
            ),
            event("message_stop", json!({"type":"message_stop"})),
        ]
        .concat();
        let (mut processor, usage) = self::processor(true, false);
        processor.on_response_header(&mut header(200, "text/event-stream"));
        let mut session = MockPluginSession::new();
        session.expect_add_error().times(1).return_const(());
        let output =
            run_chunk(&mut processor, &mut session, Some(stream.as_bytes()), true).expect("fixed stream error");
        assert_eq!(output.as_ref(), STREAM_ERROR);
        assert_eq!(usage.reported_tokens(), None);
    }

    #[test]
    fn malformed_utf8_and_nonempty_initial_content_fail_closed() {
        let valid_start = event(
            "message_start",
            json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"provider","role":"assistant","content":[],"usage":{"input_tokens":1}}}),
        );
        let cases = [
            b"event: ping\ndata: \xff\n\n".to_vec(),
            event(
                "message_start",
                json!({"type":"message_start","message":{"type":"message","id":"msg_1","model":"provider","role":"assistant","content":[{"type":"text","text":"lost"}],"usage":{"input_tokens":1}}}),
            )
            .into_bytes(),
            format!(
                "{valid_start}{}",
                event(
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"lost"}}),
                )
            )
            .into_bytes(),
            format!(
                "{valid_start}{}",
                event(
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"weather","input":{"lost":true}}}),
                )
            )
            .into_bytes(),
        ];

        for input in cases {
            let (mut processor, _) = self::processor(true, false);
            processor.on_response_header(&mut header(200, "text/event-stream"));
            let mut session = MockPluginSession::new();
            session
                .expect_add_error()
                .withf(|status| *status == EdgionStatus::AiResponseTranslationFailed)
                .times(1)
                .return_const(());
            let output = run_chunk(&mut processor, &mut session, Some(&input), false).expect("fixed stream error");
            assert_eq!(output.as_ref(), STREAM_ERROR);
            assert!(!String::from_utf8_lossy(&output).contains("lost"));
        }
    }

    #[test]
    fn message_stop_is_terminal_and_eos_requires_an_idle_decoder() {
        for suffix in [
            event("ping", json!({"type":"ping"})),
            event("future_event", json!({"type":"future_event"})),
            "event: ping\ndata: {".to_string(),
        ] {
            let input = format!("{}{suffix}", streaming_fixture());
            let (mut processor, _) = self::processor(true, false);
            processor.on_response_header(&mut header(200, "text/event-stream"));
            let mut session = MockPluginSession::new();
            session
                .expect_add_error()
                .withf(|status| *status == EdgionStatus::AiResponseTranslationFailed)
                .times(1)
                .return_const(());
            let output = run_chunk(&mut processor, &mut session, Some(input.as_bytes()), true)
                .expect("terminal violation emits a fixed stream error");
            assert_eq!(output.as_ref(), STREAM_ERROR);
            assert!(!String::from_utf8_lossy(&output).contains("[DONE]"));
        }
    }

    #[test]
    fn bounds_and_stop_reason_mapping_are_stable() {
        for (anthropic, openai) in [
            ("end_turn", "stop"),
            ("stop_sequence", "stop"),
            ("pause_turn", "stop"),
            ("max_tokens", "length"),
            ("model_context_window_exceeded", "length"),
            ("tool_use", "tool_calls"),
            ("refusal", "content_filter"),
        ] {
            assert_eq!(map_stop_reason(anthropic), Some(openai));
        }
        assert_eq!(map_stop_reason("not_a_real_reason"), None);
        assert!(valid_tool_name("tool-name_1"));
        assert!(!valid_tool_name("bad tool"));
        assert!(!valid_tool_name(&"a".repeat(MAX_TOOL_NAME_BYTES + 1)));

        let oversized = vec![b'x'; MAX_BODY_BYTES + 1];
        let (mut processor, _) = processor(false, false);
        processor.on_response_header(&mut header(200, "application/json"));
        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiResponseTranslationFailed)
            .times(1)
            .return_const(());
        let output = run_chunk(&mut processor, &mut session, Some(&oversized), false).expect("bounded error");
        let output: Value = serde_json::from_slice(&output).expect("fixed JSON");
        assert_eq!(output["error"]["code"], "response_too_large");
    }
}
