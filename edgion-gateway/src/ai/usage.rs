//! Bounded OpenAI usage extraction for streaming and non-streaming responses.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::resources::edgion_backend::ParsedAiModelPricing;
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

/// Renders one injected mid-stream error event.
///
/// Every injection leads with a blank line, and that prefix is load-bearing
/// rather than cosmetic — do not delete it. These bytes are appended *after*
/// the current chunk, whose end is wherever the upstream's chunk boundary
/// happened to fall: nothing on the observational path rewrites or realigns
/// it. If the upstream ended mid-event, an unprefixed injection would
/// *continue* that partial event instead of starting a new one, so the client
/// would parse one event whose data is truncated JSON glued to the error JSON
/// — invalid, and dropped by every conforming client. The stream would still
/// end cleanly with valid framing, which is exactly what makes the loss silent:
/// the injected event is the client's *only* way to tell a deliberate quota
/// cutoff from an upstream hang-up, and it would be gone. A hostile provider
/// forces this by writing one byte per chunk; ordinary TCP segmentation causes
/// it intermittently. `\n\n` closes any partial event (first newline ends the
/// dangling line, second dispatches it) and dispatches nothing at an already
/// clean boundary, so it is correct unconditionally and needs no decoder-state
/// inspection.
///
/// The body reuses whatever the admission path already renders for the same
/// outcome rather than inventing a mid-stream vocabulary, so a client that
/// learned to key off `error.code` from a 429 recognizes the streaming form
/// the same way. It is kept minimal and free of internal detail: the diagnosis
/// belongs in the access log and `EdgionStatus`, not in a body the client
/// reads.
fn injected_sse_error_event(error: AiProxyError) -> Bytes {
    let (_, body) = render_openai_error(error);
    Bytes::from(format!("\n\nevent: error\ndata: {body}\n\n"))
}

/// Injected before terminating an over-quota stream, reusing the exact
/// message/type/code of the 429 admission-time rejection
/// (`AiProxyError::QuotaExceeded`, see `ai/error.rs`).
pub(crate) static QUOTA_EXCEEDED_SSE_EVENT: LazyLock<Bytes> =
    LazyLock::new(|| injected_sse_error_event(AiProxyError::QuotaExceeded));

/// Injected before terminating a stream whose quota can no longer be
/// evaluated, reusing `AiProxyError::QuotaUnavailable` — the same error the
/// admission path renders when enforcement cannot be performed (no quota
/// identity, quota backend unreachable). Deliberately does not say "SSE
/// decoding degraded": that is an internal detail belonging to the access log
/// and `EdgionStatus`, not to a body the client reads.
pub(crate) static QUOTA_UNENFORCEABLE_SSE_EVENT: LazyLock<Bytes> =
    LazyLock::new(|| injected_sse_error_event(AiProxyError::QuotaUnavailable));

const MAX_TOLERATED_MALFORMED_SSE_EVENTS: u64 = 1;

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
/// function and its `terminate_quota_unenforceable` sibling (mutually
/// exclusive within one chunk) are the only sink producers in the shipped
/// response-processor chains, each pushes at most one ~130-byte event, and the chain stops
/// immediately after (`terminate()` aborts remaining processors for the
/// chunk per the design doc §3.4) — so the sink's 1 MiB per-batch budget can
/// never actually be exhausted by the time this call runs. The branch stays
/// as a fail-safe for the first future processor that shares the sink ahead
/// of this one and spends enough of the budget to matter; its test below
/// exercises the branch directly by pre-filling the budget, which is not
/// evidence of a live path today.
pub(crate) fn terminate_over_quota(
    shared: &AiUsageShared,
    sink: &mut ResponseBodySink,
    session: &mut dyn PluginSession,
    inject: bool,
) {
    terminate_ai_stream(
        shared,
        sink,
        session,
        inject,
        &QUOTA_EXCEEDED_SSE_EVENT,
        EdgionStatus::AiQuotaOverLimitTerminated,
    );
}

