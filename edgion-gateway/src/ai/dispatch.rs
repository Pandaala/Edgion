//! First AI upstream-attempt preparation.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::resources::edgion_backend::AiProvider;
use edgion_resources::resources::http_route::HTTPRouteFilterType;
use pingora_proxy::ResponseBodySink;
use serde_json::Value;

use crate::ai::anthropic::{
    prepare_anthropic_request, AnthropicRequestError, AnthropicResponseProcessor, ANTHROPIC_VERSION,
};
use crate::ai::attempt::{
    AttemptScheduler, BackendCredentials, DispatchFailureKind, DispatchOutcome, NextStep, SchedulerEvent,
    MAX_AI_ROUTE_RETRIES,
};
use crate::ai::routing::{pick, CandidateEligibility, CostSnapshot, PickerInput, RouteCandidate};
use crate::ai::{
    AiAttemptPlan, AiProviderLabel, AiProxyError, AiRequestContext, AiUsageShared, ReadyAiAttemptPlan,
    UsageExtractionProcessor,
};
use crate::backends::{SelectedAiBackend, SelectedHttpBackend, SelectedLogicalBackend};
use crate::plugins::runtime::session_adapter::PingoraSessionAdapter;
use crate::plugins::runtime::traits::{PluginSession, StreamingResponseProcessor};
use crate::EdgionHttpContext;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

pub struct PreparedAiAttempt {
    pub selected: SelectedAiBackend,
    pub provider: AiProviderLabel,
    pub pricing: Option<edgion_resources::resources::edgion_backend::ParsedAiModelPricing>,
    pub redis_ref: Option<String>,
    pub on_redis_failure: edgion_resources::resources::edgion_plugins::OnRedisFailure,
    pub credential_limits: Option<edgion_resources::resources::edgion_backend::AiCredentialLimits>,
    pub estimated_input_tokens: u64,
    pub max_output_tokens: u64,
    pub usage: Arc<AiUsageShared>,
    pub eos: Arc<AtomicBool>,
    pub limiter_core: Arc<crate::plugins::common::redis_rate_limiter::RedisRateLimiter>,
    ai_context: Option<AiRequestContext>,
    response_processor: Option<Box<dyn StreamingResponseProcessor>>,
}

pub fn commit_ai_attempt_context(
    ctx: &mut EdgionHttpContext,
    target: &mut PreparedAiAttempt,
) -> Result<(), AiDispatchError> {
    let quota = match ctx.ai_attempt_plan.as_mut() {
        Some(AiAttemptPlan::Ready(plan)) => plan.quota.take(),
        _ => None,
    };
    let mut context = target.ai_context.take().ok_or(AiDispatchError::InvalidConfiguration)?;
    context.quota = quota;
    ctx.replace_ai_attempt_context(context);
    ctx.ai_stream_processor = target.response_processor.take();
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiDispatchError {
    Rejected,
    Unavailable,
    InvalidConfiguration,
    RequestDeadlineExceeded,
    GrpcDeadlineExceeded,
    /// OpenAI-compatible client error; must not become an HTML 500.
    Client(AiProxyError),
}

/// Resolve the binding end-to-end deadline for an AI predispatch decision.
/// Request timeout takes precedence when both deadlines expire together,
/// matching the outer proxy gate and retry-backoff checks.
#[inline]
pub(crate) fn ai_predispatch_deadline(ctx: &EdgionHttpContext) -> Option<AiDispatchError> {
    if ctx.request_timeout_exceeded() {
        Some(AiDispatchError::RequestDeadlineExceeded)
    } else if ctx.grpc_deadline_exceeded() {
        Some(AiDispatchError::GrpcDeadlineExceeded)
    } else {
        None
    }
}

/// A failure while constructing an AI upstream peer before dispatch starts.
/// Variants encode the scheduler scope so endpoint failures cannot silently
/// fall back to credential-by-credential retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiPeerPredispatchFailure {
    Endpoint,
}

impl AiPeerPredispatchFailure {
    /// Release predispatch accounting and advance at the failure's scope.
    pub fn finish(self, ctx: &mut EdgionHttpContext) -> bool {
        ctx.ai_attempt_runtime.finish_predispatch();
        match self {
            Self::Endpoint => handle_ai_predispatch_service_failure(ctx),
        }
    }
}

