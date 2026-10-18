use super::{
    claim_response_local_reply, complete_local_reply_delivery, final_response_eligibility, response_compression_level,
    response_filter, sanitize_forwarded_response_headers, EdgionHttpProxy, DEFAULT_COMPRESSION_LEVEL,
};
use crate::ctx::ResponseStreamKind;
use crate::observe::AccessLogger;
use crate::observe::RequestDiagnostic;
use crate::routes::grpc::GrpcPortRouteManager;
use crate::routes::http::routes_mgr::HttpPortRouteManager;
use crate::EdgionHttpContext;
use edgion_resources::{EdgionErrorCode, EdgionInfoCode, ListenerKey};
use pingora_core::protocols::http::compression::ResponseCompressionCtx;
use pingora_core::{Error, ErrorType};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::Session;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

fn proxy() -> EdgionHttpProxy {
    EdgionHttpProxy {
        listener_port: 80,
        is_https_listener: false,
        listener_key: ListenerKey("127.0.0.1:80/HTTP".to_string()),
        server_header_opts: Default::default(),
        access_logger: Arc::new(AccessLogger::default()),
        egc: crate::config::bind_gateway_config(None),
        enable_http2: false,
        preflight_handler: super::super::PreflightHandler,
        http_route_manager: Arc::new(HttpPortRouteManager::default()),
        grpc_route_manager: Arc::new(GrpcPortRouteManager::default()),
    }
}

async fn h1_session() -> (tokio::io::DuplexStream, Session) {
    let (mut client, server) = tokio::io::duplex(4096);
    client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    session.read_request().await.unwrap();
    (client, session)
}

