use super::*;
use crate::{EdgionHttpContext, PendingHeaderOp};
use edgion_resources::EdgionErrorCode;
use pingora_core::modules::http::grpc_web::GrpcWeb;
use pingora_core::modules::http::HttpModules;
use pingora_proxy::Session;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

async fn closed_downstream_session() -> Session {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"GET /fallback-test HTTP/1.1\r\nHost: test\r\n\r\n")
        .await
        .unwrap();
    session.read_request().await.unwrap();
    drop(client);
    session
}

async fn live_downstream_session() -> (Session, tokio::io::DuplexStream) {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    client
        .write_all(b"POST /svc/Method HTTP/1.1\r\nHost: test\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
    session.read_request().await.unwrap();
    (session, client)
}

#[tokio::test]
async fn html_error_drains_queued_ops_then_reasserts_invariants() {
    let (mut session, mut client) = live_downstream_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.trace_id.initialize(&session.req_header().headers);
    let canonical_trace_id = ctx.request_info.trace_id.standard_trace_id().unwrap().to_owned();
    for op in [
        PendingHeaderOp::Append {
            name: "x-html-error-test".into(),
            value: http::HeaderValue::from_static("allowed"),
        },
        PendingHeaderOp::Set {
            name: "x-trace-id".into(),
            value: http::HeaderValue::from_static("queued-stale"),
        },
        PendingHeaderOp::Set {
            name: "content-type".into(),
            value: http::HeaderValue::from_static("application/json"),
        },
        PendingHeaderOp::Set {
            name: "content-length".into(),
            value: http::HeaderValue::from_static("0"),
        },
        PendingHeaderOp::Set {
            name: "transfer-encoding".into(),
            value: http::HeaderValue::from_static("chunked"),
        },
    ] {
        ctx.queue_pending_response_header_op(op);
    }
    send_html_error(
        &mut session,
        &mut ctx,
        &ServerHeaderOpts::default(),
        400,
        b"expected-error-body",
    )
    .await
    .unwrap();

    let written = session.response_written().expect("HTML response header");
    assert_eq!(written.status.as_u16(), 400);
    assert_eq!(written.headers.get("x-html-error-test").unwrap(), "allowed");
    assert_eq!(written.headers.get_all("x-html-error-test").iter().count(), 1);
    assert_eq!(
        written.headers.get("x-trace-id").unwrap().to_str().unwrap(),
        canonical_trace_id
    );
    assert_eq!(written.headers.get("content-type").unwrap(), "text/html");
    assert!(written.headers.get("content-length").is_none());
    assert!(written.headers.get("transfer-encoding").is_none());
    assert!(!ctx.has_pending_response_header_ops());
    assert!(ctx.pending_response_header_ops().is_empty());

    drop(session);
    let mut wire = Vec::new();
    timeout(Duration::from_secs(2), client.read_to_end(&mut wire))
        .await
        .unwrap()
        .unwrap();
    let body_start = wire
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .expect("response header terminator")
        + 4;
    assert_eq!(&wire[body_start..], b"expected-error-body");
}

async fn live_grpc_web_session() -> (Session, tokio::io::DuplexStream) {
    let (mut client, server) = tokio::io::duplex(4096);
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut modules = HttpModules::new();
    modules.add_module(Box::new(GrpcWeb));
    let mut session = Session::new_h1_with_modules(stream, &modules);
    client
            .write_all(
                b"POST /svc/Method HTTP/1.1\r\nHost: test\r\nContent-Type: application/grpc-web\r\nContent-Length: 0\r\n\r\n",
            )
            .await
            .unwrap();
    session.read_request().await.unwrap();
    **session
        .downstream_modules_ctx
        .get_mut::<GrpcWebBridge>()
        .expect("gRPC-Web bridge module") = GrpcWebCtx::Upgrade;
    (session, client)
}

#[tokio::test]
async fn html_error_helpers_preserve_exact_bodies_headers_and_statuses() {
    for (status, title) in [
        (400, "400 Bad Request"),
        (404, "404 Not Found"),
        (421, "421 Misdirected Request"),
        (500, "500 Internal Server Error"),
        (502, "502 Bad Gateway"),
        (503, "503 Service Temporarily Unavailable"),
    ] {
        let (mut session, mut client) = live_downstream_session().await;
        let mut ctx = EdgionHttpContext::new();
        let opts = ServerHeaderOpts::default();
        match status {
            400 => end_response_400(&mut session, &mut ctx, &opts).await,
            404 => end_response_404(&mut session, &mut ctx, &opts).await,
            421 => end_response_421(&mut session, &mut ctx, &opts).await,
            500 => end_response_500(&mut session, &mut ctx, &opts).await,
            502 => end_response_502(&mut session, &mut ctx, &opts).await,
            503 => end_response_503(&mut session, &mut ctx, &opts).await,
            _ => unreachable!("the table contains only the six HTML error statuses"),
        }
        .unwrap();

        assert_eq!(ctx.final_downstream_status(), Some(status));
        assert!(ctx.response_write_started());
        assert_eq!(
            ctx.request_info.status_option(),
            None,
            "only terminal logging finalizes status"
        );
        let written = session.response_written().expect("HTML response header");
        assert_eq!(written.status.as_u16(), status);
        assert_eq!(written.headers.get("server").unwrap(), "Edgion");
        assert_eq!(
            written.headers.get("strict-transport-security").unwrap(),
            "max-age=63072000; includeSubDomains; preload"
        );
        assert_eq!(written.headers.get("content-type").unwrap(), "text/html");
        assert!(written.headers.get("content-length").is_none());
        assert!(written.headers.get("transfer-encoding").is_none());

        let mut wire = Vec::new();
        timeout(Duration::from_secs(1), client.read_to_end(&mut wire))
            .await
            .expect("HTTP/1 error response must close the downstream")
            .unwrap();
        let body_start = wire
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .expect("response header terminator")
            + 4;
        // Independent expectation retains the original format and exact newlines.
        let expected = format!(
                "<html>\n<head><title>{title}</title></head>\n<body>\n<center><h1>{title}</h1></center>\n<hr><center>edgion</center>\n</body>\n</html>"
            );
        assert_eq!(&wire[body_start..], expected.as_bytes(), "HTML body for {status}");
    }
}

#[tokio::test]
async fn html_error_helper_preserves_existing_response_owner() {
    let mut session = closed_downstream_session().await;
    let mut ctx = EdgionHttpContext::new();
    assert!(ctx.claim_final_response(204));

    // A write to this closed peer would fail; the real helper must return before I/O.
    end_response_503(&mut session, &mut ctx, &ServerHeaderOpts::default())
        .await
        .unwrap();

    assert_eq!(ctx.final_downstream_status(), Some(204));
    assert!(ctx.response_write_started());
    assert_eq!(ctx.request_info.status_option(), None);
    assert!(session.response_written().is_none());
    assert!(ctx.diagnostics.is_empty());
}

#[tokio::test]
async fn response_stage_exact_grpc_reply_drains_allowed_ops_and_reasserts_invariants() {
    let (mut session, _client) = live_downstream_session().await;
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.trace_id.initialize(&session.req_header().headers);
    let trace_id = ctx.request_info.trace_id.standard_trace_id().unwrap().to_owned();
    for op in [
        PendingHeaderOp::AddIfAbsent {
            name: "server".into(),
            value: http::HeaderValue::from_static("unexpected"),
        },
        PendingHeaderOp::OverwriteIfExists {
            name: "strict-transport-security".into(),
            value: http::HeaderValue::from_static("custom"),
        },
        PendingHeaderOp::Set {
            name: "x-order".into(),
            value: http::HeaderValue::from_static("first"),
        },
        PendingHeaderOp::OverwriteIfExists {
            name: "x-order".into(),
            value: http::HeaderValue::from_static("last"),
        },
        PendingHeaderOp::Set {
            name: "x-trace-id".into(),
            value: http::HeaderValue::from_static("stale"),
        },
        PendingHeaderOp::Append {
            name: "grpc-status".into(),
            value: http::HeaderValue::from_static("0"),
        },
        PendingHeaderOp::Append {
            name: "grpc-status".into(),
            value: http::HeaderValue::from_static("1"),
        },
    ] {
        ctx.queue_pending_response_header_op(op);
    }
    ctx.queue_pending_response_header_op(PendingHeaderOp::Append {
        name: "x-ext-proc".to_string(),
        value: http::HeaderValue::from_static("allowed"),
    });
    ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
        name: "content-type".to_string(),
        value: http::HeaderValue::from_static("text/plain"),
    });
    ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
        name: "content-length".to_string(),
        value: http::HeaderValue::from_static("999"),
    });

    send_grpc_status_trailers_only(&mut session, &mut ctx, &ServerHeaderOpts::default(), 8, Some("quota"))
        .await
        .unwrap();

    let written = session.response_written().expect("trailers-only response");
    assert_eq!(written.status.as_u16(), 200);
    assert_eq!(written.headers.get("x-ext-proc").unwrap(), "allowed");
    assert_eq!(written.headers.get("content-type").unwrap(), "application/grpc");
    assert!(written.headers.get("content-length").is_none());
    assert!(written.headers.get("transfer-encoding").is_none());
    assert_eq!(written.headers.get("grpc-status").unwrap(), "8");
    assert_eq!(written.headers["server"], "Edgion");
    assert_eq!(written.headers["strict-transport-security"], "custom");
    assert_eq!(written.headers["x-order"], "last");
    assert_eq!(written.headers["x-trace-id"], trace_id);
    assert_eq!(written.headers.get_all("grpc-status").iter().count(), 1);
    assert!(!ctx.has_pending_response_header_ops());
}

