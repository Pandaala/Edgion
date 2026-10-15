use super::{
    append_proxy_error_to_upstream, can_reuse_downstream_after_proxy_error, classify_proxy_status,
    proxy_error_diagnostic, record_proxy_error_attribution, select_proxy_outcome_status,
    should_append_proxy_error_to_upstream, unary_transport_error, write_proxy_error_reply,
};
use crate::observe::RequestDiagnostic;
use crate::routes::http::proxy_http::pg_upstream_response_filter::select_normalized_upstream_candidate;
use crate::runtime::server::ServerHeaderOpts;
use crate::{BackendContext, EdgionHttpContext, PeerTimeoutSource};
use edgion_resources::EdgionErrorCode;
use pingora_core::{ConnectionClosed, Error, ErrorType};
use pingora_proxy::Session;
use tokio::io::AsyncWriteExt;

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

fn selected_unary_context() -> EdgionHttpContext {
    use crate::ai::conversion::chat_to_anthropic::{budget::UnaryBudget, head::UnaryHead, limits::UnaryLimits};
    let limits = UnaryLimits::resolve(&crate::ai::config::UnaryConfig::default()).unwrap();
    let pool = UnaryBudget::new(1);
    let mut ctx = EdgionHttpContext::default();
    ctx.ensure_ai_state().unary_owner = Some(std::sync::Arc::new(UnaryHead::new(limits, pool.acquire().unwrap())));
    ctx
}

fn test_proxy() -> super::EdgionHttpProxy {
    super::EdgionHttpProxy {
        listener_port: 0,
        is_https_listener: false,
        listener_key: edgion_resources::ListenerKey("unit-fail-to-proxy".into()),
        server_header_opts: Default::default(),
        access_logger: std::sync::Arc::new(crate::observe::AccessLogger::new()),
        egc: crate::config::bind_gateway_config(None),
        enable_http2: false,
        preflight_handler: super::super::PreflightHandler,
        http_route_manager: std::sync::Arc::new(crate::routes::http::routes_mgr::HttpPortRouteManager::new()),
        grpc_route_manager: std::sync::Arc::new(crate::routes::grpc::GrpcPortRouteManager::new()),
    }
}

#[tokio::test]
async fn fail_to_proxy_preserves_http_denial_over_selected_unary_renderer() {
    for error in [
        Error::new(ErrorType::HTTPStatus(502)),
        Error::new_up(ErrorType::ReadError),
    ] {
        let (mut session, _client) = live_downstream_session().await;
        let mut ctx = selected_unary_context();
        ctx.ai_state()
            .unwrap()
            .unary_owner
            .as_ref()
            .unwrap()
            .observe_header(true, true);
        ctx.set_response_denial(crate::ResponseStageDenial::Http(502));
        let result = super::fail_to_proxy(&test_proxy(), &mut session, &error, &mut ctx).await;
        assert_eq!(result.error_code, 502);
        let response = session.response_written().unwrap();
        assert_eq!(response.status.as_u16(), 502);
        assert_eq!(response.headers["content-length"], "0");
        assert!(matches!(
            ctx.response_denial(),
            Some(crate::ResponseStageDenial::Http(502))
        ));
    }
}

#[tokio::test]
async fn fail_to_proxy_keeps_explicit_grpc_status_over_unary_timeout() {
    let (mut session, _client) = live_downstream_session().await;
    let mut ctx = selected_unary_context();
    ctx.is_grpc_route_matched = true;
    ctx.set_response_denial(crate::ResponseStageDenial::Grpc {
        status: 7,
        message: None,
    });
    let error = Error::new_up(ErrorType::ReadTimedout);
    super::fail_to_proxy(&test_proxy(), &mut session, &error, &mut ctx).await;
    let response = session.response_written().unwrap();
    assert_eq!(response.status.as_u16(), 200);
    assert_eq!(response.headers["grpc-status"], "7");
    assert_eq!(ctx.request_info.grpc_status(), Some(7));
    assert!(ctx.response_denial().is_none());
    assert!(ctx.response_write_started());
}

#[test]
fn unary_transport_eligibility_preserves_error_sources_and_deadline_wrappers() {
    for error in [
        Error::new_up(ErrorType::ReadError),
        Error::new_up(ErrorType::ReadTimedout),
        Error::new_up(ErrorType::ConnectTimedout),
        Error::because(
            ErrorType::HTTPStatus(504),
            "connect deadline",
            Error::new_up(ErrorType::ConnectTimedout),
        ),
    ] {
        assert!(
            unary_transport_error(&error, classify_proxy_status(&error)),
            "{error:?}"
        );
    }
    for error in [
        Error::new_down(ErrorType::ReadTimedout),
        Error::new(ErrorType::InternalError),
        Error::new_up(ErrorType::HTTPStatus(502)),
        Error::new_up(ErrorType::HTTPStatus(503)),
        Error::new(ErrorType::HTTPStatus(504)),
        Error::because(
            ErrorType::HTTPStatus(504),
            "local error",
            Error::new(ErrorType::InternalError),
        ),
        Error::because(
            ErrorType::HTTPStatus(504),
            "client error",
            Error::new_down(ErrorType::ReadTimedout),
        ),
    ] {
        assert!(
            !unary_transport_error(&error, classify_proxy_status(&error)),
            "{error:?}"
        );
    }
}