#[tokio::test]
async fn empty_local_reply_h1_preserves_close_and_framing() {
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    for method in [http::Method::GET, http::Method::HEAD] {
        let (mut client, mut session) = h1_session().await;
        session.req_header_mut().set_method(method);
        let mut ctx = EdgionHttpContext::default();
        let mut reply = ResponseHeader::build(502, None).unwrap();
        reply.insert_header("content-length", "0").unwrap();
        ctx.stage_response_local_reply(Box::new(reply), bytes::Bytes::new());
        let mut upstream = ResponseHeader::build(200, None).unwrap();
        upstream.insert_header("x-upstream-private", "discard").unwrap();
        response_filter(&proxy(), &mut session, &mut upstream, &mut ctx)
            .await
            .expect_err("the local response stops upstream forwarding");
        assert_eq!(ctx.final_downstream_status(), Some(502));
        assert_eq!(
            ctx.diagnostics.entries(),
            &[RequestDiagnostic::Info(EdgionInfoCode::PluginResponseReplaced)]
        );
        drop(session);
        let mut wire = String::new();
        tokio::time::timeout(Duration::from_secs(2), client.read_to_string(&mut wire))
            .await
            .unwrap()
            .unwrap();
        let (headers, body) = wire.split_once("\r\n\r\n").unwrap();
        let headers = headers.to_ascii_lowercase();
        assert!(headers.starts_with("http/1.1 502"));
        assert!(headers.contains("connection: close\r\n") || headers.ends_with("connection: close"));
        assert!(headers.contains("content-length: 0"));
        assert!(!headers.contains("transfer-encoding"));
        assert!(!headers.contains("x-upstream-private"));
        assert!(body.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dsl_terminate_h2_local_reply_finishes_before_session_drop() {
    use crate::plugins::runtime::PluginRuntime;
    use crate::PendingHeaderOp;
    use edgion_dsl::validator::{compile_dsl_source, ValidationLimits};
    use edgion_resources::{DslConfig, EdgionPlugin, UpstreamResponseFilterEntry};
    use pingora_core::protocols::http::v2::server::{handshake, H2Accept, HttpSession as H2Session};
    use pingora_core::protocols::Digest;
    use std::time::Duration;

    for bytecode in [false, true] {
        for method in [http::Method::GET, http::Method::HEAD] {
            tokio::time::timeout(Duration::from_secs(5), async {
                let (client_io, server_io) = tokio::io::duplex(64 * 1024);
                let client = tokio::spawn(async move {
                    let (mut sender, connection) = h2::client::handshake(client_io).await.unwrap();
                    let mut drivers = tokio::task::JoinSet::new();
                    drivers.spawn(connection);
                    sender = sender.ready().await.unwrap();
                    let request = http::Request::builder()
                        .method(method)
                        .uri("https://example.test/terminate")
                        .body(())
                        .unwrap();
                    let (response, _) = sender.send_request(request, true).unwrap();
                    let response = response.await.expect("local reply headers arrive without reset");
                    assert_eq!(response.status(), http::StatusCode::BAD_GATEWAY);
                    assert_eq!(response.headers()["x-queued"], "retained");
                    if let Some(length) = response.headers().get("content-length") {
                        assert_eq!(length, "0");
                    }
                    for name in ["connection", "transfer-encoding", "proxy-connection"] {
                        assert!(!response.headers().contains_key(name));
                    }
                    assert!(!response.headers().contains_key("x-upstream-private"));
                    assert!(!response.headers().contains_key("x-bound"));
                    assert!(!response.headers().contains_key("x-after-terminate"));
                    let mut body = response.into_body();
                    while let Some(chunk) = body.data().await {
                        assert!(chunk.expect("empty reply must end cleanly, not reset").is_empty());
                    }
                    assert!(body.trailers().await.unwrap().is_none());
                    assert!(body.is_end_stream());
                    drop(body);
                    drop(sender);
                    // Only stop the client driver after observing the actual EOS.
                    drivers.shutdown().await;
                });

                let mut connection = handshake(Box::new(server_io), None).await.unwrap();
                let digest = Arc::new(Digest::default());
                let Some(H2Accept::Session(h2_session)) = H2Session::from_h2_conn(&mut connection, Arc::clone(&digest))
                    .await
                    .unwrap()
                else {
                    panic!("H2 request reaches the response-stage fixture");
                };
                // The proxy exposes only an H1 test constructor. Replace its
                // public downstream with the real in-memory H2 session.
                let (_unused_client, placeholder) = tokio::io::duplex(1);
                let mut session = Session::new_h1(Box::new(placeholder));
                session.downstream_session =
                    Box::new(pingora_core::protocols::http::server::Session::new_http2(h2_session));
                let source = "resp.set_header(\"x-bound\", \"discard\")\nreturn terminate()";
                let config = DslConfig {
                    name: "terminate-contract".to_string(),
                    source: (!bytecode).then(|| source.to_string()),
                    bytecode: bytecode.then(|| compile_dsl_source(source, &ValidationLimits::default()).unwrap()),
                    ..Default::default()
                };
                let mut runtime = PluginRuntime::new();
                let entries = [
                    UpstreamResponseFilterEntry::new(EdgionPlugin::Dsl(config)),
                    UpstreamResponseFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
                        name: "after-terminate".to_string(),
                        source: Some("resp.set_header(\"x-after-terminate\", \"unexpected\")".to_string()),
                        ..Default::default()
                    })),
                ];
                let errors = runtime.add_from_upstream_response_filters(&entries, "test");
                assert!(errors.is_empty(), "{errors:?}");
                let mut ctx = EdgionHttpContext::default();
                for name in ["Content-Length", "Connection", "Transfer-Encoding", "Proxy-Connection"] {
                    ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
                        name: name.to_string(),
                        value: http::HeaderValue::from_static("999"),
                    });
                }
                ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
                    name: "x-queued".to_string(),
                    value: http::HeaderValue::from_static("retained"),
                });
                let mut upstream = ResponseHeader::build(200, None).unwrap();
                upstream.insert_header("x-upstream-private", "discard").unwrap();
                runtime.run_upstream_response_plugins_sync(&mut session, &mut ctx, &mut upstream);
                assert_eq!(ctx.stage_logs()[0].filters.len(), 1);
                let proxy = proxy();
                let error = response_filter(&proxy, &mut session, &mut upstream, &mut ctx)
                    .await
                    .expect_err("local replacement stops the upstream pump");
                assert!(ctx.local_reply_written());
                assert_eq!(ctx.final_downstream_status(), Some(502));
                assert_eq!(
                    ctx.diagnostics.entries(),
                    &[RequestDiagnostic::Info(EdgionInfoCode::PluginResponseReplaced)]
                );
                let outcome =
                    super::super::pg_fail_to_proxy::fail_to_proxy(&proxy, &mut session, &error, &mut ctx).await;
                assert_eq!(outcome.error_code, 502);
                // Match the post-pump abort path: no downstream.finish() here.
                drop(session);
                let (client_result, connection_result) =
                    tokio::join!(client, H2Session::from_h2_conn(&mut connection, digest));
                client_result.unwrap();
                assert!(connection_result.unwrap().is_none());
            })
            .await
            .expect("H2 local reply reaches EOS within the test deadline");
        }
    }
}

