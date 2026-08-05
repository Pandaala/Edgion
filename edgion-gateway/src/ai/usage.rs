//! Bounded OpenAI usage extraction for streaming and non-streaming responses.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::resources::edgion_ai_service::ParsedAiModelPricing;
use edgion_resources::EdgionStatus;
use pingora_proxy::ResponseBodySink;
use serde::Serialize;

use crate::ai::error::render_openai_error;
use crate::ai::quota::{AiQuotaRequest, QuotaAccumulator};
use crate::ai::AiProxyError;
use crate::ctx::ResponseStreamKind;
use crate::plugins::runtime::traits::streaming_response_processor::StreamingResponseProcessor;
use crate::plugins::runtime::PluginSession;
use crate::sse::SseDecoder;

pub const MAX_NON_STREAMING_USAGE_BYTES: usize = 1024 * 1024;

/// Injected before terminating an over-quota stream. Reuses the exact
/// message/type/code the 429 admission-time rejection renders
/// (`AiProxyError::QuotaExceeded`, see `ai/error.rs`), rather than a
/// private `quota_exceeded` vocabulary: a client that already learned to key
/// off `error.code` from the admission path recognizes a mid-stream cutoff
/// the same way. Kept minimal and free of internal detail: the diagnosis
/// belongs in the access log and EdgionStatus, not in a body the client reads.
pub(crate) static QUOTA_EXCEEDED_SSE_EVENT: LazyLock<Bytes> = LazyLock::new(|| {
    let (_, body) = render_openai_error(AiProxyError::QuotaExceeded);
    Bytes::from(format!("event: error\ndata: {body}\n\n"))
});

/// Ends a streaming AI response over quota, shared by every processor that
/// can reach this decision (`UsageExtractionProcessor` for OpenAI-shaped
/// wire formats, `AnthropicResponseProcessor` for the translated Anthropic
/// path). `inject` controls whether [`QUOTA_EXCEEDED_SSE_EVENT`] is appended
/// first: only when the caller knows the wire format the client receives is
/// SSE, since the event bytes are SSE-framed and would corrupt anything else.
///
/// Records `EdgionStatus::AiQuotaOverLimitTerminated` unconditionally so an
/// operator can tell a gateway-initiated cutoff apart from an upstream
/// hang-up, and additionally `AiQuotaErrorEventDropped` when the injection
/// itself could not fit the sink's per-batch emit budget (an earlier
/// processor already spent it) — the client then sees a bare truncation with
/// no in-band diagnostic, which is otherwise invisible outside the body.
///
/// `AiQuotaErrorEventDropped` is currently unreachable in production: this
/// function is the only sink producer in the shipped response-processor
/// chains, it pushes at most this one ~130-byte event, and the chain stops
/// immediately after (`terminate()` aborts remaining processors for the
/// chunk per the design doc §3.4) — so the sink's 1 MiB per-batch budget can
/// never actually be exhausted by the time this call runs. The branch stays
/// as a fail-safe for the first future processor that shares the sink ahead
/// of this one and spends enough of the budget to matter; its test below
/// exercises the branch directly by pre-filling the budget, which is not
/// evidence of a live path today.
pub(crate) fn terminate_over_quota(sink: &mut ResponseBodySink, session: &mut dyn PluginSession, inject: bool) {
    if inject && sink.push(QUOTA_EXCEEDED_SSE_EVENT.clone()).is_err() {
        session.add_error(EdgionStatus::AiQuotaErrorEventDropped);
    }
    sink.terminate();
    session.add_error(EdgionStatus::AiQuotaOverLimitTerminated);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProviderLabel {
    OpenAi,
    OpenAiCompatible,
    Anthropic,
}

impl AiProviderLabel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::OpenAiCompatible => "openai_compatible",
            Self::Anthropic => "anthropic",
        }
    }
}

/// Request-level AI metadata. The model is either declared by the selected
/// service or the fixed `other` bucket; it never contains prompt text.
pub struct AiRequestContext {
    pub provider: AiProviderLabel,
    pub model: String,
    pub pricing: Option<ParsedAiModelPricing>,
    pub requested_streaming: bool,
    pub usage: Arc<AiUsageShared>,
    pub quota: Option<AiQuotaRequest>,
    /// Fixed admission outcome when quota was configured but intentionally
    /// bypassed. Successful enforcement is filled during final reconciliation.
    pub quota_status: Option<&'static str>,
}

/// Final per-request GenAI usage committed by the logging phase.
///
/// The selected model is already constrained to a declared name/alias or the
/// fixed `other` bucket. No prompt, completion, body, header, or credential is
/// retained here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GenAiSummary {
    pub provider: &'static str,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_read_input_tokens: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_write_input_tokens: u64,
    pub streaming: bool,
    pub usage_estimated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_status: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    /// Bounded, credential-safe summaries for dispatched upstream attempts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<GenAiAttempt>,
    /// True only if an internal invariant would have exceeded the eight-entry
    /// or 8 KiB attempt-log boundary.
    #[serde(default, skip_serializing_if = "is_false")]
    pub attempts_truncated: bool,
}

