use super::body_declared_snapshot_tests::session_from_request;
use super::*;

#[tokio::test]
async fn upgrade_websocket_sets_discover_protocol() {
    let session = session_from_request(b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();

    detect_request_protocol(session.req_header(), &mut ctx);

    assert_eq!(ctx.request_info.discover_protocol.as_deref(), Some("websocket"));
}

#[tokio::test]
async fn upgrade_unknown_token_does_not_set_websocket() {
    let session = session_from_request(b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: foo\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();

    detect_request_protocol(session.req_header(), &mut ctx);

    assert_ne!(ctx.request_info.discover_protocol.as_deref(), Some("websocket"));
}

#[tokio::test]
async fn upgrade_comma_list_does_not_set_websocket() {
    // A comma-separated Upgrade list is not an exact "websocket" token
    // match -- treating it as one would let a client smuggle a body past
    // the websocket-handshake shape checks in `request_admission`.
    let session = session_from_request(b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket, h2c\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();

    detect_request_protocol(session.req_header(), &mut ctx);

    assert_ne!(ctx.request_info.discover_protocol.as_deref(), Some("websocket"));
}