fn response(content_length: Option<&str>) -> ResponseHeader {
    let mut response = ResponseHeader::build(200, None).unwrap();
    response.insert_header("content-type", "application/json").unwrap();
    if let Some(content_length) = content_length {
        response.insert_header("content-length", content_length).unwrap();
    }
    response
}

#[test]
fn response_sanitizer_removes_static_and_repeated_dynamic_fields() {
    let mut response = response(Some("128"));
    response.insert_header("Proxy-Connection", "keep-alive").unwrap();
    for name in super::HOP_BY_HOP_HEADERS {
        response.append_header(*name, "present").unwrap();
    }
    response
        .append_header("connection", "keep-alive, X-Origin-One, , content-length")
        .unwrap();
    response
        .append_header("connection", "X-Origin-Two, x-origin-one")
        .unwrap();
    response.insert_header("x-origin-one", "private-one").unwrap();
    response.insert_header("x-origin-two", "private-two").unwrap();
    response.append_header("set-cookie", "a=1").unwrap();
    response.append_header("set-cookie", "b=2").unwrap();
    response.insert_header("x-end-to-end", "retained").unwrap();

    assert!(sanitize_forwarded_response_headers(&mut response).unwrap());

    for name in super::HOP_BY_HOP_HEADERS {
        assert!(!response.headers.contains_key(*name), "{name}");
    }
    assert!(!response.headers.contains_key("x-origin-one"));
    assert!(!response.headers.contains_key("proxy-connection"));
    assert!(!response.headers.contains_key("x-origin-two"));
    assert!(!response.headers.contains_key("content-length"));
    assert_eq!(response.headers.get_all("set-cookie").iter().count(), 2);
    assert_eq!(response.headers["x-end-to-end"], "retained");
}

#[test]
fn response_sanitizer_repeated_options_do_not_add_allocations() {
    let mut unique = response(Some("128"));
    unique.append_header("connection", "X-First, X-Second").unwrap();
    unique.insert_header("x-first", "removed").unwrap();
    unique.insert_header("x-second", "removed").unwrap();
    let mut repeated = unique.clone();
    repeated
        .append_header("connection", " , x-first, X-FIRST, x-second, X-Second, , ")
        .unwrap();

    let (unique_result, unique_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        sanitize_forwarded_response_headers(std::hint::black_box(&mut unique))
    });
    let (repeated_result, repeated_stats) = crate::test_allocation_counter::count_allocation_stats(|| {
        sanitize_forwarded_response_headers(std::hint::black_box(&mut repeated))
    });
    assert!(!unique_result.unwrap());
    assert!(!repeated_result.unwrap());
    assert_eq!(unique.headers, repeated.headers);
    assert!(!repeated.headers.contains_key("x-first"));
    assert!(!repeated.headers.contains_key("x-second"));
    assert_eq!(repeated_stats, unique_stats);
}

#[test]
fn response_sanitizer_ignores_invalid_ascii_option_and_keeps_valid_siblings() {
    let mut response = response(None);
    response
        .append_header("connection", "X-First, bad token, X-Second")
        .unwrap();
    response.insert_header("x-first", "private-one").unwrap();
    response.insert_header("x-second", "private-two").unwrap();

    assert!(!sanitize_forwarded_response_headers(&mut response).unwrap());

    assert!(!response.headers.contains_key("connection"));
    assert!(!response.headers.contains_key("x-first"));
    assert!(!response.headers.contains_key("x-second"));
}