/// Ends a streaming AI response whose quota can no longer be evaluated because
/// SSE decoding degraded. Fails closed on the same reasoning as the admission
/// path: a stream whose framing cannot be parsed is a stream whose cost cannot
/// be bounded, and letting it run would leave the upstream generating billable
/// tokens that no flush will ever observe.
///
/// Gated by the caller on exactly the conditions that gate
/// [`terminate_over_quota`], so this adds no new failure shape and no new
/// body-length surface — see `UsageExtractionProcessor::changes_body_length`.
pub(crate) fn terminate_quota_unenforceable(
    shared: &AiUsageShared,
    sink: &mut ResponseBodySink,
    session: &mut dyn PluginSession,
    inject: bool,
) {
    terminate_ai_stream(
        shared,
        sink,
        session,
        inject,
        &QUOTA_UNENFORCEABLE_SSE_EVENT,
        EdgionStatus::AiSseDegradedQuotaTerminated,
    );
}

fn terminate_malformed_quota_unenforceable(
    shared: &AiUsageShared,
    sink: &mut ResponseBodySink,
    session: &mut dyn PluginSession,
    inject: bool,
) {
    terminate_ai_stream(
        shared,
        sink,
        session,
        inject,
        &QUOTA_UNENFORCEABLE_SSE_EVENT,
        EdgionStatus::AiSseMalformedQuotaTerminated,
    );
}

/// Terminating stops the response immediately, so no further task — including
/// the `end_of_stream = true` one — ever reaches the response-body hook. That
/// is load-bearing (design doc §3.4: settlement must not depend on the eos
/// callback), which is why the deliberate ending is latched here, at the only
/// place that decides it, rather than being inferred later from a missing eos.
/// Without it, `AiAttemptRuntimeState::finalize` cannot tell a gateway-initiated
/// cut from an upstream truncation and has to retain the credential's whole
/// input + max-output reservation for the rest of the rate-limit window.
fn terminate_ai_stream(
    shared: &AiUsageShared,
    sink: &mut ResponseBodySink,
    session: &mut dyn PluginSession,
    inject: bool,
    event: &LazyLock<Bytes>,
    terminal_status: EdgionStatus,
) {
    if inject && sink.push((*event).clone()).is_err() {
        session.add_error(EdgionStatus::AiQuotaErrorEventDropped);
    }
    sink.terminate();
    shared.mark_gateway_terminated();
    session.add_error(terminal_status);
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

/// Request-level AI metadata. The model is the effective logical model used
/// for dispatch after any quota downgrade. Under unknown-model passthrough it
/// is client-controlled but bounded to 512 UTF-8 bytes; it never contains
/// prompt text.
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
/// The effective model is a declared name/alias unless unknown-model
/// passthrough retained the bounded client value. No prompt, completion, body,
/// header, or credential is retained here.
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
    pub backend: String,
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
    reported_usage: ArcSwapOption<ReportedAiUsage>,
    sse_degraded: AtomicBool,
    non_streaming_overflowed: AtomicBool,
    gateway_terminated: AtomicBool,
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
        self.reported_usage.load_full().map(|usage| *usage)
    }

    pub fn sse_degraded(&self) -> bool {
        self.sse_degraded.load(Ordering::Acquire)
    }

    pub fn non_streaming_overflowed(&self) -> bool {
        self.non_streaming_overflowed.load(Ordering::Acquire)
    }

    /// True once the gateway itself ended the stream (over quota, or quota no
    /// longer enforceable). Read by attempt settlement, which must not mistake
    /// the absent eos callback for an upstream truncation.
    pub fn gateway_terminated(&self) -> bool {
        self.gateway_terminated.load(Ordering::Acquire)
    }

    fn mark_gateway_terminated(&self) {
        self.gateway_terminated.store(true, Ordering::Release);
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

    pub(crate) fn observe_reported_usage(
        &self,
        input: u64,
        output: u64,
        cache_read_input: u64,
        cache_write_input: u64,
    ) {
        // One processor owns updates. Provider usage frames are cumulative,
        // so every component keeps a high-water mark and never refunds an
        // earlier observation. ArcSwap publishes the immutable multi-field
        // snapshot atomically to concurrent logging readers.
        let current = self.reported_usage();
        let input = current.map_or(input, |usage| usage.input_tokens.max(input));
        let output = current.map_or(output, |usage| usage.output_tokens.max(output));
        let cache_read_input = current.map_or(cache_read_input, |usage| {
            usage.cache_read_input_tokens.max(cache_read_input)
        });
        let cache_write_input = current.map_or(cache_write_input, |usage| {
            usage.cache_write_input_tokens.max(cache_write_input)
        });
        let cache_total = cache_read_input.saturating_add(cache_write_input);
        let (cache_read_input, cache_write_input) = if cache_total <= input {
            (cache_read_input, cache_write_input)
        } else {
            // Preserve authoritative totals but discard inconsistent optional
            // detail so cost cannot exceed the provider-reported input total.
            (0, 0)
        };
        self.reported_usage.store(Some(Arc::new(ReportedAiUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: cache_read_input,
            cache_write_input_tokens: cache_write_input,
        })));
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
    /// Latches the one-shot `AiSseDecodeDegraded` record. The decoder's
    /// degraded flag is sticky, so without this every subsequent chunk of the
    /// stream would append the same status again.
    sse_degraded_reported: bool,
    /// Latches the one-shot `AiSseDecodeMalformed` record. The decoder's
    /// malformed-event count is cumulative, so later chunks must not append
    /// the observation status again.
    sse_malformed_reported: bool,
    /// Whether this instance owns provider usage extraction. Provider-specific
    /// translators disable it while retaining the character estimate.
    extract_reported_usage: bool,
}

