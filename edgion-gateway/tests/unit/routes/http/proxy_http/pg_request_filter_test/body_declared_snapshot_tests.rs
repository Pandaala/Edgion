use super::*;
use tokio::io::AsyncWriteExt;

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

    assert!(ctx.request_info.method_is_head);
}

#[tokio::test]
async fn non_head_request_snapshots_method_is_head_false() {
    let mut session = session_from_request(b"GET /status HTTP/1.1\r\nHost: test\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();

    snapshot_body_declared(&mut session, &mut ctx);

    assert!(!ctx.request_info.method_is_head);
}
