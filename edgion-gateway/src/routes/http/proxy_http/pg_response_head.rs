use super::content_type::response_is_identity_encoded;
use crate::EdgionHttpContext;
use pingora_core::{Error, ErrorType, Result};
use pingora_http::ResponseHeader;
use pingora_proxy::{
    ResponseHeadBoundary, ResponseHeadBoundaryAction, ResponseHeadCommitPlan, ResponseHeadOutcome, ResponseHeadSource,
    ResponseHeadUsage, Session,
};

fn active_claim(ctx: &EdgionHttpContext) -> Option<pingora_proxy::ResponseHeadHoldLimits> {
    ctx.ai_state()?
        .response_head_hold_claim
        .filter(|claim| claim.active)
        .map(|claim| claim.limits)
}

pub(crate) fn response_head_may_hold(ctx: &EdgionHttpContext) -> bool {
    ctx.grpc_transcoding.is_some()
        || ctx
            .ai_state()
            .is_some_and(|state| state.unary_possible || state.response_head_hold_claim.is_some())
}

pub(crate) fn response_head_commit_plan(
    _session: &Session,
    source: ResponseHeadSource,
    response: &ResponseHeader,
    ctx: &EdgionHttpContext,
) -> Result<ResponseHeadCommitPlan> {
    if let Some(plan) = &ctx.grpc_transcoding {
        return plan.head_plan(source, response);
    }
    if let Some(owner) = ctx.ai_state().and_then(|state| state.unary_owner.as_ref()) {
        return owner.plan(source, response, ctx);
    }
    let Some(limits) = active_claim(ctx) else {
        return Ok(ResponseHeadCommitPlan::Immediate);
    };

    let final_eligible = ctx
        .ai_state()
        .and_then(|state| state.final_response_eligibility)
        .is_some_and(|eligibility| eligibility.allows_sse());
    if !final_eligible
        || !(200..300).contains(&response.status.as_u16())
        || !response_is_identity_encoded(&response.headers)
    {
        if let Some((_, owner)) = ctx.ai_state().and_then(|state| state.attached_guardrail.as_ref()) {
            owner.publish_head(
                ResponseHeadOutcome::Failed(ResponseHeadBoundary::Unsupported),
                ResponseHeadUsage::default(),
                limits,
            );
        }
        let mut error = Error::explain(
            ErrorType::HTTPStatus(503),
            "Guardrail response became ineligible after the final response onion",
        );
        error.set_retry(false);
        return Err(error);
    }

    if let Some((_, owner)) = ctx.ai_state().and_then(|state| state.attached_guardrail.as_ref()) {
        owner.prepare_head_replacement(response, &ctx.request_info.trace_id);
    }

    Ok(ResponseHeadCommitPlan::hold(limits))
}

pub(crate) fn response_head_hold_boundary(
    boundary: ResponseHeadBoundary,
    ctx: &EdgionHttpContext,
) -> ResponseHeadBoundaryAction {
    if let Some(plan) = &ctx.grpc_transcoding {
        return plan.boundary(boundary);
    }
    if let Some(owner) = ctx
        .ai_state()
        .and_then(|state| state.unary_owner.as_ref())
        .filter(|owner| owner.raw_success())
    {
        return ResponseHeadBoundaryAction::Fail(if boundary == ResponseHeadBoundary::Timeout {
            owner.timeout_failure()
        } else {
            crate::ai::conversion::chat_to_anthropic::head::failure(false)
        });
    }
    let error_type = match boundary {
        ResponseHeadBoundary::InputLimit
        | ResponseHeadBoundary::OutputLimit
        | ResponseHeadBoundary::ChunkLimit
        | ResponseHeadBoundary::EventLimit
        | ResponseHeadBoundary::MetadataLimit
        | ResponseHeadBoundary::WorkLimit
        | ResponseHeadBoundary::Timeout => ErrorType::HTTPStatus(503),
        _ => ErrorType::InternalError,
    };
    let mut error = Error::explain(
        error_type,
        format!("Guardrail response-head Hold failed closed at {}", boundary.as_str()),
    );
    error.set_retry(false);
    ResponseHeadBoundaryAction::Fail(error)
}

pub(crate) fn response_head_hold_outcome(
    outcome: ResponseHeadOutcome,
    usage: ResponseHeadUsage,
    ctx: &mut EdgionHttpContext,
) {
    let Some(state) = ctx.ai_state() else {
        return;
    };
    let Some(claim) = state.response_head_hold_claim else {
        return;
    };
    if claim.owner != crate::ctx::ResponseHeadHoldOwner::Guardrail {
        return;
    }
    let Some((_, owner)) = state.attached_guardrail.as_ref() else {
        return;
    };
    owner.publish_head(outcome, usage, claim.limits);
}

/// Claim the sole downstream final-head writer after Pingora has resolved an
/// Immediate, Release, or Replace decision, but before downstream header
/// preparation or writer handoff. The response onion only selects a candidate;
/// keeping the claim here leaves a held head replaceable until its decision.
pub(crate) fn response_head_will_commit(
    _session: &Session,
    chosen_header: &ResponseHeader,
    ctx: &mut EdgionHttpContext,
) -> Result<()> {
    // A real protocol switch becomes terminal only after Pingora reports that
    // the 101 was written. It is not an ordinary final response and must not
    // enter the pre-write 200..=599 claim lifecycle.
    if chosen_header.status == http::StatusCode::SWITCHING_PROTOCOLS {
        return Ok(());
    }
    claim_response_head(chosen_header, ctx)?;
    crate::response_cache::lifecycle::seal_headers(ctx, chosen_header);
    Ok(())
}

fn claim_response_head(chosen_header: &ResponseHeader, ctx: &mut EdgionHttpContext) -> Result<()> {
    if ctx.claim_final_response(chosen_header.status.as_u16()) {
        return Ok(());
    }
    let mut error = Error::explain(
        ErrorType::InternalError,
        "Edgion final response writer was already claimed before will-commit",
    );
    error.set_retry(false);
    Err(error)
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_response_head_test.rs"]
mod tests;
