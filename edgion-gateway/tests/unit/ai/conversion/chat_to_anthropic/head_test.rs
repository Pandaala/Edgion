use super::*;
#[test]
fn compact_projection_drops_preallocated_capacity_and_counts_repeated_fields() {
    let mut original = HeaderMap::with_capacity(1024);
    original.insert("x-small", HeaderValue::from_static("one"));
    let fresh = compact_map(&original, 8192, 64).unwrap();
    assert_eq!(fresh["x-small"], "one");
    assert!(fresh.capacity() < original.capacity());
    for _ in 0..64 {
        original.append("x-small", HeaderValue::from_static("one"));
    }
    assert!(compact_map(&original, 65536, 64).is_err());
    let mut original = HeaderMap::new();
    original.insert("x-large", HeaderValue::from_bytes(&vec![b'a'; 8192]).unwrap());
    assert!(compact_map(&original, 8192, 64).is_err());
}

fn admitted_context() -> (EdgionHttpContext, Arc<UnaryHead>) {
    let limits = UnaryLimits::resolve(&crate::ai::config::UnaryConfig::default()).unwrap();
    let pool = super::super::budget::UnaryBudget::new(1);
    let owner = Arc::new(UnaryHead::new(limits, pool.acquire().unwrap()));
    owner.observe_header(true, true);
    let mut ctx = EdgionHttpContext::default();
    ctx.ensure_ai_state().unary_owner = Some(owner.clone());
    ctx.ensure_ai_state().final_response_eligibility = Some(crate::response::terminal::FinalResponseEligibility {
        method_allows_body: true,
        status_allows_body: true,
        stream_kind: crate::ctx::ResponseStreamKind::None,
    });
    (ctx, owner)
}

#[test]
fn unary_revalidates_final_metadata_and_only_conflicts_with_active_guard() {
    let (mut ctx, owner) = admitted_context();
    let mut head = ResponseHeader::build(200, None).unwrap();
    head.insert_header("content-type", "application/json").unwrap();
    compact_final_header(&mut head, &ctx).unwrap();
    ctx.ensure_ai_state().response_head_hold_claim = Some(crate::ctx::ResponseHeadHoldClaim {
        owner: crate::ctx::ResponseHeadHoldOwner::Guardrail,
        limits: pingora_proxy::ResponseHeadHoldLimits::new(1, 1, 1, 1, 1, 1, std::time::Duration::from_secs(1)),
        active: false,
    });
    assert!(matches!(
        owner.plan(ResponseHeadSource::Origin, &head, &ctx).unwrap(),
        ResponseHeadCommitPlan::Hold(_)
    ));
    ctx.ensure_ai_state().response_head_hold_claim.as_mut().unwrap().active = true;
    assert_eq!(
        owner.plan(ResponseHeadSource::Origin, &head, &ctx).unwrap_err().etype(),
        &ErrorType::HTTPStatus(502)
    );
    ctx.ensure_ai_state().response_head_hold_claim = None;
    head.insert_header("content-encoding", "gzip").unwrap();
    assert!(owner.plan(ResponseHeadSource::Origin, &head, &ctx).is_err());
    owner.observe_header(false, true);
    assert!(matches!(
        owner.plan(ResponseHeadSource::Origin, &head, &ctx).unwrap(),
        ResponseHeadCommitPlan::Immediate
    ));
}

#[test]
fn retained_trailer_and_error_template_projections_drop_original_capacity() {
    let (ctx, owner) = admitted_context();
    let mut trailers = HeaderMap::with_capacity(4096);
    trailers.insert("x-result", HeaderValue::from_static("complete"));
    let original = trailers.capacity();
    compact_trailers(&mut trailers, &ctx).unwrap();
    assert!(trailers.capacity() < original);
    assert_eq!(trailers["x-result"], "complete");
    let mut head = ResponseHeader::build(200, Some(4096)).unwrap();
    head.insert_header("content-type", "application/json").unwrap();
    head.append_header("server", "fixture").unwrap();
    compact_final_header(&mut head, &ctx).unwrap();
    owner.plan(ResponseHeadSource::Origin, &head, &ctx).unwrap();
    let (error, body) = owner.error_reply(502, &ctx).unwrap();
    assert_eq!(error.headers["server"], "fixture");
    assert_eq!(error.headers["content-length"], body.len().to_string());
    assert!(error.headers.capacity() < original);
    eprintln!("unary compact metadata: trailer_fields={}, trailer_capacity={}, error_fields={}, error_capacity={}; original_capacity={original}",trailers.len(),trailers.capacity(),error.headers.len(),error.headers.capacity());
}

