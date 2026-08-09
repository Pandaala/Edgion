//! Request-owned runtime state for one AI credential attempt.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use edgion_resources::resources::edgion_ai_service::{AiCredentialLimits, ParsedAiModelPricing};
use edgion_resources::resources::edgion_plugins::OnRedisFailure;

use crate::backends::outlier_detection::{EjectionAdmission, FailureClass, HalfOpenProbe};
use crate::link_sys::get_redis_client;
use crate::plugins::common::redis_rate_limiter::{
    build_credential_reservation_keys, CredentialReservationKeys, CredentialReservationLimits,
    CredentialReservationOperation, CredentialReservationStatus, RedisRateLimiter,
};

use super::{
    adaptive::current_unix_second, estimated_cost_usd, AdaptiveTelemetryState, AiCredentialOutlierHandle,
    AiProviderLabel, AiUsageShared, GenAiAttempt, GenAiAttemptFailure, GenAiAttemptUsageSource, ReportedAiUsage,
};

const KEY_PREFIX: &str = "edgion:aic:";
const WINDOW: Duration = Duration::from_secs(60);
const SETTLEMENT_GRACE: Duration = Duration::from_secs(5);
const REDIS_OPERATION_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_PENDING_SETTLEMENTS: usize = super::attempt::MAX_AI_DISPATCH_ATTEMPTS as usize;
pub const MAX_AI_ATTEMPT_LOG_BYTES: usize = 8 * 1024;
const MAX_SERVICE_KEY_BYTES: usize = 63 + 1 + 253;
const MAX_CREDENTIAL_ALIAS_BYTES: usize = 64;

pub struct AiAttemptAdmissionInput {
    pub namespace: String,
    pub service_uid: String,
    pub credential_alias: String,
    pub service_name: String,
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
    pub circuit_breaker: Option<Arc<crate::ai::circuit_breaker::AiServiceCircuitBreaker>>,
    pub limiter_core: Arc<RedisRateLimiter>,
}