#[test]
fn response_sanitizer_rejects_non_text_connection_atomically() {
    let mut response = response(Some("128"));
    response.append_header("connection", "X-First").unwrap();
    response
        .append_header("connection", http::HeaderValue::from_bytes(b"X-Opaque, \xff").unwrap())
        .unwrap();
    response.append_header("connection", "X-Last").unwrap();
    response.insert_header("x-first", "private-one").unwrap();
    response.insert_header("x-last", "private-two").unwrap();
    let before = response.headers.clone();

    let error = sanitize_forwarded_response_headers(&mut response).unwrap_err();

    assert!(matches!(error.etype(), ErrorType::InvalidHTTPHeader));
    assert_eq!(response.headers, before);
}

#[test]
fn response_sanitizer_preserves_switching_protocols() {
    let mut response = ResponseHeader::build(101, None).unwrap();
    response.append_header("connection", "Upgrade, X-Handshake").unwrap();
    response.append_header("upgrade", "websocket").unwrap();
    response.insert_header("x-handshake", "retained").unwrap();
    let before = response.headers.clone();

    assert!(!sanitize_forwarded_response_headers(&mut response).unwrap());

    assert_eq!(response.headers, before);
}

#[test]
fn response_sanitizer_cleans_both_response_boundaries() {
    let mut response = response(Some("128"));
    response.append_header("connection", "X-Origin").unwrap();
    response.insert_header("x-origin", "private").unwrap();
    response.insert_header("transfer-encoding", "chunked").unwrap();

    let mut had_transfer_encoding = sanitize_forwarded_response_headers(&mut response).unwrap();
    assert!(!response.headers.contains_key("x-origin"));

    response.append_header("connection", "X-Plugin").unwrap();
    response.insert_header("x-plugin", "private").unwrap();
    response.insert_header("transfer-encoding", "chunked").unwrap();
    had_transfer_encoding |= sanitize_forwarded_response_headers(&mut response).unwrap();

    assert!(had_transfer_encoding);
    assert!(!response.headers.contains_key("connection"));
    assert!(!response.headers.contains_key("x-plugin"));
    assert!(!response.headers.contains_key("transfer-encoding"));
    assert_eq!(
        response_compression_level(true, ResponseStreamKind::None, had_transfer_encoding, &response),
        0
    );
}

#[tokio::test]
async fn response_filter_sanitizes_interim_headers_without_final_bookkeeping() {
    for status in [100, 102, 103] {
        let (_client, mut session) = h1_session().await;
        let mut ctx = EdgionHttpContext::default();
        let mut upstream = ResponseHeader::build(status, None).unwrap();
        upstream.append_header("connection", "X-Interim").unwrap();
        upstream.insert_header("x-interim", "private").unwrap();
        upstream.insert_header("keep-alive", "timeout=5").unwrap();

        response_filter(&proxy(), &mut session, &mut upstream, &mut ctx)
            .await
            .unwrap();

        assert!(!upstream.headers.contains_key("connection"), "status {status}");
        assert!(!upstream.headers.contains_key("x-interim"), "status {status}");
        assert!(!upstream.headers.contains_key("keep-alive"), "status {status}");
        assert_eq!(ctx.final_downstream_status(), None, "status {status}");
        assert!(ctx.stage_logs().is_empty(), "status {status}");
    }
}

#[tokio::test]
async fn settled_local_reply_precedes_opaque_upstream_connection() {
    let (_client, mut session) = h1_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.stage_response_local_reply(Box::new(ResponseHeader::build(403, None).unwrap()), bytes::Bytes::new());
    let mut upstream = ResponseHeader::build(200, None).unwrap();
    upstream
        .append_header("connection", http::HeaderValue::from_bytes(b"X-Opaque, \xff").unwrap())
        .unwrap();
    upstream.insert_header("x-opaque", "private").unwrap();

    let error = response_filter(&proxy(), &mut session, &mut upstream, &mut ctx)
        .await
        .expect_err("the settled local reply stops upstream forwarding");

    assert!(matches!(error.etype(), ErrorType::InternalError));
    assert!(ctx.local_reply_written());
    assert_eq!(ctx.final_downstream_status(), Some(403));
    assert!(upstream.headers.contains_key("connection"));
    assert!(upstream.headers.contains_key("x-opaque"));
}

