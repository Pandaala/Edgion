//! Request-owned runtime state for one AI credential attempt.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use edgion_resources::resources::edgion_backend::{AiCredentialLimits, ParsedAiModelPricing};
use edgion_resources::resources::edgion_plugins::OnRedisFailure;

use crate::backends::outlier_detection::{EjectionAdmission, FailureClass, HalfOpenProbe};
use crate::link_sys::get_redis_client;
use crate::plugins::common::redis_rate_limiter::{
    build_credential_reservation_keys, CredentialReservationKeys, CredentialReservationLimits,
    CredentialReservationOperation, CredentialReservationStatus, RedisRateLimiter,
};

use crate::ai::routing::adaptive::current_unix_second;
use crate::ai::{
    estimated_cost_usd, AdaptiveTelemetryState, AiCredentialOutlierHandle, AiProviderLabel, AiUsageShared,
    GenAiAttempt, GenAiAttemptFailure, GenAiAttemptUsageSource,
};

use crate::ai::accounting::usage::AiUsageUnavailableReason;

const KEY_PREFIX: &str = "edgion:aic:";
const WINDOW: Duration = Duration::from_secs(60);
const SETTLEMENT_GRACE: Duration = Duration::from_secs(5);
const REDIS_OPERATION_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_PENDING_SETTLEMENTS: usize = super::attempt::MAX_AI_DISPATCH_ATTEMPTS as usize;
pub const MAX_AI_ATTEMPT_LOG_BYTES: usize = 8 * 1024;
const MAX_BACKEND_KEY_BYTES: usize = 63 + 1 + 253;
const MAX_CREDENTIAL_ALIAS_BYTES: usize = 64;

pub struct AiAttemptAdmissionInput {
    pub namespace: String,
    pub backend_uid: String,
    pub credential_alias: String,
    pub backend_name: String,
    pub provider: AiProviderLabel,
    pub pricing: Option<ParsedAiModelPricing>,
    pub redis_ref: Option<String>,
    pub on_redis_failure: OnRedisFailure,
    pub limits: Option<AiCredentialLimits>,
    pub estimated_input_tokens: u64,
    pub max_output_tokens: u64,
    pub usage: Arc<AiUsageShared>,
    pub eos: Arc<AtomicBool>,
    pub outlier: Option<AiCredentialOutlierHandle>,
    pub adaptive: Option<Arc<AdaptiveTelemetryState>>,
    pub circuit_breaker: Option<Arc<crate::ai::runtime::circuit_breaker::LogicalBackendCircuitBreaker>>,
    pub limiter_core: Arc<RedisRateLimiter>,
}

#[allow(clippy::large_enum_variant)]
pub enum AiAttemptAdmission {
    Ready(AiAttemptRuntime),
    Skip,
    SkipBackend,
    DependencyDenied,
}

pub enum AiAttemptFailure {
    Known(FailureClass),
    Ambiguous(FailureClass),
    BackendRequestTimeout,
}

pub struct AiAttemptRuntime {
    reservation: Option<CredentialReservationLease>,
    outlier: Option<OutlierLease>,
    usage: Arc<AiUsageShared>,
    eos: Arc<AtomicBool>,
    estimated_input_tokens: u64,
    backend_name: String,
    provider: AiProviderLabel,
    credential_alias: String,
    pricing: Option<ParsedAiModelPricing>,
    adaptive: Option<Arc<AdaptiveTelemetryState>>,
    status: Option<u16>,
    retry_after_seconds: Option<u64>,
    dispatched: bool,
    verdict_recorded: bool,
    _circuit_breaker: Option<crate::ai::runtime::circuit_breaker::LogicalBackendCircuitBreakerPermit>,
}

impl AiAttemptRuntime {
    pub fn mark_dispatched(&mut self) {
        self.dispatched = true;
    }

    pub fn apply_retry_after(&mut self, value: &str, now: SystemTime) -> Option<Duration> {
        let cooldown = self.outlier.as_ref()?.handle.apply_retry_after(value, now)?;
        self.store_cooldown(cooldown);
        Some(cooldown)
    }

    pub fn apply_cooldown(&mut self, hint: Duration) -> Option<Duration> {
        let cooldown = self.outlier.as_ref()?.handle.apply_cooldown(hint)?;
        self.store_cooldown(cooldown);
        Some(cooldown)
    }

