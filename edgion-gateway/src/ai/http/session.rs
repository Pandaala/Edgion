//! AI capability implementation for the production HTTP session adapter.

use super::PingoraSessionAdapter;
use crate::ai::{AiAttemptPlan, AiRequestContext};

impl crate::plugins::runtime::traits::AiSessionCapability for PingoraSessionAdapter<'_> {
    fn capture_ai_client_api(&mut self, api: edgion_resources::resources::edgion_plugins::AiApi) -> bool {
        self.ctx.capture_ai_client_api(api)
    }

    fn set_ai_request_context(&mut self, context: AiRequestContext) -> bool {
        self.ctx.try_set_ai_request_context(context)
    }

    fn set_ai_attempt_plan(&mut self, plan: AiAttemptPlan) -> bool {
        self.ctx.try_set_ai_attempt_plan(plan)
    }

    fn record_ai_guard_decision(&mut self, identity: String, generation: Option<u64>) {
        let state = self.ctx.ensure_ai_state();
        if !state.guard_decided.iter().any(|existing| existing == &identity) {
            state.guard_decided.push(identity);
        }
        if generation.is_some() {
            state.guard_body_generation = generation;
        }
    }

    fn semantic_guard_active(&self) -> bool {
        self.ctx.semantic_guard_active()
    }

    fn publish_canonical_records(
        &mut self,
        batch: crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch,
    ) -> Result<(), crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch> {
        self.ctx.publish_canonical_records(batch)
    }

    fn take_canonical_records(&mut self) -> Option<crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch> {
        self.ctx.take_canonical_records()
    }

    fn ai_terminal_render_context(
        &self,
    ) -> Option<(
        crate::ai::protocols::openai::terminal::AiClientStreamDialect,
        crate::response::terminal::FinalResponseEligibility,
    )> {
        self.ctx.ai_terminal_render_context()
    }

    fn disable_semantic_guard(&mut self) {
        if !self.ctx.response_body_processing_started {
            if let Some(state) = self.ctx.ai_state_mut() {
                state.semantic_guard_claimed = false;
                state.canonical_record_batch = None;
                state.semantic_processor = None;
                if state
                    .response_head_hold_claim
                    .is_some_and(|claim| claim.owner == crate::ctx::ResponseHeadHoldOwner::Guardrail)
                {
                    state.response_head_hold_claim = None;
                }
            }
        }
    }

    fn install_semantic_response_processor(
        &mut self,
        processor: Box<dyn crate::plugins::runtime::traits::StreamingResponseProcessor>,
    ) -> bool {
        if let Some(plan) = &self.ctx.grpc_transcoding {
            plan.reject_mutation();
            return false;
        }
        if self.ctx.response_processors_frozen()
            || self
                .ctx
                .ai_state()
                .is_some_and(|state| state.semantic_processor.is_some())
        {
            return false;
        }
        let state = self.ctx.ensure_ai_state();
        state.semantic_processor = Some(processor);
        if let Some(claim) = state.response_head_hold_claim.as_mut() {
            if claim.owner == crate::ctx::ResponseHeadHoldOwner::Guardrail {
                claim.active = true;
            }
        }
        true
    }

    fn install_attached_guardrail(
        &mut self,
        identity: String,
        owner: std::sync::Arc<crate::ai::guards::semantic::AttachedGuardrail>,
        head_limits: Option<pingora_proxy::ResponseHeadHoldLimits>,
    ) -> bool {
        if let Some(plan) = &self.ctx.grpc_transcoding {
            plan.reject_mutation();
            return false;
        }
        if self.ctx.ai_state().is_some_and(|state| {
            state.attached_guardrail.is_some()
                || state.semantic_guard_claimed
                || (head_limits.is_some() && state.response_head_hold_claim.is_some())
        }) {
            return false;
        }
        let state = self.ctx.ensure_ai_state();
        state.attached_guardrail = Some((identity, owner));
        state.semantic_guard_claimed = true;
        if let Some(limits) = head_limits {
            state.response_head_hold_claim = Some(crate::ctx::ResponseHeadHoldClaim {
                owner: crate::ctx::ResponseHeadHoldOwner::Guardrail,
                limits,
                active: false,
            });
        }
        self.ctx.reserve_response_framing();
        true
    }

    fn attached_guardrail(
        &self,
        identity: &str,
    ) -> Option<std::sync::Arc<crate::ai::guards::semantic::AttachedGuardrail>> {
        self.ctx
            .ai_state()?
            .attached_guardrail
            .as_ref()
            .filter(|(installed, _)| installed == identity)
            .map(|(_, owner)| std::sync::Arc::clone(owner))
    }
}