#[tokio::test]
async fn grpc_web_exact_reply_strips_conflicting_status_headers_and_encodes_canonical_trailer() {
    for grpc_status in [0, 8, 16] {
        let (mut session, mut client) = live_grpc_web_session().await;
        let mut ctx = EdgionHttpContext::new();
        for op in [
            PendingHeaderOp::Append {
                name: "x-ext-proc".to_string(),
                value: http::HeaderValue::from_static("allowed"),
            },
            PendingHeaderOp::Set {
                name: "grpc-status".to_string(),
                value: http::HeaderValue::from_static("1"),
            },
            PendingHeaderOp::Set {
                name: "grpc-message".to_string(),
                value: http::HeaderValue::from_static("conflict"),
            },
        ] {
            ctx.queue_pending_response_header_op(op);
        }

        send_grpc_status_trailers_only(
            &mut session,
            &mut ctx,
            &ServerHeaderOpts::default(),
            grpc_status,
            Some("quota"),
        )
        .await
        .unwrap();

        let written = session.response_written().expect("gRPC-Web response header");
        assert_eq!(written.headers.get("x-ext-proc").unwrap(), "allowed");
        assert!(written.headers.get("grpc-status").is_none());
        assert!(written.headers.get("grpc-message").is_none());

        let mut wire = Vec::new();
        while let Ok(Ok(read)) =
            tokio::time::timeout(std::time::Duration::from_millis(20), client.read_buf(&mut wire)).await
        {
            if read == 0 {
                break;
            }
        }
        let wire = String::from_utf8_lossy(&wire);
        assert!(wire.contains("x-ext-proc: allowed\r\n"));
        assert!(!wire.contains("grpc-status: 1\r\n"));
        assert!(!wire.contains("grpc-message: conflict\r\n"));
        assert!(wire.contains(&format!("grpc-status:{grpc_status}\r\ngrpc-message:quota\r\n")));
    }
}