pub enum AiAttemptAdmission {
    Ready(AiAttemptRuntime),
    Skip,
    SkipService,
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
    service_name: String,
    provider: AiProviderLabel,
    credential_alias: String,
    pricing: Option<ParsedAiModelPricing>,
    adaptive: Option<Arc<AdaptiveTelemetryState>>,
    status: Option<u16>,
    retry_after_seconds: Option<u64>,
    dispatched: bool,
    verdict_recorded: bool,
    _circuit_breaker: Option<crate::ai::circuit_breaker::AiServiceCircuitBreakerPermit>,
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
        match self.usage.reported_tokens() {
            Some((input, output)) => input.checked_add(output),
            None => Some(self.estimated_input_tokens),
        }
    }

    fn final_charged_tokens(&self) -> Option<u64> {
        if let Some((input, output)) = self.usage.reported_tokens() {
            return input.checked_add(output);
        }
        let output = self.usage.output_chars().checked_add(3)?.checked_div(4)?;
        self.estimated_input_tokens.checked_add(output)
    }

    fn ledger_entry(
        &self,
        number: u8,
        failure: Option<GenAiAttemptFailure>,
        ttft_ms: Option<u64>,
        include_estimated_output: bool,
    ) -> Option<GenAiAttempt> {
        let (usage, usage_source) = match self.usage.reported_usage() {
            Some(usage) => (usage, GenAiAttemptUsageSource::Reported),
            None => (
                ReportedAiUsage {
                    input_tokens: self.estimated_input_tokens,
                    output_tokens: include_estimated_output
                        .then(|| ceil_chars_div_four(self.usage.output_chars()))
                        .unwrap_or_default(),
                    cache_read_input_tokens: 0,
                    cache_write_input_tokens: 0,
                },
                GenAiAttemptUsageSource::Estimated,
            ),
        };
        usage.input_tokens.checked_add(usage.output_tokens)?;
        Some(GenAiAttempt {
            number,
            service: bounded_string(&self.service_name, MAX_SERVICE_KEY_BYTES),
            provider: self.provider.as_str(),
            credential: bounded_string(&self.credential_alias, MAX_CREDENTIAL_ALIAS_BYTES),
            status: self.status,
            failure,
            retry_after_seconds: self.retry_after_seconds,
            ttft_ms,
            usage_source,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            estimated_cost_usd: self.pricing.map(|pricing| estimated_cost_usd(pricing, usage)),
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
                None => return AiAttemptAdmission::SkipService,
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
            service_name: bounded_string(&input.service_name, MAX_SERVICE_KEY_BYTES),
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

    pub fn charged_request_tokens(&self) -> Result<u64, ()> {
        (!self.accounting_overflow)
            .then_some(self.charged_request_tokens)
            .ok_or(())
    }

    pub fn metric_totals(&self) -> Result<&[AiAttemptTokenTotals], ()> {
        (!self.accounting_overflow)
            .then_some(self.metric_totals.as_slice())
            .ok_or(())
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
                        self.accounting_overflow = true;
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
            let successful_final = eos && active.status.is_some_and(|status| (200..300).contains(&status));
            let successful_ttft_ms = successful_final.then_some(ttft_ms).flatten();
            let operation = if !dispatched {
                SettlementOperation::Release
            } else if eos {
                active.record_success_once();
                match active.final_charged_tokens() {
                    Some(charged) => SettlementOperation::Reconcile(charged),
                    None => {
                        self.accounting_overflow = true;
                        SettlementOperation::Retain
                    }
                }
            } else {
                SettlementOperation::Retain
            };
            let entry = if dispatched {
                let failure = (!eos).then_some(GenAiAttemptFailure::IncompleteResponse);
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
        let Some(charged) = entry.input_tokens.checked_add(entry.output_tokens) else {
            self.accounting_overflow = true;
            return;
        };
        let Some(charged_request_tokens) = self.charged_request_tokens.checked_add(charged) else {
            self.accounting_overflow = true;
            return;
        };
        let metric_index = self
            .metric_totals
            .iter()
            .position(|total| total.provider == provider && total.usage_source == entry.usage_source);
        let metric_total = metric_index.map(|index| self.metric_totals[index]);
        let Some(metric_input_tokens) = metric_total.map_or(Some(entry.input_tokens), |total| {
            total.input_tokens.checked_add(entry.input_tokens)
        }) else {
            self.accounting_overflow = true;
            return;
        };
        let Some(metric_output_tokens) = metric_total.map_or(Some(entry.output_tokens), |total| {
            total.output_tokens.checked_add(entry.output_tokens)
        }) else {
            self.accounting_overflow = true;
            return;
        };

        self.charged_request_tokens = charged_request_tokens;
        let total = AiAttemptTokenTotals {
            provider,
            usage_source: entry.usage_source,
            input_tokens: metric_input_tokens,
            output_tokens: metric_output_tokens,
        };
        if let Some(index) = metric_index {
            self.metric_totals[index] = total;
        } else {
            self.metric_totals.push(total);
        }
        if let Some(adaptive) = adaptive {
            let second = current_unix_second();
            adaptive.record_charged_tokens(second, charged);
            if let Some(ttft_micros) = successful_ttft_ms.and_then(|millis| millis.checked_mul(1_000)) {
                adaptive.record_success_ttft(second, ttft_micros);
            }
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

const fn ceil_chars_div_four(chars: u64) -> u64 {
    chars / 4 + if chars % 4 == 0 { 0 } else { 1 }
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
        input.service_uid.as_bytes(),
        input.credential_alias.as_bytes(),
        window_id,
        request_id.as_bytes(),
    )
    .map_err(|_| ())?;
    let limits = CredentialReservationLimits {
        rpm: limits.requests_per_minute.map(u64::from),
        tpm: limits.tokens_per_minute.map(u64::from),
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
mod tests {
    use super::*;

    fn closed_outlier_handle() -> AiCredentialOutlierHandle {
        use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};

        let store = crate::ai::credential_outlier::AiCredentialOutlierStore::new();
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionAIService,
            namespace: "default".to_string(),
            name: "provider".to_string(),
        };
        store.configure_service(target.clone(), None, ["primary".to_string()]);
        store
            .current_handle(&target)
            .and_then(|service| service.capture("primary"))
            .expect("credential handle")
    }

    fn admission_input(
        outlier: Option<AiCredentialOutlierHandle>,
        limits: Option<AiCredentialLimits>,
        on_redis_failure: OnRedisFailure,
    ) -> AiAttemptAdmissionInput {
        AiAttemptAdmissionInput {
            namespace: "default".to_string(),
            service_uid: "service-uid".to_string(),
            credential_alias: "primary".to_string(),
            service_name: "default/provider".to_string(),
            provider: AiProviderLabel::OpenAiCompatible,
            pricing: None,
            redis_ref: None,
            on_redis_failure,
            limits,
            estimated_input_tokens: 5,
            max_output_tokens: 10,
            usage: Arc::new(AiUsageShared::default()),
            eos: new_eos_latch(),
            outlier,
            adaptive: None,
            circuit_breaker: None,
            limiter_core: Arc::new(RedisRateLimiter::new()),
        }
    }

    fn attempt(input: u64, output_chars: u64, eos: bool) -> AiAttemptRuntime {
        let usage = Arc::new(AiUsageShared::default());
        usage.set_input_chars(input.saturating_mul(4));
        usage.add_output_units_for_test(output_chars);
        let latch = new_eos_latch();
        if eos {
            mark_eos(&latch);
        }
        AiAttemptRuntime {
            reservation: None,
            outlier: None,
            usage,
            eos: latch,
            estimated_input_tokens: input,
            service_name: "default/provider".to_string(),
            provider: AiProviderLabel::OpenAiCompatible,
            credential_alias: "primary".to_string(),
            pricing: None,
            adaptive: None,
            status: None,
            retry_after_seconds: None,
            dispatched: true,
            verdict_recorded: false,
            _circuit_breaker: None,
        }
    }

    #[test]
    fn response_success_waits_for_eos_and_transport_failure_is_single_verdict() {
        let mut current = attempt(1, 0, false);
        current.record_response(200, None);
        assert!(
            !current.verdict_recorded,
            "a response header is not a completed success"
        );
        assert!(current.record_failure_once(FailureClass::LocalOrigin));
        assert!(!current.record_failure_once(FailureClass::LocalOrigin));
    }

    #[test]
    fn cooldown_delta_seconds_rounds_up_consistently() {
        assert_eq!(cooldown_delta_seconds(Duration::ZERO), 1);
        assert_eq!(cooldown_delta_seconds(Duration::from_millis(1_500)), 2);
        assert_eq!(cooldown_delta_seconds(Duration::from_secs(3)), 3);
    }

    #[test]
    fn half_open_probe_is_not_completed_by_a_success_header() {
        use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
        use edgion_resources::resources::outlier_detection::OutlierDetectionResolved;

        let store = crate::ai::credential_outlier::AiCredentialOutlierStore::new();
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionAIService,
            namespace: "default".to_string(),
            name: "provider".to_string(),
        };
        store.configure_service(
            target.clone(),
            Some(OutlierDetectionResolved {
                consecutive_errors: 1,
                consecutive_gateway_errors: 1,
                consecutive_local_origin_failures: 1,
                ejection_seconds: 1,
                max_ejection_seconds: 10,
                max_ejection_percent: 100,
            }),
            ["primary".to_string()],
        );
        let handle = store
            .current_handle(&target)
            .and_then(|service| service.capture("primary"))
            .expect("credential handle");
        handle.force_expired_cooldown_for_test();
        let EjectionAdmission::Probe(probe) = handle.admit() else {
            panic!("expired credential must admit one half-open probe");
        };

        let mut current = attempt(1, 0, false);
        current.outlier = Some(OutlierLease {
            handle: handle.clone(),
            probe: Some(probe),
        });
        current.record_response(200, None);
        assert!(!current.verdict_recorded);
        assert!(current.outlier.as_ref().is_some_and(|outlier| outlier.probe.is_some()));
        assert!(current.record_failure_once(FailureClass::LocalOrigin));
        assert!(matches!(handle.admit(), EjectionAdmission::Rejected));
    }

    #[tokio::test]
    async fn successful_eos_completes_the_half_open_probe() {
        let handle = closed_outlier_handle();
        handle.force_expired_cooldown_for_test();
        let EjectionAdmission::Probe(probe) = handle.admit() else {
            panic!("expired credential must admit one half-open probe");
        };
        let mut current = attempt(1, 0, true);
        current.status = Some(200);
        current.outlier = Some(OutlierLease {
            handle: handle.clone(),
            probe: Some(probe),
        });
        let mut state = AiAttemptRuntimeState::default();
        state.install(current);
        state.finalize(None).await;
        assert!(matches!(handle.admit(), EjectionAdmission::Closed));
    }

    #[tokio::test]
    async fn admission_guards_active_overflow_outlier_and_redis_policy() {
        let mut missing_outlier = AiAttemptRuntimeState::default();
        assert!(matches!(
            missing_outlier
                .admit(admission_input(None, None, OnRedisFailure::Deny))
                .await,
            AiAttemptAdmission::Skip
        ));

        let handle = closed_outlier_handle();
        let mut active = AiAttemptRuntimeState::default();
        let AiAttemptAdmission::Ready(runtime) = active
            .admit(admission_input(Some(handle.clone()), None, OnRedisFailure::Deny))
            .await
        else {
            panic!("a closed credential without limits must be admitted");
        };
        active.install(runtime);
        assert!(matches!(
            active
                .admit(admission_input(Some(handle.clone()), None, OnRedisFailure::Deny))
                .await,
            AiAttemptAdmission::DependencyDenied
        ));

        let limits = Some(AiCredentialLimits {
            requests_per_minute: Some(1),
            tokens_per_minute: None,
        });
        let mut fail_open = AiAttemptRuntimeState::default();
        assert!(matches!(
            fail_open
                .admit(admission_input(
                    Some(handle.clone()),
                    limits.clone(),
                    OnRedisFailure::Allow,
                ))
                .await,
            AiAttemptAdmission::Ready(_)
        ));
        let mut fail_closed = AiAttemptRuntimeState::default();
        assert!(matches!(
            fail_closed
                .admit(admission_input(Some(handle), limits, OnRedisFailure::Deny))
                .await,
            AiAttemptAdmission::DependencyDenied
        ));
    }

    #[tokio::test]
    async fn failed_attempts_accumulate_input_and_final_eos_adds_output() {
        let mut state = AiAttemptRuntimeState::default();
        state.install(attempt(5, 0, false));
        state.mark_dispatched();
        state.finish_failure(AiAttemptFailure::Known(FailureClass::Gateway));

        let final_attempt = attempt(7, 9, true);
        assert_eq!(final_attempt.final_charged_tokens(), Some(10));
        state.install(final_attempt);
        state.finalize(Some(12)).await;
        assert!(state.ever_dispatched());
        assert_eq!(state.attempts().len(), 2);
        assert_eq!(state.charged_request_tokens(), Ok(15));
    }

    #[tokio::test]
    async fn adaptive_settlement_charges_every_dispatch_but_ttft_requires_2xx_eos() {
        use std::collections::HashSet;

        use crate::ai::{AdaptiveTelemetryKey, AdaptiveTelemetryStore};
        use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};

        fn telemetry(store: &AdaptiveTelemetryStore, suffix: &str, second: u64) -> Arc<AdaptiveTelemetryState> {
            let key = AdaptiveTelemetryKey::new(
                format!("route-{suffix}"),
                "rule-a".to_string(),
                PolicyTargetKey {
                    target_kind: PolicyTargetKind::EdgionAIService,
                    namespace: "default".to_string(),
                    name: "provider".to_string(),
                },
                "model-a".to_string(),
            )
            .unwrap();
            store.reconcile_keys(&HashSet::from([key.clone()]));
            let state = store.get(&key).unwrap();
            state.tick(second);
            state
        }

        let second = current_unix_second();
        let store = AdaptiveTelemetryStore::new();
        let success = telemetry(&store, "success", second);
        let failed_status = telemetry(&store, "failed-status", second);
        let incomplete = telemetry(&store, "incomplete", second);

        for (adaptive, status, eos) in [
            (Arc::clone(&success), 200, true),
            (Arc::clone(&failed_status), 500, true),
            (Arc::clone(&incomplete), 200, false),
        ] {
            let mut runtime = AiAttemptRuntimeState::default();
            let mut active = attempt(5, 8, eos);
            active.status = Some(status);
            active.adaptive = Some(adaptive);
            runtime.install(active);
            runtime.finalize(Some(12)).await;
            assert_eq!(runtime.charged_request_tokens(), Ok(7));
        }

        for state in [&success, &failed_status, &incomplete] {
            state.tick(second + 1);
            assert_eq!(state.snapshot().charged_tokens_60s, 7);
        }
        assert_eq!(success.snapshot().latency_samples, 1);
        assert_eq!(success.snapshot().latency_p50_micros, Some(15_000));
        assert_eq!(failed_status.snapshot().latency_samples, 0);
        assert_eq!(incomplete.snapshot().latency_samples, 0);
    }

    #[tokio::test]
    async fn token_totals_cover_every_attempt_by_fixed_provider_and_source() {
        let mut state = AiAttemptRuntimeState::default();
        let mut failed = attempt(5, 0, false);
        failed.provider = AiProviderLabel::OpenAi;
        failed.usage.observe_reported_usage(3, 4, 0, 0);
        state.install(failed);
        state.mark_dispatched();
        state.finish_failure(AiAttemptFailure::Known(FailureClass::Gateway));

        let mut final_attempt = attempt(5, 9, true);
        final_attempt.provider = AiProviderLabel::Anthropic;
        state.install(final_attempt);
        state.mark_dispatched();
        state.finalize(None).await;

        assert_eq!(state.charged_request_tokens(), Ok(15));
        assert_eq!(
            state.metric_totals(),
            Ok([
                AiAttemptTokenTotals {
                    provider: AiProviderLabel::OpenAi,
                    usage_source: GenAiAttemptUsageSource::Reported,
                    input_tokens: 3,
                    output_tokens: 4,
                },
                AiAttemptTokenTotals {
                    provider: AiProviderLabel::Anthropic,
                    usage_source: GenAiAttemptUsageSource::Estimated,
                    input_tokens: 5,
                    output_tokens: 3,
                },
            ]
            .as_slice())
        );
    }

    #[test]
    fn ambiguous_failures_log_observed_input_while_redis_reservation_is_retained() {
        let mut state = AiAttemptRuntimeState::default();
        state.install(attempt(5, 100, false));
        state.mark_dispatched();
        state.finish_failure(AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin));
        state.install(attempt(7, 100, false));
        state.mark_dispatched();
        state.finish_failure(AiAttemptFailure::BackendRequestTimeout);

        // Both failure branches use SettlementOperation::Retain, preserving
        // the full Redis reservation. Access and identity accounting do not
        // pretend the unobserved reserved output was consumed.
        assert_eq!(state.charged_request_tokens(), Ok(12));
        assert!(state.attempts().iter().all(|entry| entry.output_tokens == 0));
    }

    #[tokio::test]
    async fn attempt_outcomes_choose_the_expected_settlement_operation() {
        let mut predispatch = AiAttemptRuntimeState::default();
        predispatch.install(attempt(5, 0, false));
        predispatch.finish_predispatch();
        assert_eq!(
            predispatch.settlement_decisions_for_test(),
            [SettlementOperation::Release]
        );

        let mut known = AiAttemptRuntimeState::default();
        known.install(attempt(5, 0, false));
        known.finish_failure(AiAttemptFailure::Known(FailureClass::Gateway));
        assert_eq!(
            known.settlement_decisions_for_test(),
            [SettlementOperation::Reconcile(5)]
        );

        for failure in [
            AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin),
            AiAttemptFailure::BackendRequestTimeout,
        ] {
            let mut state = AiAttemptRuntimeState::default();
            state.install(attempt(5, 0, false));
            state.finish_failure(failure);
            assert_eq!(state.settlement_decisions_for_test(), [SettlementOperation::Retain]);
        }

        let mut undispatched = AiAttemptRuntimeState::default();
        let mut current = attempt(5, 0, false);
        current.dispatched = false;
        undispatched.install(current);
        undispatched.finalize(None).await;
        assert_eq!(
            undispatched.settlement_decisions_for_test(),
            [SettlementOperation::Release]
        );

        let mut complete = AiAttemptRuntimeState::default();
        complete.install(attempt(5, 9, true));
        complete.finalize(None).await;
        assert_eq!(
            complete.settlement_decisions_for_test(),
            [SettlementOperation::Reconcile(8)]
        );

        let mut incomplete = AiAttemptRuntimeState::default();
        incomplete.install(attempt(5, 9, false));
        incomplete.finalize(None).await;
        assert_eq!(
            incomplete.settlement_decisions_for_test(),
            [SettlementOperation::Retain]
        );

        let mut overflow = AiAttemptRuntimeState::default();
        let overflowed = attempt(1, 0, true);
        overflowed.usage.observe_reported_usage(u64::MAX, 1, 0, 0);
        overflow.install(overflowed);
        overflow.finalize(None).await;
        assert_eq!(overflow.settlement_decisions_for_test(), [SettlementOperation::Retain]);
        assert_eq!(overflow.charged_request_tokens(), Err(()));
    }

    #[test]
    fn settlement_decisions_map_to_redis_operations() {
        assert_eq!(
            redis_settlement_operation(SettlementOperation::Release),
            Some(CredentialReservationOperation::ReleasePredispatch)
        );
        assert_eq!(
            redis_settlement_operation(SettlementOperation::Reconcile(7)),
            Some(CredentialReservationOperation::Reconcile { charged_tokens: 7 })
        );
        assert_eq!(redis_settlement_operation(SettlementOperation::Retain), None);
    }

    #[test]
    fn serialized_attempt_has_only_the_safe_bounded_schema() {
        let entry = GenAiAttempt {
            number: 1,
            service: "default/service-a".to_string(),
            provider: "openai",
            credential: "primary".to_string(),
            status: Some(429),
            failure: Some(GenAiAttemptFailure::RetryableStatus),
            retry_after_seconds: Some(3),
            ttft_ms: Some(11),
            usage_source: GenAiAttemptUsageSource::Reported,
            input_tokens: 2,
            output_tokens: 1,
            estimated_cost_usd: Some("0.00001".to_string()),
        };
        let value = serde_json::to_value(entry).unwrap();
        let keys: std::collections::BTreeSet<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "credential",
                "estimated_cost_usd",
                "failure",
                "input_tokens",
                "number",
                "output_tokens",
                "provider",
                "retry_after_seconds",
                "service",
                "status",
                "ttft_ms",
                "usage_source",
            ]
            .into_iter()
            .collect()
        );
        for forbidden in ["endpoint", "ip", "secret", "body", "provider_error", "tool_args"] {
            assert!(!value.as_object().unwrap().contains_key(forbidden));
        }
    }

    #[test]
    fn reported_usage_overflow_is_explicit() {
        let attempt = attempt(1, 0, true);
        attempt.usage.observe_reported_usage(u64::MAX, 1, 0, 0);
        assert_eq!(attempt.final_charged_tokens(), None);
    }

    #[tokio::test]
    async fn ledger_is_bounded_to_eight_entries_and_eight_kib() {
        let mut state = AiAttemptRuntimeState::default();
        for _ in 0..MAX_PENDING_SETTLEMENTS {
            let mut current = attempt(1, 0, false);
            current.service_name = "s".repeat(2_000);
            current.credential_alias = "c".repeat(2_000);
            state.install(current);
            state.mark_dispatched();
            state.finish_failure(AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin));
        }
        assert_eq!(state.attempts().len(), MAX_PENDING_SETTLEMENTS);
        assert!(serde_json::to_vec(state.attempts()).unwrap().len() <= MAX_AI_ATTEMPT_LOG_BYTES);
        assert!(state.attempts().iter().all(|entry| {
            entry.service.len() <= MAX_SERVICE_KEY_BYTES && entry.credential.len() <= MAX_CREDENTIAL_ALIAS_BYTES
        }));

        state.install(attempt(1, 0, false));
        state.mark_dispatched();
        state.finish_failure(AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin));
        assert_eq!(state.attempts().len(), MAX_PENDING_SETTLEMENTS);
        assert!(state.attempts_truncated());
    }

    #[tokio::test]
    async fn accounting_overflow_fails_closed_without_wrapping() {
        let mut state = AiAttemptRuntimeState::default();
        state.install(attempt(u64::MAX, 0, false));
        state.mark_dispatched();
        state.finish_failure(AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin));
        state.install(attempt(1, 0, false));
        state.mark_dispatched();
        state.finish_failure(AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin));
        assert_eq!(state.charged_request_tokens(), Err(()));
        assert!(matches!(
            state
                .admit(admission_input(
                    Some(closed_outlier_handle()),
                    None,
                    OnRedisFailure::Deny,
                ))
                .await,
            AiAttemptAdmission::DependencyDenied
        ));
    }
}
