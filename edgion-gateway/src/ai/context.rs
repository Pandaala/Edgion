//! Lazily allocated state for one AI HTTP exchange.

use crate::ai::protocols::openai::terminal::AiClientStreamDialect;
use crate::ai::{AiAttemptPlan, AiRequestContext, GenAiSummary};
use crate::ctx::{EdgionHttpContext, ResponseHeadHoldClaim};
use crate::plugins::runtime::traits::StreamingResponseProcessor;
use crate::response::terminal::FinalResponseEligibility;
use std::sync::Arc;

/// AI-only state allocated on first effective AI plugin action.
///
/// Keep this boxed from [`EdgionHttpContext`]: several payloads have large
/// inline representations even while wrapped in `Option`, especially
/// `AiAttemptPlan`. The box keeps ordinary HTTP requests independent of that
/// storage cost while retaining one allocation and one ownership boundary for
/// the complete AI exchange.
#[derive(Default)]
pub(crate) struct AiHttpState {
    /// Bounded request metadata and shared atomic usage counters. Contains no
    /// prompt or credential.
    pub(crate) request: Option<AiRequestContext>,
    /// Immutable canonical request and candidate snapshot for forwarding. The
    /// canonical body is never rendered by Debug or access logging.
    pub(crate) attempt_plan: Option<AiAttemptPlan>,
    /// Request-body generation observed by `AiGuard` when its decision settled.
    pub(crate) guard_body_generation: Option<u64>,
    /// Guard entries that already decided; prevents retry re-entry.
    pub(crate) guard_decided: Vec<String>,
    /// Active-attempt and pending Redis settlement state.
    pub(crate) attempt_runtime: crate::ai::runtime::attempt_runtime::AiAttemptRuntimeState,
    /// Request-captured typed destination policy for the selected AI backend.
    pub(crate) effective_policy: Option<Arc<crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy>>,
    /// Pristine post-rule request used to rebuild later AI attempts.
    pub(crate) request_baseline: Option<pingora_http::RequestHeader>,
    /// Replaceable provider-aware response processor for the active attempt.
    pub(crate) stream_processor: Option<Box<dyn StreamingResponseProcessor>>,
    /// Client-facing stream dialect selected at canonical AI ingress.
    pub(crate) client_stream_dialect: Option<AiClientStreamDialect>,
    /// Final method/status/media eligibility captured after the response onion.
    pub(crate) final_response_eligibility: Option<FinalResponseEligibility>,
    /// Immutable GenAI usage summary committed once during final logging.
    pub(crate) summary: Option<GenAiSummary>,
    /// Whether a semantic response guard claimed the canonical AI source.
    pub(crate) semantic_guard_claimed: bool,
    /// The single request-scoped semantic Guardrail owner and its compiled
    /// request-entry identity. Multiple matched entries are rejected.
    pub(crate) attached_guardrail: Option<(String, std::sync::Arc<crate::ai::guards::semantic::AttachedGuardrail>)>,
    /// Installed before cache lookup and activated only after the final
    /// response onion installs Guardrail's semantic processor.
    pub(crate) response_head_hold_claim: Option<ResponseHeadHoldClaim>,
    /// Selected unary exchange owner; also retains the admitted reservation.
    pub(crate) unary_possible: bool,
    pub(crate) unary_owner: Option<Arc<crate::ai::conversion::chat_to_anthropic::head::UnaryHead>>,
    /// Same-callback handoff from the canonical source to the semantic guard.
    pub(crate) canonical_record_batch: Option<crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch>,
    /// Canonical semantic guard, run after the AI source processor.
    pub(crate) semantic_processor: Option<Box<dyn StreamingResponseProcessor>>,
}

impl EdgionHttpContext {
    pub(crate) fn try_set_ai_request_context(&mut self, context: AiRequestContext) -> bool {
        let state = self.ensure_ai_state();
        if state.request.is_some() {
            false
        } else {
            state.request = Some(context);
            state.client_stream_dialect = Some(AiClientStreamDialect::OpenAiCompatibleSse);
            true
        }
    }

    /// Replace attempt-local provider/usage state while retaining the one
    /// request-wide quota handle across AI failover attempts.
    pub(crate) fn replace_ai_attempt_context(&mut self, mut context: AiRequestContext) {
        let state = self.ensure_ai_state();
        state.client_stream_dialect = Some(AiClientStreamDialect::OpenAiCompatibleSse);
        if let Some(mut previous) = state.request.take() {
            if context.quota.is_none() {
                context.quota = previous.quota.take();
            }
            if context.quota_status.is_none() {
                context.quota_status = previous.quota_status;
            }
        }
        state.request = Some(context);
    }

    pub(crate) fn try_set_ai_attempt_plan(&mut self, plan: AiAttemptPlan) -> bool {
        let unary_possible = matches!(&plan, AiAttemptPlan::Ready(ready) if !ready.requested_streaming && ready.candidates.iter().any(|candidate| candidate.weight > 0 && candidate.model.is_some() && candidate.backend.as_ref().is_some_and(|backend| backend.provider == edgion_resources::resources::edgion_backend::AiProvider::Anthropic)));
        let state = self.ensure_ai_state();
        if state.attempt_plan.is_some() {
            false
        } else {
            state.unary_possible = unary_possible;
            state.attempt_plan = Some(plan);
            true
        }
    }

    #[inline]
    pub(crate) fn ai_state(&self) -> Option<&AiHttpState> {
        self.ai_state.as_deref()
    }

    #[inline]
    pub(crate) fn ai_state_mut(&mut self) -> Option<&mut AiHttpState> {
        self.ai_state.as_deref_mut()
    }

    #[inline]
    pub(crate) fn ensure_ai_state(&mut self) -> &mut AiHttpState {
        self.ai_state
            .get_or_insert_with(|| Box::new(AiHttpState::default()))
            .as_mut()
    }

    #[inline]
    pub(crate) fn has_ai_attempt_plan(&self) -> bool {
        self.ai_state().is_some_and(|state| state.attempt_plan.is_some())
    }

    #[inline]
    pub(crate) fn take_ai_attempt_plan(&mut self) -> Option<AiAttemptPlan> {
        self.ai_state_mut()?.attempt_plan.take()
    }

    #[inline]
    pub(crate) fn restore_ai_attempt_plan(&mut self, plan: AiAttemptPlan) {
        self.ensure_ai_state().attempt_plan = Some(plan);
    }
}

#[cfg(test)]
#[path = "../../tests/unit/ai/context_test.rs"]
mod tests;
