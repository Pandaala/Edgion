//! Bounded OpenAI usage extraction for streaming and non-streaming responses.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, LazyLock};

use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::resources::edgion_backend::ParsedAiModelPricing;
use edgion_resources::resources::edgion_plugins::AiUsageObservation;
use edgion_resources::EdgionErrorCode;
use pingora_proxy::ResponseBodySink;
#[cfg(test)]
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Serialize;

use crate::ai::protocols::openai::canonical_sse::{observe_canonical_event, CanonicalRecordBatch};
use crate::ai::protocols::openai::error::render_openai_error;
use crate::ai::protocols::openai::json_usage::{JsonUsageObserver, JsonUsageOutcome, ResponseOutcome};
use crate::ai::quota::{AiQuotaRequest, QuotaAccumulator};
use crate::response::terminal::{ResponseTerminalCause, TerminalEmission};

use crate::ai::AiProxyError;
use crate::ctx::ResponseStreamKind;
use crate::plugins::runtime::traits::streaming_response_processor::StreamingResponseProcessor;
use crate::plugins::runtime::PluginSession;
use crate::sse::window::WindowRecord;
use crate::sse::{SseBlockStatus, SseDecoder, SseRecordRef};

#[cfg(test)]
include!("../../../tests/support/ai/accounting/usage_test/27_macro_definition.rs");

#[cfg(test)]
include!("../../../tests/support/ai/accounting/usage_test/68_macro_definition.rs");

#[cfg(test)]
include!("../../../tests/support/ai/accounting/usage_test/89_macro_definition.rs");

#[cfg(test)]
include!("../../../tests/support/ai/accounting/usage_test/104_macro_definition.rs");

#[cfg(test)]
include!("../../../tests/support/ai/accounting/usage_test/119_macro_definition.rs");

#[cfg(test)]
include!("../../../tests/support/ai/accounting/usage_test/158_macro_definition.rs");

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
/// belongs in the access log and `EdgionErrorCode`, not in a body the client
/// reads.
fn injected_sse_error_event(error: AiProxyError) -> Bytes {
    let (_, body) = render_openai_error(error);
    Bytes::from(format!("\n\nevent: error\ndata: {body}\n\n"))
}

/// Injected before terminating an over-quota stream, reusing the exact
/// message/type/code of the 429 admission-time rejection
/// (`AiProxyError::QuotaExceeded`, see `ai/protocols/openai/error.rs`).
pub(crate) static QUOTA_EXCEEDED_SSE_EVENT: LazyLock<Bytes> =
    LazyLock::new(|| injected_sse_error_event(AiProxyError::QuotaExceeded));

