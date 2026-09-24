use super::*;
use crate::EdgionHttpContext;

#[test]
fn build_stashes_status_and_framed_body() {
    let mut ctx = EdgionHttpContext::default();
    let applied = build_response_local_reply(
        &mut ctx,
        403,
        Some(Bytes::from_static(br#"{"error":"forbidden"}"#)),
        false,
    );
    assert!(applied);
    let (h, body) = ctx
        .take_pending_response_local_reply()
        .expect("successful build must stage a complete local reply");
    assert_eq!(h.status.as_u16(), 403);
    assert_eq!(h.headers.get("content-type").unwrap().to_str().unwrap(), "text/plain");
    assert_eq!(h.headers.get("content-length").unwrap().to_str().unwrap(), "21");
    assert!(h.headers.get("content-encoding").is_none());
    assert_eq!(body.as_ref(), b"{\"error\":\"forbidden\"}");
}

#[test]
fn build_none_body_is_zero_length() {
    let mut ctx = EdgionHttpContext::default();
    assert!(build_response_local_reply(&mut ctx, 500, None, false));
    let (h, body) = ctx.take_pending_response_local_reply().unwrap();
    assert_eq!(h.headers.get("content-length").unwrap().to_str().unwrap(), "0");
    assert!(body.is_empty());
}

/// The response-stage builder deliberately erases the absent/empty
/// distinction: it resolves the body with `unwrap_or_default()`, so a
/// replacement reply frames identically either way. Pinned because the
/// request-stage writer does NOT erase it (`None` ends the response on the
/// header write, `Some(empty)` adds a zero-length body write), and that
/// asymmetry is easy to "fix" in the wrong direction here.
#[test]
fn build_collapses_empty_and_absent_body_to_one_framing() {
    let mut absent_ctx = EdgionHttpContext::default();
    assert!(build_response_local_reply(&mut absent_ctx, 500, None, false));
    let (absent_h, absent_body) = absent_ctx.take_pending_response_local_reply().unwrap();

    let mut empty_ctx = EdgionHttpContext::default();
    assert!(build_response_local_reply(
        &mut empty_ctx,
        500,
        Some(Bytes::new()),
        false
    ));
    let (empty_h, empty_body) = empty_ctx.take_pending_response_local_reply().unwrap();

    assert_eq!(empty_h.status.as_u16(), 500);
    assert_eq!(absent_h.status, empty_h.status);
    // Assert the literal framing, not merely that the two agree — equality
    // alone would still pass if `content-length` vanished from both.
    assert_eq!(empty_h.headers.get("content-length").unwrap().to_str().unwrap(), "0");
    assert_eq!(absent_h.headers.get("content-length").unwrap().to_str().unwrap(), "0");
    assert!(empty_body.is_empty());
    assert!(absent_body.is_empty());
}

/// The body is raw `Bytes`, so a non-UTF-8 local reply reaches the wire
/// verbatim. The former `String` representation could not express this
/// without a lossy round-trip.
#[test]
fn build_preserves_non_utf8_body_bytes() {
    const RAW: &[u8] = &[0x00, 0xff, 0xfe, 0x80, 0x41];
    let mut ctx = EdgionHttpContext::default();
    assert!(build_response_local_reply(
        &mut ctx,
        418,
        Some(Bytes::from_static(RAW)),
        false
    ));
    let (h, body) = ctx.take_pending_response_local_reply().unwrap();
    assert_eq!(h.headers.get("content-length").unwrap().to_str().unwrap(), "5");
    assert_eq!(body.as_ref(), RAW);
}

/// A `from_static` body is forwarded without copying: the staged reply
/// points at the original static buffer.
#[test]
fn build_forwards_static_body_without_copying() {
    static BODY: &[u8] = b"a fixed fail-close body";
    let mut ctx = EdgionHttpContext::default();
    assert!(build_response_local_reply(
        &mut ctx,
        403,
        Some(Bytes::from_static(BODY)),
        false
    ));
    let (_, body) = ctx.take_pending_response_local_reply().unwrap();
    assert_eq!(body.as_ptr(), BODY.as_ptr());
}

#[test]
fn build_head_is_bodyless() {
    let mut ctx = EdgionHttpContext::default();
    assert!(build_response_local_reply(&mut ctx, 403, Some("denied".into()), true));
    let (h, body) = ctx.take_pending_response_local_reply().unwrap();
    assert!(h.headers.get("content-length").is_none());
    assert!(body.is_empty());
}

#[test]
fn build_204_is_bodyless() {
    let mut ctx = EdgionHttpContext::default();
    assert!(build_response_local_reply(&mut ctx, 204, Some("ignored".into()), false));
    let (h, body) = ctx.take_pending_response_local_reply().unwrap();
    assert!(h.headers.get("content-length").is_none());
    assert!(body.is_empty());
}

#[test]
fn build_grpc_route_is_refused() {
    let mut ctx = EdgionHttpContext::default();
    ctx.is_grpc_route_matched = true;
    assert!(!build_response_local_reply(&mut ctx, 502, Some("x".into()), false));
    assert!(!ctx.response_stage_settled());
}

#[test]
fn build_drains_queued_ops_then_reasserts_content_length() {
    let mut ctx = EdgionHttpContext::default();
    ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
        name: "content-type".to_string(),
        value: http::HeaderValue::from_static("application/json"),
    });
    ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
        name: "content-length".to_string(),
        value: http::HeaderValue::from_static("999"),
    });
    assert!(build_response_local_reply(&mut ctx, 502, Some("{}".into()), false));
    let (h, _) = ctx.take_pending_response_local_reply().unwrap();
    // Ordinary content-type survives; the rejected length cannot override body-owned framing.
    assert_eq!(
        h.headers.get("content-type").unwrap().to_str().unwrap(),
        "application/json"
    );
    assert_eq!(h.headers.get("content-length").unwrap().to_str().unwrap(), "2");
}

#[test]
fn apply_pending_ops_is_nondestructive_and_applies() {
    let mut ctx = EdgionHttpContext::default();
    ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
        name: "x-foo".to_string(),
        value: http::HeaderValue::from_static("bar"),
    });
    let mut hdr = ResponseHeader::build(200, None).unwrap();
    ctx.apply_pending_response_header_ops(&mut hdr);
    // Applied onto the header...
    assert_eq!(hdr.headers.get("x-foo").unwrap().to_str().unwrap(), "bar");
    // ...but the queue is left intact (non-destructive) for a later local reply.
    assert_eq!(ctx.pending_response_header_ops().len(), 1);
}

#[test]
fn queued_ops_survive_phase6_apply_onto_local_reply() {
    // Simulates the two-phase flow: the sync stage applies queued ops onto
    // the (soon-discarded) upstream header non-destructively; a response-phase
    // plugin then replaces the response, and build_response_local_reply
    // re-applies the still-intact queue onto the fresh reply. Proves BOTH the
    // sync and async local-reply paths preserve the ops, since neither
    // pre-drains the queue.
    let mut ctx = EdgionHttpContext::default();
    ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
        name: "x-foo".to_string(),
        value: http::HeaderValue::from_static("bar"),
    });
    let mut upstream = ResponseHeader::build(200, None).unwrap();
    ctx.apply_pending_response_header_ops(&mut upstream);
    assert!(build_response_local_reply(&mut ctx, 403, Some("denied".into()), false));
    let (reply, _) = ctx.take_pending_response_local_reply().unwrap();
    assert_eq!(reply.headers.get("x-foo").unwrap().to_str().unwrap(), "bar");
    // The builder's destructive drain consumed the queue.
    assert!(!ctx.has_pending_response_header_ops());
}