    fn store_cooldown(&mut self, cooldown: Duration) {
        self.retry_after_seconds = Some(cooldown_delta_seconds(cooldown));
    }

    pub fn record_response(&mut self, status: u16, class: Option<FailureClass>) {
        self.status = Some(status);
        if let Some(class) = class {
            self.record_failure_once(class);
        }
        // A non-failure response header is not yet a success: the streamed body
        // can still reset or time out. Successful attempts commit their sole
        // outlier verdict only after the final EOS latch is observed.
    }

    fn record_failure_once(&mut self, class: FailureClass) -> bool {
        if self.verdict_recorded {
            return false;
        }
        self.record_failure(class);
        self.verdict_recorded = true;
        true
    }

    fn record_success_once(&mut self) -> bool {
        if self.verdict_recorded {
            return false;
        }
        self.record_success();
        self.verdict_recorded = true;
        true
    }

    fn record_failure(&mut self, class: FailureClass) {
        let Some(outlier) = self.outlier.as_mut() else {
            return;
        };
        if let Some(probe) = outlier.probe.take() {
            probe.failure(class);
        } else {
            let _ = outlier.handle.record_failure(class);
        }
    }

    fn record_success(&mut self) {
        let Some(outlier) = self.outlier.as_mut() else {
            return;
        };
        if let Some(probe) = outlier.probe.take() {
            probe.success();
        } else {
            outlier.handle.record_success();
        }
    }

    fn charged_tokens(&self) -> Option<u64> {
        let usage = self
            .usage
            .accounting_snapshot(self.estimated_input_tokens, false)
            .usage?;
        usage.input_tokens.checked_add(usage.output_tokens)
    }

    fn final_charged_tokens(&self) -> Option<u64> {
        let usage = self
            .usage
            .accounting_snapshot(self.estimated_input_tokens, true)
            .usage?;
        usage.input_tokens.checked_add(usage.output_tokens)
    }

    fn ledger_entry(
        &self,
        number: u8,
        failure: Option<GenAiAttemptFailure>,
        ttft_ms: Option<u64>,
        include_estimated_output: bool,
    ) -> Option<GenAiAttempt> {
        let snapshot = self
            .usage
            .accounting_snapshot(self.estimated_input_tokens, include_estimated_output);
        Some(GenAiAttempt {
            number,
            backend: bounded_string(&self.backend_name, MAX_BACKEND_KEY_BYTES),
            provider: self.provider.as_str(),
            credential: bounded_string(&self.credential_alias, MAX_CREDENTIAL_ALIAS_BYTES),
            status: self.status,
            failure,
            retry_after_seconds: self.retry_after_seconds,
            ttft_ms,
            usage_source: snapshot.source,
            input_tokens: snapshot.usage.map(|usage| usage.input_tokens),
            output_tokens: snapshot.usage.map(|usage| usage.output_tokens),
            estimated_cost_usd: snapshot
                .usage
                .and_then(|usage| self.pricing.map(|pricing| estimated_cost_usd(pricing, usage))),
        })
    }
}

struct OutlierLease {
    handle: AiCredentialOutlierHandle,
    probe: Option<HalfOpenProbe>,
}