#[tokio::test]
async fn selected_unary_preheader_failures_write_fixed_json_and_preserve_attribution() {
    use tokio::io::AsyncReadExt;
    for error in [
        Error::new_up(ErrorType::ReadError),
        Error::new_up(ErrorType::ReadTimedout),
        Error::because(
            ErrorType::HTTPStatus(504),
            "connect deadline",
            Error::new_up(ErrorType::ConnectTimedout),
        ),
    ] {
        let (mut session, mut client) = live_downstream_session().await;
        let mut ctx = selected_unary_context();
        assert!(!ctx.ai_state().unwrap().unary_owner.as_ref().unwrap().raw_success());
        ctx.request_info.trace_id.initialize(&session.req_header().headers);
        let code = classify_proxy_status(&error);
        assert!(!write_proxy_error_reply(&mut session, &mut ctx, &ServerHeaderOpts::default(), code, &error).await);
        let written = session.response_written().unwrap();
        assert_eq!(written.status.as_u16(), code);
        assert_eq!(ctx.final_downstream_status(), Some(code));
        assert_eq!(written.headers["content-type"], "application/json");
        assert_eq!(written.headers["cache-control"], "no-store");
        assert_eq!(
            written.headers["x-trace-id"],
            ctx.request_info.trace_id.standard_trace_id().unwrap()
        );
        let length: usize = written.headers["content-length"].to_str().unwrap().parse().unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(client.read_u8().await.unwrap());
        }
        let mut body = vec![0; length];
        client.read_exact(&mut body).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            parsed["error"]["code"],
            if code == 504 {
                "upstream_timeout"
            } else {
                "upstream_response_invalid"
            }
        );
    }
}

#[tokio::test]
async fn selected_unary_preheader_reply_keeps_denials_grpc_and_local_statuses() {
    for (code, denial, grpc, selected) in [
        (502, true, false, true),
        (503, false, false, true),
        (504, false, false, true),
        (502, false, false, false),
        (504, false, true, true),
    ] {
        let (mut session, _client) = live_downstream_session().await;
        let mut ctx = if selected {
            selected_unary_context()
        } else {
            EdgionHttpContext::default()
        };
        if denial {
            ctx.set_response_denial(crate::ResponseStageDenial::Http(code));
        }
        ctx.is_grpc_route_matched = grpc;
        let error = if code == 502 {
            Error::new_up(ErrorType::ReadError)
        } else {
            Error::new(ErrorType::HTTPStatus(code))
        };
        assert!(!write_proxy_error_reply(&mut session, &mut ctx, &ServerHeaderOpts::default(), code, &error).await);
        let response = session.response_written().unwrap();
        if grpc {
            assert_eq!(response.status.as_u16(), 200);
            assert_eq!(response.headers["content-type"], "application/grpc");
            assert_eq!(ctx.request_info.grpc_status(), Some(4));
        } else {
            assert_eq!(response.status.as_u16(), code);
            assert_eq!(response.headers["content-length"], "0");
        }
    }
}

#[tokio::test]
async fn request_id_error_reply_keeps_generated_framing() {
    let (mut session, _client) = live_downstream_session().await;
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.trace_id.initialize(&session.req_header().headers);
    ctx.queue_pending_response_header_op(crate::PendingHeaderOp::Set {
        name: "content-length".into(),
        value: http::HeaderValue::from_static("999"),
    });
    ctx.queue_pending_response_header_op(crate::PendingHeaderOp::Set {
        name: "transfer-encoding".into(),
        value: http::HeaderValue::from_static("chunked"),
    });
    assert!(
        !write_proxy_error_reply(
            &mut session,
            &mut ctx,
            &ServerHeaderOpts::default(),
            502,
            &Error::new(ErrorType::HTTPStatus(502))
        )
        .await
    );
    let response = session.response_written().unwrap();
    assert_eq!(response.headers["content-length"], "0");
    assert!(!response.headers.contains_key("transfer-encoding"));
    assert_eq!(
        response.headers["x-trace-id"].to_str().unwrap(),
        ctx.request_info.trace_id.standard_trace_id().unwrap()
    );
}