const fn is_zero(value: &u64) -> bool {
    *value == 0
}

const fn is_false(value: &bool) -> bool {
    !*value
}

/// One bounded access-log record for a dispatched AI upstream attempt.
///
/// This type deliberately has no endpoint, address, Secret, request/response
/// body, provider error text, or tool-argument field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GenAiAttempt {
    pub number: u8,
    pub service: String,
    pub provider: &'static str,
    pub credential: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<GenAiAttemptFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    pub usage_source: GenAiAttemptUsageSource,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenAiAttemptFailure {
    RetryableStatus,
    AmbiguousTransport,
    BackendRequestTimeout,
    IncompleteResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenAiAttemptUsageSource {
    Reported,
    Estimated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportedAiUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
}

impl ReportedAiUsage {
    pub const fn ordinary_input_tokens(self) -> u64 {
        self.input_tokens
            .saturating_sub(self.cache_read_input_tokens)
            .saturating_sub(self.cache_write_input_tokens)
    }
}

/// Cross-phase usage state. Only counters and flags cross into logging; body
/// bytes and parsed JSON remain private to `UsageExtractionProcessor`.
#[derive(Debug, Default)]
pub struct AiUsageShared {
    input_chars: AtomicU64,
    output_chars: AtomicU64,
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
    cache_read_input_tokens: AtomicU64,
    cache_write_input_tokens: AtomicU64,
    reported_usage: AtomicBool,
    sse_degraded: AtomicBool,
    non_streaming_overflowed: AtomicBool,
}

impl AiUsageShared {
    pub fn set_input_chars(&self, chars: u64) {
        self.input_chars.store(chars, Ordering::Relaxed);
    }

    pub fn input_chars(&self) -> u64 {
        self.input_chars.load(Ordering::Relaxed)
    }

    pub fn output_chars(&self) -> u64 {
        self.output_chars.load(Ordering::Relaxed)
    }

    pub fn reported_tokens(&self) -> Option<(u64, u64)> {
        self.reported_usage()
            .map(|usage| (usage.input_tokens, usage.output_tokens))
    }

    pub fn reported_usage(&self) -> Option<ReportedAiUsage> {
        if self.reported_usage.load(Ordering::Acquire) {
            Some(ReportedAiUsage {
                input_tokens: self.input_tokens.load(Ordering::Relaxed),
                output_tokens: self.output_tokens.load(Ordering::Relaxed),
                cache_read_input_tokens: self.cache_read_input_tokens.load(Ordering::Relaxed),
                cache_write_input_tokens: self.cache_write_input_tokens.load(Ordering::Relaxed),
            })
        } else {
            None
        }
    }

    pub fn sse_degraded(&self) -> bool {
        self.sse_degraded.load(Ordering::Acquire)
    }

    pub fn non_streaming_overflowed(&self) -> bool {
        self.non_streaming_overflowed.load(Ordering::Acquire)
    }

    fn add_output_chars(&self, delta: u64) {
        let _ = self
            .output_chars
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_add(delta))
            });
    }

    #[cfg(test)]
    pub(crate) fn add_output_units_for_test(&self, delta: u64) {
        self.add_output_chars(delta);
    }

    pub(crate) fn publish_reported_usage(
        &self,
        input: u64,
        output: u64,
        cache_read_input: u64,
        cache_write_input: u64,
    ) {
        // One processor owns publication. Release the flag only after both
        // values are visible so logging never observes a half-published pair.
        if self.reported_usage.load(Ordering::Relaxed) {
            return;
        }
        self.input_tokens.store(input, Ordering::Relaxed);
        self.output_tokens.store(output, Ordering::Relaxed);
        let cache_total = cache_read_input.saturating_add(cache_write_input);
        let (cache_read_input, cache_write_input) = if cache_total <= input {
            (cache_read_input, cache_write_input)
        } else {
            // Preserve authoritative totals but discard inconsistent optional
            // detail so cost cannot exceed the provider-reported input total.
            (0, 0)
        };
        self.cache_read_input_tokens.store(cache_read_input, Ordering::Relaxed);
        self.cache_write_input_tokens
            .store(cache_write_input, Ordering::Relaxed);
        self.reported_usage.store(true, Ordering::Release);
    }
}

