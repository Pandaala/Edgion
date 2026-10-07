//! First AI upstream-attempt preparation.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ai::protocols::openai::canonical_sse::SemanticSseSource;
use crate::ai::protocols::openai::request;
use crate::ai::runtime::attempt_plan::AiOperation;
use async_trait::async_trait;
use bytes::Bytes;
use edgion_resources::resources::edgion_backend::AiProvider;
use edgion_resources::resources::edgion_plugins::{AiApi, AiConversion, AiUsageObservation};
use edgion_resources::resources::http_route::HTTPRouteFilterType;
use pingora_proxy::ResponseBodySink;

use crate::ai::conversion::chat_to_anthropic::{
    prepare_anthropic_request, AnthropicRequestError, AnthropicResponseProcessor,
};
use crate::ai::routing::candidate::{pick, CandidateEligibility, CostSnapshot, PickerInput, RouteCandidate};
use crate::ai::runtime::attempt::{
    AttemptScheduler, BackendCredentials, DispatchFailureKind, DispatchOutcome, NextStep, SchedulerEvent,
    MAX_AI_ROUTE_RETRIES,
};
use crate::ai::SelectedAiBackend;
use crate::ai::{
    AiAttemptPlan, AiProviderLabel, AiProxyError, AiRequestContext, AiUsageShared, ReadyAiAttemptPlan,
    UsageExtractionProcessor,
};
use crate::backends::SelectedHttpBackend;
#[cfg(test)]
use crate::backends::SelectedLogicalBackend;
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
    unary_owner: Option<Arc<crate::ai::conversion::chat_to_anthropic::head::UnaryHead>>,
}

