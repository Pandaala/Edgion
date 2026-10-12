use super::*;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn original_method_snapshot_survives_live_method_rewrite() {
    let mut session = session_from_request(b"GET /users/42 HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();
    snapshot_body_declared(&mut session, &mut ctx);
    session.req_header_mut().set_method(http::Method::POST);
    assert_eq!(ctx.request_info.method, Some(http::Method::GET));
    assert_eq!(session.req_header().method, http::Method::POST);
}

/// Build a real HTTP/1.1 `Session` from literal request bytes.
///
/// `pub(super)` so `protocol_detection_tests` (a sibling module) can reuse
/// it instead of duplicating the duplex-pair plumbing.
pub(super) async fn session_from_request(raw_request: &[u8]) -> Session {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(raw_request)
        .await
        .expect("write request into duplex pair");
    drop(client); // header (and body, if any) bytes are already buffered
    session
        .read_request()
        .await
        .expect("read_request should succeed")
        .then_some(())
        .expect("client must not have closed before sending the request");
    session
}

#[tokio::test]
async fn post_with_content_length_and_body_declares_body_true() {
    let mut session =
        session_from_request(b"POST /submit HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\n\r\nhello").await;
    let mut ctx = EdgionHttpContext::default();

    snapshot_body_declared(&mut session, &mut ctx);

    assert_eq!(ctx.request_info.body_declared, Some(true));
}

#[tokio::test]
async fn get_with_no_content_length_or_transfer_encoding_declares_body_false() {
    let mut session = session_from_request(b"GET /status HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();

    snapshot_body_declared(&mut session, &mut ctx);

    assert_eq!(ctx.request_info.body_declared, Some(false));
}

/// The `method_is_head` half of the snapshot, off real H1 request bytes:
/// the response-side admission predicate reads this field instead of the
/// plugin-mutable live header, so an inverted comparison here would put the
/// hole straight back (see `RequestInfo::method_is_head`).
#[tokio::test]
async fn head_request_snapshots_method_is_head() {
    let mut session = session_from_request(b"HEAD /status HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();

    snapshot_body_declared(&mut session, &mut ctx);

    assert!(ctx.request_info.method_is_head());
}

#[tokio::test]
async fn non_head_request_snapshots_method_is_head_false() {
    let mut session = session_from_request(b"GET /status HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();

    snapshot_body_declared(&mut session, &mut ctx);

    assert!(!ctx.request_info.method_is_head());
}

#[tokio::test]
async fn physical_admission_uses_original_method_length_and_canonical_grpc_facts() {
    for upgrade in ["websocket", "foo"] {
        let request = format!("POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\nUpgrade: {upgrade}\r\n\r\nhello");
        let mut session = session_from_request(request.as_bytes()).await;
        let mut ctx = EdgionHttpContext::default();
        snapshot_body_declared(&mut session, &mut ctx);
        detect_request_protocol(session.req_header(), &mut ctx);
        assert!(finite_http_request_body(&ctx));
        session.req_header_mut().set_method(http::Method::CONNECT);
        session.req_header_mut().remove_header("content-length");
        assert_eq!(ctx.request_info.declared_content_length, Some(5));
        assert!(finite_http_request_body(&ctx));
        ctx.request_info.is_grpc_request = true;
        assert!(!finite_http_request_body(&ctx));
        ctx.request_info.is_grpc_request = false;
        ctx.is_grpc_route_matched = true;
        assert!(!finite_http_request_body(&ctx));
    }
    let mut session = session_from_request(b"CONNECT test:443 HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();
    snapshot_body_declared(&mut session, &mut ctx);
    session.req_header_mut().set_method(http::Method::POST);
    assert!(!finite_http_request_body(&ctx));
}

#[tokio::test]
async fn method_snapshot_preserves_reverse_rewrite_and_extension_methods() {
    for (entry, rewritten) in [("POST", "GET"), ("HEAD", "GET"), ("REPORT", "POST")] {
        let request = format!("{entry} /entry HTTP/1.1\r\nHost: test\r\n\r\n");
        let mut session = session_from_request(request.as_bytes()).await;
        let mut ctx = EdgionHttpContext::default();
        snapshot_body_declared(&mut session, &mut ctx);
        session
            .req_header_mut()
            .set_method(http::Method::from_bytes(rewritten.as_bytes()).unwrap());
        assert_eq!(ctx.request_info.request_method().unwrap().as_str(), entry);
        assert_eq!(ctx.request_info.method_is_head(), entry == "HEAD");
        assert!(!ctx.request_info.method_is_connect());
        assert_eq!(session.req_header().method.as_str(), rewritten);
    }
}

#[test]
fn missing_method_snapshot_keeps_physical_cap_and_cannot_claim_head() {
    let ctx = EdgionHttpContext::default();
    assert!(ctx.request_info.request_method().is_none());
    assert!(!ctx.request_info.method_is_head());
    assert!(!ctx.request_info.method_is_connect());
    assert!(finite_http_request_body(&ctx));
}