/// Calculate a deterministic fixed-point USD estimate and render it without
/// floating point. Prices carry six fractional USD digits per million tokens;
/// multiplying by a token count therefore yields a value with 12 USD decimals.
pub fn estimated_cost_usd(pricing: ParsedAiModelPricing, usage: ReportedAiUsage) -> String {
    const USD_SCALE: u128 = 1_000_000_000_000;
    let numerator = (usage.ordinary_input_tokens() as u128) * (pricing.input.get() as u128)
        + (usage.cache_read_input_tokens as u128) * (pricing.cache_read_input.get() as u128)
        + (usage.cache_write_input_tokens as u128) * (pricing.cache_write_input.get() as u128)
        + (usage.output_tokens as u128) * (pricing.output.get() as u128);
    let whole = numerator / USD_SCALE;
    let fraction = numerator % USD_SCALE;
    if fraction == 0 {
        return whole.to_string();
    }
    let fraction = format!("{fraction:012}");
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

/// Incremental UTF-8 character counter. A valid leading byte is counted
/// immediately, so an abort with no EOS still accounts for an incomplete
/// scalar. At most four bytes are retained to validate later continuation
/// bytes; bytes proven invalid are counted individually.
#[derive(Default)]
struct Utf8CharCounter {
    pending: [u8; 4],
    pending_len: usize,
    expected_len: usize,
}

#[derive(Default)]
enum SseDataLineState {
    #[default]
    MatchingName,
    NameComplete,
    ValueStart,
    Value,
    Ignore,
}

/// Fixed-state observer for the semantic values of SSE `data` fields.
///
/// This intentionally runs independently of exact-usage decoding: it can count
/// content before a line or event is complete, and continues after the bounded
/// decoder degrades. Framing and all non-`data` fields are ignored. Multiple
/// `data` lines in one event contribute the newline inserted by SSE semantics.
#[derive(Default)]
struct SseDataCharCounter {
    chars: Utf8CharCounter,
    state: SseDataLineState,
    matched_name_bytes: usize,
    event_has_data: bool,
    line_semantics_started: bool,
    skip_lf_after_cr: bool,
    bom_prefix: [u8; 3],
    bom_prefix_len: usize,
    bom_checked: bool,
}

impl SseDataCharCounter {
    fn feed(&mut self, bytes: &[u8]) -> u64 {
        let mut count = 0_u64;
        let mut index = 0;

        while !self.bom_checked && index < bytes.len() {
            self.bom_prefix[self.bom_prefix_len] = bytes[index];
            self.bom_prefix_len += 1;
            index += 1;

            let prefix = &self.bom_prefix[..self.bom_prefix_len];
            if !b"\xef\xbb\xbf".starts_with(prefix) {
                let replay = self.bom_prefix;
                let replay_len = self.bom_prefix_len;
                self.bom_prefix_len = 0;
                self.bom_checked = true;
                for &byte in &replay[..replay_len] {
                    count = count.saturating_add(self.feed_byte(byte));
                }
            } else if self.bom_prefix_len == self.bom_prefix.len() {
                self.bom_prefix_len = 0;
                self.bom_checked = true;
            }
        }

        if !self.bom_checked {
            return count;
        }
        for &byte in &bytes[index..] {
            count = count.saturating_add(self.feed_byte(byte));
        }
        count
    }

    fn feed_byte(&mut self, byte: u8) -> u64 {
        if self.skip_lf_after_cr {
            self.skip_lf_after_cr = false;
            if byte == b'\n' {
                return 0;
            }
        }

        match byte {
            b'\r' => {
                let count = self.finish_line();
                self.skip_lf_after_cr = true;
                count
            }
            b'\n' => self.finish_line(),
            _ => self.feed_line_byte(byte),
        }
    }

    fn feed_line_byte(&mut self, byte: u8) -> u64 {
        match self.state {
            SseDataLineState::MatchingName => {
                const NAME: &[u8] = b"data";
                if byte == NAME[self.matched_name_bytes] {
                    self.matched_name_bytes += 1;
                    if self.matched_name_bytes == NAME.len() {
                        self.state = SseDataLineState::NameComplete;
                    }
                } else {
                    self.state = SseDataLineState::Ignore;
                }
                0
            }
            SseDataLineState::NameComplete => {
                if byte == b':' {
                    self.state = SseDataLineState::ValueStart;
                } else {
                    self.state = SseDataLineState::Ignore;
                }
                0
            }
            SseDataLineState::ValueStart => {
                self.state = SseDataLineState::Value;
                if byte == b' ' {
                    0
                } else {
                    self.feed_value_byte(byte)
                }
            }
            SseDataLineState::Value => self.feed_value_byte(byte),
            SseDataLineState::Ignore => 0,
        }
    }

    fn feed_value_byte(&mut self, byte: u8) -> u64 {
        let mut count = self.start_line_semantics();
        count = count.saturating_add(self.chars.feed(&[byte]));
        count
    }

    fn start_line_semantics(&mut self) -> u64 {
        if self.line_semantics_started {
            return 0;
        }
        self.line_semantics_started = true;
        if self.event_has_data {
            self.chars.feed(b"\n")
        } else {
            0
        }
    }

    fn finish_line(&mut self) -> u64 {
        let blank_line = matches!(self.state, SseDataLineState::MatchingName) && self.matched_name_bytes == 0;
        let is_data = matches!(
            self.state,
            SseDataLineState::NameComplete | SseDataLineState::ValueStart | SseDataLineState::Value
        );
        let mut count = 0_u64;
        if is_data {
            count = count.saturating_add(self.start_line_semantics());
            count = count.saturating_add(self.chars.finish());
            self.event_has_data = true;
        }
        if blank_line {
            self.event_has_data = false;
        }
        self.state = SseDataLineState::MatchingName;
        self.matched_name_bytes = 0;
        self.line_semantics_started = false;
        count
    }

    fn finish(&mut self) -> u64 {
        self.chars.finish()
    }
}

impl Utf8CharCounter {
    fn feed(&mut self, bytes: &[u8]) -> u64 {
        let mut count = 0_u64;
        for &byte in bytes {
            count = count.saturating_add(self.feed_byte(byte));
        }
        count
    }

    fn feed_byte(&mut self, byte: u8) -> u64 {
        if self.pending_len == 0 {
            if byte.is_ascii() {
                return 1;
            }
            self.expected_len = match byte {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => return 1,
            };
            self.pending[0] = byte;
            self.pending_len = 1;
            return 1;
        }

        if byte & 0xc0 != 0x80 {
            // The leading byte was counted when received. Continuations that
            // are now known to be invalid each add one more character.
            let invalid = u64::try_from(self.pending_len.saturating_sub(1)).unwrap_or(u64::MAX);
            self.pending_len = 0;
            self.expected_len = 0;
            return invalid.saturating_add(self.feed_byte(byte));
        }

        self.pending[self.pending_len] = byte;
        self.pending_len += 1;
        if self.pending_len != self.expected_len {
            return 0;
        }

        let len = self.pending_len;
        self.pending_len = 0;
        self.expected_len = 0;
        if std::str::from_utf8(&self.pending[..len]).is_ok() {
            0
        } else {
            u64::try_from(len.saturating_sub(1)).unwrap_or(u64::MAX)
        }
    }

    fn finish(&mut self) -> u64 {
        self.pending_len = 0;
        self.expected_len = 0;
        0
    }
}

pub struct UsageExtractionProcessor {
    shared: Arc<AiUsageShared>,
    quota: Option<Arc<QuotaAccumulator>>,
    /// Whether the client asked for a streaming response. Gates termination
    /// separately from `stream_kind`: a non-streaming completion has already
    /// finished generating by the time its body is written, so there is no
    /// token burn left to stop, and truncating it under a stripped
    /// content-length would only read as a network failure to the client.
    requested_streaming: bool,
    /// Resolved from the route's `AiTokenQuotaConfig.onExceeded`: `true` only
    /// for `Reject`. `Throttle`/`Downgrade` are an operator-declared "do not
    /// hard-fail this user" policy already honored at admission; gating the
    /// mid-stream cut on this too means it stays honored for the rest of the
    /// request instead of being silently overridden once the stream starts.
    terminate_on_over_limit: bool,
    sse: SseDecoder,
    non_streaming: Vec<u8>,
    non_streaming_overflowed: bool,
    non_streaming_parsed: bool,
    non_streaming_chars: Utf8CharCounter,
    sse_chars: SseDataCharCounter,
    chars_finished: bool,
}

impl UsageExtractionProcessor {
    pub fn new(shared: Arc<AiUsageShared>) -> Self {
        Self::new_with_quota(shared, None, false, false)
    }

    pub fn new_with_quota(
        shared: Arc<AiUsageShared>,
        quota: Option<Arc<QuotaAccumulator>>,
        requested_streaming: bool,
        terminate_on_over_limit: bool,
    ) -> Self {
        Self {
            shared,
            quota,
            requested_streaming,
            terminate_on_over_limit,
            sse: SseDecoder::with_default_limit(),
            non_streaming: Vec::new(),
            non_streaming_overflowed: false,
            non_streaming_parsed: false,
            non_streaming_chars: Utf8CharCounter::default(),
            sse_chars: SseDataCharCounter::default(),
            chars_finished: false,
        }
    }

    fn observe_chunk(&mut self, chunk: &[u8], stream_kind: ResponseStreamKind) {
        match stream_kind {
            ResponseStreamKind::Sse => {
                self.shared.add_output_chars(self.sse_chars.feed(chunk));
                let shared = &self.shared;
                let quota = self.quota.as_ref();
                self.sse.feed_each(chunk, |event| {
                    if event.data != "[DONE]" {
                        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(event.data.as_bytes()) {
                            if let Some(quota) = quota {
                                quota.add_output_units(approximate_openai_output_units(&value));
                            }
                            try_publish_usage_value(shared, &value);
                        }
                    }
                });
                if self.sse.degraded() {
                    self.shared.sse_degraded.store(true, Ordering::Release);
                }
            }
            ResponseStreamKind::None => {
                self.shared.add_output_chars(self.non_streaming_chars.feed(chunk));
                self.observe_non_streaming(chunk);
            }
        }
    }

    fn observe_non_streaming(&mut self, chunk: &[u8]) {
        if self.non_streaming_overflowed {
            return;
        }
        let fits = self
            .non_streaming
            .len()
            .checked_add(chunk.len())
            .is_some_and(|size| size <= MAX_NON_STREAMING_USAGE_BYTES);
        if fits {
            self.non_streaming.extend_from_slice(chunk);
        } else {
            self.non_streaming_overflowed = true;
            self.shared.non_streaming_overflowed.store(true, Ordering::Release);
            self.non_streaming.clear();
        }
    }

    fn finish(&mut self, stream_kind: ResponseStreamKind) {
        if !self.chars_finished {
            let remaining = match stream_kind {
                ResponseStreamKind::Sse => self.sse_chars.finish(),
                ResponseStreamKind::None => self.non_streaming_chars.finish(),
            };
            self.shared.add_output_chars(remaining);
            self.chars_finished = true;
        }
        if stream_kind == ResponseStreamKind::None && !self.non_streaming_overflowed && !self.non_streaming_parsed {
            self.non_streaming_parsed = true;
            try_publish_usage(&self.shared, &self.non_streaming);
        }
    }
}

fn approximate_openai_output_units(value: &serde_json::Value) -> u64 {
    value
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .filter(|choice| {
                    let Some(delta) = choice.get("delta").and_then(serde_json::Value::as_object) else {
                        return false;
                    };
                    delta
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|content| !content.is_empty())
                        || delta
                            .get("tool_calls")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|calls| {
                                calls.iter().any(|call| {
                                    call.get("function")
                                        .and_then(serde_json::Value::as_object)
                                        .and_then(|function| function.get("arguments"))
                                        .and_then(serde_json::Value::as_str)
                                        .is_some_and(|arguments| !arguments.is_empty())
                                })
                            })
                })
                .count() as u64
        })
        .unwrap_or(0)
}