struct CredentialReservationLease {
    client: Arc<crate::link_sys::providers::redis::RedisLinkClient>,
    core: Arc<RedisRateLimiter>,
    keys: CredentialReservationKeys,
    limits: CredentialReservationLimits,
    ttl: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettlementOperation {
    Release,
    Reconcile(u64),
    Retain,
}

struct PendingSettlement {
    reservation: Option<CredentialReservationLease>,
    operation: SettlementOperation,
}

#[derive(Default)]
pub struct AiAttemptRuntimeState {
    active: Option<AiAttemptRuntime>,
    pending: VecDeque<PendingSettlement>,
    ever_dispatched: bool,
    charged_request_tokens: u64,
    accounting_overflow: bool,
    usage_kinds: u8,
    metric_totals: Vec<AiAttemptTokenTotals>,
    attempts: Vec<GenAiAttempt>,
    attempts_truncated: bool,
    #[cfg(test)]
    settlement_decisions: Vec<SettlementOperation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiAttemptTokenTotals {
    pub provider: AiProviderLabel,
    pub usage_source: GenAiAttemptUsageSource,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

// The universal logging callback is the final async drain. If process failure
// or a framework bug skips that callback, no detached task is spawned: Redis
// reservations remain conservatively charged only until their bounded TTL,
// while RAII probe and circuit-breaker leases still release on context drop.

impl AiAttemptRuntimeState {
    pub async fn admit(&mut self, input: AiAttemptAdmissionInput) -> AiAttemptAdmission {
        if self.active.is_some() || self.accounting_overflow {
            return AiAttemptAdmission::DependencyDenied;
        }
        let circuit_breaker = match input.circuit_breaker.as_ref() {
            Some(state) => match state.try_acquire() {
                Some(permit) => Some(permit),
                None => return AiAttemptAdmission::SkipBackend,
            },
            None => None,
        };
        let outlier = match input.outlier {
            Some(ref handle) => match handle.admit() {
                EjectionAdmission::Rejected => return AiAttemptAdmission::Skip,
                EjectionAdmission::Closed => Some(OutlierLease {
                    handle: handle.clone(),
                    probe: None,
                }),
                EjectionAdmission::Probe(probe) => Some(OutlierLease {
                    handle: handle.clone(),
                    probe: Some(probe),
                }),
            },
            None => return AiAttemptAdmission::Skip,
        };

        let reservation = match input.limits.as_ref() {
            None => None,
            Some(limits) => match reserve(&input, limits).await {
                Ok(Some(lease)) => Some(lease),
                Ok(None) => return AiAttemptAdmission::Skip,
                Err(()) if input.on_redis_failure == OnRedisFailure::Allow => None,
                Err(()) => return AiAttemptAdmission::DependencyDenied,
            },
        };
        AiAttemptAdmission::Ready(AiAttemptRuntime {
            reservation,
            outlier,
            usage: input.usage,
            eos: input.eos,
            estimated_input_tokens: input.estimated_input_tokens,
            backend_name: bounded_string(&input.backend_name, MAX_BACKEND_KEY_BYTES),
            provider: input.provider,
            credential_alias: bounded_string(&input.credential_alias, MAX_CREDENTIAL_ALIAS_BYTES),
            pricing: input.pricing,
            adaptive: input.adaptive,
            status: None,
            retry_after_seconds: None,
            dispatched: false,
            verdict_recorded: false,
            _circuit_breaker: circuit_breaker,
        })
    }

    pub fn install(&mut self, attempt: AiAttemptRuntime) {
        self.active = Some(attempt);
    }

    pub fn mark_dispatched(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.mark_dispatched();
            self.ever_dispatched = true;
        }
    }

    pub fn ever_dispatched(&self) -> bool {
        self.ever_dispatched
    }

    #[allow(clippy::result_unit_err)]
    pub fn charged_request_tokens(&self) -> Result<u64, ()> {
        (!self.accounting_overflow && self.unresolved_usage_source().is_none())
            .then_some(self.charged_request_tokens)
            .ok_or(())
    }

    #[allow(clippy::result_unit_err)]
    pub fn metric_totals(&self) -> Result<&[AiAttemptTokenTotals], ()> {
        Ok(self.metric_totals.as_slice())
    }

    pub(crate) fn accounting_overflow(&self) -> bool {
        self.accounting_overflow
    }

    pub(crate) fn unresolved_usage_source(&self) -> Option<GenAiAttemptUsageSource> {
        match self.usage_kinds {
            4 => Some(GenAiAttemptUsageSource::NotObserved),
            kinds if kinds & 2 != 0 || kinds & 5 == 5 => Some(GenAiAttemptUsageSource::Unknown),
            _ => None,
        }
    }

    pub fn attempts(&self) -> &[GenAiAttempt] {
        &self.attempts
    }

    pub fn attempts_truncated(&self) -> bool {
        self.attempts_truncated
    }

    pub fn active_mut(&mut self) -> Option<&mut AiAttemptRuntime> {
        self.active.as_mut()
    }

    #[cfg(test)]
    pub(crate) fn record_test_attempt(&mut self, entry: GenAiAttempt, provider: AiProviderLabel) {
        self.ever_dispatched = true;
        self.commit_ledger(Some(entry), provider, None, None);
    }

    #[cfg(test)]
    fn settlement_decisions_for_test(&self) -> &[SettlementOperation] {
        &self.settlement_decisions
    }

    pub fn finish_predispatch(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        self.push(active.reservation, SettlementOperation::Release);
    }

    pub fn finish_failure(&mut self, failure: AiAttemptFailure) {
        let Some(mut active) = self.active.take() else {
            return;
        };
        if active
            .usage
            .accounting_snapshot(active.estimated_input_tokens, false)
            .unavailable
            == Some(AiUsageUnavailableReason::ArithmeticOverflow)
        {
            self.accounting_overflow = true;
        }
        match failure {
            AiAttemptFailure::Known(class) => {
                active.record_failure_once(class);
                let number = self.next_attempt_number();
                let entry = active.ledger_entry(number, Some(GenAiAttemptFailure::RetryableStatus), None, false);
                let provider = active.provider;
                let adaptive = active.adaptive.clone();
                match active.charged_tokens() {
                    Some(charged) => self.push(active.reservation, SettlementOperation::Reconcile(charged)),
                    None => {
                        if active
                            .usage
                            .accounting_snapshot(active.estimated_input_tokens, false)
                            .unavailable
                            == Some(AiUsageUnavailableReason::ArithmeticOverflow)
                        {
                            self.accounting_overflow = true;
                        }
                        self.push(active.reservation, SettlementOperation::Retain);
                    }
                }
                self.commit_ledger(entry, provider, adaptive, None);
            }
            AiAttemptFailure::Ambiguous(class) => {
                active.record_failure_once(class);
                let number = self.next_attempt_number();
                let entry = active.ledger_entry(number, Some(GenAiAttemptFailure::AmbiguousTransport), None, false);
                let provider = active.provider;
                let adaptive = active.adaptive.clone();
                // Redis retains the full input + max-output reservation because
                // the transport outcome is unknown. The access/identity ledger
                // intentionally records only usage that can be observed or
                // conservatively estimated from the request.
                self.push(active.reservation, SettlementOperation::Retain);
                self.commit_ledger(entry, provider, adaptive, None);
            }
            AiAttemptFailure::BackendRequestTimeout => {
                active.record_failure_once(FailureClass::LocalOrigin);
                let number = self.next_attempt_number();
                let entry = active.ledger_entry(number, Some(GenAiAttemptFailure::BackendRequestTimeout), None, false);
                let provider = active.provider;
                let adaptive = active.adaptive.clone();
                self.push(active.reservation, SettlementOperation::Retain);
                self.commit_ledger(entry, provider, adaptive, None);
            }
        }
    }

    pub async fn drain_pending(&mut self) {
        while let Some(pending) = self.pending.pop_front() {
            settle(pending).await;
        }
    }

    pub async fn finalize(&mut self, ttft_ms: Option<u64>) {
        if let Some(mut active) = self.active.take() {
            let dispatched = active.dispatched;
            let eos = active.eos.load(Ordering::Acquire);
            // A gateway-initiated mid-stream cut (over quota, or quota no
            // longer enforceable) never produces an eos callback: the sink
            // terminates the response first, by design (response-body pipeline
            // spec §3.4). "No eos" therefore means one of two very different
            // things for transport: an upstream truncation is ambiguous, while
            // a deliberate gateway cut is not an incomplete provider response.
            // Usage quality is independent: even EOS or a gateway cut retains
            // the credential reservation when accounting is unavailable.
            let settled = eos || active.usage.gateway_terminated();
            let successful_final = eos
                && active.status.is_some_and(|status| (200..300).contains(&status))
                && active.usage.permits_latency_sample();
            let successful_ttft_ms = successful_final.then_some(ttft_ms).flatten();
            if active
                .usage
                .accounting_snapshot(active.estimated_input_tokens, true)
                .unavailable
                == Some(AiUsageUnavailableReason::ArithmeticOverflow)
            {
                self.accounting_overflow = true;
            }
            let operation = self.settlement_for(&mut active, settled, eos);
            let entry = if dispatched {
                let failure = if !settled {
                    Some(GenAiAttemptFailure::IncompleteResponse)
                } else {
                    active.usage.provider_failure()
                };
                let number = self.next_attempt_number();
                active.ledger_entry(number, failure, successful_ttft_ms, true)
            } else {
                None
            };
            let provider = active.provider;
            let adaptive = active.adaptive.clone();
            self.push(active.reservation, operation);
            if dispatched {
                self.commit_ledger(entry, provider, adaptive, successful_ttft_ms);
            }
        }
        self.drain_pending().await;
    }

    /// Chooses how a finished attempt settles its credential reservation.
    ///
    /// `settled` means the gateway knows the exchange is over — the upstream
    /// delivered EOS, or the gateway itself terminated the stream. Reconciliation
    /// additionally requires reported or valid estimated usage. Ambiguous
    /// transport or unavailable accounting retains the full input + max-output
    /// charge to protect the credential from unobserved consumption. A gateway
    /// cut with valid accounting reconciles its reservation; retaining that known
    /// remainder would hold capacity against other tenants until the window TTL.
    ///
    /// Accepted accounting bias, per the billing-adjacent rule that a trade-off
    /// must be stated: on the Anthropic path the reconciled estimate can be
    /// short by roughly one chunk's characters ÷ 4, because a terminate stops
    /// the processor chain for that chunk (spec §3.4), and the translator's own
    /// `!sink.is_terminated()` gate skips its character counter. There is no
    /// following generic observer. That is a bounded,
    /// systematic tilt toward under-charging; it is accepted rather than
    /// corrected because the correction would require running processors after
    /// a terminate, which is exactly the post-terminate byte-leak class the
    /// termination feature exists to prevent.
    fn settlement_for(&mut self, active: &mut AiAttemptRuntime, settled: bool, eos: bool) -> SettlementOperation {
        if !active.dispatched {
            return SettlementOperation::Release;
        }
        if !settled {
            return SettlementOperation::Retain;
        }
        if eos
            && active.status.is_some_and(|status| (200..300).contains(&status))
            && active.usage.permits_credential_success()
        {
            // The outlier-detection verdict still requires a real upstream EOS:
            // a gateway-initiated cut says nothing about the credential's
            // health, so it must not be committed as a success sample.
            active.record_success_once();
        }
        match active.final_charged_tokens() {
            Some(charged) => SettlementOperation::Reconcile(charged),
            None => {
                if active
                    .usage
                    .accounting_snapshot(active.estimated_input_tokens, true)
                    .unavailable
                    == Some(AiUsageUnavailableReason::ArithmeticOverflow)
                {
                    self.accounting_overflow = true;
                }
                SettlementOperation::Retain
            }
        }
    }

    fn next_attempt_number(&self) -> u8 {
        u8::try_from(self.attempts.len() + 1).unwrap_or(u8::MAX)
    }

    fn commit_ledger(
        &mut self,
        entry: Option<GenAiAttempt>,
        provider: AiProviderLabel,
        adaptive: Option<Arc<AdaptiveTelemetryState>>,
        successful_ttft_ms: Option<u64>,
    ) {
        let Some(entry) = entry else {
            self.accounting_overflow = true;
            return;
        };
        self.usage_kinds |= match entry.usage_source {
            GenAiAttemptUsageSource::Reported | GenAiAttemptUsageSource::Estimated => 1,
            GenAiAttemptUsageSource::Unknown => 2,
            GenAiAttemptUsageSource::NotObserved => 4,
        };
        if let (Some(input), Some(output)) = (entry.input_tokens, entry.output_tokens) {
            if let Some(charged) = input.checked_add(output) {
                if let Some(total) = self.charged_request_tokens.checked_add(charged) {
                    self.charged_request_tokens = total;
                } else {
                    self.accounting_overflow = true;
                }
                let metric_index = self
                    .metric_totals
                    .iter()
                    .position(|total| total.provider == provider && total.usage_source == entry.usage_source);
                let prior = metric_index.map(|index| self.metric_totals[index]);
                let input_total = prior.map_or(Some(input), |total| total.input_tokens.checked_add(input));
                let output_total = prior.map_or(Some(output), |total| total.output_tokens.checked_add(output));
                if let (Some(input_tokens), Some(output_tokens)) = (input_total, output_total) {
                    let total = AiAttemptTokenTotals {
                        provider,
                        usage_source: entry.usage_source,
                        input_tokens,
                        output_tokens,
                    };
                    if let Some(index) = metric_index {
                        self.metric_totals[index] = total;
                    } else {
                        self.metric_totals.push(total);
                    }
                } else {
                    self.accounting_overflow = true;
                }
                if let Some(adaptive) = &adaptive {
                    let second = current_unix_second();
                    adaptive.record_charged_tokens(second, charged);
                }
            } else {
                self.accounting_overflow = true;
            }
        }
        if let (Some(adaptive), Some(ttft_micros)) = (
            adaptive,
            successful_ttft_ms.and_then(|millis| millis.checked_mul(1_000)),
        ) {
            adaptive.record_success_ttft(current_unix_second(), ttft_micros);
        }
        if self.attempts.len() >= MAX_PENDING_SETTLEMENTS {
            self.attempts_truncated = true;
            return;
        }
        self.attempts.push(entry);
        if serde_json::to_vec(&self.attempts).map_or(true, |serialized| serialized.len() > MAX_AI_ATTEMPT_LOG_BYTES) {
            self.attempts.pop();
            self.attempts_truncated = true;
        }
    }

    fn push(&mut self, reservation: Option<CredentialReservationLease>, operation: SettlementOperation) {
        #[cfg(test)]
        self.settlement_decisions.push(operation);
        if reservation.is_none() {
            return;
        }
        if self.pending.len() < MAX_PENDING_SETTLEMENTS {
            self.pending.push_back(PendingSettlement { reservation, operation });
        }
        // Overflow can only follow an invariant violation. Dropping the lease
        // retains the conservative Redis charge until its bounded TTL.
    }
}

fn bounded_string(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

pub(crate) fn cooldown_delta_seconds(cooldown: Duration) -> u64 {
    cooldown
        .as_secs()
        .saturating_add(u64::from(cooldown.subsec_nanos() > 0))
        .max(1)
}

async fn reserve(
    input: &AiAttemptAdmissionInput,
    limits: &AiCredentialLimits,
) -> Result<Option<CredentialReservationLease>, ()> {
    let redis_ref = input.redis_ref.as_deref().ok_or(())?;
    let client = get_redis_client(redis_ref).ok_or(())?;
    if !client.healthy() {
        return Err(());
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let window_id = now.as_secs() / WINDOW.as_secs();
    let elapsed = Duration::from_secs(now.as_secs() % WINDOW.as_secs())
        .saturating_add(Duration::from_nanos(u64::from(now.subsec_nanos())));
    let ttl = WINDOW
        .saturating_sub(elapsed)
        .saturating_add(SETTLEMENT_GRACE)
        .max(Duration::from_millis(1));
    let request_id = uuid::Uuid::new_v4().simple().to_string();
    let keys = build_credential_reservation_keys(
        KEY_PREFIX,
        input.namespace.as_bytes(),
        input.backend_uid.as_bytes(),
        input.credential_alias.as_bytes(),
        window_id,
        request_id.as_bytes(),
    )
    .map_err(|_| ())?;
    let limits = CredentialReservationLimits {
        rpm: limits.requests_per_minute,
        tpm: limits.tokens_per_minute,
    };
    let core = Arc::clone(&input.limiter_core);
    let result = tokio::time::timeout(
        REDIS_OPERATION_TIMEOUT,
        core.apply_credential_reservation(
            &client,
            &keys,
            limits,
            ttl,
            CredentialReservationOperation::Reserve {
                estimated_input_tokens: input.estimated_input_tokens,
                max_output_tokens: input.max_output_tokens,
            },
        ),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?;
    match result.status {
        CredentialReservationStatus::Reserved | CredentialReservationStatus::Duplicate => {
            Ok(Some(CredentialReservationLease {
                client,
                core,
                keys,
                limits,
                ttl,
            }))
        }
        CredentialReservationStatus::DeniedRpm | CredentialReservationStatus::DeniedTpm => Ok(None),
        _ => Err(()),
    }
}

async fn settle(pending: PendingSettlement) {
    let Some(lease) = pending.reservation else {
        return;
    };
    let Some(operation) = redis_settlement_operation(pending.operation) else {
        return;
    };
    let _ = tokio::time::timeout(
        REDIS_OPERATION_TIMEOUT,
        lease
            .core
            .apply_credential_reservation(&lease.client, &lease.keys, lease.limits, lease.ttl, operation),
    )
    .await;
}

fn redis_settlement_operation(operation: SettlementOperation) -> Option<CredentialReservationOperation> {
    match operation {
        SettlementOperation::Release => Some(CredentialReservationOperation::ReleasePredispatch),
        SettlementOperation::Reconcile(charged_tokens) => {
            Some(CredentialReservationOperation::Reconcile { charged_tokens })
        }
        SettlementOperation::Retain => None,
    }
}

pub fn new_eos_latch() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

pub fn mark_eos(latch: &AtomicBool) {
    latch.store(true, Ordering::Release);
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/runtime/attempt_runtime_test.rs"]
mod tests;
