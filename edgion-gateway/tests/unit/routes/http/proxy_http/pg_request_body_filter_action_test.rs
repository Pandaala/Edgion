use super::*;
use crate::observe::RequestDiagnostic;
use edgion_resources::resources::edgion_plugins::{WafConfig, WafMode};
use edgion_resources::EdgionErrorCode;
use edgion_waf::seclang::{RequestHead, RuleEngineMode, RuntimeLimits, Transaction};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn terminate_grpc_reply_borrows_text_and_preserves_status_semantics() {
    let text = b"quota";
    let (mapped_status, default_msg) = crate::runtime::server::http_to_grpc_status(403);
    for exact in [None, Some(0), Some(8)] {
        let expected_status = exact.unwrap_or(mapped_status);
        let fallback = exact.is_none().then_some(default_msg);
        assert_eq!(
            terminate_grpc_reply(403, exact, None),
            (expected_status, fallback, false)
        );
        assert_eq!(
            terminate_grpc_reply(403, exact, Some(&[0xff])),
            (expected_status, fallback, true)
        );
        assert_eq!(
            terminate_grpc_reply(403, exact, Some(b"")),
            (expected_status, Some(""), false)
        );
        let (status, message, invalid) = terminate_grpc_reply(403, exact, Some(text));
        assert_eq!((status, message, invalid), (expected_status, Some("quota"), false));
        assert_eq!(message.unwrap().as_ptr(), text.as_ptr());
    }
}

#[tokio::test]
async fn terminate_http_binary_reply_preserves_body_and_framing() {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 1\r\n\r\n")
        .await
        .unwrap();
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    let body = Bytes::from_static(&[0, 0xff, b'a', 0x80]);
    write_terminate_reply(
        &mut session,
        &mut ctx,
        403,
        Some(0),
        Some(body.clone()),
        vec![
            crate::PendingHeaderOp::Set {
                name: "content-length".to_string(),
                value: http::HeaderValue::from_static("999"),
            },
            crate::PendingHeaderOp::Set {
                name: "transfer-encoding".to_string(),
                value: http::HeaderValue::from_static("chunked"),
            },
        ],
    )
    .await;
    let header = session.response_written().unwrap();
    assert_eq!(header.status.as_u16(), 403);
    assert_eq!(header.headers.get("content-length").unwrap(), "4");
    assert!(!header.headers.contains_key("transfer-encoding"));
    assert!(!header.headers.contains_key("grpc-status"));
    drop(session);
    let mut wire = Vec::new();
    client.read_to_end(&mut wire).await.unwrap();
    let body_start = wire.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap() + 4;
    assert_eq!(&wire[body_start..], body.as_ref());
}

#[tokio::test]
async fn invalid_terminate_status_uses_a_bodyless_final_500() {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 1\r\n\r\n")
        .await
        .unwrap();
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();

    write_terminate_reply(
        &mut session,
        &mut ctx,
        100,
        None,
        Some(Bytes::from_static(b"internal detail")),
        Vec::new(),
    )
    .await;

    assert_eq!(ctx.final_downstream_status(), Some(500));
    drop(session);
    let mut wire = Vec::new();
    client.read_to_end(&mut wire).await.unwrap();
    assert!(!wire
        .windows(b"internal detail".len())
        .any(|window| window == b"internal detail"));
}

#[tokio::test]
async fn terminate_grpc_invalid_body_records_diagnostic_without_exact_message() {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: test\r\nContent-Length: 1\r\n\r\n")
        .await
        .unwrap();
    session.read_request().await.unwrap();
    let mut ctx = EdgionHttpContext::default();
    ctx.is_grpc_route_matched = true;
    write_terminate_reply(
        &mut session,
        &mut ctx,
        403,
        Some(0),
        Some(Bytes::from_static(&[0xff])),
        Vec::new(),
    )
    .await;
    let header = session.response_written().unwrap();
    assert_eq!(header.headers.get("grpc-status").unwrap(), "0");
    assert!(!header.headers.contains_key("grpc-message"));
    assert_eq!(ctx.request_info.grpc_status(), Some(0));
    assert_eq!(
        ctx.diagnostics
            .entries()
            .iter()
            .filter(|entry| { **entry == RequestDiagnostic::Error(EdgionErrorCode::PluginGrpcMessageBodyNotUtf8) })
            .count(),
        1
    );
}