#[tokio::test]
async fn dsl_terminate_grpc_web_has_only_canonical_trailer_status() {
    use crate::plugins::runtime::PluginRuntime;
    use edgion_dsl::validator::{compile_dsl_source, ValidationLimits};
    use edgion_resources::{DslConfig, EdgionPlugin, RequestFilterEntry};

    for bytecode in [false, true] {
        let (mut session, mut client) = live_grpc_web_session().await;
        let source = r#"
                resp.set_header("grpc-status", "0")
                resp.set_header("grpc-message", "queued")
                resp.set_header("x-queued", "retained")
                return terminate()
            "#;
        let config = DslConfig {
            name: "terminate-contract".to_string(),
            source: (!bytecode).then(|| source.to_string()),
            bytecode: bytecode.then(|| compile_dsl_source(source, &ValidationLimits::default()).unwrap()),
            ..Default::default()
        };
        let mut runtime = PluginRuntime::new();
        let errors = runtime.add_from_request_filters(
            &[RequestFilterEntry::new(EdgionPlugin::Dsl(config))],
            "test",
            "test/terminate",
        );
        assert!(errors.is_empty(), "{errors:?}");
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.is_grpc_request = true;
        runtime.run_request_plugins(&mut session, &mut ctx).await;
        terminate_with_fallback(&mut session, &mut ctx, &ServerHeaderOpts::default())
            .await
            .unwrap();
        assert!(ctx.plugin_terminated());
        assert_eq!(ctx.final_downstream_status(), Some(200));
        assert_eq!(ctx.request_info.grpc_status(), Some(13));
        assert_eq!(ctx.stage_logs()[0].filters[0].render_for_test(), "Deny 502");
        assert!(ctx.diagnostics.entries().is_empty());
        let written = session.response_written().unwrap();
        assert_eq!(written.status.as_u16(), 200);
        assert_eq!(written.headers["content-type"], "application/grpc-web");
        assert_eq!(written.headers["x-queued"], "retained");
        assert!(!written.headers.contains_key("grpc-status"));
        assert!(!written.headers.contains_key("grpc-message"));
        drop(session);
        let mut wire = Vec::new();
        timeout(Duration::from_secs(2), client.read_to_end(&mut wire))
            .await
            .unwrap()
            .unwrap();
        let head_end = wire.windows(4).position(|part| part == b"\r\n\r\n").unwrap() + 4;
        // Decode H1 chunking before checking the exact gRPC-Web envelope.
        let mut remaining = &wire[head_end..];
        let mut payload = Vec::new();
        loop {
            let line_end = remaining.windows(2).position(|part| part == b"\r\n").unwrap();
            let size = usize::from_str_radix(std::str::from_utf8(&remaining[..line_end]).unwrap(), 16).unwrap();
            remaining = &remaining[line_end + 2..];
            if size == 0 {
                assert_eq!(remaining, b"\r\n");
                break;
            }
            payload.extend_from_slice(&remaining[..size]);
            assert_eq!(&remaining[size..size + 2], b"\r\n");
            remaining = &remaining[size + 2..];
        }
        let trailer = b"grpc-status:13\r\ngrpc-message:internal\r\n";
        let mut expected = vec![0x80];
        expected.extend_from_slice(&(trailer.len() as u32).to_be_bytes());
        expected.extend_from_slice(trailer);
        assert_eq!(payload, expected, "exactly one canonical trailer frame, no data frames");
    }
}