#[tokio::test]
async fn grpc_route_only_generic_error_writes_mapped_grpc_reply_not_html() {
    let (mut session, _client) = live_downstream_session().await;
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.is_grpc_request = false;
    ctx.is_grpc_route_matched = true;

    assert!(
        !write_proxy_error_reply(
            &mut session,
            &mut ctx,
            &ServerHeaderOpts::default(),
            403,
            &Error::new(ErrorType::HTTPStatus(403))
        )
        .await
    );

    let written = session.response_written().expect("mapped gRPC response");
    assert_eq!(written.status.as_u16(), 200);
    assert_eq!(written.headers.get("content-type").unwrap(), "application/grpc");
    assert_eq!(written.headers.get("grpc-status").unwrap(), "7");
    assert!(written.headers.get("content-length").is_none());
    assert_eq!(ctx.request_info.grpc_status(), Some(7));
}

#[tokio::test]
async fn downstream_read_timeout_writes_terminal_http_408() {
    let (mut session, _client) = live_downstream_session().await;
    let mut ctx = EdgionHttpContext::new();

    assert!(
        !write_proxy_error_reply(
            &mut session,
            &mut ctx,
            &ServerHeaderOpts::default(),
            408,
            &Error::new(ErrorType::HTTPStatus(408))
        )
        .await
    );

    assert_eq!(
        session.response_written().expect("timeout response").status.as_u16(),
        408
    );
    assert_eq!(ctx.final_downstream_status(), Some(408));
    assert!(!can_reuse_downstream_after_proxy_error(408));
}

#[tokio::test]
async fn grpc_route_downstream_read_timeout_uses_canonical_mapping() {
    let (mut session, _client) = live_downstream_session().await;
    let mut ctx = EdgionHttpContext::new();
    ctx.is_grpc_route_matched = true;

    assert!(
        !write_proxy_error_reply(
            &mut session,
            &mut ctx,
            &ServerHeaderOpts::default(),
            408,
            &Error::new(ErrorType::HTTPStatus(408))
        )
        .await
    );

    let written = session.response_written().expect("mapped gRPC response");
    assert_eq!(written.status.as_u16(), 200);
    assert_eq!(written.headers.get("content-type").unwrap(), "application/grpc");
    assert_eq!(written.headers.get("grpc-status").unwrap(), "2");
    assert_eq!(ctx.request_info.grpc_status(), Some(2));
}

#[test]
fn proxy_error_status_classification_preserves_transport_boundaries() {
    assert_eq!(classify_proxy_status(&Error::new_down(ErrorType::ReadTimedout)), 408);
    assert_eq!(classify_proxy_status(&Error::new_down(ErrorType::WriteTimedout)), 499);
    assert_eq!(classify_proxy_status(&Error::new_down(ErrorType::ReadError)), 499);
    assert_eq!(classify_proxy_status(&Error::new_down(ErrorType::WriteError)), 499);
    assert_eq!(classify_proxy_status(&Error::new_down(ConnectionClosed)), 499);
    assert_eq!(classify_proxy_status(&Error::new_up(ErrorType::ReadTimedout)), 504);
    assert_eq!(classify_proxy_status(&Error::new_up(ErrorType::WriteTimedout)), 504);
    assert_eq!(classify_proxy_status(&Error::new(ErrorType::HTTPStatus(418))), 418);
}

#[test]
fn downstream_read_timeout_has_client_timeout_diagnostic_without_attempt_error() {
    let timeout = Error::new_down(ErrorType::ReadTimedout);
    assert_eq!(
        proxy_error_diagnostic(&timeout, 408, None),
        Some(EdgionErrorCode::BodyReadTimeout)
    );
    assert!(!should_append_proxy_error_to_upstream(&timeout, 408));

    let disconnect = Error::new_down(ConnectionClosed);
    assert_eq!(
        proxy_error_diagnostic(&disconnect, 499, None),
        Some(EdgionErrorCode::ClientClosed)
    );
    assert!(!should_append_proxy_error_to_upstream(&disconnect, 499));

    let ordinary = Error::new_up(ErrorType::ConnectError);
    assert_eq!(proxy_error_diagnostic(&ordinary, 502, None), None);
    assert!(should_append_proxy_error_to_upstream(&ordinary, 502));

    let mut ctx = EdgionHttpContext::new();
    ctx.record_error(proxy_error_diagnostic(&timeout, 408, None).expect("timeout diagnostic"));
    assert_eq!(
        ctx.diagnostics.entries(),
        &[RequestDiagnostic::Error(EdgionErrorCode::BodyReadTimeout)]
    );
}

