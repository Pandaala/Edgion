//! AI operations at the plugin/session integration boundary.

use crate::ai::{AiAttemptPlan, AiRequestContext};

/// Short-lived access to the request's AI integration operations.
///
/// The request context remains the sole lazy state owner. Obtaining this
/// capability does not allocate state; all operations are explicit. Release
/// each synchronous borrow before ordinary session calls or body awaits.
#[cfg_attr(test, mockall::automock)]
#[cfg_attr(test, allow(clippy::disallowed_types))]
pub trait AiSessionCapability: Send + Sync {
    /// Capture the client API before local rejection; a conflicting API is refused.
    fn capture_ai_client_api(&mut self, api: edgion_resources::resources::edgion_plugins::AiApi) -> bool;

    /// First writer wins. Every offer is consumed, including a rejected context.
    fn set_ai_request_context(&mut self, context: AiRequestContext) -> bool;

    /// First writer wins. Every offer is consumed, including a rejected plan.
    fn set_ai_attempt_plan(&mut self, plan: AiAttemptPlan) -> bool;

    /// Deduplicate guard identity and update the generation only when supplied.
    fn record_ai_guard_decision(&mut self, identity: String, generation: Option<u64>);

    /// Read semantic ownership without creating AI state.
    fn semantic_guard_active(&self) -> bool;

    /// Publish one owned callback batch. Rejection returns the original offer and preserves any resident batch.
    fn publish_canonical_records(
        &mut self,
        batch: crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch,
    ) -> Result<(), crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch>;

    /// Transfer the resident callback batch exactly once without creating state.
    fn take_canonical_records(&mut self) -> Option<crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch>;

    /// Read client dialect and final eligibility without creating state.
    fn ai_terminal_render_context(
        &self,
    ) -> Option<(
        crate::ai::protocols::openai::terminal::AiClientStreamDialect,
        crate::response::terminal::FinalResponseEligibility,
    )>;

    /// Clear semantic withholding before body processing, preserving other head owners.
    fn disable_semantic_guard(&mut self);

    /// Consume the offered processor; reject duplicate, frozen or transcoder conflicts.
    fn install_semantic_response_processor(
        &mut self,
        processor: Box<dyn crate::plugins::runtime::traits::StreamingResponseProcessor>,
    ) -> bool;

    /// Claim one semantic owner and optional head hold; never replace an existing owner.
    fn install_attached_guardrail(
        &mut self,
        identity: String,
        owner: std::sync::Arc<crate::ai::guards::semantic::AttachedGuardrail>,
        head_limits: Option<pingora_proxy::ResponseHeadHoldLimits>,
    ) -> bool;

    /// Read the matching attached owner without creating state.
    fn attached_guardrail(
        &self,
        identity: &str,
    ) -> Option<std::sync::Arc<crate::ai::guards::semantic::AttachedGuardrail>>;
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/traits/ai_session_test.rs"]
mod tests;