#[test]
fn local_reply_body_failure_records_client_closed_and_preserves_status() {
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.claim_final_response(403));
    ctx.mark_local_reply_written();

    let result = complete_local_reply_delivery(&mut ctx, Err(Error::new(ErrorType::WriteError)));

    assert!(result.is_err());
    assert!(ctx.local_reply_written());
    assert_eq!(ctx.final_downstream_status(), Some(403));
    assert_eq!(
        ctx.diagnostics.entries(),
        &[RequestDiagnostic::Error(EdgionErrorCode::ClientClosed)]
    );
}

#[test]
fn successful_local_reply_records_replacement_once() {
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.claim_final_response(403));
    ctx.mark_local_reply_written();

    complete_local_reply_delivery(&mut ctx, Ok(())).unwrap();

    assert!(ctx.local_reply_written());
    assert_eq!(ctx.final_downstream_status(), Some(403));
    assert_eq!(
        ctx.diagnostics.entries(),
        &[RequestDiagnostic::Info(EdgionInfoCode::PluginResponseReplaced)]
    );
}

#[test]
fn valid_response_local_reply_statuses_claim_delivery() {
    for status in [200, 599] {
        let mut ctx = EdgionHttpContext::default();

        claim_response_local_reply(&mut ctx, status).unwrap();

        assert!(ctx.response_write_started(), "status {status}");
        assert_eq!(ctx.final_downstream_status(), Some(status), "status {status}");
        assert!(!ctx.local_reply_written(), "status {status}");
        assert!(ctx.diagnostics.entries().is_empty(), "status {status}");
    }
}

#[test]
fn invalid_response_local_reply_statuses_fail_closed_without_delivery_state() {
    for status in [100, 101, 199, 600] {
        let mut ctx = EdgionHttpContext::default();

        let error = claim_response_local_reply(&mut ctx, status).unwrap_err();

        assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)), "status {status}");
        assert!(!error.retry(), "status {status}");
        assert!(!ctx.response_write_started(), "status {status}");
        assert_eq!(ctx.final_downstream_status(), None, "status {status}");
        assert!(!ctx.local_reply_written(), "status {status}");
        assert!(ctx.diagnostics.entries().is_empty(), "status {status}");
    }
}

#[test]
fn already_claimed_response_rejects_local_reply_and_preserves_first_owner() {
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.claim_final_response(204));

    let error = claim_response_local_reply(&mut ctx, 403).unwrap_err();

    assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
    assert!(!error.retry());
    assert!(ctx.response_write_started());
    assert_eq!(ctx.final_downstream_status(), Some(204));
    assert!(!ctx.local_reply_written());
    assert!(ctx.diagnostics.entries().is_empty());
}

#[tokio::test]
async fn response_filter_rejects_informational_local_reply_before_downstream_write() {
    let (_client, mut session) = h1_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.stage_response_local_reply(Box::new(ResponseHeader::build(101, None).unwrap()), bytes::Bytes::new());
    let mut upstream = ResponseHeader::build(200, None).unwrap();

    let error = response_filter(&proxy(), &mut session, &mut upstream, &mut ctx)
        .await
        .unwrap_err();

    assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
    assert!(!error.retry());
    assert!(session.response_written().is_none());
    assert!(!ctx.response_write_started());
    assert_eq!(ctx.final_downstream_status(), None);
    assert!(!ctx.local_reply_written());
    assert!(ctx.diagnostics.entries().is_empty());
}

#[tokio::test]
async fn response_filter_rejects_pending_reply_after_an_existing_writer() {
    let (_client, mut session) = h1_session().await;
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.claim_final_response(204));
    session
        .write_response_header(Box::new(ResponseHeader::build(204, None).unwrap()), true)
        .await
        .unwrap();
    ctx.stage_response_local_reply(
        Box::new(ResponseHeader::build(403, None).unwrap()),
        bytes::Bytes::from_static(b"denied"),
    );
    let mut upstream = ResponseHeader::build(200, None).unwrap();

    let error = response_filter(&proxy(), &mut session, &mut upstream, &mut ctx)
        .await
        .unwrap_err();

    assert!(matches!(error.etype(), ErrorType::HTTPStatus(500)));
    assert!(!error.retry());
    assert_eq!(
        session.response_written().map(|header| header.status.as_u16()),
        Some(204)
    );
    assert!(ctx.response_write_started());
    assert_eq!(ctx.final_downstream_status(), Some(204));
    assert!(!ctx.local_reply_written());
    assert!(ctx.diagnostics.entries().is_empty());
}

