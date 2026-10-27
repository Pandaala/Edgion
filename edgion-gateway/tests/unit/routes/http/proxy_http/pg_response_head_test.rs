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
        owner: crate::ctx::ResponseHeadHoldOwner::Guardrail,
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
        owner: crate::ctx::ResponseHeadHoldOwner::Guardrail,
        limits: limits(),
        active: true,
    });
    state.final_response_eligibility = Some(crate::response::terminal::FinalResponseEligibility {
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
        let ResponseHeadBoundaryAction::Fail(error) =
            response_head_hold_boundary(boundary, &EdgionHttpContext::default())
        else {
            panic!("Guardrail boundaries must fail closed")
        };
        assert!(!error.retry());
    }
}

async fn cache_fill_across_real_transport_date(session: &mut Session) {
    use crate::response_cache::{lifecycle, tests::Harness};
    let harness = Harness::new(1024);
    let mut ctx = EdgionHttpContext::default();
    harness.arm(&mut ctx);
    let mut header = harness.response();
    header.insert_header("content-length", "5").unwrap();
    header
        .insert_header(
            "date",
            httpdate::fmt_http_date(std::time::SystemTime::now() - Duration::from_secs(5)),
        )
        .unwrap();
    let origin_date = header.headers.get("date").unwrap().clone();
    lifecycle::capture_headers(&mut ctx, &header);
    response_head_will_commit(session, &header, &mut ctx).unwrap();
    lifecycle::capture_body(&mut ctx, Some(&bytes::Bytes::from_static(b"hello")), true);
    session.write_response_header(Box::new(header), false).await.unwrap();
    assert_ne!(
        session.response_written().unwrap().headers.get("date").unwrap(),
        &origin_date,
        "real downstream transport rewrites origin Date without a sleep"
    );
    session
        .write_response_body(Some(bytes::Bytes::from_static(b"hello")), true)
        .await
        .unwrap();
    lifecycle::finalize(session, &mut ctx, None).await;
    assert_eq!(harness.entry_count(), 1);
    assert_eq!(harness.pending_bytes(), 0);
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    client
        .write_all(b"GET /cache HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    let mut replay = Session::new_h1(Box::new(server));
    replay.read_request().await.unwrap();
    let mut replay_ctx = EdgionHttpContext::default();
    harness.arm(&mut replay_ctx);
    let hit = lifecycle::prepare_hit(&mut replay, &mut replay_ctx).unwrap();
    assert_eq!(&hit.body[..], b"hello");
    lifecycle::deliver_hit(&mut replay, &mut replay_ctx, hit).await.unwrap();
    assert!(
        replay
            .response_written()
            .unwrap()
            .headers
            .get("age")
            .unwrap()
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 5,
        "actual replay Age survives transport Date replacement"
    );
}

#[tokio::test]
async fn cache_h1_commit_seal_accepts_transport_date_rewrite() {
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    client
        .write_all(b"GET /cache HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    session.read_request().await.unwrap();
    cache_fill_across_real_transport_date(&mut session).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_h2_commit_seal_accepts_transport_date_rewrite() {
    use pingora_core::protocols::http::v2::server::{handshake, H2Accept, HttpSession};
    use pingora_core::protocols::Digest;
    use std::sync::Arc;
    tokio::time::timeout(Duration::from_secs(5), async {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let client = tokio::spawn(async move {
            let (mut sender, connection) = h2::client::handshake(client_io).await.unwrap();
            let driver = tokio::spawn(connection);
            sender = sender.ready().await.unwrap();
            let request = http::Request::builder().uri("https://test/cache").body(()).unwrap();
            let (response, _) = sender.send_request(request, true).unwrap();
            let mut stream = response.await.unwrap().into_body();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.data().await {
                let chunk = chunk.unwrap();
                stream.flow_control().release_capacity(chunk.len()).unwrap();
                bytes.extend_from_slice(&chunk);
            }
            assert_eq!(bytes, b"hello");
            assert!(stream.trailers().await.unwrap().is_none());
            driver.abort();
        });
        let mut connection = handshake(Box::new(server_io), None).await.unwrap();
        let Some(H2Accept::Session(downstream)) =
            HttpSession::from_h2_conn(&mut connection, Arc::new(Digest::default()))
                .await
                .unwrap()
        else {
            panic!("real H2 request reaches commit boundary");
        };
        let (_unused, placeholder) = tokio::io::duplex(1);
        let mut session = Session::new_h1(Box::new(placeholder));
        *session.downstream_session = pingora_core::protocols::http::server::Session::new_http2(downstream);
        let driver = tokio::spawn(async move { while connection.accept().await.is_some() {} });
        cache_fill_across_real_transport_date(&mut session).await;
        client.await.unwrap();
        driver.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cache_commit_hook_rejects_onion_date_mutation_before_transport() {
    use crate::response_cache::{lifecycle, tests::Harness};
    let harness = Harness::new(1024);
    let session = session().await;
    let mut ctx = EdgionHttpContext::default();
    harness.arm(&mut ctx);
    let header = harness.response();
    lifecycle::capture_headers(&mut ctx, &header);
    let mut changed = header.clone();
    changed.insert_header("date", "Tue, 14 Nov 2023 22:13:20 GMT").unwrap();
    response_head_will_commit(&session, &changed, &mut ctx).unwrap();
    assert_eq!(harness.pending_bytes(), 0);
    assert_eq!(harness.entry_count(), 0);
    assert!(
        ctx.response_write_started(),
        "cache refusal does not refuse ordinary forwarding"
    );
}

#[tokio::test]
async fn guardrail_clamps_to_client_deadline_and_preserves_timeout_attribution() {
    use std::sync::Arc;
    use std::time::Instant;
    let mut ctx = EdgionHttpContext::default();
    let owner = Arc::new(crate::ai::guards::semantic::AttachedGuardrail::new());
    let state = ctx.ensure_ai_state();
    state.attached_guardrail = Some(("test".into(), owner.clone()));
    state.response_head_hold_claim = Some(crate::ctx::ResponseHeadHoldClaim {
        owner: crate::ctx::ResponseHeadHoldOwner::Guardrail,
        limits: limits(),
        active: true,
    });
    state.final_response_eligibility = Some(crate::response::terminal::FinalResponseEligibility {
        method_allows_body: true,
        status_allows_body: true,
        stream_kind: crate::ctx::ResponseStreamKind::Sse,
    });
    let deadline = Instant::now() + Duration::from_millis(200);
    ctx.request_info.set_grpc_client_deadline(deadline);
    let response = ResponseHeader::build(200, None).unwrap();
    let session = session().await;
    let ResponseHeadCommitPlan::Hold(_) =
        response_head_commit_plan(&session, ResponseHeadSource::Origin, &response, &ctx).unwrap()
    else {
        panic!("expected Hold")
    };
    assert_eq!(owner.head_deadline.get().unwrap().at, deadline);
    // Replanning must not rearm or extend the request-scoped deadline.
    response_head_commit_plan(&session, ResponseHeadSource::Origin, &response, &ctx).unwrap();
    assert_eq!(owner.head_deadline.get().unwrap().at, deadline);
    let ResponseHeadBoundaryAction::Fail(error) = response_head_hold_boundary(ResponseHeadBoundary::Timeout, &ctx)
    else {
        panic!("expected failure")
    };
    assert_eq!(error.etype(), &ErrorType::HTTPStatus(504));
    assert!(!error.retry());
    record_guardrail_timeout(&mut ctx, &error);
    assert_eq!(
        ctx.diagnostics.entries(),
        &[crate::observe::RequestDiagnostic::Error(
            edgion_resources::EdgionErrorCode::GrpcClientDeadlineExceeded
        )]
    );
}

#[tokio::test]
async fn expired_guardrail_activation_fails_before_starting_hold() {
    let mut ctx = EdgionHttpContext::default();
    let owner = std::sync::Arc::new(crate::ai::guards::semantic::AttachedGuardrail::new());
    let state = ctx.ensure_ai_state();
    state.attached_guardrail = Some(("test".into(), owner));
    state.response_head_hold_claim = Some(crate::ctx::ResponseHeadHoldClaim {
        owner: crate::ctx::ResponseHeadHoldOwner::Guardrail,
        limits: limits(),
        active: true,
    });
    state.final_response_eligibility = Some(crate::response::terminal::FinalResponseEligibility {
        method_allows_body: true,
        status_allows_body: true,
        stream_kind: crate::ctx::ResponseStreamKind::Sse,
    });
    ctx.request_info
        .set_grpc_client_deadline(std::time::Instant::now() - Duration::from_secs(1));
    let response = ResponseHeader::build(200, None).unwrap();
    let error = response_head_commit_plan(&session().await, ResponseHeadSource::Origin, &response, &ctx).unwrap_err();
    assert_eq!(error.etype(), &ErrorType::HTTPStatus(504));
    assert!(!error.retry());
    record_guardrail_timeout(&mut ctx, &error);
    assert_eq!(
        ctx.diagnostics.entries(),
        &[crate::observe::RequestDiagnostic::Error(
            edgion_resources::EdgionErrorCode::GrpcClientDeadlineExceeded
        )]
    );
}
