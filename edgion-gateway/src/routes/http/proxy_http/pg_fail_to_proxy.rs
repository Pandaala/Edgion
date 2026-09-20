use super::EdgionHttpProxy;
use crate::runtime::server::send_grpc_status_trailers_only;
use crate::runtime::server::server_header::ServerHeaderOpts;
use crate::send_grpc_trailers_only;
use crate::{EdgionHttpContext, PeerTimeoutSource, ResponseStageDenial};
use edgion_resources::EdgionErrorCode;
use pingora_core::{ConnectionClosed, Error, ErrorSource, ErrorType, HTTPStatus, ReadError, WriteError};
use pingora_proxy::{FailToProxy, Session};

#[inline]
fn is_downstream_read_timeout(e: &Error) -> bool {
    matches!(e.etype(), ErrorType::ReadTimedout) && matches!(e.esource(), ErrorSource::Downstream)
}

#[inline]
fn is_terminal_upstream_io_timeout(e: &Error) -> bool {
    matches!(e.etype(), ErrorType::ReadTimedout | ErrorType::WriteTimedout)
        && matches!(e.esource(), ErrorSource::Upstream)
}

#[inline]
fn classify_proxy_status(e: &Error) -> u16 {
    match e.etype() {
        HTTPStatus(code) => *code,
        ErrorType::ReadTimedout if is_downstream_read_timeout(e) => 408,
        ErrorType::ReadTimedout | ErrorType::WriteTimedout => match e.esource() {
            ErrorSource::Downstream => 499,
            ErrorSource::Upstream => 504,
            _ => 504,
        },
        ErrorType::ConnectTimedout | ErrorType::TLSHandshakeTimedout => 504,
        _ => match e.esource() {
            ErrorSource::Upstream => 502,
            ErrorSource::Downstream => match e.etype() {
                WriteError | ReadError | ConnectionClosed => 499,
                _ => 400,
            },
            ErrorSource::Internal | ErrorSource::Unset => 500,
        },
    }
}

#[inline]
fn proxy_error_diagnostic(e: &Error, code: u16, timeout_source: Option<PeerTimeoutSource>) -> Option<EdgionErrorCode> {
    if is_downstream_read_timeout(e) {
        Some(EdgionErrorCode::BodyReadTimeout)
    } else if code == 499 && !matches!(e.etype(), HTTPStatus(_)) {
        Some(EdgionErrorCode::ClientClosed)
    } else if is_terminal_upstream_io_timeout(e) {
        match timeout_source {
            Some(PeerTimeoutSource::RequestDeadline) => Some(EdgionErrorCode::RequestTimeoutExceeded),
            Some(PeerTimeoutSource::GrpcDeadline) => Some(EdgionErrorCode::GrpcClientDeadlineExceeded),
            Some(PeerTimeoutSource::BackendRequest) | None => None,
        }
    } else {
        None
    }
}

#[inline]
fn should_append_proxy_error_to_upstream(e: &Error, code: u16) -> bool {
    is_terminal_upstream_io_timeout(e) || (code != 504 && code != 499 && !is_downstream_read_timeout(e))
}

#[inline]
fn append_proxy_error_to_upstream(errors: &mut Vec<String>, e: &Error, code: u16) {
    if !should_append_proxy_error_to_upstream(e, code) {
        return;
    }

    let kind = e.etype().as_str();
    if is_terminal_upstream_io_timeout(e) && errors.iter().any(|existing| existing == kind) {
        return;
    }
    errors.push(kind.to_string());
}

#[inline]
fn record_proxy_error_attribution(ctx: &mut EdgionHttpContext, e: &Error, code: u16) {
    if let Some(diagnostic) = proxy_error_diagnostic(e, code, ctx.peer_timeout_source) {
        ctx.record_error(diagnostic);
    }
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        append_proxy_error_to_upstream(&mut upstream.err, e, code);
    }
}

#[inline]
fn can_reuse_downstream_after_proxy_error(code: u16) -> bool {
    matches!(code, 400 | 403 | 404 | 429)
}

#[inline]
fn select_proxy_outcome_status(ctx: &mut EdgionHttpContext, e: &Error, code: u16) {
    if code == 0 {
        return;
    }
    if code == 499 && !matches!(e.etype(), HTTPStatus(_)) {
        ctx.select_terminal_downstream_status(499);
    } else {
        ctx.select_final_downstream_status(code);
    }
}