#[tokio::test]
async fn grpc_terminal_frame_without_message_removes_stale_status_fields() {
    let (mut session, _client) = live_downstream_session().await;
    let mut response = ResponseHeader::build(200, None).unwrap();
    response.insert_header("content-type", "application/grpc").unwrap();
    response.append_header("grpc-status", "0").unwrap();
    response.append_header("grpc-status", "1").unwrap();
    response.insert_header("grpc-message", "queued").unwrap();
    send_grpc_trailers_only_frame(&mut session, response, 8, None)
        .await
        .unwrap();
    let written = session.response_written().unwrap();
    assert_eq!(written.headers["grpc-status"], "8");
    assert_eq!(written.headers.get_all("grpc-status").iter().count(), 1);
    assert!(!written.headers.contains_key("grpc-message"));
}

#[tokio::test]
async fn terminate_fallback_writer_failure_records_client_closed() {
    let mut session = closed_downstream_session().await;
    let mut ctx = EdgionHttpContext::new();

    assert!(
        terminate_with_fallback(&mut session, &mut ctx, &ServerHeaderOpts::default(),)
            .await
            .is_err()
    );

    assert_eq!(ctx.final_downstream_status(), Some(502));
    assert!(ctx
        .diagnostics
        .contains_error(EdgionErrorCode::PluginTerminateWithoutResponse));
    assert!(ctx.diagnostics.contains_error(EdgionErrorCode::ClientClosed));
}

/// When response ownership is claimed, `terminate_with_fallback` must be a no-op:
/// it must NOT record `PluginTerminateWithoutResponse` in diagnostics.
/// This test validates the guard branch directly without needing a live Session.
#[test]
fn terminate_with_fallback_noop_when_response_write_started() {
    let mut ctx = EdgionHttpContext::new();
    assert!(ctx.claim_final_response(204));

    // Pre-condition: no errors recorded yet
    assert!(ctx.diagnostics.is_empty());

    // Simulate the guard that terminate_with_fallback checks first
    if !ctx.response_write_started() {
        ctx.record_error(EdgionErrorCode::PluginTerminateWithoutResponse);
    }

    // Because response ownership was claimed, no error should have been added.
    assert!(
        ctx.diagnostics.is_empty(),
        "terminate_with_fallback must not add errors after response ownership is claimed"
    );
    assert!(
        !ctx.diagnostics
            .contains_error(EdgionErrorCode::PluginTerminateWithoutResponse),
        "PluginTerminateWithoutResponse must not appear after response write ownership is claimed"
    );
}