#[test]
fn unary_deadline_sources_are_absolute_and_ties_prefer_request_then_grpc() {
    use std::time::Duration;
    let now = Instant::now();
    let hold = Duration::from_secs(10);
    let soon = now + Duration::from_secs(1);
    assert_eq!(
        effective_deadline(now, hold, Some(soon), None).unwrap().source,
        DeadlineSource::Request
    );
    assert_eq!(
        effective_deadline(now, hold, None, Some(soon)).unwrap().source,
        DeadlineSource::Grpc
    );
    assert_eq!(
        effective_deadline(now, hold, Some(soon), Some(soon)).unwrap().source,
        DeadlineSource::Request
    );
    let ceiling = now + hold;
    assert_eq!(
        effective_deadline(now, hold, None, Some(ceiling)).unwrap().source,
        DeadlineSource::Grpc
    );
    assert_eq!(
        effective_deadline(now, hold, Some(ceiling), Some(ceiling))
            .unwrap()
            .source,
        DeadlineSource::Request
    );
    assert_eq!(
        effective_deadline(now, hold, None, None).unwrap().source,
        DeadlineSource::Hold
    );
}

#[test]
fn unary_timeout_diagnostic_requires_generated_expiry_and_preserves_actual_source() {
    use crate::observe::RequestDiagnostic;
    use edgion_resources::EdgionErrorCode;
    for (source, expected) in [
        (DeadlineSource::Request, Some(EdgionErrorCode::RequestTimeoutExceeded)),
        (DeadlineSource::Grpc, Some(EdgionErrorCode::GrpcClientDeadlineExceeded)),
        (DeadlineSource::Hold, None),
    ] {
        let (mut ctx, owner) = admitted_context();
        owner
            .deadline
            .set(EffectiveDeadline {
                at: Instant::now(),
                source,
            })
            .ok()
            .unwrap();
        record_timeout_diagnostic(&mut ctx, &failure(true));
        assert!(ctx.diagnostics.entries().is_empty());
        let error = owner.timeout_failure();
        assert_eq!(error.etype(), &ErrorType::HTTPStatus(504));
        let actual = Error::new_up(ErrorType::ReadTimedout);
        ctx.peer_timeout_source = Some(crate::PeerTimeoutSource::BackendRequest);
        record_timeout_diagnostic(&mut ctx, &actual);
        assert!(ctx.diagnostics.entries().is_empty());
        assert_eq!(ctx.peer_timeout_source, Some(crate::PeerTimeoutSource::BackendRequest));
        assert_eq!(actual.esource(), &pingora_core::ErrorSource::Upstream);
        record_timeout_diagnostic(&mut ctx, &error);
        let expected: Vec<_> = expected.into_iter().map(RequestDiagnostic::Error).collect();
        assert_eq!(ctx.diagnostics.entries(), expected.as_slice());
    }
}

#[test]
fn unary_expired_activation_freezes_grpc_source_before_returning_504() {
    let (mut ctx, owner) = admitted_context();
    ctx.request_info
        .set_grpc_client_deadline(Instant::now() - std::time::Duration::from_secs(1));
    let mut head = ResponseHeader::build(200, None).unwrap();
    head.insert_header("content-type", "application/json").unwrap();
    let error = owner.plan(ResponseHeadSource::Origin, &head, &ctx).unwrap_err();
    assert_eq!(owner.deadline.get().unwrap().source, DeadlineSource::Grpc);
    record_timeout_diagnostic(&mut ctx, &error);
    assert_eq!(
        ctx.diagnostics.entries(),
        &[crate::observe::RequestDiagnostic::Error(
            edgion_resources::EdgionErrorCode::GrpcClientDeadlineExceeded
        )]
    );
}