pub fn commit_ai_attempt_context(
    ctx: &mut EdgionHttpContext,
    target: &mut PreparedAiAttempt,
) -> Result<(), AiDispatchError> {
    let quota = match ctx.ensure_ai_state().attempt_plan.as_mut() {
        Some(AiAttemptPlan::Ready(plan)) => plan.quota.take(),
        _ => None,
    };
    let mut context = target.ai_context.take().ok_or(AiDispatchError::InvalidConfiguration)?;
    context.quota = quota;
    ctx.replace_ai_attempt_context(context);
    ctx.ensure_ai_state().stream_processor = target.response_processor.take();
    ctx.ensure_ai_state().unary_owner = target.unary_owner.take();
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
        if let Some(state) = ctx.ai_state_mut() {
            state.attempt_runtime.finish_predispatch();
        }
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
    let Some(mut plan) = ctx.take_ai_attempt_plan() else {
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
    ctx.restore_ai_attempt_plan(plan);
    result
}

/// Cross the dispatch boundary only after Pingora has connected and is about
/// to write the upstream request. Pre-connect failures consume no scheduler
/// attempt, quota reservation, or retry-budget denominator sample.
pub fn mark_ai_dispatch_started(ctx: &mut EdgionHttpContext) -> Result<(), AiDispatchError> {
    let Some(mut plan) = ctx.take_ai_attempt_plan() else {
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
        let state = ctx.ensure_ai_state();
        state.attempt_runtime.mark_dispatched();
        if let Some(budget) = ctx
            .ai_state()
            .and_then(|state| state.effective_policy.as_ref())
            .and_then(|policy| policy.retry_budget.as_ref())
        {
            budget.on_attempt();
        }
    }
    ctx.restore_ai_attempt_plan(plan);
    result
}

/// Handle a failure before the first upstream byte can be written. This skips
/// the candidate without consuming scheduler capacity or retry budget.
pub fn handle_ai_predispatch_failure(ctx: &mut EdgionHttpContext, gates_allow_retry: bool) -> bool {
    let Some(mut plan) = ctx.take_ai_attempt_plan() else {
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
    ctx.restore_ai_attempt_plan(plan);
    retry
}

pub fn handle_ai_predispatch_service_failure(ctx: &mut EdgionHttpContext) -> bool {
    let Some(mut plan) = ctx.take_ai_attempt_plan() else {
        return false;
    };
    let retry = match &mut plan {
        AiAttemptPlan::Ready(ready) if matches!(ready.scheduler.next_step(), NextStep::Dispatch(_)) => {
            ready.scheduler.skip_predispatch_backend().is_ok()
        }
        _ => false,
    };
    ctx.restore_ai_attempt_plan(plan);
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
    let Some(mut plan) = ctx.take_ai_attempt_plan() else {
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
                .ai_state()
                .and_then(|state| state.effective_policy.as_ref())
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
    ctx.restore_ai_attempt_plan(plan);
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
    if !matches!(
        (plan.api, plan.operation),
        (AiApi::OpenaiChat, AiOperation::ChatCompletions)
            | (AiApi::OpenaiResponses, AiOperation::Responses)
            | (AiApi::AnthropicMessages, AiOperation::Messages)
    ) || plan.body_generation != ctx.request_body.generation()
    {
        ctx.record_error(edgion_resources::EdgionErrorCode::BodyMutationConflict);
        return Err(AiDispatchError::Client(AiProxyError::BackendUnavailable));
    }
    if let Some(baseline) = ctx.ai_state().and_then(|state| state.request_baseline.clone()) {
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
    // Retire the prior prepared attempt before acquiring another reservation.
    if let Some(state) = ctx.ai_state_mut() {
        if let Some(mut processor) = state.stream_processor.take() {
            processor.release_inflight();
        }
        state.unary_owner = None;
    }
    let unary_owner = if plan.conversion == Some(AiConversion::ChatToAnthropic) && !plan.requested_streaming {
        if ctx
            .ai_state()
            .and_then(|state| state.response_head_hold_claim)
            .is_some_and(|claim| claim.active && claim.owner != crate::ctx::ResponseHeadHoldOwner::AnthropicUnary)
        {
            return Err(AiDispatchError::Client(AiProxyError::BackendUnavailable));
        }
        if model.provider_model.is_empty() || model.provider_model.len() > 512 {
            return Err(AiDispatchError::Client(AiProxyError::BackendUnavailable));
        }
        let runtime = crate::ai::config::runtime().ok_or(AiDispatchError::Client(AiProxyError::BackendUnavailable))?;
        let reservation = runtime
            .budget
            .acquire()
            .ok_or(AiDispatchError::Client(AiProxyError::BackendUnavailable))?;
        if let Some(error) = ai_predispatch_deadline(ctx) {
            return Err(error);
        }
        Some(Arc::new(
            crate::ai::conversion::chat_to_anthropic::head::UnaryHead::new(runtime.limits, reservation),
        ))
    } else {
        None
    };
    let (body, requested_streaming, anthropic_usage_chunk) = match plan.conversion {
        None if plan.api == AiApi::AnthropicMessages && ai_backend.provider == AiProvider::Anthropic => {
            let body = if model.provider_model == plan.logical_model {
                plan.request_body.clone()
            } else {
                crate::ai::protocols::anthropic::request::rewrite_model(&plan.request_body, &model.provider_model)
                    .map_err(AiDispatchError::Client)?
                    .unwrap_or_else(|| plan.request_body.clone())
            };
            (body, plan.requested_streaming, false)
        }
        None if matches!(ai_backend.provider, AiProvider::OpenAI | AiProvider::OpenAICompatible)
            && (plan.api != AiApi::OpenaiResponses || ai_backend.provider == AiProvider::OpenAI) =>
        {
            let body = if model.provider_model == plan.logical_model {
                plan.request_body.clone()
            } else {
                request::rewrite_model(&plan.request_body, &model.provider_model)
                    .map_err(AiDispatchError::Client)?
                    .unwrap_or_else(|| plan.request_body.clone())
            };
            (body, plan.requested_streaming, false)
        }
        Some(AiConversion::ChatToAnthropic) if ai_backend.provider == AiProvider::Anthropic => {
            let value =
                serde_json::from_slice(&plan.request_body).map_err(|_| AiDispatchError::InvalidConfiguration)?;
            let max_output_bytes = plan.request_body.len().saturating_mul(2).saturating_add(64 * 1024);
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
        _ => return Err(AiDispatchError::InvalidConfiguration),
    };
    if !plan.body_is_current || body.as_ptr() != plan.request_body.as_ptr() || body.len() != plan.request_body.len() {
        let is_plan = body.as_ptr() == plan.request_body.as_ptr() && body.len() == plan.request_body.len();
        ctx.overwrite_request_body(body)
            .map_err(|_| AiDispatchError::InvalidConfiguration)?;
        plan.body_generation = ctx.request_body.generation();
        plan.body_is_current = is_plan;
    }

    if ctx.ai_state().is_none_or(|state| state.request_baseline.is_none()) {
        ctx.ensure_ai_state().request_baseline = Some(session.req_header().clone());
    }
    apply_backend_request_headers(session, ctx, selected_route_backend)?;
    let observation_supported = {
        let mut adapter = PingoraSessionAdapter::new(session, ctx);
        crate::ai::connectors::provider::prepare_provider_request(
            &mut adapter,
            ai_backend.provider,
            plan.api,
            &ai_backend.endpoint,
            &credential.credential,
            &plan.canonical_path,
            plan.canonical_query.as_deref(),
            plan.usage_observation == AiUsageObservation::Provider || plan.conversion.is_some(),
        )?
    };
    if !observation_supported
        && (plan.quota.is_some()
            || ctx
                .ai_state()
                .and_then(|s| s.request.as_ref())
                .is_some_and(|r| r.quota.is_some())
            || plan.selection == edgion_resources::resources::edgion_plugins::AiBackendSelection::LowestUsage
            || plan
                .candidates
                .iter()
                .any(|c| c.weight > 0 && c.backend.as_ref().is_some_and(|b| b.has_credential_tpm)))
    {
        return Err(AiDispatchError::Client(AiProxyError::UnsupportedParameter));
    }

    let usage = Arc::new(if plan.api != AiApi::OpenaiChat {
        AiUsageShared::with_reported_only(plan.usage_observation)
    } else {
        AiUsageShared::with_observation(plan.usage_observation)
    });
    if plan.usage_observation == AiUsageObservation::Provider
        && (plan.observation_unavailable || !observation_supported)
    {
        usage.mark_observation_unavailable();
    }
    usage.set_input_chars(plan.input_chars);
    let eos = crate::ai::runtime::attempt_runtime::new_eos_latch();
    let quota_accumulator = plan.quota.as_ref().map(|quota| quota.accumulator()).or_else(|| {
        ctx.ai_state()
            .and_then(|state| state.request.as_ref())
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
    let needs_processor = plan.conversion.is_some()
        || plan.usage_observation == AiUsageObservation::Provider
        || ctx.ai_state().is_some_and(|state| state.semantic_guard_claimed);
    let mut response_processors: Vec<Box<dyn StreamingResponseProcessor>> = if needs_processor {
        Vec::with_capacity(1)
    } else {
        Vec::new()
    };
    if plan.conversion == Some(AiConversion::ChatToAnthropic) {
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        response_processors.push(Box::new(AnthropicResponseProcessor::new_with_quota_and_unary(
            model.provider_model.clone(),
            requested_streaming,
            anthropic_usage_chunk,
            created,
            Arc::clone(&usage),
            quota_accumulator.clone(),
            plan.terminate_on_over_limit,
            unary_owner.clone(),
        )));
    }
    if plan.api == AiApi::AnthropicMessages
        && plan.usage_observation == AiUsageObservation::Provider
        && observation_supported
    {
        response_processors.push(Box::new(
            crate::ai::protocols::anthropic::processor::MessagesProcessor::new(
                Arc::clone(&usage),
                quota_accumulator.clone(),
                requested_streaming,
                plan.terminate_on_over_limit,
                model.provider_model.clone(),
            ),
        ));
    }
    if plan.api != AiApi::AnthropicMessages
        && plan.conversion.is_none()
        && plan.usage_observation == AiUsageObservation::Provider
    {
        let processor = if plan.api == AiApi::OpenaiResponses {
            UsageExtractionProcessor::new_responses(
                Arc::clone(&usage),
                quota_accumulator,
                requested_streaming,
                plan.terminate_on_over_limit,
            )
        } else {
            UsageExtractionProcessor::new_with_quota(
                Arc::clone(&usage),
                quota_accumulator,
                requested_streaming,
                plan.terminate_on_over_limit,
            )
        };
        response_processors.push(Box::new(processor));
    }
    if plan.conversion.is_none()
        && plan.usage_observation == AiUsageObservation::Off
        && ctx.ai_state().is_some_and(|state| state.semantic_guard_claimed)
    {
        response_processors.push(Box::new(SemanticSseSource::new()));
    }
    let response_processor: Box<dyn StreamingResponseProcessor> = Box::new(AiAttemptResponseProcessor {
        processors: response_processors,
        usage: Arc::clone(&usage),
        eos: Arc::clone(&eos),
    });

    ctx.selected_backend = Some(SelectedHttpBackend::Ai(selected_backend.logical().clone()));
    let target = PreparedAiAttempt {
        selected: selected_backend,
        provider,
        pricing: model.pricing,
        redis_ref: ai_backend.credential_pool.redis_ref.clone(),
        on_redis_failure: ai_backend.credential_pool.on_redis_failure.clone(),
        credential_limits: credential.limits.clone(),
        estimated_input_tokens: plan.input_chars.saturating_add(3) / 4,
        max_output_tokens: plan
            .request_max_output_tokens
            .or(ai_backend.max_output_tokens.map(u64::from))
            .unwrap_or(0),
        usage,
        eos,
        limiter_core: Arc::clone(&ai_backend.credential_pool.limiter_core),
        ai_context: Some(context),
        response_processor: Some(response_processor),
        unary_owner,
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
    plan.scheduler = AttemptScheduler::new(&backends, if plan.continuation { 0 } else { retry_cap })
        .map_err(|_| AiDispatchError::InvalidConfiguration)?;
    plan.retry_cap_initialized = true;
    Ok(())
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
                request_max_output_tokens: plan.request_max_output_tokens,
            },
            plan.selection,
        ) {
            Ok(selected) => selected,
            Err(crate::ai::routing::candidate::PickerError::NoEligibleCandidate) => {
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
    // AI dispatch borrows the native modifier's plan without executing the
    // backend plugin chain. Honor that chain's publication-time validation.
    if backend.plugin_runtime.rejects_invalid_config() {
        return Err(AiDispatchError::InvalidConfiguration);
    }
    let mut adapter = PingoraSessionAdapter::new(session, ctx);
    for filter in backend.schema.filters.as_deref().unwrap_or_default() {
        match filter.filter_type {
            HTTPRouteFilterType::RequestHeaderModifier => {
                let plan = backend
                    .plugin_runtime
                    .backend_request_header_modifier()
                    .ok_or(AiDispatchError::InvalidConfiguration)?
                    .plan();
                for header in plan.set() {
                    adapter
                        .set_request_header_value(header.name(), header.value().clone())
                        .map_err(|_| AiDispatchError::InvalidConfiguration)?;
                }
                for header in plan.add() {
                    adapter
                        .append_request_header_value(header.name(), header.value().clone())
                        .map_err(|_| AiDispatchError::InvalidConfiguration)?;
                }
                for name in plan.remove() {
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

struct AiAttemptResponseProcessor {
    processors: Vec<Box<dyn StreamingResponseProcessor>>,
    usage: Arc<AiUsageShared>,
    eos: Arc<AtomicBool>,
}

#[async_trait]
impl StreamingResponseProcessor for AiAttemptResponseProcessor {
    fn on_response_header(&mut self, response: &mut ResponseHeader) {
        // Converters may replace every header. Preserve raw eligibility before
        // any child sees it so translated error text cannot become usage evidence.
        if !self.usage.observation_is_off()
            && !crate::routes::http::proxy_http::content_type::response_is_identity_encoded(&response.headers)
        {
            self.usage.mark_encoding_unavailable();
        }
        for processor in &mut self.processors {
            processor.on_response_header(response);
        }
    }

    fn changes_body_length(&self) -> bool {
        self.processors.iter().any(|processor| processor.changes_body_length())
    }

    fn finalize_incomplete(&mut self, outcome: &'static str, reason: &'static str) {
        for processor in &mut self.processors {
            processor.finalize_incomplete(outcome, reason);
        }
    }
    fn release_inflight(&mut self) {
        for processor in &mut self.processors {
            processor.release_inflight();
        }
    }
    fn on_terminal_preempted(&mut self, winner: crate::response::terminal::ResponseTerminalCause) {
        for processor in &mut self.processors {
            processor.on_terminal_preempted(winner);
        }
    }
    async fn on_event(
        &mut self,
        chunk: &mut Option<Bytes>,
        event: crate::plugins::runtime::traits::streaming_response_processor::StreamingResponseEvent,
        stream_kind: crate::ctx::ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    ) {
        for processor in &mut self.processors {
            processor.on_event(chunk, event, stream_kind, sink, session).await;
            if sink.is_terminated() {
                break;
            }
        }
        // Typed terminal evidence is emitted only after the source trailer read succeeds.
        // Release and document validation are independent of actual source completion.
        if event.end_of_stream_for_legacy() {
            crate::ai::runtime::attempt_runtime::mark_eos(&self.eos);
        }
    }
    async fn on_chunk(
        &mut self,
        chunk: &mut Option<Bytes>,
        end_of_stream: bool,
        stream_kind: crate::ctx::ResponseStreamKind,
        sink: &mut ResponseBodySink,
        session: &mut dyn PluginSession,
    ) {
        self.on_event(
            chunk,
            crate::plugins::runtime::traits::streaming_response_processor::StreamingResponseEvent::Data {
                end_of_stream,
            },
            stream_kind,
            sink,
            session,
        )
        .await;
    }
}

fn provider_label(provider: AiProvider) -> AiProviderLabel {
    match provider {
        AiProvider::OpenAI => AiProviderLabel::OpenAi,
        AiProvider::OpenAICompatible => AiProviderLabel::OpenAiCompatible,
        AiProvider::Anthropic => AiProviderLabel::Anthropic,
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/ai/runtime/dispatch_test.rs"]
mod tests;
