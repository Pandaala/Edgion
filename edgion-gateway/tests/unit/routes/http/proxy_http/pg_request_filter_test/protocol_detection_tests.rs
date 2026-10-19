use super::body_declared_snapshot_tests::session_from_request;
use super::*;

#[test]
fn protocol_labels_are_allocation_free_and_preserve_serialized_values() {
    let cases = [
        (None, None, None, false),
        (None, Some("text/plain"), None, false),
        (None, Some("Application/Grpc"), Some("grpc"), true),
        (None, Some("APPLICATION/GRPC-WEB+PROTO"), Some("grpc-web"), true),
        (
            Some("WebSocket"),
            Some("application/grpc-web"),
            Some("websocket"),
            false,
        ),
        (Some("websocket, h2c"), Some("application/grpc"), Some("grpc"), true),
    ];
    for (upgrade, content_type, expected, grpc) in cases {
        let mut header = RequestHeader::build("GET", b"/", None).unwrap();
        if let Some(value) = upgrade {
            header.insert_header("upgrade", value).unwrap();
        }
        if let Some(value) = content_type {
            header.insert_header("content-type", value).unwrap();
        }
        let mut ctx = EdgionHttpContext::default();
        let (_, allocations) = crate::test_allocation_counter::count_allocations(|| {
            detect_request_protocol(&header, &mut ctx);
        });
        assert_eq!(allocations, 0);
        assert_eq!(ctx.request_info.discover_protocol, expected);
        assert_eq!(ctx.request_info.is_grpc_request, grpc);
        let serialized = serde_json::to_value(&ctx.request_info).unwrap();
        assert_eq!(serialized.get("discover_protocol").and_then(|v| v.as_str()), expected);
        assert_eq!(serialized.get("discover_protocol").is_some(), expected.is_some());
    }

    let mut header = RequestHeader::build("GET", b"/", None).unwrap();
    header.insert_header("upgrade", "websocket").unwrap();
    for label in ["", "WebSocket", "grpc", "grpc-web"] {
        for grpc in [false, true] {
            let mut ctx = EdgionHttpContext::default();
            ctx.request_info.discover_protocol = Some(label);
            ctx.request_info.is_grpc_request = grpc;
            let (_, allocations) = crate::test_allocation_counter::count_allocations(|| {
                detect_request_protocol(&header, &mut ctx);
            });
            assert_eq!(allocations, 0);
            assert_eq!(ctx.request_info.discover_protocol, Some(label));
            assert_eq!(ctx.request_info.is_grpc_request, grpc);
        }
    }

    let mut header = RequestHeader::build("GET", b"/", None).unwrap();
    header
        .insert_header("content-type", http::HeaderValue::from_bytes(b"\xff").unwrap())
        .unwrap();
    let mut ctx = EdgionHttpContext::default();
    detect_request_protocol(&header, &mut ctx);
    assert_eq!(ctx.request_info.discover_protocol, None);
    assert!(!ctx.request_info.is_grpc_request);
}

#[tokio::test]
async fn upgrade_websocket_sets_discover_protocol() {
    let session = session_from_request(b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();

    detect_request_protocol(session.req_header(), &mut ctx);

    assert_eq!(ctx.request_info.discover_protocol, Some("websocket"));
}

#[tokio::test]
async fn upgrade_unknown_token_does_not_set_websocket() {
    let session = session_from_request(b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: foo\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();

    detect_request_protocol(session.req_header(), &mut ctx);

    assert_ne!(ctx.request_info.discover_protocol, Some("websocket"));
}

#[tokio::test]
async fn upgrade_comma_list_does_not_set_websocket() {
    // A comma-separated Upgrade list is not an exact "websocket" token
    // match -- treating it as one would let a client smuggle a body past
    // the websocket-handshake shape checks in `request_admission`.
    let session = session_from_request(b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket, h2c\r\n\r\n").await;
    let mut ctx = EdgionHttpContext::default();

    detect_request_protocol(session.req_header(), &mut ctx);

    assert_ne!(ctx.request_info.discover_protocol, Some("websocket"));
}