/// Injected before terminating a stream whose quota can no longer be
/// evaluated, reusing `AiProxyError::QuotaUnavailable` — the same error the
/// admission path renders when enforcement cannot be performed (no quota
/// identity, quota backend unreachable). Deliberately does not say "SSE
/// decoding degraded": that is an internal detail belonging to the access log
/// and `EdgionErrorCode`, not to a body the client reads.
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
/// Records `EdgionErrorCode::AiQuotaOverLimitTerminated` unconditionally so an
/// operator can tell a gateway-initiated cutoff apart from an upstream
/// hang-up, and additionally `AiQuotaErrorEventDropped` when the injection
/// itself could not fit either of the sink's independent per-batch byte or
/// nonempty-chunk emit budgets (an earlier processor already spent one) — the
/// client then sees a bare truncation with no in-band diagnostic, which is
/// otherwise invisible outside the body.
///
/// `AiQuotaErrorEventDropped` is currently unreachable in production: this
/// function and its `terminate_quota_unenforceable` sibling (mutually
/// exclusive within one chunk) are the only sink producers in the shipped
/// response-processor chains, each pushes at most one ~130-byte event, and the
/// chain stops immediately after (`terminate()` aborts remaining processors
/// for the chunk per the design doc §3.4) — so neither the sink's 1 MiB byte
/// budget nor its 2048-nonempty-chunk budget can be exhausted by the time this
/// call runs. The branch stays as a fail-safe for the first future processor
/// that shares the sink ahead of this one and spends enough of either budget
/// to matter; its test below exercises the branch directly by pre-filling the
/// byte budget, which is not evidence of a live path today.
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
        Some(&QUOTA_EXCEEDED_SSE_EVENT),
        EdgionErrorCode::AiQuotaOverLimitTerminated,
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
        Some(&QUOTA_UNENFORCEABLE_SSE_EVENT),
        EdgionErrorCode::AiSseDegradedQuotaTerminated,
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
        Some(&QUOTA_UNENFORCEABLE_SSE_EVENT),
        EdgionErrorCode::AiSseMalformedQuotaTerminated,
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
pub(crate) fn terminate_ai_stream(
    shared: &AiUsageShared,
    sink: &mut ResponseBodySink,
    session: &mut dyn PluginSession,
    inject: bool,
    event: Option<&Bytes>,
    terminal_status: EdgionErrorCode,
) {
    let cause = match terminal_status {
        EdgionErrorCode::AiQuotaOverLimitTerminated => ResponseTerminalCause::QuotaExceeded,
        _ => ResponseTerminalCause::QuotaUnavailable,
    };
    if !session.try_claim_response_terminal(cause) {
        return;
    }
    let emission = if inject {
        if event.is_none_or(|event| sink.push(event.clone()).is_err()) {
            session.record_error(EdgionErrorCode::AiQuotaErrorEventDropped);
            TerminalEmission::NoEvent
        } else {
            TerminalEmission::Rendered
        }
    } else {
        TerminalEmission::NoEvent
    };
    session.mark_response_terminal_emission(emission);
    sink.terminate();
    shared.mark_gateway_terminated();
    session.record_error(terminal_status);
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
    pub streaming: bool,
    pub usage_estimated: bool,
    pub usage_source: GenAiAttemptUsageSource,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
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
    ProviderFailed,
    ProviderIncomplete,
    ProviderCancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenAiAttemptUsageSource {
    Reported,
    Estimated,
    Unknown,
    #[serde(rename = "notObserved")]
    NotObserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AiUsageUnavailableReason {
    ObservationUnavailable,
    IncompleteJson,
    MissingProviderUsage,
    ArithmeticOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AiUsageSnapshot {
    pub source: GenAiAttemptUsageSource,
    pub usage: Option<ReportedAiUsage>,
    pub unavailable: Option<AiUsageUnavailableReason>,
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
    observation_unavailable: AtomicBool,
    encoding_unavailable: AtomicBool,
    observation_diagnostic_recorded: AtomicBool,
    observation_off: AtomicBool,
    reported_only: bool,
    response_outcome: AtomicU8,
    json_started: AtomicBool,
    json_complete: AtomicBool,
    gateway_terminated: AtomicBool,
}

impl AiUsageShared {
    pub(crate) fn with_observation(observation: AiUsageObservation) -> Self {
        Self {
            observation_off: AtomicBool::new(observation == AiUsageObservation::Off),
            ..Self::default()
        }
    }

    pub(crate) fn with_reported_only(observation: AiUsageObservation) -> Self {
        Self {
            reported_only: true,
            ..Self::with_observation(observation)
        }
    }

    pub(crate) fn observe_response_outcome(&self, outcome: ResponseOutcome) {
        self.response_outcome.store(outcome as u8, Ordering::Release);
    }
    pub(crate) fn response_outcome(&self) -> ResponseOutcome {
        match self.response_outcome.load(Ordering::Acquire) {
            1 => ResponseOutcome::Completed,
            2 => ResponseOutcome::Incomplete,
            3 => ResponseOutcome::Failed,
            4 => ResponseOutcome::Cancelled,
            _ => ResponseOutcome::Unknown,
        }
    }
    pub(crate) fn permits_provider_success(&self) -> bool {
        !self.reported_only
            || (!self.observation_unavailable() && self.response_outcome() == ResponseOutcome::Completed)
    }
    /// Credential transport recovery does not assert model completion when observation is off.
    pub(crate) fn permits_credential_success(&self) -> bool {
        !self.gateway_terminated() && (self.observation_is_off() || self.permits_provider_success())
    }
    pub(crate) fn permits_latency_sample(&self) -> bool {
        !self.reported_only || self.observation_is_off() || self.permits_provider_success()
    }
    pub(crate) fn provider_failure(&self) -> Option<GenAiAttemptFailure> {
        if !self.reported_only || self.observation_is_off() {
            return None;
        }
        match self.response_outcome() {
            ResponseOutcome::Failed => Some(GenAiAttemptFailure::ProviderFailed),
            ResponseOutcome::Incomplete => Some(GenAiAttemptFailure::ProviderIncomplete),
            ResponseOutcome::Cancelled => Some(GenAiAttemptFailure::ProviderCancelled),
            _ => None,
        }
    }

    pub(crate) fn observation_is_off(&self) -> bool {
        self.observation_off.load(Ordering::Relaxed)
    }

    pub(crate) fn mark_json_started(&self) {
        self.json_started.store(true, Ordering::Release);
    }
    pub(crate) fn mark_json_complete(&self) {
        self.json_complete.store(true, Ordering::Release);
    }
    pub(crate) fn mark_json_unavailable(&self) {
        if !self.json_complete.load(Ordering::Acquire) {
            self.mark_observation_unavailable();
        }
    }
    pub fn set_input_chars(&self, chars: u64) {
        if self.observation_is_off() {
            return;
        }
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

    pub(crate) fn encoding_unavailable(&self) -> bool {
        self.encoding_unavailable.load(Ordering::Acquire)
    }

    pub(crate) fn observation_unavailable(&self) -> bool {
        self.observation_unavailable.load(Ordering::Acquire)
    }

    pub(crate) fn mark_encoding_unavailable(&self) {
        self.encoding_unavailable.store(true, Ordering::Release);
        self.mark_observation_unavailable();
    }

    pub(crate) fn mark_observation_unavailable(&self) {
        self.observation_unavailable.store(true, Ordering::Release);
    }

    pub(crate) fn claim_unavailable_diagnostic(&self) -> bool {
        !self.observation_diagnostic_recorded.swap(true, Ordering::AcqRel)
    }

    pub(crate) fn accounting_snapshot(&self, estimated_input: u64, include_output: bool) -> AiUsageSnapshot {
        let unavailable = if self.observation_off.load(Ordering::Acquire) {
            return AiUsageSnapshot {
                source: GenAiAttemptUsageSource::NotObserved,
                usage: None,
                unavailable: None,
            };
        } else if self.observation_unavailable() {
            Some(AiUsageUnavailableReason::ObservationUnavailable)
        } else if self.json_started.load(Ordering::Acquire) && !self.json_complete.load(Ordering::Acquire) {
            Some(AiUsageUnavailableReason::IncompleteJson)
        } else {
            None
        };
        if unavailable.is_some() {
            return AiUsageSnapshot {
                source: GenAiAttemptUsageSource::Unknown,
                usage: None,
                unavailable,
            };
        }
        if self.reported_only && self.reported_usage().is_none() {
            return AiUsageSnapshot {
                source: GenAiAttemptUsageSource::Unknown,
                usage: None,
                unavailable: Some(AiUsageUnavailableReason::MissingProviderUsage),
            };
        }
        let (usage, source) = self.reported_usage().map_or_else(
            || {
                (
                    ReportedAiUsage {
                        input_tokens: estimated_input,
                        output_tokens: if include_output {
                            ceil_chars_div_four(self.output_chars())
                        } else {
                            0
                        },
                        cache_read_input_tokens: 0,
                        cache_write_input_tokens: 0,
                    },
                    GenAiAttemptUsageSource::Estimated,
                )
            },
            |usage| (usage, GenAiAttemptUsageSource::Reported),
        );
        if usage.input_tokens.checked_add(usage.output_tokens).is_none() {
            AiUsageSnapshot {
                source: GenAiAttemptUsageSource::Unknown,
                usage: None,
                unavailable: Some(AiUsageUnavailableReason::ArithmeticOverflow),
            }
        } else {
            AiUsageSnapshot {
                source,
                usage: Some(usage),
                unavailable: None,
            }
        }
    }

    /// True once the gateway itself ended the stream (over quota, or quota no
    /// longer enforceable). Read by attempt settlement, which must not mistake
    /// the absent eos callback for an upstream truncation.
    pub fn gateway_terminated(&self) -> bool {
        self.gateway_terminated.load(Ordering::Acquire)
    }

    pub(crate) fn mark_gateway_terminated(&self) {
        self.gateway_terminated.store(true, Ordering::Release);
    }

    pub(crate) fn add_output_chars(&self, delta: u64) {
        if self.observation_is_off() {
            return;
        }
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
        if self.observation_is_off() {
            return;
        }
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
        let cache_total = cache_read_input.checked_add(cache_write_input);
        let (cache_read_input, cache_write_input) = if cache_total.is_some_and(|total| total <= input) {
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
    json: Option<Box<JsonUsageObserver>>,
    observation_failure_reported: bool,
    non_streaming_chars: Utf8CharCounter,
    chars_finished: bool,
    /// Latches the one-shot `AiSseDecodeDegraded` record. The decoder's
    /// degraded flag is sticky, so without this every subsequent chunk of the
    /// stream would append the same status again.
    sse_degraded_reported: bool,
    /// Latches the one-shot `AiSseDecodeMalformed` record. The decoder's
    /// malformed-event count is cumulative, so later chunks must not append
    /// the observation status again.
    sse_malformed_reported: bool,
    semantic_source_overflowed: bool,
    responses: bool,
    response_terminal_seen: bool,
    response_sequence: Option<u64>,
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
        let mut sse = SseDecoder::with_default_limit();
        sse.enable_data_char_counting();
        Self {
            shared,
            quota,
            requested_streaming,
            terminate_on_over_limit,
            sse,
            json: None,
            observation_failure_reported: false,
            non_streaming_chars: Utf8CharCounter::default(),
            chars_finished: false,
            sse_degraded_reported: false,
            sse_malformed_reported: false,
            semantic_source_overflowed: false,
            responses: false,
            response_terminal_seen: false,
            response_sequence: None,
        }
    }

    pub(crate) fn new_responses(
        shared: Arc<AiUsageShared>,
        quota: Option<Arc<QuotaAccumulator>>,
        requested_streaming: bool,
        terminate_on_over_limit: bool,
    ) -> Self {
        let mut processor = Self::new_with_quota(shared, quota, requested_streaming, terminate_on_over_limit);
        // Responses output text/done/full-output repeats and hidden reasoning cannot
        // constitute a complete billing estimate. Count only provider usage totals.
        processor.sse = SseDecoder::with_default_limit();
        processor.responses = true;
        processor
    }

    fn observe_chunk(
        &mut self,
        chunk: Bytes,
        stream_kind: ResponseStreamKind,
        semantic_guard_active: bool,
    ) -> Option<CanonicalRecordBatch> {
        match stream_kind {
            ResponseStreamKind::Sse => {
                let shared = &self.shared;
                let quota = self.quota.as_ref();
                let mut batch = semantic_guard_active.then(CanonicalRecordBatch::new);
                let mut batch_overflowed = false;
                let responses = self.responses;
                let terminal_seen = &mut self.response_terminal_seen;
                let sequence = &mut self.response_sequence;
                self.sse.feed_records(chunk, |record| match record {
                    SseRecordRef::Block { status, event } => {
                        if responses {
                            match status {
                                SseBlockStatus::Dataless => return,
                                SseBlockStatus::Poisoned => {
                                    shared.mark_observation_unavailable();
                                    return;
                                }
                                SseBlockStatus::Data => {}
                            }
                            let observed = crate::ai::protocols::openai::responses::observe_event(&event);
                            if let Some(value) = observed.sequence {
                                *sequence = Some(sequence.map_or(value, |previous| previous.max(value)));
                            }
                            if observed.unavailable {
                                shared.mark_observation_unavailable();
                            }
                            if let Some(outcome) = observed.outcome {
                                if *terminal_seen && outcome != shared.response_outcome() {
                                    shared.mark_observation_unavailable();
                                }
                                *terminal_seen = true;
                                shared.observe_response_outcome(outcome);
                                if let Some(usage) = observed.usage {
                                    shared.observe_reported_usage(
                                        usage.input_tokens,
                                        usage.output_tokens,
                                        usage.cache_read_input_tokens,
                                        usage.cache_write_input_tokens,
                                    );
                                    if let Some(quota) = quota {
                                        quota.observe_output_total(usage.output_tokens);
                                    }
                                }
                            }
                            return;
                        }
                        let observation = observe_canonical_event(&event);
                        if let Some(quota) = quota {
                            quota.add_output_units(observation.output_units);
                        }
                        let usage = observation.usage;
                        if let (Some(input_tokens), Some(output_tokens)) =
                            (usage.prompt_tokens, usage.completion_tokens)
                        {
                            shared.observe_reported_usage(
                                input_tokens,
                                output_tokens,
                                usage.cached_tokens.or(usage.cache_read_input_tokens).unwrap_or(0),
                                usage.cache_creation_input_tokens.unwrap_or(0),
                            );
                        }
                        if let Some(batch) = &mut batch {
                            if batch
                                .push_observed(SseRecordRef::Block { status, event }, Some(observation))
                                .is_err()
                            {
                                batch_overflowed = true;
                            }
                        }
                    }
                    other => {
                        if let Some(batch) = &mut batch {
                            if batch.push_observed(other, None).is_err() {
                                batch_overflowed = true;
                            }
                        }
                    }
                });
                self.shared.add_output_chars(self.sse.take_data_chars());
                if self.responses && (self.sse.degraded() || self.sse.malformed_events() != 0) {
                    self.shared.mark_observation_unavailable();
                }
                if self.sse.degraded() {
                    self.shared.sse_degraded.store(true, Ordering::Release);
                    if let Some(batch) = &mut batch {
                        batch.mark_unavailable();
                    }
                }
                if batch_overflowed {
                    self.semantic_source_overflowed = true;
                    None
                } else {
                    batch
                }
            }
            ResponseStreamKind::None => {
                if !self.responses {
                    self.shared.add_output_chars(self.non_streaming_chars.feed(&chunk));
                }
                self.observe_non_streaming(&chunk);
                None
            }
        }
    }

    fn observe_non_streaming(&mut self, chunk: &[u8]) {
        if chunk.is_empty() || self.shared.observation_unavailable() {
            return;
        }
        self.shared.json_started.store(true, Ordering::Release);
        let observer = self.json.get_or_insert_with(|| {
            Box::new(if self.responses {
                JsonUsageObserver::responses(false)
            } else {
                JsonUsageObserver::new()
            })
        });
        observer.feed(chunk);
        if observer.failed() {
            self.shared.mark_observation_unavailable();
        }
    }

    fn finish(&mut self, stream_kind: ResponseStreamKind) {
        if !self.chars_finished {
            let remaining = match stream_kind {
                ResponseStreamKind::Sse => self.sse.finish_data_chars(),
                ResponseStreamKind::None => self.non_streaming_chars.finish(),
            };
            self.shared.add_output_chars(remaining);
            self.chars_finished = true;
        }
        if stream_kind == ResponseStreamKind::None && !self.shared.observation_unavailable() {
            let observer = self.json.get_or_insert_with(|| {
                Box::new(if self.responses {
                    JsonUsageObserver::responses(false)
                } else {
                    JsonUsageObserver::new()
                })
            });
            let result = observer.finish();
            if self.responses {
                let outcome = if matches!(result, JsonUsageOutcome::Unavailable(_)) {
                    ResponseOutcome::Unknown
                } else {
                    observer.response_outcome()
                };
                self.response_terminal_seen = outcome != ResponseOutcome::Unknown;
                self.shared.observe_response_outcome(outcome);
                if !self.response_terminal_seen {
                    self.shared.mark_observation_unavailable();
                }
            }
            match result {
                JsonUsageOutcome::Reported(usage) if !self.shared.observation_unavailable() => {
                    self.shared.observe_reported_usage(
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.cache_read_input_tokens,
                        usage.cache_write_input_tokens,
                    );
                    if self.responses {
                        if let Some(quota) = &self.quota {
                            quota.observe_output_total(usage.output_tokens);
                        }
                    }
                    self.shared.json_complete.store(true, Ordering::Release);
                }
                JsonUsageOutcome::Missing => {
                    self.shared.json_complete.store(true, Ordering::Release);
                }
                JsonUsageOutcome::Unavailable(_) | JsonUsageOutcome::Reported(_) => {
                    self.shared.mark_observation_unavailable()
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/accounting/usage_test/legacy_usage_observer.rs"]
mod legacy_usage_observer;

#[cfg(test)]
use legacy_usage_observer::{approximate_openai_output_units, observe_openai_streaming_event, UsageEventObservation};

#[async_trait]
impl StreamingResponseProcessor for UsageExtractionProcessor {
    fn on_response_header(&mut self, response: &mut pingora_http::ResponseHeader) {
        if !crate::routes::http::proxy_http::content_type::response_is_identity_encoded(&response.headers) {
            self.shared.mark_encoding_unavailable();
        }
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
        if self.shared.encoding_unavailable.load(Ordering::Acquire) {
            if !self.observation_failure_reported {
                self.observation_failure_reported = true;
                if self.shared.claim_unavailable_diagnostic() {
                    session.record_error(EdgionErrorCode::AiUsageObservationUnavailable);
                }
            }
            return;
        }
        let semantic_guard_active = session.semantic_guard_active() && stream_kind == ResponseStreamKind::Sse;
        let mut canonical_batch = None;
        if let Some(bytes) = chunk.take() {
            let original = bytes.clone();
            let batch = self.observe_chunk(bytes, stream_kind, semantic_guard_active);
            if let Some(batch) = batch {
                canonical_batch = Some(batch);
            } else if semantic_guard_active && self.semantic_source_overflowed {
                if session.try_claim_response_terminal(ResponseTerminalCause::ProcessorFailure) {
                    session.mark_response_terminal_emission(TerminalEmission::NoEvent);
                    self.shared.mark_gateway_terminated();
                    sink.terminate();
                }
            } else {
                *chunk = Some(original);
            }
        }
        if self.shared.observation_unavailable() && !self.observation_failure_reported {
            self.observation_failure_reported = true;
            if self.shared.claim_unavailable_diagnostic() {
                session.record_error(EdgionErrorCode::AiUsageObservationUnavailable);
            }
        }
        // Recorded for every degraded stream, quota'd or not: even where no
        // enforcement is at stake, degradation silently downgrades reported
        // usage to the character estimate, and an operator cannot tell that
        // from the numbers alone. Latched so a sticky flag cannot append this
        // once per remaining chunk.
        if !self.sse_degraded_reported && self.sse.degraded() {
            self.sse_degraded_reported = true;
            session.record_error(EdgionErrorCode::AiSseDecodeDegraded);
        }
        if !self.sse_malformed_reported && self.sse.malformed_events() != 0 {
            self.sse_malformed_reported = true;
            session.record_error(EdgionErrorCode::AiSseDecodeMalformed);
        }
        if end_of_stream {
            if self.responses && stream_kind == ResponseStreamKind::Sse {
                let mut incomplete_tail = false;
                self.sse.finish_records(|record| {
                    if matches!(record, SseRecordRef::EosTail(_)) {
                        incomplete_tail = true;
                    }
                });
                if incomplete_tail
                    || !self.response_terminal_seen
                    || self.sse.degraded()
                    || self.sse.malformed_events() != 0
                {
                    self.shared.mark_observation_unavailable();
                }
            }
            if semantic_guard_active {
                let mut batch = canonical_batch.take().unwrap_or_else(CanonicalRecordBatch::new);
                let mut finish_overflowed = false;
                self.sse.finish_records(|record| {
                    if batch.push(WindowRecord::from_sse_record(record)).is_err() {
                        finish_overflowed = true;
                    }
                });
                // An empty batch is the explicit EOS signal that makes the
                // guard resolve a below-threshold window retained earlier.
                if (finish_overflowed || session.publish_canonical_records(batch).is_err())
                    && session.try_claim_response_terminal(ResponseTerminalCause::ProcessorFailure)
                {
                    session.mark_response_terminal_emission(TerminalEmission::NoEvent);
                    self.shared.mark_gateway_terminated();
                    sink.terminate();
                }
            }
            self.finish(stream_kind);
            if self.shared.observation_unavailable() && !self.observation_failure_reported {
                self.observation_failure_reported = true;
                if self.shared.claim_unavailable_diagnostic() {
                    session.record_error(EdgionErrorCode::AiUsageObservationUnavailable);
                }
            }
            return;
        }
        if let Some(batch) = canonical_batch {
            if session.publish_canonical_records(batch).is_err()
                && session.try_claim_response_terminal(ResponseTerminalCause::ProcessorFailure)
            {
                session.mark_response_terminal_emission(TerminalEmission::NoEvent);
                self.shared.mark_gateway_terminated();
                sink.terminate();
            }
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
        if self.responses {
            let error = if self.quota.as_ref().is_some_and(|quota| quota.over_limit()) {
                Some((AiProxyError::QuotaExceeded, EdgionErrorCode::AiQuotaOverLimitTerminated))
            } else if self.sse.degraded() || self.shared.observation_unavailable() {
                Some((
                    AiProxyError::QuotaUnavailable,
                    EdgionErrorCode::AiSseDegradedQuotaTerminated,
                ))
            } else {
                None
            };
            if let Some((error, code)) = error {
                let event = crate::ai::protocols::openai::responses::local_error(error, self.response_sequence);
                terminate_ai_stream(
                    &self.shared,
                    sink,
                    session,
                    stream_kind == ResponseStreamKind::Sse,
                    event.as_ref(),
                    code,
                );
            }
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
        // A semantic source normally transfers the current transport bytes to
        // the canonical handoff for Guardrail. A terminal claimed in this AI
        // slot preempts Guardrail, so reclaim those bytes for the pump before
        // the driver discards the stale handoff. This preserves quota's
        // established current-chunk-before-terminal-event wire contract.
        if sink.is_terminated() && semantic_guard_active && chunk.is_none() {
            *chunk = session
                .take_canonical_records()
                .and_then(|batch| batch.into_raw_bytes().ok());
        }
    }
}

const fn ceil_chars_div_four(chars: u64) -> u64 {
    chars / 4 + if chars.is_multiple_of(4) { 0 } else { 1 }
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/accounting/usage_test.rs"]
mod tests;