async fn write_proxy_error_reply(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
    code: u16,
) -> bool {
    if ctx.request_info.is_grpc_request || ctx.is_grpc_route_matched {
        send_grpc_trailers_only(session, ctx, server_header_opts, code)
            .await
            .is_err()
    } else {
        let mut resp = pingora_core::protocols::http::ServerSession::generate_error(code);
        server_header_opts.apply_to_response(&mut resp);
        ctx.request_info.trace_id.reconcile_response(&mut resp);
        if ctx.claim_final_response(code) {
            session
                .write_error_response(resp, bytes::Bytes::default())
                .await
                .is_err()
        } else {
            false
        }
    }
}

#[inline]
pub async fn fail_to_proxy(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    e: &Error,
    ctx: &mut EdgionHttpContext,
) -> FailToProxy {
    // filter-04b: the local reply was already written downstream by
    // response_filter. Do not write an error page or pollute upstream.err.
    // NOTE (filter-04c): can_reuse_downstream is reported true to express intent,
    // but the post-proxy abort path (the sentinel Err returned by response_filter)
    // does not consult this field and tears the H1 downstream connection down.
    // This is safe because response_filter disables keepalive before writing;
    // preserving reuse is a separate Pingora lifecycle feature. See
    // skills/04-review/http-body/intentional-design/response-local-reply-h1-close-is-feature.md.
    if ctx.local_reply_written() {
        return FailToProxy {
            error_code: ctx.final_downstream_status().unwrap_or(500),
            can_reuse_downstream: true,
        };
    }

    // A response-stage `ErrGrpcError` the stage itself could not write (the sync
    // runner cannot await). Deliver the plugin's exact gRPC status here, ahead
    // of the generic status handling below, which would otherwise re-derive a
    // code from the synthetic HTTP 200 the abort carries. Marks the response
    // committed, so that generic block then skips this request.
    if let Some(ResponseStageDenial::Grpc { status, message }) = ctx.take_response_denial() {
        if send_grpc_status_trailers_only(
            session,
            ctx,
            &edgion_http.server_header_opts,
            status,
            message.as_deref(),
        )
        .await
        .is_err()
        {
            // Delivery failed: the downstream connection is dead.
            ctx.record_error(EdgionErrorCode::ClientClosed);
        }
    }

    let code = classify_proxy_status(e);

    // A stalled request-body upload is a client timeout, not a disconnect.
    // Other downstream transport failures keep the ClientClosed diagnostic.
    record_proxy_error_attribution(ctx, e, code);

    // A gateway-internal error that surfaced while the captured request body
    // was being replayed to the upstream is a body-replay failure (buffer
    // invariant violation or spill-storage read error). `upstream.err` alone
    // would show only the indistinguishable "InternalError", so record a
    // dedicated status; the error-log line (join on the standard trace ID) carries the
    // precise cause text. Upstream-attributed errors mid-replay (e.g. a write
    // timeout while pushing the replayed body) are deliberately excluded —
    // those are upstream failures, not buffer failures.
    if crate::request_body::request_body_replay_active(session)
        && (matches!(e.esource(), ErrorSource::Internal) || matches!(e.etype(), ErrorType::InternalError))
    {
        ctx.record_error(EdgionErrorCode::BodyReplayFailed);
    }

    if code > 0 {
        select_proxy_outcome_status(ctx, e, code);

        if let Some(upstream) = ctx.get_current_upstream_mut() {
            // Preserve a real response status stamped by upstream_response_filter;
            // this code is only the gateway's final outcome for the request.
            upstream.status.get_or_insert(code);
        }
    }

    if code > 0 && code != 499 && !ctx.response_write_started() {
        // gRPC classification uses both the media-type and route-derived facts;
        // a GRPCRoute match must never fall through to Pingora's HTML writer.
        if write_proxy_error_reply(session, ctx, &edgion_http.server_header_opts, code).await {
            // Delivery failed: the downstream connection is dead.
            ctx.record_error(EdgionErrorCode::ClientClosed);
        }
    }

    FailToProxy {
        error_code: code,
        can_reuse_downstream: can_reuse_downstream_after_proxy_error(code),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_proxy_error_to_upstream, can_reuse_downstream_after_proxy_error, classify_proxy_status,
        proxy_error_diagnostic, record_proxy_error_attribution, select_proxy_outcome_status,
        should_append_proxy_error_to_upstream, write_proxy_error_reply,
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
        assert!(!write_proxy_error_reply(&mut session, &mut ctx, &ServerHeaderOpts::default(), 502).await);
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

        assert!(!write_proxy_error_reply(&mut session, &mut ctx, &ServerHeaderOpts::default(), 403,).await);

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

        assert!(!write_proxy_error_reply(&mut session, &mut ctx, &ServerHeaderOpts::default(), 408).await);

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

        assert!(!write_proxy_error_reply(&mut session, &mut ctx, &ServerHeaderOpts::default(), 408).await);

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
}