#[async_trait]
impl StreamingResponseProcessor for UsageExtractionProcessor {
    fn name(&self) -> &str {
        "ai-usage"
    }

    /// Terminating on over-quota shortens the body, and injecting an error
    /// event lengthens it. Either way the upstream `content-length` cannot
    /// stand, so it must be stripped at the response-header boundaries.
    ///
    /// All three conditions below are required, not just `quota.is_some()`:
    /// `on_chunk`'s termination gate (below) only ever fires when
    /// `requested_streaming && terminate_on_over_limit && quota.over_limit()`,
    /// so a quota'd request that is non-streaming, or whose route configured
    /// `Throttle`/`Downgrade` instead of `Reject`, can never actually have its
    /// body mutated by this processor — declaring `true` for it anyway would
    /// strip `content-length` from a response this processor provably leaves
    /// untouched. This declaration must track that gate exactly: narrower
    /// than the real behavior is the dangerous direction (a protocol bug —
    /// the body changes but the header still promises the old length),
    /// broader only costs a needless strip.
    fn changes_body_length(&self) -> bool {
        self.quota.is_some() && self.requested_streaming && self.terminate_on_over_limit
    }

    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        stream_kind: ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    ) {
        if let Some(bytes) = chunk.as_deref() {
            self.observe_chunk(bytes, stream_kind);
        }
        if end_of_stream {
            self.finish(stream_kind);
            return;
        }
        // Gated on `requested_streaming`, not `stream_kind`. In the ordinary
        // non-streaming case (`stream_kind == None`) `quota.over_limit()`
        // can never even become true here: nothing in the `None` arm of
        // `observe_chunk` feeds the quota accumulator, so there is nothing
        // for this gate to hold back there. The case that actually reaches
        // this branch with something to hold back is a provider that
        // answers with an SSE body despite the client's non-streaming
        // request (`stream_kind == Sse` while `requested_streaming` is
        // false) — the SSE arm of `observe_chunk` does feed the
        // accumulator, so without this gate that response could be
        // truncated out from under a caller who asked for, and expects, one
        // complete JSON body. An unrecognized *streaming* format (requested
        // streaming, but not classified as SSE) still terminates — the burn
        // really does stop there — it just skips the SSE-framed injection.
        //
        // Also gated on `terminate_on_over_limit`, resolved from the route's
        // `onExceeded` policy: `Reject` is the only action that wants a
        // hard mid-stream cut. `Throttle`/`Downgrade` are the operator
        // declaring "do not hard-fail this user" -- already honored at
        // admission -- so under those actions the stream runs to completion
        // exactly as it did before this gate existed, even once quota goes
        // over limit.
        if self.requested_streaming
            && self.terminate_on_over_limit
            && self.quota.as_ref().is_some_and(|quota| quota.over_limit())
        {
            // Settlement already happened via observe_chunk; the logging
            // phase records the outcome.
            terminate_over_quota(sink, session, stream_kind == ResponseStreamKind::Sse);
        }
    }
}