#[test]
fn final_response_eligibility_uses_live_method_status_and_stream_kind() {
    for status in [
        http::StatusCode::CONTINUE,
        http::StatusCode::NO_CONTENT,
        http::StatusCode::NOT_MODIFIED,
    ] {
        assert!(!final_response_eligibility(&http::Method::GET, status, ResponseStreamKind::Sse).allows_sse());
    }
    assert!(
        !final_response_eligibility(&http::Method::HEAD, http::StatusCode::OK, ResponseStreamKind::Sse).allows_sse()
    );
    assert!(final_response_eligibility(&http::Method::GET, http::StatusCode::OK, ResponseStreamKind::Sse).allows_sse());
    assert!(
        !final_response_eligibility(&http::Method::GET, http::StatusCode::OK, ResponseStreamKind::None).allows_sse()
    );
}

#[test]
fn compression_requires_one_valid_content_length() {
    assert_eq!(
        response_compression_level(true, ResponseStreamKind::None, false, &response(Some("128"))),
        DEFAULT_COMPRESSION_LEVEL
    );
    assert_eq!(
        response_compression_level(true, ResponseStreamKind::None, false, &response(None)),
        0
    );
    assert_eq!(
        response_compression_level(true, ResponseStreamKind::None, false, &response(Some("invalid"))),
        0
    );

    let mut duplicate = response(Some("128"));
    duplicate.append_header("content-length", "128").unwrap();
    assert_eq!(
        response_compression_level(true, ResponseStreamKind::None, false, &duplicate),
        0
    );

    let mut conflicting_framing = response(Some("128"));
    conflicting_framing
        .insert_header("transfer-encoding", "chunked")
        .unwrap();
    assert_eq!(
        response_compression_level(true, ResponseStreamKind::None, false, &conflicting_framing),
        0
    );
    assert_eq!(
        response_compression_level(true, ResponseStreamKind::None, true, &response(Some("128"))),
        0
    );
}

#[test]
fn final_header_mutation_can_disable_compression() {
    let mut response = response(Some("128"));
    assert_eq!(
        response_compression_level(true, ResponseStreamKind::None, false, &response),
        DEFAULT_COMPRESSION_LEVEL
    );

    response.remove_header("content-length");
    assert_eq!(
        response_compression_level(true, ResponseStreamKind::None, false, &response),
        0
    );
}

#[test]
fn compression_respects_config_and_sse_exemption() {
    let mut response = response(Some("128"));
    response.append_header("content-length", "invalid").unwrap();
    response.insert_header("transfer-encoding", "chunked").unwrap();
    assert_eq!(
        response_compression_level(false, ResponseStreamKind::None, false, &response),
        0
    );
    assert_eq!(
        response_compression_level(true, ResponseStreamKind::Sse, false, &response),
        0
    );
}

#[test]
fn enabled_request_negotiation_survives_final_level_selection() {
    let mut compression = ResponseCompressionCtx::new(DEFAULT_COMPRESSION_LEVEL, false, false);
    let mut request = RequestHeader::build("GET", b"/", None).unwrap();
    request.insert_header("accept-encoding", "gzip").unwrap();
    compression.request_filter(&request);

    let mut response = response(Some("128"));
    compression.adjust_level(response_compression_level(
        true,
        ResponseStreamKind::None,
        false,
        &response,
    ));
    compression.response_header_filter(&mut response, false);

    assert_eq!(response.headers.get("content-encoding").unwrap(), "gzip");
    assert!(response.headers.get("content-length").is_none());
}

#[test]
fn final_unknown_length_selection_disables_negotiated_compression() {
    let mut compression = ResponseCompressionCtx::new(DEFAULT_COMPRESSION_LEVEL, false, false);
    let mut request = RequestHeader::build("GET", b"/", None).unwrap();
    request.insert_header("accept-encoding", "gzip").unwrap();
    compression.request_filter(&request);

    let mut response = response(None);
    compression.adjust_level(response_compression_level(
        true,
        ResponseStreamKind::None,
        false,
        &response,
    ));
    compression.response_header_filter(&mut response, false);

    assert!(response.headers.get("content-encoding").is_none());
}