fn anthropic_request_error(error: AnthropicRequestError) -> AiDispatchError {
    AiDispatchError::Client(match error {
        AnthropicRequestError::UnsupportedParameter => AiProxyError::UnsupportedParameter,
        AnthropicRequestError::InvalidRequestBody => AiProxyError::MalformedBody,
        AnthropicRequestError::MissingRequiredParameter => AiProxyError::MissingRequiredParameter,
        AnthropicRequestError::OutputTooLarge => AiProxyError::RequestTooLarge,
        AnthropicRequestError::SerializationFailed => AiProxyError::MalformedBody,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiRetryDecision {
    Retry,
    Final,
    BudgetDenied,
}

#[derive(Debug, PartialEq, Eq)]
struct SelectedTarget {
    candidate: usize,
    credential: usize,
}

/// Prepare one provider attempt without using the generic upstream-jump slot.
pub fn prepare_first_attempt(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    entropy: u128,
    route_retry_cap: Option<u32>,
    global_retry_cap: u32,
) -> Result<PreparedAiAttempt, AiDispatchError> {
    // Every predispatch scan iteration enters through this function. Re-check
    // both end-to-end deadlines before taking or mutating the plan so slow
    // admission/endpoint failures cannot keep scanning after the caller's
    // budget has expired.
    if let Some(error) = ai_predispatch_deadline(ctx) {
        return Err(error);
    }
    let Some(mut plan) = ctx.ai_attempt_plan.take() else {
        return Err(AiDispatchError::InvalidConfiguration);
    };
    let result = match &mut plan {
        AiAttemptPlan::Rejected(_) => Err(AiDispatchError::Rejected),
        AiAttemptPlan::Ready(ready) => {
            let retry_cap = resolve_ai_retry_cap(route_retry_cap, global_retry_cap)?;
            initialize_retry_cap(ready, retry_cap)?;
            prepare_ready_attempt(session, ctx, ready, entropy)
        }
    };
    ctx.ai_attempt_plan = Some(plan);
    result
}

/// Cross the dispatch boundary only after Pingora has connected and is about
/// to write the upstream request. Pre-connect failures consume no scheduler
/// attempt, quota reservation, or retry-budget denominator sample.
pub fn mark_ai_dispatch_started(ctx: &mut EdgionHttpContext) -> Result<(), AiDispatchError> {
    let Some(mut plan) = ctx.ai_attempt_plan.take() else {
        return Ok(());
    };
    let mut newly_dispatched = false;
    let result = match &mut plan {
        AiAttemptPlan::Rejected(_) => Err(AiDispatchError::Rejected),
        AiAttemptPlan::Ready(ready) => match ready.scheduler.next_step() {
            NextStep::Dispatch(_) => ready.scheduler.start_dispatch().map_or_else(
                |_| Err(AiDispatchError::InvalidConfiguration),
                |transition| {
                    newly_dispatched = matches!(transition.event, SchedulerEvent::DispatchStarted { .. });
                    newly_dispatched
                        .then_some(())
                        .ok_or(AiDispatchError::InvalidConfiguration)
                },
            ),
            NextStep::AwaitResult(_) => Ok(()),
            _ => Err(AiDispatchError::InvalidConfiguration),
        },
    };
    if newly_dispatched {
        ctx.ai_attempt_runtime.mark_dispatched();
        if let Some(budget) = ctx
            .ai_effective_policy
            .as_ref()
            .and_then(|policy| policy.retry_budget.as_ref())
        {
            budget.on_attempt();
        }
    }
    ctx.ai_attempt_plan = Some(plan);
    result
}

/// Handle a failure before the first upstream byte can be written. This skips
/// the candidate without consuming scheduler capacity or retry budget.
pub fn handle_ai_predispatch_failure(ctx: &mut EdgionHttpContext, gates_allow_retry: bool) -> bool {
    let Some(mut plan) = ctx.ai_attempt_plan.take() else {
        return false;
    };
    let retry = match &mut plan {
        AiAttemptPlan::Ready(ready)
            if gates_allow_retry && matches!(ready.scheduler.next_step(), NextStep::Dispatch(_)) =>
        {
            ready.scheduler.skip_predispatch().is_ok()
        }
        _ => false,
    };
    ctx.ai_attempt_plan = Some(plan);
    retry
}

pub fn handle_ai_predispatch_service_failure(ctx: &mut EdgionHttpContext) -> bool {
    let Some(mut plan) = ctx.ai_attempt_plan.take() else {
        return false;
    };
    let retry = match &mut plan {
        AiAttemptPlan::Ready(ready) if matches!(ready.scheduler.next_step(), NextStep::Dispatch(_)) => {
            ready.scheduler.skip_predispatch_backend().is_ok()
        }
        _ => false,
    };
    ctx.ai_attempt_plan = Some(plan);
    retry
}

/// Resolve one final response header before provider translation runs.
pub fn handle_ai_response(
    ctx: &mut EdgionHttpContext,
    wants_retry: bool,
    gates_allow_retry: bool,
) -> Result<AiRetryDecision, AiDispatchError> {
    handle_dispatched_outcome(ctx, wants_retry, gates_allow_retry, false, false)
}

/// Resolve a post-dispatch transport/proxy failure.
pub fn handle_ai_transport_failure(
    ctx: &mut EdgionHttpContext,
    retryable: bool,
    gates_allow_retry: bool,
    ambiguous: bool,
) -> Result<AiRetryDecision, AiDispatchError> {
    handle_dispatched_outcome(ctx, retryable, gates_allow_retry, ambiguous, true)
}

fn handle_dispatched_outcome(
    ctx: &mut EdgionHttpContext,
    wants_retry: bool,
    gates_allow_retry: bool,
    ambiguous: bool,
    transport_failure: bool,
) -> Result<AiRetryDecision, AiDispatchError> {
    let Some(mut plan) = ctx.ai_attempt_plan.take() else {
        return Err(AiDispatchError::InvalidConfiguration);
    };
    let result = match &mut plan {
        AiAttemptPlan::Rejected(_) => Err(AiDispatchError::Rejected),
        AiAttemptPlan::Ready(ready) if ambiguous && !wants_retry => ready
            .scheduler
            .finish_dispatch(DispatchOutcome::AmbiguousFailure)
            .map(|_| AiRetryDecision::Final)
            .map_err(|_| AiDispatchError::InvalidConfiguration),
        AiAttemptPlan::Ready(ready) if !wants_retry => ready
            .scheduler
            .finish_dispatch(if transport_failure {
                DispatchOutcome::NonRetryableFailure
            } else {
                DispatchOutcome::Completed
            })
            .map(|_| AiRetryDecision::Final)
            .map_err(|_| AiDispatchError::InvalidConfiguration),
        AiAttemptPlan::Ready(ready)
            if !gates_allow_retry || ready.scheduler.dispatch_count() >= ready.scheduler.max_dispatches() =>
        {
            ready
                .scheduler
                .deny_retry(if ambiguous {
                    DispatchFailureKind::Ambiguous
                } else {
                    DispatchFailureKind::Retryable
                })
                .map(|_| AiRetryDecision::Final)
                .map_err(|_| AiDispatchError::InvalidConfiguration)
        }
        AiAttemptPlan::Ready(ready) => {
            let budget_allowed = ctx
                .ai_effective_policy
                .as_ref()
                .and_then(|policy| policy.retry_budget.as_ref())
                .is_none_or(|budget| budget.try_retry());
            if !budget_allowed {
                ready
                    .scheduler
                    .deny_retry(if ambiguous {
                        DispatchFailureKind::Ambiguous
                    } else {
                        DispatchFailureKind::Retryable
                    })
                    .map_err(|_| AiDispatchError::InvalidConfiguration)?;
                Ok(AiRetryDecision::BudgetDenied)
            } else if ambiguous {
                ready
                    .scheduler
                    .retry_ambiguous_dispatch()
                    .map(|_| AiRetryDecision::Retry)
                    .map_err(|_| AiDispatchError::InvalidConfiguration)
            } else {
                ready
                    .scheduler
                    .finish_dispatch(DispatchOutcome::RetryableFailure)
                    .map(|_| AiRetryDecision::Retry)
                    .map_err(|_| AiDispatchError::InvalidConfiguration)
            }
        }
    };
    ctx.ai_attempt_plan = Some(plan);
    result
}

fn resolve_ai_retry_cap(route_retry_cap: Option<u32>, global_retry_cap: u32) -> Result<u8, AiDispatchError> {
    match route_retry_cap {
        Some(retries) if retries > u32::from(MAX_AI_ROUTE_RETRIES) => Err(AiDispatchError::InvalidConfiguration),
        Some(retries) => Ok(retries as u8),
        None => Ok(global_retry_cap.min(u32::from(MAX_AI_ROUTE_RETRIES)) as u8),
    }
}

fn prepare_ready_attempt(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    plan: &mut ReadyAiAttemptPlan,
    entropy: u128,
) -> Result<PreparedAiAttempt, AiDispatchError> {
    if let Some(baseline) = ctx.ai_request_baseline.clone() {
        *session.req_header_mut() = baseline;
        let config = edgion_resources::path_normalization::get_path_normalization_config();
        ctx.refresh_normalized_path(session.req_header(), config);
    }
    let selected = select_target(plan, entropy)?;
    let candidate = plan
        .candidates
        .get(selected.candidate)
        .ok_or(AiDispatchError::InvalidConfiguration)?;
    let selected_route_backend = candidate.logical.compiled();
    let ai_backend = candidate.backend.as_ref().ok_or(AiDispatchError::Unavailable)?;
    let model = candidate.model.as_ref().ok_or(AiDispatchError::Unavailable)?;
    let credential = ai_backend
        .credential_pool
        .credentials
        .get(selected.credential)
        .ok_or(AiDispatchError::InvalidConfiguration)?;
    let credential_outlier = candidate
        .credential_outliers
        .get(selected.credential)
        .cloned()
        .flatten();
    let selected_backend = SelectedAiBackend::new(
        candidate.logical.clone(),
        Arc::clone(ai_backend),
        candidate.effective_policy.clone(),
        candidate.backend_tls_policy.clone(),
        candidate.adaptive.clone(),
        credential_outlier.clone(),
        selected.credential,
        credential.name.clone(),
        candidate.weight,
    )
    .ok_or(AiDispatchError::InvalidConfiguration)?;
    let (body, requested_streaming, anthropic_usage_chunk) = match ai_backend.provider {
        AiProvider::OpenAI | AiProvider::OpenAICompatible => {
            let body = openai_provider_body(&plan.canonical_openai_body, &model.provider_model)?;
            (body, plan.requested_streaming, false)
        }
        AiProvider::Anthropic => {
            let value = serde_json::from_slice(&plan.canonical_openai_body)
                .map_err(|_| AiDispatchError::InvalidConfiguration)?;
            let max_output_bytes = plan
                .canonical_openai_body
                .len()
                .saturating_mul(2)
                .saturating_add(64 * 1024);
            let translated = prepare_anthropic_request(
                value,
                &model.provider_model,
                ai_backend.max_output_tokens,
                plan.inject_usage_tracking,
                max_output_bytes,
            )
            .map_err(anthropic_request_error)?;
            (
                Bytes::from(translated.body),
                translated.requested_streaming,
                translated.effective_usage_chunk,
            )
        }
    };
    ctx.overwrite_request_body(body)
        .map_err(|_| AiDispatchError::InvalidConfiguration)?;

    if ctx.ai_request_baseline.is_none() {
        ctx.ai_request_baseline = Some(session.req_header().clone());
    }
    apply_backend_request_headers(session, ctx, &selected_route_backend)?;
    let uri = provider_uri(
        ai_backend.provider,
        &ai_backend.endpoint.path_prefix,
        &plan.canonical_path,
        plan.canonical_query.as_deref(),
    );
    {
        let mut adapter = PingoraSessionAdapter::new(session, ctx);
        for header in [
            "authorization",
            "x-api-key",
            "api-key",
            "anthropic-version",
            "anthropic-beta",
            "accept-encoding",
        ] {
            adapter
                .remove_request_header(header)
                .map_err(|_| AiDispatchError::InvalidConfiguration)?;
        }
        adapter
            .set_upstream_uri(&uri)
            .map_err(|_| AiDispatchError::InvalidConfiguration)?;
        adapter
            .set_upstream_host(&ai_backend.endpoint.host)
            .map_err(|_| AiDispatchError::InvalidConfiguration)?;
        match ai_backend.provider {
            AiProvider::OpenAI | AiProvider::OpenAICompatible => {
                let authorization = format!("Bearer {}", credential.credential);
                adapter
                    .set_request_header("authorization", &authorization)
                    .map_err(|_| AiDispatchError::InvalidConfiguration)?;
            }
            AiProvider::Anthropic => {
                for (name, value) in [
                    ("x-api-key", credential.credential.as_str()),
                    ("anthropic-version", ANTHROPIC_VERSION),
                    ("content-type", "application/json"),
                ] {
                    adapter
                        .set_request_header(name, value)
                        .map_err(|_| AiDispatchError::InvalidConfiguration)?;
                }
            }
        }
    }

    let usage = Arc::new(AiUsageShared::default());
    usage.set_input_chars(plan.input_chars);
    let eos = crate::ai::attempt_runtime::new_eos_latch();
    let quota_accumulator = plan.quota.as_ref().map(|quota| quota.accumulator()).or_else(|| {
        ctx.ai
            .as_ref()
            .and_then(|ai| ai.quota.as_ref())
            .map(|quota| quota.accumulator())
    });
    let provider = provider_label(ai_backend.provider);
    let context = AiRequestContext {
        provider,
        model: plan.logical_model.clone(),
        pricing: model.pricing,
        requested_streaming,
        usage: Arc::clone(&usage),
        quota: None,
        quota_status: plan.quota_status,
    };
    let mut response_processors: Vec<Box<dyn StreamingResponseProcessor>> = Vec::with_capacity(1);
    if ai_backend.provider == AiProvider::Anthropic {
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        response_processors.push(Box::new(AnthropicResponseProcessor::new_with_quota(
            model.provider_model.clone(),
            requested_streaming,
            anthropic_usage_chunk,
            created,
            Arc::clone(&usage),
            quota_accumulator.clone(),
            plan.terminate_on_over_limit,
        )));
    }
    if ai_backend.provider != AiProvider::Anthropic {
        response_processors.push(Box::new(UsageExtractionProcessor::new_with_quota(
            Arc::clone(&usage),
            quota_accumulator,
            requested_streaming,
            plan.terminate_on_over_limit,
        )));
    }
    let response_processor: Box<dyn StreamingResponseProcessor> = Box::new(AiAttemptResponseProcessor {
        processors: response_processors,
        eos: Arc::clone(&eos),
    });

    ctx.selected_backend = Some(SelectedHttpBackend::Routed(SelectedLogicalBackend::Ai(
        selected_backend.clone(),
    )));
    let target = PreparedAiAttempt {
        selected: selected_backend,
        provider,
        pricing: model.pricing,
        redis_ref: ai_backend.credential_pool.redis_ref.clone(),
        on_redis_failure: ai_backend.credential_pool.on_redis_failure.clone(),
        credential_limits: credential.limits.clone(),
        estimated_input_tokens: plan.input_chars.saturating_add(3) / 4,
        max_output_tokens: effective_max_output_tokens(&plan.canonical_openai_body, ai_backend.max_output_tokens)?,
        usage,
        eos,
        limiter_core: Arc::clone(&ai_backend.credential_pool.limiter_core),
        ai_context: Some(context),
        response_processor: Some(response_processor),
    };
    Ok(target)
}

fn initialize_retry_cap(plan: &mut ReadyAiAttemptPlan, retry_cap: u8) -> Result<(), AiDispatchError> {
    if plan.retry_cap_initialized {
        return Ok(());
    }
    let weights: Vec<Vec<u32>> = plan
        .candidates
        .iter()
        .map(|candidate| {
            candidate
                .backend
                .as_ref()
                .map(|backend| {
                    backend
                        .credential_pool
                        .credentials
                        .iter()
                        .map(|credential| credential.weight)
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();
    let backends: Vec<_> = weights.iter().map(|weights| BackendCredentials { weights }).collect();
    plan.scheduler = AttemptScheduler::new(&backends, retry_cap).map_err(|_| AiDispatchError::InvalidConfiguration)?;
    plan.retry_cap_initialized = true;
    Ok(())
}

fn openai_provider_body(canonical: &[u8], provider_model: &str) -> Result<Bytes, AiDispatchError> {
    let mut value: Value = serde_json::from_slice(canonical).map_err(|_| AiDispatchError::InvalidConfiguration)?;
    value
        .as_object_mut()
        .ok_or(AiDispatchError::InvalidConfiguration)?
        .insert("model".to_string(), Value::String(provider_model.to_string()));
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|_| AiDispatchError::InvalidConfiguration)
}

fn select_target(plan: &mut ReadyAiAttemptPlan, entropy: u128) -> Result<SelectedTarget, AiDispatchError> {
    loop {
        let service_step = match plan.scheduler.next_step() {
            NextStep::PickBackend(step) => Some(step),
            NextStep::PickCredential(step) => {
                let candidate_index = usize::from(step.backend);
                match select_credential(plan, candidate_index, entropy) {
                    Err(AiDispatchError::Unavailable) => continue,
                    result => return result,
                }
            }
            NextStep::Stop(_) => return Err(AiDispatchError::Unavailable),
            _ => return Err(AiDispatchError::InvalidConfiguration),
        };
        let service_step = service_step.expect("PickBackend branch supplies state");
        let route_candidates: Vec<_> =
            plan.candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    let dispatchable_credential =
                        candidate.backend.as_ref().is_some_and(|backend| {
                            backend.credential_pool.credentials.iter().enumerate().any(
                                |(credential_index, credential)| {
                                    credential.weight > 0
                                        && candidate
                                            .credential_outliers
                                            .get(credential_index)
                                            .is_some_and(Option::is_some)
                                },
                            )
                        });
                    let cost = candidate.model.as_ref().and_then(|model| {
                        model.pricing.map(|pricing| CostSnapshot {
                            input_price_micro_usd_per_million: pricing.input.get(),
                            output_price_micro_usd_per_million: pricing.output.get(),
                            service_max_output_tokens: candidate
                                .backend
                                .as_ref()
                                .and_then(|backend| backend.max_output_tokens)
                                .map(u64::from),
                        })
                    });
                    RouteCandidate {
                        slot: u8::try_from(candidate.slot).unwrap_or(u8::MAX),
                        order: candidate.order,
                        weight: candidate.weight,
                        eligibility: CandidateEligibility {
                            valid_ref: true,
                            model_compatible: candidate.model.is_some(),
                            runtime_available: dispatchable_credential,
                            untried: service_step.excluded_backends & (1u16 << index) == 0,
                        },
                        adaptive: candidate
                            .adaptive
                            .as_ref()
                            .map(|state| state.snapshot())
                            .unwrap_or_default(),
                        cost,
                    }
                })
                .collect();
        let selected = match pick(
            PickerInput {
                candidates: &route_candidates,
                entropy,
                selection_sequence: u64::from(plan.selection_sequence),
                input_tokens: plan.input_chars.saturating_add(3) / 4,
                request_max_output_tokens: request_max_tokens(&plan.canonical_openai_body),
            },
            plan.selection,
        ) {
            Ok(selected) => selected,
            Err(crate::ai::routing::PickerError::NoEligibleCandidate) => {
                let transition = plan
                    .scheduler
                    .complete_backend_scan()
                    .map_err(|_| AiDispatchError::InvalidConfiguration)?;
                if matches!(transition.event, SchedulerEvent::RoundAdvanced { .. }) {
                    continue;
                }
                return Err(AiDispatchError::Unavailable);
            }
            Err(_) => return Err(AiDispatchError::InvalidConfiguration),
        };
        let candidate_index = plan
            .candidates
            .iter()
            .position(|candidate| candidate.slot == usize::from(selected.slot))
            .ok_or(AiDispatchError::InvalidConfiguration)?;
        plan.scheduler
            .select_backend(u8::try_from(candidate_index).map_err(|_| AiDispatchError::InvalidConfiguration)?)
            .map_err(|_| AiDispatchError::InvalidConfiguration)?;
        match select_credential(plan, candidate_index, entropy) {
            Err(AiDispatchError::Unavailable) => continue,
            result => return result,
        }
    }
}

fn select_credential(
    plan: &mut ReadyAiAttemptPlan,
    candidate_index: usize,
    entropy: u128,
) -> Result<SelectedTarget, AiDispatchError> {
    let backend = plan.candidates[candidate_index]
        .backend
        .as_ref()
        .ok_or(AiDispatchError::Unavailable)?;
    let eligible = backend
        .credential_pool
        .credentials
        .iter()
        .enumerate()
        .fold(0u16, |mask, (index, _credential)| {
            if plan.candidates[candidate_index]
                .credential_outliers
                .get(index)
                .is_some_and(Option::is_some)
            {
                mask | (1u16 << index)
            } else {
                mask
            }
        });
    let transition = plan
        .scheduler
        .select_credential(eligible, entropy as u64)
        .map_err(|_| AiDispatchError::InvalidConfiguration)?;
    let target = match transition.event {
        SchedulerEvent::CredentialSelected { target } => target,
        SchedulerEvent::BackendExhausted { .. } => return Err(AiDispatchError::Unavailable),
        _ => return Err(AiDispatchError::InvalidConfiguration),
    };
    Ok(SelectedTarget {
        candidate: usize::from(target.backend),
        credential: usize::from(target.credential),
    })
}

fn apply_backend_request_headers(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    backend: &crate::routes::http::match_unit::CompiledBackend,
) -> Result<(), AiDispatchError> {
    let mut adapter = PingoraSessionAdapter::new(session, ctx);
    for filter in backend.schema.filters.as_deref().unwrap_or_default() {
        match filter.filter_type {
            HTTPRouteFilterType::RequestHeaderModifier => {
                let config = filter
                    .request_header_modifier
                    .as_ref()
                    .ok_or(AiDispatchError::InvalidConfiguration)?;
                for header in config.set.as_deref().unwrap_or_default() {
                    adapter
                        .set_request_header(&header.name, &header.value)
                        .map_err(|_| AiDispatchError::InvalidConfiguration)?;
                }
                for header in config.add.as_deref().unwrap_or_default() {
                    adapter
                        .append_request_header(&header.name, &header.value)
                        .map_err(|_| AiDispatchError::InvalidConfiguration)?;
                }
                for name in config.remove.as_deref().unwrap_or_default() {
                    adapter
                        .remove_request_header(name)
                        .map_err(|_| AiDispatchError::InvalidConfiguration)?;
                }
            }
            HTTPRouteFilterType::ResponseHeaderModifier => {}
            _ => return Err(AiDispatchError::InvalidConfiguration),
        }
    }
    Ok(())
}

fn provider_uri(provider: AiProvider, prefix: &str, canonical_path: &str, canonical_query: Option<&str>) -> String {
    let downstream_path = match provider {
        AiProvider::Anthropic => "/v1/messages",
        AiProvider::OpenAI | AiProvider::OpenAICompatible => canonical_path,
    };
    let prefix = prefix.trim_end_matches('/');
    let path = downstream_path.trim_start_matches('/');
    let mut result = if prefix.is_empty() {
        format!("/{path}")
    } else {
        format!("{prefix}/{path}")
    };
    if provider != AiProvider::Anthropic {
        if let Some(query) = canonical_query {
            result.push('?');
            result.push_str(query);
        }
    }
    result
}

struct AiAttemptResponseProcessor {
    processors: Vec<Box<dyn StreamingResponseProcessor>>,
    eos: Arc<AtomicBool>,
}

#[async_trait]
impl StreamingResponseProcessor for AiAttemptResponseProcessor {
    fn name(&self) -> &str {
        "AiAttemptResponse"
    }

    fn on_response_header(&mut self, response: &mut ResponseHeader) {
        for processor in &mut self.processors {
            processor.on_response_header(response);
        }
    }

    fn changes_body_length(&self) -> bool {
        self.processors.iter().any(|processor| processor.changes_body_length())
    }

    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        stream_kind: crate::ctx::ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    ) {
        for processor in &mut self.processors {
            processor
                .on_chunk(chunk, end_of_stream, stream_kind, sink, session)
                .await;
            if sink.is_terminated() {
                break;
            }
        }
        if end_of_stream {
            crate::ai::attempt_runtime::mark_eos(&self.eos);
        }
    }
}

/// Reads the client-declared output-token bound from a canonical OpenAI body.
/// Current OpenAI clients send `max_completion_tokens`; `max_tokens` is the
/// deprecated spelling and only serves as a fallback. Every consumer of the
/// request-declared bound must resolve it through this helper so cost ranking
/// and quota accounting never disagree about what the client asked for.
fn request_output_bound(value: &Value) -> Option<u64> {
    value
        .get("max_completion_tokens")
        .and_then(Value::as_u64)
        .or_else(|| value.get("max_tokens").and_then(Value::as_u64))
}

fn request_max_tokens(body: &[u8]) -> Option<u64> {
    request_output_bound(&serde_json::from_slice::<Value>(body).ok()?)
}

fn effective_max_output_tokens(body: &[u8], service_max: Option<u32>) -> Result<u64, AiDispatchError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| AiDispatchError::InvalidConfiguration)?;
    let max_tokens = value.get("max_tokens").and_then(Value::as_u64);
    let max_completion_tokens = value.get("max_completion_tokens").and_then(Value::as_u64);
    if max_tokens.is_some() && max_completion_tokens.is_some() && max_tokens != max_completion_tokens {
        return Err(AiDispatchError::Client(AiProxyError::MalformedBody));
    }
    Ok(match (request_output_bound(&value), service_max.map(u64::from)) {
        (Some(request), _) => request,
        (None, Some(backend)) => backend,
        (None, None) => 0,
    })
}

fn provider_label(provider: AiProvider) -> AiProviderLabel {
    match provider {
        AiProvider::OpenAI => AiProviderLabel::OpenAi,
        AiProvider::OpenAICompatible => AiProviderLabel::OpenAiCompatible,
        AiProvider::Anthropic => AiProviderLabel::Anthropic,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap, HashSet};

    use super::*;
    use crate::ai::attempt::{AttemptScheduler, BackendCredentials};
    use crate::ai::{
        AiBackendStore, AiCandidateModel, AiPlanCandidate, ReadyAiAttemptPlan, RuntimeAiBackend, RuntimeAiCredential,
        RuntimeAiCredentialPool, RuntimeAiEndpoint,
    };
    use crate::backends::policy::backend_traffic::{EffectiveBackendTrafficPolicy, RetryBudget};
    use crate::plugins::PluginRuntime;
    use crate::routes::http::match_unit::{CompiledBackend, HttpRouteRuleUnit, MatchSpecificity};
    use crate::{MatchInfo, Rsc};
    use edgion_resources::resources::common::RefDenied;
    use edgion_resources::resources::edgion_backend::AiCredentialLimits;
    use edgion_resources::resources::edgion_backend_traffic_policy::{BudgetDetails, RetryConstraintConfig};
    use edgion_resources::resources::edgion_plugins::{AiBackendSelection, OnRedisFailure};
    use edgion_resources::{HTTPBackendRef, HTTPRouteMatch, HTTPRouteRule, ParsedRouteTimeouts, RouteTimeout};
    use std::time::{Duration, Instant};
    use tokio::io::AsyncWriteExt;

    fn candidate_scheduler() -> AttemptScheduler {
        let mut scheduler = AttemptScheduler::new(&[BackendCredentials { weights: &[1] }], 0).unwrap();
        scheduler.select_backend(0).unwrap();
        scheduler.select_credential(1, 0).unwrap();
        scheduler
    }

    fn candidate_plan() -> ReadyAiAttemptPlan {
        ReadyAiAttemptPlan {
            canonical_openai_body: Bytes::from_static(br#"{"model":"logical"}"#),
            canonical_path: "/v1/chat/completions".to_string(),
            canonical_query: Some("trace=1".to_string()),
            logical_model: "logical".to_string(),
            requested_streaming: false,
            inject_usage_tracking: true,
            input_chars: 19,
            selection: AiBackendSelection::Weighted,
            selection_sequence: 0,
            candidates: Vec::new(),
            scheduler: candidate_scheduler(),
            retry_cap_initialized: true,
            backend_snapshot: AiBackendStore::new().snapshot(),
            quota: None,
            quota_status: None,
            terminate_on_over_limit: false,
        }
    }

    fn candidate_plan_with_retries(retries: u8) -> ReadyAiAttemptPlan {
        let mut plan = candidate_plan();
        plan.scheduler = AttemptScheduler::new(&[BackendCredentials { weights: &[1, 1] }], retries).unwrap();
        plan.scheduler.select_backend(0).unwrap();
        plan.scheduler.select_credential(u16::MAX, 0).unwrap();
        plan
    }

    fn retry_policy(percent: u8) -> (Arc<EffectiveBackendTrafficPolicy>, Arc<RetryBudget>) {
        let budget = Arc::new(RetryBudget::from_config(&RetryConstraintConfig {
            budget: BudgetDetails {
                percent,
                interval: "1h".to_string(),
            },
            min_retry_rate: None,
        }));
        let policy = Arc::new(EffectiveBackendTrafficPolicy {
            resolved_target_class:
                edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Ai,
            load_balancer: None,
            upstream_authority: None,
            upstream_authority_config: None,
            retry_budget: Some(Arc::clone(&budget)),
            circuit_breaker_limit: None,
            connect_timeout: None,
            outlier_detection: None,
            ai_credential_outlier: None,
            logical_circuit_breaker: None,
        });
        (policy, budget)
    }

    fn selectable_plan(credentials: Vec<RuntimeAiCredential>) -> ReadyAiAttemptPlan {
        let weights: Vec<u32> = credentials.iter().map(|credential| credential.weight).collect();
        let aliases: Vec<String> = credentials.iter().map(|credential| credential.name.clone()).collect();
        let outlier_store = crate::ai::AiCredentialOutlierStore::new();
        let target = edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKey {
            target_kind: edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKind::EdgionBackend,
            namespace: "default".to_string(),
            name: "provider".to_string(),
        };
        outlier_store.configure_backend(target.clone(), None, aliases.clone());
        let outlier_service = outlier_store.current_handle(&target).unwrap();
        let credential_outliers = aliases.iter().map(|alias| outlier_service.capture(alias)).collect();
        let backend = Arc::new(RuntimeAiBackend {
            uid: "backend-uid".to_string(),
            provider: AiProvider::OpenAICompatible,
            endpoint: RuntimeAiEndpoint {
                domain: "provider.example".to_string(),
                port: 443,
                use_tls: true,
                sni: "provider.example".to_string(),
                host: "provider.example".to_string(),
                path_prefix: String::new(),
            },
            streaming: true,
            max_output_tokens: Some(256),
            credential_pool: RuntimeAiCredentialPool {
                redis_ref: None,
                on_redis_failure: OnRedisFailure::Deny,
                credentials,
                limiter_core: Arc::new(crate::plugins::common::redis_rate_limiter::RedisRateLimiter::new()),
            },
            models: HashMap::new(),
            wildcard_models: Vec::new(),
            canonical_models: HashSet::new(),
            public_model_ids: BTreeSet::new(),
        });
        let mut plan = candidate_plan();
        plan.scheduler = AttemptScheduler::new(&[BackendCredentials { weights: &weights }], 0).unwrap();
        plan.candidates = vec![AiPlanCandidate {
            logical: compiled_ai_logical(None),
            slot: 0,
            order: 0,
            weight: 1,
            backend_key: "default/provider".to_string(),
            backend: Some(backend),
            model: Some(AiCandidateModel {
                provider_model: "provider-model".to_string(),
                pricing: None,
            }),
            effective_policy: None,
            backend_tls_policy: None,
            adaptive: None,
            credential_outliers,
        }];
        plan
    }

    fn compiled_ai_logical(ref_denied: Option<RefDenied>) -> crate::backends::LogicalBackendHandle {
        CompiledBackend::compile(
            HTTPBackendRef {
                name: "provider".to_string(),
                namespace: Some("default".to_string()),
                port: None,
                weight: Some(1),
                group: Some("edgion.io".to_string()),
                kind: Some("EdgionBackend".to_string()),
                filters: None,
                backend_tls_policy: None,
                ref_denied,
            },
            "default",
            0,
            Arc::new(PluginRuntime::new()),
        )
        .logical_handle()
    }

    fn denied_route_unit() -> Arc<HttpRouteRuleUnit> {
        let schema = HTTPBackendRef {
            name: "provider".to_string(),
            namespace: Some("tenant-b".to_string()),
            port: None,
            weight: Some(1),
            group: Some("edgion.io".to_string()),
            kind: Some("EdgionBackend".to_string()),
            filters: None,
            backend_tls_policy: None,
            ref_denied: Some(RefDenied {
                target_namespace: "tenant-b".to_string(),
                target_name: "provider".to_string(),
                reason: Some("NoMatchingReferenceGrant".to_string()),
            }),
        };
        let backend = CompiledBackend::compile(schema.clone(), "tenant-a", 0, Arc::new(PluginRuntime::new()));
        Arc::new(HttpRouteRuleUnit {
            resource_key: "tenant-a/route".to_string(),
            matched_info: MatchInfo {
                rsc: Rsc::new("HTTPRoute", "tenant-a", "route"),
                rule_id: 0,
                match_id: 0,
                sv: 0,
                m: HTTPRouteMatch {
                    path: None,
                    headers: None,
                    query_params: None,
                    method: None,
                },
                labels_meta: None,
                annotations_meta: None,
            },
            rule: Arc::new(HTTPRouteRule {
                name: None,
                resolved_terminal_route_uid: None,
                resolved_terminal_rule_identity: None,
                matches: None,
                filters: None,
                backend_refs: Some(vec![schema]),
                timeouts: None,
                retry: None,
                session_persistence: None,
                parsed_timeouts: None,
                parsed_retry: None,
                parsed_forward_raw_path: None,
                parsed_allow_non_idempotent_retry: None,
                parsed_mirror_tuning: None,
            }),
            route: None,
            path_regex: None,
            parent_refs: None,
            compiled_header_regexes: Vec::new(),
            compiled_query_regexes: Vec::new(),
            has_query_matchers: false,
            specificity: MatchSpecificity {
                has_method: false,
                header_count: 0,
                query_count: 0,
                creation_ts_ms: 0,
                route_key: "tenant-a/route".to_string(),
                rule_index: 0,
                match_index: 0,
            },
            metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
            plugin_runtime: Arc::new(PluginRuntime::new()),
            backends: Arc::new(vec![backend]),
            backend_finder: Arc::new(crate::lb::BackendSelector::new()),
        })
    }

    fn request_deadline_route_unit(timeout: Duration) -> Arc<HttpRouteRuleUnit> {
        let mut unit = (*denied_route_unit()).clone();
        let mut rule = (*unit.rule).clone();
        rule.parsed_timeouts = Some(ParsedRouteTimeouts {
            request_timeout: RouteTimeout::Duration(timeout),
            backend_request_timeout: RouteTimeout::Unset,
        });
        unit.rule = Arc::new(rule);
        Arc::new(unit)
    }

    async fn test_session() -> Session {
        let (mut client, server) = tokio::io::duplex(4096);
        let stream: pingora_core::protocols::Stream = Box::new(server);
        let mut session = Session::new_h1(stream);
        client
            .write_all(b"POST /v1/chat/completions HTTP/1.1\r\nHost: test\r\nContent-Length: 0\r\n\r\n")
            .await
            .expect("write request");
        drop(client);
        session
            .read_request()
            .await
            .expect("read_request should succeed")
            .then_some(())
            .expect("request must be present");
        session
    }

    #[tokio::test]
    async fn dispatch_rejects_denied_backend_before_request_mutation() {
        let credential = RuntimeAiCredential {
            name: "primary".to_string(),
            weight: 1,
            credential: "must-not-install".to_string(),
            limits: None,
        };
        let mut ctx = EdgionHttpContext::new();
        ctx.route_unit = Some(denied_route_unit());
        let mut plan = selectable_plan(vec![credential]);
        plan.candidates[0].logical = compiled_ai_logical(Some(RefDenied {
            target_namespace: "default".to_string(),
            target_name: "provider".to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        }));
        ctx.ai_attempt_plan = Some(AiAttemptPlan::Ready(plan));
        let mut session = test_session().await;

        assert!(matches!(
            prepare_first_attempt(&mut session, &mut ctx, 0, None, 0),
            Err(AiDispatchError::InvalidConfiguration)
        ));
        assert!(ctx.selected_backend.is_none());
        assert!(ctx.ai_request_baseline.is_none());
        assert!(ctx.request_body.read_all().await.unwrap().is_none());
        assert!(session.req_header().headers.get("authorization").is_none());
    }

    #[tokio::test]
    async fn dispatch_uses_candidate_handle_after_route_snapshot_changes() {
        let credential = RuntimeAiCredential {
            name: "primary".to_string(),
            weight: 1,
            credential: "secret".to_string(),
            limits: None,
        };
        let mut ctx = EdgionHttpContext::new();
        use pingora_core::protocols::http::RequestBodyBuffer;
        let (mut storage, view) = crate::request_body::BodyStorage::new(64 * 1024, 1024 * 1024);
        storage.finish().await.unwrap();
        ctx.request_body.install(view);
        // This route generation contains a denied slot. The captured plan owns
        // an earlier permitted logical handle and must not re-read this route.
        ctx.route_unit = Some(denied_route_unit());
        ctx.ai_attempt_plan = Some(AiAttemptPlan::Ready(selectable_plan(vec![credential])));
        let mut session = test_session().await;

        let prepared = prepare_first_attempt(&mut session, &mut ctx, 0, None, 0).unwrap();

        assert_eq!(prepared.selected.logical().identity().namespace, "default");
        assert_eq!(prepared.selected.logical().identity().name, "provider");
        assert!(matches!(
            ctx.selected_backend.as_ref().and_then(SelectedHttpBackend::routed),
            Some(SelectedLogicalBackend::Ai(_))
        ));
    }

    #[tokio::test]
    async fn prepare_first_attempt_rechecks_request_deadline_before_plan_mutation() {
        let credential = RuntimeAiCredential {
            name: "primary".to_string(),
            weight: 1,
            credential: "secret".to_string(),
            limits: None,
        };
        let mut ctx = EdgionHttpContext::new();
        ctx.route_unit = Some(request_deadline_route_unit(Duration::from_millis(1)));
        ctx.start_time = Instant::now() - Duration::from_secs(1);
        ctx.ai_attempt_plan = Some(AiAttemptPlan::Ready(selectable_plan(vec![credential])));
        let mut session = test_session().await;

        assert!(matches!(
            prepare_first_attempt(&mut session, &mut ctx, 0, None, 0),
            Err(AiDispatchError::RequestDeadlineExceeded)
        ));
        let Some(AiAttemptPlan::Ready(plan)) = ctx.ai_attempt_plan.as_ref() else {
            panic!("deadline gate must preserve the ready plan");
        };
        assert_eq!(plan.scheduler.dispatch_count(), 0);
        assert!(matches!(plan.scheduler.next_step(), NextStep::PickBackend(_)));
    }

    #[tokio::test]
    async fn prepare_first_attempt_rechecks_grpc_deadline_before_plan_mutation() {
        let credential = RuntimeAiCredential {
            name: "primary".to_string(),
            weight: 1,
            credential: "secret".to_string(),
            limits: None,
        };
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.grpc_client_deadline = Some(Instant::now() - Duration::from_secs(1));
        ctx.ai_attempt_plan = Some(AiAttemptPlan::Ready(selectable_plan(vec![credential])));
        let mut session = test_session().await;

        assert!(matches!(
            prepare_first_attempt(&mut session, &mut ctx, 0, None, 0),
            Err(AiDispatchError::GrpcDeadlineExceeded)
        ));
        let Some(AiAttemptPlan::Ready(plan)) = ctx.ai_attempt_plan.as_ref() else {
            panic!("deadline gate must preserve the ready plan");
        };
        assert_eq!(plan.scheduler.dispatch_count(), 0);
        assert!(matches!(plan.scheduler.next_step(), NextStep::PickBackend(_)));
    }

    #[test]
    fn openai_attempt_rewrites_only_provider_model() {
        let body = openai_provider_body(br#"{"model":"logical","messages":[]}"#, "provider-a").unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["model"], "provider-a");
        assert_eq!(value["messages"], serde_json::json!([]));
    }

    #[test]
    fn anthropic_attempt_translation_is_bounded_and_native() {
        let value = serde_json::json!({
            "model": "logical",
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let translated = prepare_anthropic_request(value, "claude-provider", Some(256), true, 64 * 1024).unwrap();
        let body: Value = serde_json::from_slice(&translated.body).unwrap();
        assert_eq!(body["model"], "claude-provider");
        assert_eq!(body["max_tokens"], 256);
    }

    #[test]
    fn request_max_tokens_prefers_max_completion_tokens() {
        assert_eq!(request_max_tokens(br#"{"max_completion_tokens":128}"#), Some(128));
        assert_eq!(request_max_tokens(br#"{"max_tokens":64}"#), Some(64));
        // The deprecated spelling never wins over the current one.
        assert_eq!(
            request_max_tokens(br#"{"max_tokens":64,"max_completion_tokens":128}"#),
            Some(128)
        );
        assert_eq!(request_max_tokens(br#"{"model":"logical"}"#), None);
        assert_eq!(request_max_tokens(b"not json"), None);
    }

    #[test]
    fn conflicting_request_output_bounds_are_client_errors() {
        assert_eq!(
            effective_max_output_tokens(br#"{"max_tokens":64,"max_completion_tokens":128}"#, None),
            Err(AiDispatchError::Client(AiProxyError::MalformedBody))
        );
        assert_eq!(
            effective_max_output_tokens(br#"{"max_tokens":64,"max_completion_tokens":64}"#, None),
            Ok(64)
        );
    }

    #[test]
    fn cost_weighted_accepts_max_completion_tokens_only_body_without_service_max() {
        let priced = |slot: u8, output_price: u64| RouteCandidate {
            slot,
            order: slot,
            weight: 1,
            eligibility: CandidateEligibility {
                valid_ref: true,
                model_compatible: true,
                runtime_available: true,
                untried: true,
            },
            adaptive: crate::ai::routing::AdaptiveSnapshot::default(),
            cost: Some(CostSnapshot {
                input_price_micro_usd_per_million: 1_000_000,
                output_price_micro_usd_per_million: output_price,
                // No `defaults.maxOutputTokens`: the request bound is the only input.
                service_max_output_tokens: None,
            }),
        };
        let candidates = [priced(0, 8_000_000), priced(1, 1_000_000)];
        let picker = |body: &[u8]| {
            pick(
                PickerInput {
                    candidates: &candidates,
                    entropy: 0,
                    selection_sequence: 0,
                    input_tokens: 16,
                    request_max_output_tokens: request_max_tokens(body),
                },
                AiBackendSelection::CostWeighted,
            )
        };
        // Before the shared parser this body produced MissingCostInput -> HTML 500.
        assert!(picker(br#"{"model":"logical","max_completion_tokens":1024}"#).is_ok());
        assert_eq!(
            picker(br#"{"model":"logical"}"#),
            Err(crate::ai::routing::PickerError::MissingCostInput)
        );
    }

    #[test]
    fn provider_uri_uses_canonical_path_and_query() {
        assert_eq!(
            provider_uri(
                AiProvider::OpenAICompatible,
                "/openai/v1",
                "/chat/completions",
                Some("trace=1")
            ),
            "/openai/v1/chat/completions?trace=1"
        );
        assert_eq!(
            provider_uri(AiProvider::Anthropic, "/native", "/chat/completions", Some("ignored=1")),
            "/native/v1/messages"
        );
    }

    #[test]
    fn dispatch_mark_is_idempotent() {
        let mut ctx = EdgionHttpContext::new();
        ctx.ai_attempt_plan = Some(AiAttemptPlan::Ready(candidate_plan()));
        mark_ai_dispatch_started(&mut ctx).unwrap();
        mark_ai_dispatch_started(&mut ctx).unwrap();
        let Some(AiAttemptPlan::Ready(plan)) = ctx.ai_attempt_plan.as_ref() else {
            panic!("ready plan must remain installed");
        };
        assert_eq!(plan.scheduler.dispatch_count(), 1);
    }

    #[test]
    fn dispatch_retry_budget_denies_and_counts_each_dispatch_once() {
        let mut denied = EdgionHttpContext::new();
        denied.ai_attempt_plan = Some(AiAttemptPlan::Ready(candidate_plan_with_retries(1)));
        denied.ai_effective_policy = Some(retry_policy(0).0);
        mark_ai_dispatch_started(&mut denied).unwrap();
        assert_eq!(
            handle_ai_transport_failure(&mut denied, true, true, false),
            Ok(AiRetryDecision::BudgetDenied)
        );
        let Some(AiAttemptPlan::Ready(plan)) = denied.ai_attempt_plan.as_ref() else {
            panic!("ready plan must remain installed");
        };
        assert_eq!(
            plan.scheduler.next_step(),
            NextStep::Stop(crate::ai::attempt::StopReason::RetryDenied)
        );

        let mut sampled_once = EdgionHttpContext::new();
        sampled_once.ai_attempt_plan = Some(AiAttemptPlan::Ready(candidate_plan()));
        let (policy, budget) = retry_policy(51);
        sampled_once.ai_effective_policy = Some(policy);
        mark_ai_dispatch_started(&mut sampled_once).unwrap();
        mark_ai_dispatch_started(&mut sampled_once).unwrap();
        assert!(budget.try_retry(), "one sampled dispatch permits the first retry");
        assert!(
            !budget.try_retry(),
            "an idempotent dispatch mark must not enlarge the denominator"
        );
    }

    #[test]
    fn rejected_plan_never_crosses_dispatch_boundary() {
        let mut ctx = EdgionHttpContext::new();
        ctx.ai_attempt_plan = Some(AiAttemptPlan::Rejected(crate::ai::AiPlanError::InvalidBackend));
        assert_eq!(mark_ai_dispatch_started(&mut ctx), Err(AiDispatchError::Rejected));
    }

    #[test]
    fn anthropic_request_errors_map_to_client_facing_proxy_errors() {
        assert_eq!(
            anthropic_request_error(AnthropicRequestError::UnsupportedParameter),
            AiDispatchError::Client(AiProxyError::UnsupportedParameter)
        );
        assert_eq!(
            anthropic_request_error(AnthropicRequestError::InvalidRequestBody),
            AiDispatchError::Client(AiProxyError::MalformedBody)
        );
        assert_eq!(
            anthropic_request_error(AnthropicRequestError::MissingRequiredParameter),
            AiDispatchError::Client(AiProxyError::MissingRequiredParameter)
        );
        assert_eq!(
            anthropic_request_error(AnthropicRequestError::OutputTooLarge),
            AiDispatchError::Client(AiProxyError::RequestTooLarge)
        );
        assert_eq!(
            anthropic_request_error(AnthropicRequestError::SerializationFailed),
            AiDispatchError::Client(AiProxyError::MalformedBody)
        );
    }

    #[test]
    fn limited_credentials_participate_in_weighted_selection() {
        let unlimited = RuntimeAiCredential {
            name: "unlimited".to_string(),
            weight: 1,
            credential: "secret-unlimited".to_string(),
            limits: None,
        };
        let limited = RuntimeAiCredential {
            name: "limited".to_string(),
            weight: 100,
            credential: "secret-limited".to_string(),
            limits: Some(AiCredentialLimits {
                requests_per_minute: Some(1),
                tokens_per_minute: None,
            }),
        };
        let mut plan = selectable_plan(vec![unlimited, limited]);
        let selected = select_target(&mut plan, 2).unwrap();
        assert_eq!(selected.credential, 1);
    }

    #[test]
    fn limited_only_pool_is_dispatch_candidate() {
        let limited = RuntimeAiCredential {
            name: "limited".to_string(),
            weight: 1,
            credential: "secret-limited".to_string(),
            limits: Some(AiCredentialLimits {
                requests_per_minute: Some(1),
                tokens_per_minute: None,
            }),
        };
        let mut plan = selectable_plan(vec![limited]);
        assert_eq!(select_target(&mut plan, 0).unwrap().credential, 0);
    }

    #[test]
    fn target_selection_reports_unavailable_when_every_candidate_is_exhausted() {
        let credential = || RuntimeAiCredential {
            name: "primary".to_string(),
            weight: 1,
            credential: "secret-primary".to_string(),
            limits: None,
        };
        let mut no_credentials = selectable_plan(vec![credential()]);
        no_credentials.candidates[0].credential_outliers = vec![None];
        assert!(matches!(
            select_target(&mut no_credentials, 0),
            Err(AiDispatchError::Unavailable)
        ));
        assert_eq!(
            no_credentials.scheduler.next_step(),
            NextStep::Stop(crate::ai::attempt::StopReason::Unavailable)
        );
        assert_eq!(no_credentials.scheduler.dispatch_count(), 0);

        let mut incompatible_model = selectable_plan(vec![credential()]);
        incompatible_model.candidates[0].model = None;
        assert!(matches!(
            select_target(&mut incompatible_model, 0),
            Err(AiDispatchError::Unavailable)
        ));
    }

    #[test]
    fn explicit_retry_cap_is_strict_but_global_fallback_is_bounded() {
        assert_eq!(resolve_ai_retry_cap(Some(7), 99), Ok(7));
        assert_eq!(
            resolve_ai_retry_cap(Some(8), 0),
            Err(AiDispatchError::InvalidConfiguration)
        );
        assert_eq!(resolve_ai_retry_cap(None, 99), Ok(7));
        assert_eq!(resolve_ai_retry_cap(None, 3), Ok(3));
    }

    #[test]
    fn retry_cap_initialization_is_once_only_and_preserves_progress() {
        let credential = RuntimeAiCredential {
            name: "primary".to_string(),
            weight: 1,
            credential: "secret-primary".to_string(),
            limits: None,
        };
        let mut plan = selectable_plan(vec![credential]);
        plan.retry_cap_initialized = false;
        initialize_retry_cap(&mut plan, 3).unwrap();
        assert_eq!(plan.scheduler.max_dispatches(), 4);
        plan.scheduler.select_backend(0).unwrap();
        plan.scheduler.select_credential(1, 0).unwrap();
        plan.scheduler.start_dispatch().unwrap();

        initialize_retry_cap(&mut plan, 0).unwrap();
        assert_eq!(plan.scheduler.max_dispatches(), 4);
        assert_eq!(plan.scheduler.dispatch_count(), 1);
    }

    #[test]
    fn endpoint_predispatch_failure_releases_and_skips_the_whole_service() {
        let first_weights = [1, 1];
        let second_weights = [1];
        let mut plan = candidate_plan();
        plan.scheduler = AttemptScheduler::new(
            &[
                BackendCredentials {
                    weights: &first_weights,
                },
                BackendCredentials {
                    weights: &second_weights,
                },
            ],
            0,
        )
        .unwrap();
        plan.scheduler.select_backend(0).unwrap();
        plan.scheduler.select_credential(u16::MAX, 0).unwrap();
        let mut ctx = EdgionHttpContext::new();
        ctx.ai_attempt_plan = Some(AiAttemptPlan::Ready(plan));

        assert!(AiPeerPredispatchFailure::Endpoint.finish(&mut ctx));
        let Some(AiAttemptPlan::Ready(ready)) = ctx.ai_attempt_plan.as_mut() else {
            panic!("ready plan must remain installed");
        };
        assert!(matches!(ready.scheduler.next_step(), NextStep::PickBackend(_)));
        assert_eq!(
            ready.scheduler.select_backend(0),
            Err(crate::ai::attempt::SchedulerError::BackendAlreadyExcluded)
        );
        assert!(ready.scheduler.select_backend(1).is_ok());
        assert_eq!(ready.scheduler.dispatch_count(), 0);
    }

    #[test]
    fn ambiguous_failure_replays_only_when_explicitly_requested() {
        let mut denied_ctx = EdgionHttpContext::new();
        denied_ctx.ai_attempt_plan = Some(AiAttemptPlan::Ready(candidate_plan_with_retries(1)));
        mark_ai_dispatch_started(&mut denied_ctx).unwrap();
        assert_eq!(
            handle_ai_transport_failure(&mut denied_ctx, false, true, true),
            Ok(AiRetryDecision::Final)
        );
        let Some(AiAttemptPlan::Ready(denied)) = denied_ctx.ai_attempt_plan.as_ref() else {
            panic!("ready plan must remain installed");
        };
        assert_eq!(
            denied.scheduler.next_step(),
            NextStep::Stop(crate::ai::attempt::StopReason::AmbiguousFailure)
        );
        assert_eq!(denied.scheduler.failures()[0].kind, DispatchFailureKind::Ambiguous);

        let mut allowed_ctx = EdgionHttpContext::new();
        allowed_ctx.ai_attempt_plan = Some(AiAttemptPlan::Ready(candidate_plan_with_retries(1)));
        mark_ai_dispatch_started(&mut allowed_ctx).unwrap();
        assert_eq!(
            handle_ai_transport_failure(&mut allowed_ctx, true, true, true),
            Ok(AiRetryDecision::Retry)
        );
        let Some(AiAttemptPlan::Ready(allowed)) = allowed_ctx.ai_attempt_plan.as_ref() else {
            panic!("ready plan must remain installed");
        };
        assert!(matches!(allowed.scheduler.next_step(), NextStep::PickCredential(_)));
        assert_eq!(allowed.scheduler.failures()[0].kind, DispatchFailureKind::Ambiguous);
    }

    #[tokio::test]
    async fn composite_ai_processor_stops_its_inner_chain_on_terminate() {
        // AiAttemptResponseProcessor wraps its own Vec of sub-processors and
        // is itself a chain: an inner terminate must stop it exactly like the
        // outer driver stops the top-level chain, or a sub-processor after
        // the terminating one would silently keep running.
        use crate::plugins::runtime::traits::session::MockPluginSession;
        use parking_lot::Mutex;

        struct Terminator;
        #[async_trait]
        impl StreamingResponseProcessor for Terminator {
            fn name(&self) -> &str {
                "terminator"
            }
            async fn on_chunk(
                &mut self,
                _chunk: &mut Option<Bytes>,
                _end_of_stream: bool,
                _stream_kind: crate::ctx::ResponseStreamKind,
                sink: &mut ResponseBodySink,
                _session: &mut dyn PluginSession,
            ) {
                sink.terminate();
            }
        }

        struct MarkRan {
            ran: Arc<Mutex<bool>>,
        }
        #[async_trait]
        impl StreamingResponseProcessor for MarkRan {
            fn name(&self) -> &str {
                "mark-ran"
            }
            async fn on_chunk(
                &mut self,
                _chunk: &mut Option<Bytes>,
                _end_of_stream: bool,
                _stream_kind: crate::ctx::ResponseStreamKind,
                _sink: &mut ResponseBodySink,
                _session: &mut dyn PluginSession,
            ) {
                *self.ran.lock() = true;
            }
        }

        let ran = Arc::new(Mutex::new(false));
        let mut composite = AiAttemptResponseProcessor {
            processors: vec![Box::new(Terminator), Box::new(MarkRan { ran: ran.clone() })],
            eos: Arc::new(AtomicBool::new(false)),
        };
        let mut chunk = Some(Bytes::from_static(b"x"));
        let mut sink = ResponseBodySink::new();
        let mut session = MockPluginSession::new();

        composite
            .on_chunk(
                &mut chunk,
                false,
                crate::ctx::ResponseStreamKind::None,
                &mut sink,
                &mut session,
            )
            .await;

        assert!(sink.is_terminated());
        assert!(!*ran.lock(), "the sub-processor after an inner terminate must not run");
    }
}