fn try_publish_usage(shared: &AiUsageShared, json: &[u8]) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(json) else {
        return;
    };
    try_publish_usage_value(shared, &value);
}

fn try_publish_usage_value(shared: &AiUsageShared, value: &serde_json::Value) {
    let Some(usage) = value.get("usage").and_then(serde_json::Value::as_object) else {
        return;
    };
    let Some(input) = usage.get("prompt_tokens").and_then(serde_json::Value::as_u64) else {
        return;
    };
    let Some(output) = usage.get("completion_tokens").and_then(serde_json::Value::as_u64) else {
        return;
    };
    let cache_read = usage
        .get("prompt_tokens_details")
        .and_then(serde_json::Value::as_object)
        .and_then(|details| details.get("cached_tokens"))
        .and_then(serde_json::Value::as_u64)
        .or_else(|| usage.get("cache_read_input_tokens").and_then(serde_json::Value::as_u64))
        .unwrap_or(0);
    let cache_write = usage
        .get("cache_creation_input_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    shared.publish_reported_usage(input, output, cache_read, cache_write);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::quota::QuotaFinalStatus;
    use crate::plugins::runtime::traits::session::MockPluginSession;

    async fn feed(processor: &mut UsageExtractionProcessor, chunks: &[(&[u8], bool)], kind: ResponseStreamKind) {
        let mut session = MockPluginSession::new();
        let mut sink = ResponseBodySink::new();
        for (chunk, eos) in chunks {
            let original = Bytes::copy_from_slice(chunk);
            let mut body = Some(original.clone());
            processor.on_chunk(&mut body, *eos, kind, &mut sink, &mut session).await;
            assert_eq!(body, Some(original), "usage observation must not mutate the response");
        }
    }

    #[tokio::test]
    async fn extracts_non_streaming_usage_at_eos() {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        feed(
            &mut processor,
            &[(
                br#"{"id":"x","usage":{"prompt_tokens":12,"completion_tokens":7,"total_tokens":19}}"#,
                true,
            )],
            ResponseStreamKind::None,
        )
        .await;
        assert_eq!(shared.reported_tokens(), Some((12, 7)));
    }

    #[tokio::test]
    async fn extracts_final_split_sse_usage_and_ignores_done() {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        feed(
            &mut processor,
            &[
                (b"data: {\"usage\":{\"prompt_tokens\":4,", false),
                (b"\"completion_tokens\":9}}\n\n", false),
                (b"data: [DONE]\n\n", true),
            ],
            ResponseStreamKind::Sse,
        )
        .await;
        assert_eq!(shared.reported_tokens(), Some((4, 9)));
    }

    #[tokio::test]
    async fn malformed_or_partial_usage_remains_unreported() {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        feed(
            &mut processor,
            &[(br#"{"usage":{"prompt_tokens":1}}"#, true)],
            ResponseStreamKind::None,
        )
        .await;
        assert_eq!(shared.reported_tokens(), None);

        let malformed_shared = Arc::new(AiUsageShared::default());
        let mut malformed = UsageExtractionProcessor::new(malformed_shared.clone());
        feed(&mut malformed, &[(b"{not-json", true)], ResponseStreamKind::None).await;
        assert_eq!(malformed_shared.reported_tokens(), None);
    }

    #[tokio::test]
    async fn non_streaming_exact_boundary_is_accepted_and_overflow_degrades() {
        let exact_shared = Arc::new(AiUsageShared::default());
        let mut exact = UsageExtractionProcessor::new(exact_shared.clone());
        let mut body = vec![b' '; MAX_NON_STREAMING_USAGE_BYTES];
        let usage = br#"{"usage":{"prompt_tokens":2,"completion_tokens":3}}"#;
        body[..usage.len()].copy_from_slice(usage);
        feed(&mut exact, &[(&body, true)], ResponseStreamKind::None).await;
        assert!(!exact_shared.non_streaming_overflowed());
        assert_eq!(exact_shared.reported_tokens(), Some((2, 3)));

        let overflow_shared = Arc::new(AiUsageShared::default());
        let mut overflow = UsageExtractionProcessor::new(overflow_shared.clone());
        let oversized = vec![b'x'; MAX_NON_STREAMING_USAGE_BYTES + 1];
        feed(&mut overflow, &[(&oversized, true)], ResponseStreamKind::None).await;
        assert!(overflow_shared.non_streaming_overflowed());
        assert_eq!(overflow_shared.reported_tokens(), None);
    }

    #[tokio::test]
    async fn sse_overflow_degrades_without_stopping_character_count() {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        let mut oversized = vec![b'x'; crate::sse::DEFAULT_MAX_PENDING_EVENT_BYTES + 1];
        oversized.extend_from_slice(b"\n\n");
        feed(
            &mut processor,
            &[(&oversized, false), (b"data: more", true)],
            ResponseStreamKind::Sse,
        )
        .await;
        assert!(shared.sse_degraded());
        assert_eq!(shared.output_chars(), 4);
    }

    #[tokio::test]
    async fn sse_counts_only_data_values_and_ignores_heartbeat_and_framing() {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        feed(
            &mut processor,
            &[(b": keepalive\nevent: chunk\nid: 7\nretry: 1000\ndata: hello\n\n", true)],
            ResponseStreamKind::Sse,
        )
        .await;
        assert_eq!(shared.output_chars(), 5);
    }

    #[tokio::test]
    async fn sse_split_data_prefix_and_aborted_content_are_counted_incrementally() {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        feed(
            &mut processor,
            &[(b"da", false), (b"ta: hel", false), (b"lo", false)],
            ResponseStreamKind::Sse,
        )
        .await;
        assert_eq!(shared.output_chars(), 5);
    }

    #[tokio::test]
    async fn sse_multiline_data_uses_one_semantic_newline() {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        feed(
            &mut processor,
            &[(b"data: first\ndata: second\n\n", true)],
            ResponseStreamKind::Sse,
        )
        .await;
        assert_eq!(shared.output_chars(), "first\nsecond".chars().count() as u64);
    }

    #[tokio::test]
    async fn sse_utf8_split_inside_data_value_counts_one_scalar() {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        feed(
            &mut processor,
            &[(b"data: \xe2", false), (b"\x82", false), (b"\xac\n\n", true)],
            ResponseStreamKind::Sse,
        )
        .await;
        assert_eq!(shared.output_chars(), 1);
    }

    #[tokio::test]
    async fn many_small_sse_events_in_one_chunk_are_observed_without_batching() {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        let chunk = "data: x\n\n".repeat(10_000);
        feed(&mut processor, &[(chunk.as_bytes(), true)], ResponseStreamKind::Sse).await;
        assert_eq!(shared.output_chars(), 10_000);
    }

    #[tokio::test]
    async fn split_utf8_and_abort_without_eos_publish_incremental_characters() {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        feed(
            &mut processor,
            &[(b"a\xe2", false), (b"\x82\xacb", false)],
            ResponseStreamKind::None,
        )
        .await;
        assert_eq!(shared.output_chars(), 3, "a, euro sign, b");
        assert_eq!(shared.reported_tokens(), None);
    }

    #[tokio::test]
    async fn eos_flushes_incomplete_utf8_and_is_idempotent() {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        feed(
            &mut processor,
            &[(b"\xe2", false), (b"", true), (b"", true)],
            ResponseStreamKind::None,
        )
        .await;
        assert_eq!(shared.output_chars(), 1);
    }

    #[tokio::test]
    async fn incomplete_utf8_is_visible_before_eos_and_invalid_bytes_are_deterministic() {
        let abort_shared = Arc::new(AiUsageShared::default());
        let mut abort = UsageExtractionProcessor::new(abort_shared.clone());
        feed(&mut abort, &[(b"\xe2\x82", false)], ResponseStreamKind::None).await;
        assert_eq!(abort_shared.output_chars(), 1, "an incomplete scalar is one character");

        let invalid_shared = Arc::new(AiUsageShared::default());
        let mut invalid = UsageExtractionProcessor::new(invalid_shared.clone());
        feed(&mut invalid, &[(b"\xe2\x82A", false)], ResponseStreamKind::None).await;
        assert_eq!(invalid_shared.output_chars(), 3, "invalid bytes count individually");
    }

    #[test]
    fn counters_saturate() {
        let shared = AiUsageShared::default();
        shared.set_input_chars(u64::MAX);
        shared.add_output_chars(u64::MAX);
        shared.add_output_chars(1);
        assert_eq!(shared.input_chars(), u64::MAX);
        assert_eq!(shared.output_chars(), u64::MAX);
    }

    #[tokio::test]
    async fn shared_state_never_retains_request_response_or_event_text() {
        let shared = Arc::new(AiUsageShared::default());
        shared.set_input_chars("INPUT-FIXTURE".chars().count() as u64);
        let mut processor = UsageExtractionProcessor::new(shared.clone());
        feed(
            &mut processor,
            &[(b"data: OUTPUT-EVENT-FIXTURE\n\n", false)],
            ResponseStreamKind::Sse,
        )
        .await;
        let debug = format!("{shared:?}");
        assert!(!debug.contains("INPUT-FIXTURE"));
        assert!(!debug.contains("OUTPUT-EVENT-FIXTURE"));
        assert!(!debug.contains("data:"));
    }

    #[tokio::test]
    async fn quota_over_limit_appends_an_error_event_and_terminates_without_dropping_the_current_chunk() {
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        quota.finish(QuotaFinalStatus::OverLimit);
        // terminate_on_over_limit = true: the route's `onExceeded` resolved
        // to `Reject`.
        let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, true);

        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiQuotaOverLimitTerminated)
            .times(1)
            .return_const(());
        let mut sink = ResponseBodySink::new();
        let mut body = Some(Bytes::from_static(b"data: {\"x\":1}\n\n"));
        processor
            .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;

        assert!(sink.is_terminated(), "over-limit must end the stream");
        let extra = sink.peek_extra();
        assert_eq!(extra.len(), 1);
        assert_eq!(
            extra[0], *QUOTA_EXCEEDED_SSE_EVENT,
            "the injected event must reuse the same message/type/code the 429 admission-time \
             rejection renders"
        );
        // This proves the in-flight chunk is not dropped when a terminate is
        // queued, NOT that the sink writes it before the terminate — that
        // ordering is a pump property (`ResponseBodySink::push`/`terminate`
        // have no ordering relationship with each other), exercised by the
        // fork's own `terminate_after_commit_ends_the_body_cleanly` test.
        assert_eq!(
            body,
            Some(Bytes::from_static(b"data: {\"x\":1}\n\n")),
            "the current chunk must not be dropped when the response is about to terminate"
        );
    }

    #[tokio::test]
    async fn quota_over_limit_on_a_non_streaming_response_does_not_terminate() {
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        quota.finish(QuotaFinalStatus::OverLimit);
        // requested_streaming = false: an ordinary non-streaming completion
        // has already finished generating by the time its body streams, so
        // there is no burn left to stop and terminating would only truncate
        // a JSON body for no benefit. terminate_on_over_limit = true (Reject)
        // isolates this from the separate onExceeded gate covered below.
        let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), false, true);

        let mut session = MockPluginSession::new();
        let mut sink = ResponseBodySink::new();
        let mut body = Some(Bytes::from_static(
            br#"{"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        ));
        processor
            .on_chunk(&mut body, false, ResponseStreamKind::None, &mut sink, &mut session)
            .await;

        assert!(
            !sink.is_terminated(),
            "a non-streaming completion must not be cut short"
        );
        assert!(sink.peek_extra().is_empty());
    }

    #[tokio::test]
    async fn quota_over_limit_is_not_terminated_under_throttle_or_downgrade() {
        // `terminate_on_over_limit = false` is what the plugin computes for
        // BOTH `Throttle` and `Downgrade` (see `AiProxy`'s plan construction)
        // -- the processor only ever sees the resolved boolean, so a single
        // `false` case here covers both. Either way the operator declared
        // "do not hard-fail this user", and that must still hold once the
        // stream is over limit, exactly as it did before mid-stream
        // termination existed.
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        quota.finish(QuotaFinalStatus::OverLimit);
        let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, false);

        let mut session = MockPluginSession::new();
        let mut sink = ResponseBodySink::new();
        let mut body = Some(Bytes::from_static(b"data: {\"x\":1}\n\n"));
        processor
            .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;

        assert!(
            !sink.is_terminated(),
            "Throttle/Downgrade must let an over-limit stream run to completion"
        );
        assert!(sink.peek_extra().is_empty(), "no error event without a terminate");
        assert_eq!(
            body,
            Some(Bytes::from_static(b"data: {\"x\":1}\n\n")),
            "the chunk must pass through untouched"
        );
    }

    #[tokio::test]
    async fn quota_over_limit_push_failure_is_recorded_and_still_terminates() {
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        quota.finish(QuotaFinalStatus::OverLimit);
        let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, true);

        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiQuotaErrorEventDropped)
            .times(1)
            .return_const(());
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiQuotaOverLimitTerminated)
            .times(1)
            .return_const(());
        let mut sink = ResponseBodySink::new();
        // Exhaust the per-batch emit budget an earlier processor might have
        // already spent, so this processor's own push cannot fit.
        sink.push(Bytes::from(vec![0u8; pingora_proxy::RESPONSE_BODY_EMIT_BUDGET]))
            .expect("filling the budget exactly must succeed");
        let mut body = Some(Bytes::from_static(b"data: {\"x\":1}\n\n"));
        processor
            .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;

        assert!(
            sink.is_terminated(),
            "a dropped diagnostic event must not stop the gateway from terminating"
        );
    }

    #[test]
    fn changes_body_length_tracks_every_precondition_the_terminate_gate_checks() {
        let shared = Arc::new(AiUsageShared::default());
        let observing = UsageExtractionProcessor::new(shared.clone());
        assert!(
            !observing.changes_body_length(),
            "pure observation must stay framing-neutral"
        );

        // Quota attached, streaming, Reject: this is the only combination
        // that can actually terminate or inject, so it is the only one that
        // must declare true.
        let quota = Arc::new(QuotaAccumulator::new(16));
        let enforcing = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), true, true);
        assert!(
            enforcing.changes_body_length(),
            "an attached quota accumulator under Reject can truncate or extend the body, so \
             content-length must be stripped"
        );

        // Quota attached, non-streaming: `on_chunk`'s gate requires
        // `requested_streaming`, so this processor never mutates the body
        // regardless of quota state -- declaring true here would strip
        // content-length from a response this processor provably never
        // touches.
        let quota = Arc::new(QuotaAccumulator::new(16));
        let non_streaming = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), false, true);
        assert!(
            !non_streaming.changes_body_length(),
            "a non-streaming completion is never mutated by this processor"
        );

        // Quota attached, streaming, but the route's onExceeded resolved to
        // Throttle/Downgrade (terminate_on_over_limit = false): the
        // termination gate can never fire, so this processor never mutates
        // the body here either.
        let quota = Arc::new(QuotaAccumulator::new(16));
        let throttled = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, false);
        assert!(
            !throttled.changes_body_length(),
            "Throttle/Downgrade never terminate, so content-length must not be stripped"
        );
    }

    #[test]
    fn openai_incremental_quota_counts_only_semantic_output_deltas() {
        let units = |json: &[u8]| {
            let value: serde_json::Value = serde_json::from_slice(json).unwrap();
            approximate_openai_output_units(&value)
        };
        assert_eq!(
            units(br#"{"choices":[{"delta":{"role":"assistant"}},{"delta":{"content":""}}]}"#),
            0
        );
        assert_eq!(
            units(
                br#"{"choices":[{"delta":{"content":"hello"}},{"delta":{"tool_calls":[{"function":{"arguments":"x"}}]}}]}"#
            ),
            2
        );
        assert_eq!(
            units(
                br#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":2}}"#
            ),
            0
        );
    }
}