#[test]
fn terminal_upstream_io_timeout_diagnostic_requires_matching_deadline_source() {
    for error_type in [ErrorType::ReadTimedout, ErrorType::WriteTimedout] {
        let timeout = Error::new_up(error_type);
        assert_eq!(
            proxy_error_diagnostic(&timeout, 504, Some(PeerTimeoutSource::RequestDeadline)),
            Some(EdgionErrorCode::RequestTimeoutExceeded)
        );
        assert_eq!(
            proxy_error_diagnostic(&timeout, 504, Some(PeerTimeoutSource::GrpcDeadline)),
            Some(EdgionErrorCode::GrpcClientDeadlineExceeded)
        );
        assert_eq!(
            proxy_error_diagnostic(&timeout, 504, Some(PeerTimeoutSource::BackendRequest)),
            None
        );
        assert_eq!(proxy_error_diagnostic(&timeout, 504, None), None);
    }

    for error_type in [
        ErrorType::HTTPStatus(504),
        ErrorType::ConnectTimedout,
        ErrorType::TLSHandshakeTimedout,
    ] {
        let error = Error::new_up(error_type);
        assert_eq!(
            proxy_error_diagnostic(&error, 504, Some(PeerTimeoutSource::RequestDeadline)),
            None
        );
        assert_eq!(
            proxy_error_diagnostic(&error, 504, Some(PeerTimeoutSource::GrpcDeadline)),
            None
        );
    }
}

#[test]
fn terminal_upstream_io_timeout_append_is_exactly_deduplicated() {
    let read_timeout = Error::new_up(ErrorType::ReadTimedout);
    let write_timeout = Error::new_up(ErrorType::WriteTimedout);
    let mut errors = vec!["EarlierFailure".to_string(), "ReadTimedoutDetail".to_string()];

    append_proxy_error_to_upstream(&mut errors, &read_timeout, 504);
    assert_eq!(errors, ["EarlierFailure", "ReadTimedoutDetail", "ReadTimedout"]);

    append_proxy_error_to_upstream(&mut errors, &read_timeout, 504);
    assert_eq!(errors, ["EarlierFailure", "ReadTimedoutDetail", "ReadTimedout"]);

    append_proxy_error_to_upstream(&mut errors, &write_timeout, 504);
    assert_eq!(
        errors,
        ["EarlierFailure", "ReadTimedoutDetail", "ReadTimedout", "WriteTimedout",]
    );
}

#[test]
fn attempt_error_append_preserves_non_timeout_and_exclusion_boundaries() {
    let mut errors = Vec::new();
    append_proxy_error_to_upstream(&mut errors, &Error::new_up(ErrorType::ConnectError), 502);
    assert_eq!(errors, ["ConnectError"]);

    for error in [
        Error::new(ErrorType::HTTPStatus(504)),
        Error::new_up(ErrorType::ConnectTimedout),
        Error::new_up(ErrorType::TLSHandshakeTimedout),
        Error::new_down(ErrorType::ReadTimedout),
        Error::new_down(ErrorType::WriteTimedout),
        Error::new_down(ConnectionClosed),
    ] {
        let code = classify_proxy_status(&error);
        append_proxy_error_to_upstream(&mut errors, &error, code);
    }
    assert_eq!(errors, ["ConnectError"]);
}

#[test]
fn context_attribution_wires_deadline_diagnostic_and_current_attempt_error() {
    let mut ctx = EdgionHttpContext::new();
    ctx.backend_context = Some(BackendContext {
        source: None,
        rsc: None,
        service_key: None,
        service_kind: None,
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: None,
        effective_policy_captured: false,
        effective_policy: None,
    });
    ctx.push_upstream(None, None);
    ctx.peer_timeout_source = Some(PeerTimeoutSource::GrpcDeadline);

    let timeout = Error::new_up(ErrorType::WriteTimedout);
    record_proxy_error_attribution(&mut ctx, &timeout, classify_proxy_status(&timeout));

    assert_eq!(
        ctx.diagnostics.entries(),
        &[RequestDiagnostic::Error(EdgionErrorCode::GrpcClientDeadlineExceeded)]
    );
    assert_eq!(ctx.get_current_upstream().unwrap().err, ["WriteTimedout"]);
}

#[test]
fn early_upstream_candidate_does_not_hide_downstream_close() {
    let mut ctx = EdgionHttpContext::default();
    select_normalized_upstream_candidate(&mut ctx, 200);

    let error = Error::new_down(ConnectionClosed);
    let code = classify_proxy_status(&error);
    assert_eq!(code, 499);
    select_proxy_outcome_status(&mut ctx, &error, code);

    ctx.finalize_downstream_status(None);
    assert_eq!(ctx.request_info.status(), 499);
}