impl UsageExtractionProcessor {
    pub fn new(shared: Arc<AiUsageShared>) -> Self {
        Self::new_with_quota(shared, None, false, false)
    }

    /// Track only the translated response size used by estimated settlement.
    ///
    /// Provider-specific processors already own exact usage extraction. This
    /// mode preserves the character fallback for incomplete or
    /// gateway-terminated responses without buffering and parsing the
    /// translated body a second time.
    pub fn new_estimation_only(shared: Arc<AiUsageShared>) -> Self {
        let mut processor = Self::new(shared);
        processor.extract_reported_usage = false;
        processor
    }

    #[cfg(test)]
    pub(crate) fn extracts_reported_usage_for_test(&self) -> bool {
        self.extract_reported_usage
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
            sse_degraded_reported: false,
            sse_malformed_reported: false,
            extract_reported_usage: true,
        }
    }

    fn observe_chunk(&mut self, chunk: &[u8], stream_kind: ResponseStreamKind) {
        match stream_kind {
            ResponseStreamKind::Sse => {
                self.shared.add_output_chars(self.sse_chars.feed(chunk));
                if !self.extract_reported_usage {
                    return;
                }
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
                if self.extract_reported_usage {
                    self.observe_non_streaming(chunk);
                }
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
        if self.extract_reported_usage
            && stream_kind == ResponseStreamKind::None
            && !self.non_streaming_overflowed
            && !self.non_streaming_parsed
        {
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
    /// All three common mutation gates below are required, not just
    /// `quota.is_some()`: a quota exists, the client requested streaming, and
    /// `terminate_on_over_limit` resolved to `true` for `Reject`. Beyond that
    /// gate, actual over-limit, decoder degradation, or the cumulative
    /// malformed-event threshold may terminate the response. A quota'd
    /// request that is non-streaming, or whose route configured
    /// `Throttle`/`Downgrade`, can never have its body mutated by this
    /// processor. This declaration must track the three common gates exactly:
    /// narrower than the real behavior is the dangerous direction (a protocol
    /// bug — the body changes but the header still promises the old length),
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
        // Recorded for every degraded stream, quota'd or not: even where no
        // enforcement is at stake, degradation silently downgrades reported
        // usage to the character estimate, and an operator cannot tell that
        // from the numbers alone. Latched so a sticky flag cannot append this
        // once per remaining chunk.
        if !self.sse_degraded_reported && self.sse.degraded() {
            self.sse_degraded_reported = true;
            session.add_error(EdgionStatus::AiSseDecodeDegraded);
        }
        if !self.sse_malformed_reported && self.sse.malformed_events() != 0 {
            self.sse_malformed_reported = true;
            session.add_error(EdgionStatus::AiSseDecodeMalformed);
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
        //
        // The over-limit, degradation, and cumulative malformed-event arms
        // below share these three gates exactly, so `changes_body_length`
        // continues to describe every possible mutation by this processor.
        if !(self.requested_streaming && self.terminate_on_over_limit && self.quota.is_some()) {
            return;
        }
        if self.quota.as_ref().is_some_and(|quota| quota.over_limit()) {
            // Settlement already happened via observe_chunk; the logging
            // phase records the outcome.
            terminate_over_quota(&self.shared, sink, session, stream_kind == ResponseStreamKind::Sse);
        } else if self.sse.degraded() {
            // `quota.add_output_units` is only ever called from inside
            // `feed_each`'s visitor, and a degraded decoder returns from
            // `feed_each` immediately for the rest of the stream. So from here
            // on nothing grows the accumulator, no flush is triggered, and
            // `over_limit()` can never flip — the branch above is dead for this
            // request. Without this arm the upstream would keep generating
            // billable tokens with enforcement silently switched off.
            //
            // Checked after the over-limit arm on purpose: if the limit was
            // already reached, that is the accurate reason to report, and the
            // two terminations must stay mutually exclusive because either one
            // ends the stream.
            terminate_quota_unenforceable(&self.shared, sink, session, stream_kind == ResponseStreamKind::Sse);
        } else if self.sse.malformed_events() > MAX_TOLERATED_MALFORMED_SSE_EVENTS {
            // A single poisoned event remains tolerated. Repeated poisoned
            // events make quota observation attacker-controllable because
            // they are dropped before the visitor can advance the accumulator.
            // The cumulative count deliberately survives valid events.
            terminate_malformed_quota_unenforceable(
                &self.shared,
                sink,
                session,
                stream_kind == ResponseStreamKind::Sse,
            );
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
    shared.observe_reported_usage(input, output, cache_read, cache_write);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::quota::QuotaFinalStatus;
    use crate::plugins::runtime::traits::session::MockPluginSession;

    async fn feed(processor: &mut UsageExtractionProcessor, chunks: &[(&[u8], bool)], kind: ResponseStreamKind) {
        let mut session = MockPluginSession::new();
        // Tests that assert on recorded statuses build their own session; this
        // shared driver only has to tolerate them.
        session.expect_add_error().return_const(());
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
    async fn estimation_only_counts_without_buffering_or_parsing() {
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new_estimation_only(shared.clone());
        let valid = br#"{"usage":{"prompt_tokens":2,"completion_tokens":3}}"#;
        feed(&mut processor, &[(valid, true)], ResponseStreamKind::None).await;
        assert_eq!(shared.output_chars(), valid.len() as u64);
        assert_eq!(shared.reported_tokens(), None);

        let oversized_shared = Arc::new(AiUsageShared::default());
        let mut oversized_processor = UsageExtractionProcessor::new_estimation_only(oversized_shared.clone());
        let oversized = vec![b'x'; MAX_NON_STREAMING_USAGE_BYTES + 1];
        feed(
            &mut oversized_processor,
            &[(&oversized, true)],
            ResponseStreamKind::None,
        )
        .await;
        assert_eq!(oversized_shared.output_chars(), oversized.len() as u64);
        assert!(!oversized_shared.non_streaming_overflowed());

        let sse_shared = Arc::new(AiUsageShared::default());
        let mut sse_processor = UsageExtractionProcessor::new_estimation_only(sse_shared.clone());
        let data = br#"{"usage":{"prompt_tokens":5,"completion_tokens":8}}"#;
        let event = [b"data: ".as_slice(), data, b"\n\n"].concat();
        feed(&mut sse_processor, &[(&event, true)], ResponseStreamKind::Sse).await;
        assert_eq!(sse_shared.output_chars(), data.len() as u64);
        assert_eq!(sse_shared.reported_tokens(), None);
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
    fn output_counter_saturates() {
        let shared = AiUsageShared::default();
        shared.add_output_chars(u64::MAX);
        shared.add_output_chars(1);
        assert_eq!(shared.output_chars(), u64::MAX);
    }

    #[test]
    fn reported_usage_keeps_component_high_water_marks() {
        let shared = AiUsageShared::default();
        shared.observe_reported_usage(10, 0, 2, 0);
        shared.observe_reported_usage(10, 7, 3, 1);
        shared.observe_reported_usage(9, 3, 1, 0);
        assert_eq!(
            shared.reported_usage(),
            Some(ReportedAiUsage {
                input_tokens: 10,
                output_tokens: 7,
                cache_read_input_tokens: 3,
                cache_write_input_tokens: 1,
            })
        );
    }

    #[tokio::test]
    async fn quota_over_limit_appends_an_error_event_and_terminates_without_dropping_the_current_chunk() {
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        quota.finish(QuotaFinalStatus::OverLimit);
        // terminate_on_over_limit = true: the route's `onExceeded` resolved
        // to `Reject`.
        let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), true, true);

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
        assert!(
            shared.gateway_terminated(),
            "settlement can only tell this apart from an upstream truncation through this latch, \
             because a terminate guarantees no eos callback ever follows"
        );
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
        let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), true, false);

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
        assert!(
            !shared.gateway_terminated(),
            "the latch must follow the terminate, not the over-limit condition: this stream ends \
             at its own eos and settles like any other"
        );
        assert!(sink.peek_extra().is_empty(), "no error event without a terminate");
        assert_eq!(
            body,
            Some(Bytes::from_static(b"data: {\"x\":1}\n\n")),
            "the chunk must pass through untouched"
        );
    }

    /// Builds one unterminated SSE event past the decoder's cap, which is what
    /// permanently degrades it.
    fn oversized_sse_event() -> Bytes {
        Bytes::from(vec![b'x'; crate::sse::DEFAULT_MAX_PENDING_EVENT_BYTES + 1])
    }

    fn malformed_sse_with_valid_middle() -> Bytes {
        Bytes::from_static(b"data: \xff\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"valid\"}}]}\n\ndata: \xfe\n\n")
    }

    #[tokio::test]
    async fn first_malformed_sse_event_is_observed_once_and_tolerated() {
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), true, true);
        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiSseDecodeMalformed)
            .times(1)
            .return_const(());
        let mut sink = ResponseBodySink::new();

        let original = Bytes::from_static(b"data: \xff\n\n");
        let mut body = Some(original.clone());
        processor
            .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;
        let mut later = Some(Bytes::from_static(b"data: {\"choices\":[]}\n\n"));
        processor
            .on_chunk(&mut later, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;

        assert_eq!(body, Some(original), "observation must leave upstream bytes untouched");
        assert!(!sink.is_terminated());
        assert!(sink.peek_extra().is_empty());
        assert!(!shared.gateway_terminated());
    }

    #[tokio::test]
    async fn second_cumulative_malformed_sse_event_fails_quota_closed() {
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), true, true);
        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiSseDecodeMalformed)
            .times(1)
            .return_const(());
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiSseMalformedQuotaTerminated)
            .times(1)
            .return_const(());
        let mut sink = ResponseBodySink::new();
        let original = malformed_sse_with_valid_middle();
        let mut body = Some(original.clone());

        processor
            .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;

        assert_eq!(
            body,
            Some(original),
            "the terminating callback must still forward its current bytes"
        );
        assert!(sink.is_terminated());
        assert!(shared.gateway_terminated());
        assert_eq!(sink.peek_extra().len(), 1);
        assert_eq!(sink.peek_extra()[0], *QUOTA_UNENFORCEABLE_SSE_EVENT);
    }

    #[tokio::test]
    async fn multiple_invalid_lines_in_first_event_consume_one_tolerance() {
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, true);
        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiSseDecodeMalformed)
            .times(1)
            .return_const(());
        let mut sink = ResponseBodySink::new();
        let mut body = Some(Bytes::from_static(b"data: \xff\ndata: \xfe\nevent: \xff\n\n"));

        processor
            .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;

        assert!(!sink.is_terminated());
        assert!(sink.peek_extra().is_empty());
    }

    #[tokio::test]
    async fn malformed_sse_never_terminates_without_every_reject_gate() {
        for (quota, requested_streaming, terminate_on_over_limit) in [
            (None, true, true),
            (Some(Arc::new(QuotaAccumulator::new(16))), true, false),
            (Some(Arc::new(QuotaAccumulator::new(16))), false, true),
        ] {
            let shared = Arc::new(AiUsageShared::default());
            let mut processor = UsageExtractionProcessor::new_with_quota(
                shared.clone(),
                quota,
                requested_streaming,
                terminate_on_over_limit,
            );
            let mut session = MockPluginSession::new();
            session
                .expect_add_error()
                .withf(|status| *status == EdgionStatus::AiSseDecodeMalformed)
                .times(1)
                .return_const(());
            let mut sink = ResponseBodySink::new();
            let mut body = Some(malformed_sse_with_valid_middle());

            processor
                .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
                .await;

            assert!(!sink.is_terminated());
            assert!(sink.peek_extra().is_empty());
            assert!(!shared.gateway_terminated());
        }
    }

    #[tokio::test]
    async fn over_limit_precedes_repeated_malformed_sse_events() {
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        quota.finish(QuotaFinalStatus::OverLimit);
        let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, true);
        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiSseDecodeMalformed)
            .times(1)
            .return_const(());
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiQuotaOverLimitTerminated)
            .times(1)
            .return_const(());
        let mut sink = ResponseBodySink::new();
        let mut body = Some(malformed_sse_with_valid_middle());

        processor
            .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;

        assert_eq!(sink.peek_extra().len(), 1);
        assert_eq!(sink.peek_extra()[0], *QUOTA_EXCEEDED_SSE_EVENT);
    }

    #[tokio::test]
    async fn decoder_degradation_precedes_repeated_malformed_sse_events() {
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, true);
        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiSseDecodeMalformed)
            .times(1)
            .return_const(());
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiSseDecodeDegraded)
            .times(1)
            .return_const(());
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiSseDegradedQuotaTerminated)
            .times(1)
            .return_const(());
        let mut bytes = malformed_sse_with_valid_middle().to_vec();
        bytes.extend_from_slice(&oversized_sse_event());
        let mut body = Some(Bytes::from(bytes));
        let mut sink = ResponseBodySink::new();

        processor
            .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;

        assert_eq!(sink.peek_extra().len(), 1);
        assert_eq!(sink.peek_extra()[0], *QUOTA_UNENFORCEABLE_SSE_EVENT);
        assert!(sink.is_terminated(), "only the degradation cause should terminate");
    }

    #[tokio::test]
    async fn oversized_sse_event_must_not_disable_mid_stream_quota_enforcement() {
        // The regression this locks down: `quota.add_output_units` is only
        // reachable from inside `feed_each`'s visitor, and a degraded decoder
        // returns from `feed_each` immediately for the rest of the stream. So
        // once degraded, nothing grows the accumulator, no flush runs, and
        // `over_limit()` can never flip -- enforcement was silently off for the
        // remainder of the request while the upstream kept billing.
        let shared = Arc::new(AiUsageShared::default());
        // Deliberately NOT over limit: the point is that enforcement is
        // unreachable from here on, not that the limit was already hit.
        let quota = Arc::new(QuotaAccumulator::new(16));
        let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota), true, true);

        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiSseDecodeDegraded)
            .times(1)
            .return_const(());
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiSseDegradedQuotaTerminated)
            .times(1)
            .return_const(());
        let mut sink = ResponseBodySink::new();
        let mut body = Some(oversized_sse_event());
        processor
            .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;

        assert!(shared.sse_degraded());
        assert!(
            sink.is_terminated(),
            "a stream whose framing can no longer be parsed is a stream whose cost can no longer \
             be bounded, so quota enforcement must fail closed"
        );
        assert!(
            shared.gateway_terminated(),
            "this cut is as deliberate as the over-limit one, so it must settle its reservation \
             the same way instead of holding it for the rest of the window"
        );
        let extra = sink.peek_extra();
        assert_eq!(extra.len(), 1);
        assert_eq!(
            extra[0], *QUOTA_UNENFORCEABLE_SSE_EVENT,
            "the client must see the same error the admission path renders when quota cannot be \
             enforced, not a private degradation vocabulary"
        );
    }

    #[test]
    fn every_injected_event_stays_parseable_after_an_unterminated_event() {
        // Pins the BEHAVIOR the leading blank line exists for, not its bytes:
        // asserting the sink carries the constant compares it to itself and
        // would keep passing if the prefix were deleted. The injection lands
        // at whatever byte offset the upstream's chunk boundary fell on, so a
        // conforming client can always be partway through a `data:` line when
        // it arrives -- certainly so on the degraded path, and reachable on
        // the over-limit path through ordinary TCP segmentation or a provider
        // writing one byte per chunk. Decoding that client's-eye view with the
        // same WHATWG decoder is what proves the boundary actually works.
        for (event, expected_code) in [
            (&QUOTA_EXCEEDED_SSE_EVENT, "token_quota_exceeded"),
            (&QUOTA_UNENFORCEABLE_SSE_EVENT, "token_quota_unavailable"),
        ] {
            let mut decoder = crate::sse::SseDecoder::with_default_limit();
            let mut events = Vec::new();
            decoder.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial", &mut events);
            decoder.feed(event.as_ref(), &mut events);

            assert_eq!(
                events.len(),
                2,
                "the injected error must arrive as its own event, not concatenated onto the truncated one"
            );
            assert_eq!(events[1].event.as_deref(), Some("error"));
            let value: serde_json::Value =
                serde_json::from_str(&events[1].data).expect("the injected error must parse as JSON client-side");
            assert_eq!(value["error"]["code"], expected_code);
        }
    }

    #[test]
    fn an_injected_event_dispatches_nothing_extra_at_a_clean_boundary() {
        // The other half of the unconditional-prefix argument: a blank line
        // with nothing buffered is a no-op dispatch, so paying two bytes to
        // cover the mid-event case never costs a spurious event at a boundary
        // that was already clean (the Anthropic translator's output always is,
        // since it re-emits only complete events).
        let mut decoder = crate::sse::SseDecoder::with_default_limit();
        let mut events = Vec::new();
        decoder.feed(b"data: {\"choices\":[]}\n\n", &mut events);
        decoder.feed(QUOTA_EXCEEDED_SSE_EVENT.as_ref(), &mut events);

        assert_eq!(
            events.len(),
            2,
            "the leading blank line must not dispatch an empty event"
        );
        assert_eq!(events[1].event.as_deref(), Some("error"));
    }

    #[tokio::test]
    async fn sse_degradation_without_quota_enforcement_only_records_the_signal() {
        // Degradation predates mid-stream enforcement and was harmless on its
        // own: usage falls back to the character estimate. That must stay
        // true, so an unquota'd stream is never cut for a framing reason.
        let shared = Arc::new(AiUsageShared::default());
        let mut processor = UsageExtractionProcessor::new(shared.clone());

        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiSseDecodeDegraded)
            .times(1)
            .return_const(());
        let mut sink = ResponseBodySink::new();
        let mut body = Some(oversized_sse_event());
        processor
            .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;
        // A second chunk proves the sticky decoder flag does not re-append the
        // status once per remaining chunk; `times(1)` above is the assertion.
        let mut more = Some(Bytes::from_static(b"data: {\"x\":1}\n\n"));
        processor
            .on_chunk(&mut more, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;

        assert!(shared.sse_degraded());
        assert!(!sink.is_terminated(), "no quota enforcement, so nothing to fail closed");
        assert!(sink.peek_extra().is_empty());
    }

    #[tokio::test]
    async fn an_already_over_limit_stream_reports_over_limit_not_degradation() {
        // Both conditions true at once: the accurate reason is that the limit
        // was actually reached, and the two terminations must stay mutually
        // exclusive because either one alone ends the stream.
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new(16));
        quota.finish(QuotaFinalStatus::OverLimit);
        let mut processor = UsageExtractionProcessor::new_with_quota(shared, Some(quota), true, true);

        let mut session = MockPluginSession::new();
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiSseDecodeDegraded)
            .times(1)
            .return_const(());
        session
            .expect_add_error()
            .withf(|status| *status == EdgionStatus::AiQuotaOverLimitTerminated)
            .times(1)
            .return_const(());
        let mut sink = ResponseBodySink::new();
        let mut body = Some(oversized_sse_event());
        processor
            .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
            .await;

        let extra = sink.peek_extra();
        assert_eq!(extra.len(), 1, "exactly one termination, never both");
        assert_eq!(extra[0], *QUOTA_EXCEEDED_SSE_EVENT);
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

    #[tokio::test]
    async fn openai_streaming_processor_updates_quota_only_for_semantic_deltas() {
        let shared = Arc::new(AiUsageShared::default());
        let quota = Arc::new(QuotaAccumulator::new_for_test(100));
        let mut processor = UsageExtractionProcessor::new_with_quota(shared.clone(), Some(quota.clone()), true, false);

        for (event, expected) in [
            (r#"{"choices":[{"delta":{"role":"assistant"}}]}"#, 0),
            (r#"{"choices":[{"delta":{"content":""}}]}"#, 0),
            (r#"{"choices":[{"delta":{"content":"hello"}}]}"#, 1),
            (
                r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":""}}]}}]}"#,
                1,
            ),
            (
                r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"x"}}]}}]}"#,
                2,
            ),
            (
                r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":7,"total_tokens":17}}"#,
                2,
            ),
        ] {
            let framed = format!("data: {event}\n\n");
            feed(&mut processor, &[(framed.as_bytes(), false)], ResponseStreamKind::Sse).await;
            assert_eq!(quota.observed_output_for_test(), expected);
        }
        feed(&mut processor, &[(b"data: [DONE]\n\n", true)], ResponseStreamKind::Sse).await;
        assert_eq!(shared.reported_tokens(), Some((10, 7)));
    }
}