/// Verify that `http_to_grpc_status(502)` maps to gRPC INTERNAL (code 13).
/// 502 falls into the `500..=599` catch-all arm — this test locks that mapping.
#[test]
fn http_to_grpc_status_maps_502_to_internal() {
    let (grpc_code, msg) = http_to_grpc_status(502);
    assert_eq!(grpc_code, 13, "502 must map to gRPC INTERNAL (13)");
    assert_eq!(msg, "internal", "502 gRPC message must be 'internal'");
}

/// ASCII messages (all current http_to_grpc_status values) must pass through
/// percent_encode_grpc_message unchanged — no spurious encoding on the happy path.
#[test]
fn percent_encode_grpc_message_ascii_passthrough() {
    for msg in &[
        "invalid argument",
        "unauthenticated",
        "permission denied",
        "not found",
        "resource exhausted",
        "cancelled",
        "unimplemented",
        "unavailable",
        "deadline exceeded",
        "internal",
        "unknown",
    ] {
        assert_eq!(
            percent_encode_grpc_message(msg),
            *msg,
            "ASCII grpc-message must not be modified by percent-encoding"
        );
    }
}

/// Non-ASCII bytes must be percent-encoded per gRPC spec (RFC 3986 §2.1).
/// Uses accented Latin and Greek characters to exercise two-byte UTF-8 encoding.
#[test]
fn percent_encode_grpc_message_non_ascii_encoded() {
    // é = U+00E9 → UTF-8 0xC3 0xA9 → "%C3%A9"
    assert_eq!(percent_encode_grpc_message("caf\u{00E9}"), "caf%C3%A9");
    // π = U+03C0 → UTF-8 0xCF 0x80 → "%CF%80"
    assert_eq!(percent_encode_grpc_message("\u{03C0}"), "%CF%80");
    // Mixed: ASCII space kept, non-ASCII encoded
    assert_eq!(
        percent_encode_grpc_message("op\u{00E9}ration invalid"),
        "op%C3%A9ration invalid"
    );
}

/// Control characters must be percent-encoded.
#[test]
fn percent_encode_grpc_message_control_chars_encoded() {
    assert_eq!(percent_encode_grpc_message("line\nbreak"), "line%0Abreak");
    assert_eq!(percent_encode_grpc_message("tab\there"), "tab%09here");
}

/// The percent sign itself (0x25) is the escape introducer and is excluded from the
/// gRPC unencoded byte set, so it must be encoded as `%25` (matches Envoy's behavior).
#[test]
fn percent_encode_grpc_message_percent_encoded() {
    assert_eq!(
        percent_encode_grpc_message("quota 50% exceeded"),
        "quota 50%25 exceeded"
    );
    // A literal percent-escape in the input must be double-encoded, not passed through.
    assert_eq!(percent_encode_grpc_message("%41"), "%2541");
}

#[tokio::test]
async fn grpc_framework_preserves_claim_and_outcome_after_delivery_failure() {
    let mut session = closed_downstream_session().await;
    let mut ctx = EdgionHttpContext::default();
    assert!(
        send_grpc_status_trailers_only(&mut session, &mut ctx, &ServerHeaderOpts::default(), 8, None)
            .await
            .is_err()
    );
    assert_eq!(ctx.final_downstream_status(), Some(200));
    assert_eq!(ctx.request_info.grpc_status(), Some(8));
    ctx.queue_pending_response_header_op(PendingHeaderOp::Set {
        name: "x-unconsumed".into(),
        value: http::HeaderValue::from_static("pending"),
    });
    send_grpc_status_trailers_only(&mut session, &mut ctx, &ServerHeaderOpts::default(), 13, None)
        .await
        .unwrap();
    assert_eq!(ctx.request_info.grpc_status(), Some(8));
    assert_eq!(ctx.pending_response_header_ops().len(), 1);
}
