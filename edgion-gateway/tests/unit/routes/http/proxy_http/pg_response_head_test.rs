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