fn streaming_waf_context(body_limit: u64) -> EdgionHttpContext {
    let config = WafConfig {
        rules: "SecRule REQUEST_BODY \"@pm blocked\" \"id:1,phase:2,deny,status:409\"\n".to_string(),
        mode: WafMode::On,
        priority: 0,
        request_body: Default::default(),
        ..WafConfig::default()
    };
    let generation = Arc::new(config.compile_generation().expect("test WAF compiles"));
    let request = RequestHead::new(
        b"POST / HTTP/1.1",
        b"POST",
        b"/",
        b"/",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"request-1",
        &[],
    );
    let mut transaction = Transaction::new_shared_with_modes(
        generation,
        request,
        RuntimeLimits::default(),
        edgion_waf::seclang::EvaluationMode::Accelerated,
        RuleEngineMode::On,
    )
    .unwrap();
    transaction.execute_request_headers().unwrap();
    transaction.start_request_body_stream().unwrap();
    let owner = Arc::new(());
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.select_waf(Some(crate::ctx::WafSelection::new(
        Arc::clone(&owner),
        crate::ctx::WafLogSource::Route,
        0,
        crate::ctx::WafLogMode::On,
    ))));
    let waf = ctx.waf.as_deref_mut().unwrap();
    assert!(waf.install(Arc::clone(&owner), transaction));
    assert!(waf.enable_request_body_streaming(&owner, body_limit));
    ctx.activate_waf_request_body_streaming();
    ctx
}

fn waf_log_value(ctx: &EdgionHttpContext) -> serde_json::Value {
    serde_json::to_value(ctx.waf_access_log().expect("selected WAF summary")).unwrap()
}

#[test]
fn native_waf_stream_scans_cross_chunk_without_body_storage() {
    let mut ctx = streaming_waf_context(1024);

    assert!(run_waf_body_stream(b"prefix-blo", RequestBodyEvent::Data, &mut ctx).is_none());
    assert!(matches!(
        run_waf_body_stream(b"cked", RequestBodyEvent::Data, &mut ctx),
        Some(ChunkVerdict::Terminate { status: 409, .. })
    ));
    assert!(!ctx.request_body.has_cached_body());
    assert!(!ctx.waf.as_deref().unwrap().request_body_streaming());
    assert!(ctx.stage_logs().is_empty());
    let waf = waf_log_value(&ctx);
    assert_eq!(waf["outcome"], "blocked");
    assert_eq!(waf["phases"]["request_body"]["findings"], 1);
    assert_eq!(waf["intervention"]["phase"], "request_body");
    assert_eq!(waf["intervention"]["rule_id"], 1);
    assert_eq!(waf["intervention"]["status"], 409);
}

#[test]
fn native_waf_stream_fails_closed_on_gateway_cap_and_abandoned_body() {
    let mut capped = streaming_waf_context(3);
    assert!(matches!(
        run_waf_body_stream(b"four", RequestBodyEvent::Data, &mut capped),
        Some(ChunkVerdict::Terminate { status: 413, .. })
    ));
    assert!(capped.stage_logs().is_empty());
    let capped_waf = waf_log_value(&capped);
    assert_eq!(capped_waf["outcome"], "error");
    assert_eq!(capped_waf["terminal_error"]["phase"], "request_body");
    assert_eq!(capped_waf["terminal_error"]["code"], "body_limit");
    assert_eq!(capped_waf["terminal_error"]["status"], 413);

    let mut abandoned = streaming_waf_context(1024);
    assert!(matches!(
        run_waf_body_stream(&[], RequestBodyEvent::Abandoned, &mut abandoned),
        Some(ChunkVerdict::Terminate { status: 400, .. })
    ));
    assert!(!abandoned.waf.as_deref().unwrap().request_body_streaming());
    assert!(abandoned.stage_logs().is_empty());
    let abandoned_waf = waf_log_value(&abandoned);
    assert_eq!(abandoned_waf["outcome"], "error");
    assert_eq!(abandoned_waf["terminal_error"]["code"], "body_stream_abandoned");
    assert_eq!(abandoned_waf["terminal_error"]["status"], 400);
}

#[test]
fn terminate_writer_classifies_grpc_route_without_content_type_fact() {
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.is_grpc_request = false;
    ctx.is_grpc_route_matched = true;
    assert!(is_canonical_grpc(&ctx));
}

#[test]
fn terminate_does_not_replace_a_response_selected_by_pingora() {
    assert!(!request_body_should_write_terminate_reply(true));
    assert!(request_body_should_write_terminate_reply(false));
}

#[tokio::test]
async fn grpc_route_only_terminate_writes_exact_status_and_local_header() {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"POST /svc/Method HTTP/1.1\r\nHost: test\r\nContent-Length: 1\r\n\r\n")
        .await
        .unwrap();
    session.read_request().await.unwrap();

    let mut ctx = EdgionHttpContext::default();
    ctx.is_grpc_route_matched = true;
    write_terminate_reply(
        &mut session,
        &mut ctx,
        403,
        Some(8),
        Some(Bytes::from_static(b"quota")),
        vec![crate::PendingHeaderOp::Append {
            name: "x-local".to_string(),
            value: http::HeaderValue::from_static("reply"),
        }],
    )
    .await;

    let written = session.response_written().expect("gRPC local reply header");
    assert_eq!(written.status.as_u16(), 200);
    assert_eq!(written.headers.get("grpc-status").unwrap(), "8");
    assert_eq!(written.headers.get("x-local").unwrap(), "reply");
    assert_eq!(ctx.request_info.grpc_status(), Some(8));
}
