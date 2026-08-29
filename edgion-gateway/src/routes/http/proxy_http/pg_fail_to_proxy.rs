use super::EdgionHttpProxy;
use crate::runtime::server::send_grpc_status_trailers_only;
use crate::runtime::server::server_header::ServerHeaderOpts;
use crate::send_grpc_trailers_only;
use crate::{EdgionHttpContext, ResponseStageDenial};
use edgion_resources::EdgionErrorCode;
use pingora_core::{ConnectionClosed, Error, ErrorSource, ErrorType, HTTPStatus, ReadError, WriteError};
use pingora_proxy::{FailToProxy, Session};

#[inline]
fn classify_proxy_status(e: &Error) -> u16 {
    match e.etype() {
        HTTPStatus(code) => *code,
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
    // skills/04-review/architecture/response-local-reply-h1-close-is-feature.md.
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

    // The proxy-ending error was a downstream failure — the client is gone.
    // Recorded in the request-level diagnostics so access-log analysis can
    // join it with the upstreams array to quantify retry work spent on
    // callers that had already disconnected. HTTPStatus(499) is excluded:
    // that is an upstream's literal 499 response, not a client disconnect.
    if code == 499 && !matches!(e.etype(), HTTPStatus(_)) {
        ctx.record_error(EdgionErrorCode::ClientClosed);
    }

    // A gateway-internal error that surfaced while the captured request body
    // was being replayed to the upstream is a body-replay failure (buffer
    // invariant violation or spill-storage read error). `upstream.err` alone
    // would show only the indistinguishable "InternalError", so record a
    // dedicated status; the error-log line (join on x-trace-id) carries the
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
            if code != 504 && code != 499 {
                upstream.err.push(e.etype().as_str().to_string());
            }
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
        can_reuse_downstream: matches!(code, 400 | 403 | 404 | 429),
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_proxy_status, select_proxy_outcome_status, write_proxy_error_reply};
    use crate::routes::http::proxy_http::pg_upstream_response_filter::select_normalized_upstream_candidate;
    use crate::runtime::server::ServerHeaderOpts;
    use crate::EdgionHttpContext;
    use pingora_core::{ConnectionClosed, Error};
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
        assert_eq!(ctx.request_info.grpc_status, Some(7));
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
