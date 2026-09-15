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
    ctx.ai_state()
        .is_some_and(|state| state.response_head_hold_claim.is_some())
}

pub(crate) fn response_head_commit_plan(
    _session: &Session,
    _source: ResponseHeadSource,
    response: &ResponseHeader,
    ctx: &EdgionHttpContext,
) -> Result<ResponseHeadCommitPlan> {
    let Some(limits) = active_claim(ctx) else {
        return Ok(ResponseHeadCommitPlan::Immediate);
    };

    let final_eligible = ctx
        .ai_state()
        .and_then(|state| state.final_response_eligibility)
        .is_some_and(|eligibility| eligibility.allows_sse());
    let identity_encoding = response_is_identity_encoded(&response.headers);
    if !final_eligible || !identity_encoding || !(200..300).contains(&response.status.as_u16()) {
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

pub(crate) fn response_head_hold_boundary(boundary: ResponseHeadBoundary) -> ResponseHeadBoundaryAction {
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
    claim_response_head(chosen_header, ctx)
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
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    fn limits() -> pingora_proxy::ResponseHeadHoldLimits {
        pingora_proxy::ResponseHeadHoldLimits::new(1024, 1024, 8, 16, 4096, 32, Duration::from_secs(1))
    }

    #[test]
    fn will_commit_promotes_the_selected_candidate_to_a_write_claim() {
        let mut ctx = EdgionHttpContext::default();
        assert!(ctx.select_final_downstream_status(200));
        let replacement = ResponseHeader::build(403, None).unwrap();

        claim_response_head(&replacement, &mut ctx).unwrap();

        assert!(ctx.response_write_started());
        assert_eq!(ctx.final_downstream_status(), Some(403));
        let error = claim_response_head(&replacement, &mut ctx).unwrap_err();
        assert!(!error.retry());
    }

    async fn session() -> Session {
        let (mut client, server) = tokio::io::duplex(1024);
        client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
        let mut session = Session::new_h1(Box::new(server));
        session.read_request().await.unwrap();
        session
    }

    #[tokio::test]
    async fn will_commit_leaves_switching_protocols_to_transport_confirmation() {
        let mut ctx = EdgionHttpContext::default();
        let response = ResponseHeader::build(101, None).unwrap();
        let session = session().await;

        response_head_will_commit(&session, &response, &mut ctx).unwrap();

        assert!(!ctx.response_write_started());
        assert_eq!(ctx.final_downstream_status(), None);
        ctx.finalize_downstream_status(Some(101));
        assert_eq!(ctx.request_info.status(), 101);
    }

    #[tokio::test]
    async fn commit_plan_is_immediate_until_request_claim_is_activated() {
        let mut ctx = EdgionHttpContext::default();
        ctx.ensure_ai_state().response_head_hold_claim = Some(crate::ctx::ResponseHeadHoldClaim {
            limits: limits(),
            active: false,
        });
        let response = ResponseHeader::build(200, None).unwrap();
        let session = session().await;
        assert!(response_head_may_hold(&ctx));
        assert!(matches!(
            response_head_commit_plan(&session, ResponseHeadSource::Origin, &response, &ctx).unwrap(),
            ResponseHeadCommitPlan::Immediate
        ));
    }

    #[tokio::test]
    async fn active_final_sse_claim_selects_hold_and_ineligible_final_fails_closed() {
        let mut ctx = EdgionHttpContext::default();
        let state = ctx.ensure_ai_state();
        state.response_head_hold_claim = Some(crate::ctx::ResponseHeadHoldClaim {
            limits: limits(),
            active: true,
        });
        state.final_response_eligibility = Some(crate::ai::terminal::FinalResponseEligibility {
            method_allows_body: true,
            status_allows_body: true,
            stream_kind: crate::ctx::ResponseStreamKind::Sse,
        });
        let response = ResponseHeader::build(200, None).unwrap();
        let session = session().await;
        assert!(matches!(
            response_head_commit_plan(&session, ResponseHeadSource::Origin, &response, &ctx).unwrap(),
            ResponseHeadCommitPlan::Hold(_)
        ));

        ctx.ensure_ai_state().final_response_eligibility = None;
        let error = response_head_commit_plan(&session, ResponseHeadSource::Origin, &response, &ctx).unwrap_err();
        assert_eq!(error.etype(), &ErrorType::HTTPStatus(503));
        assert!(!error.retry());
    }

    #[test]
    fn boundary_failures_are_nonretryable() {
        for boundary in [
            ResponseHeadBoundary::Unsupported,
            ResponseHeadBoundary::InputLimit,
            ResponseHeadBoundary::Timeout,
        ] {
            let ResponseHeadBoundaryAction::Fail(error) = response_head_hold_boundary(boundary) else {
                panic!("Guardrail boundaries must fail closed")
            };
            assert!(!error.retry());
        }
    }
}
