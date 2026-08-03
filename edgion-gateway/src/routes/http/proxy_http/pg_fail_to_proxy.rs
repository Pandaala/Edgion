use super::EdgionHttpProxy;
use crate::runtime::server::send_grpc_status_trailers_only;
use crate::send_grpc_trailers_only;
use crate::{EdgionHttpContext, ResponseStageDenial};
use edgion_resources::EdgionStatus;
use pingora_core::{ConnectionClosed, Error, ErrorSource, ErrorType, HTTPStatus, ReadError, WriteError};
use pingora_proxy::{FailToProxy, Session};
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
    // This is a known perf-only divergence from Envoy, which keeps the H1
    // downstream reusable after a clean local reply. See
    // tasks/todo/filter-04c-local-reply-header-carryover.md (#3).
    if ctx.local_reply_written {
        return FailToProxy {
            error_code: ctx.request_info.status.unwrap_or(500),
            can_reuse_downstream: true,
        };
    }

    // A response-stage `ErrGrpcError` the stage itself could not write (the sync
    // runner cannot await). Deliver the plugin's exact gRPC status here, ahead
    // of the generic status handling below, which would otherwise re-derive a
    // code from the synthetic HTTP 200 the abort carries. Marks the response
    // committed, so that generic block then skips this request.
    if let Some(ResponseStageDenial::Grpc { status, message }) = ctx.response_denial.take() {
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
            ctx.add_error(EdgionStatus::ClientClosed);
        }
    }

    let code = match e.etype() {
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
    };

    // The proxy-ending error was a downstream failure — the client is gone.
    // Recorded in the request-level error chain so access-log analysis can
    // join it with the upstreams array to quantify retry work spent on
    // callers that had already disconnected. HTTPStatus(499) is excluded:
    // that is an upstream's literal 499 response, not a client disconnect.
    if code == 499 && !matches!(e.etype(), HTTPStatus(_)) {
        ctx.add_error(EdgionStatus::ClientClosed);
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
        ctx.add_error(EdgionStatus::BodyReplayFailed);
    }

    if code > 0 {
        if ctx.request_info.status.is_none() {
            ctx.request_info.status = Some(code);
        }

        if let Some(upstream) = ctx.get_current_upstream_mut() {
            upstream.status = Some(code);
            if code != 504 && code != 499 {
                upstream.err.push(e.etype().as_str().to_string());
            }
        }
    }

    if code > 0 && code != 499 && !ctx.request_info.response_committed {
        if ctx.request_info.is_grpc_request {
            // Send proper gRPC error with correct status mapping instead of an HTML error page.
            // send_grpc_trailers_only uses http_to_grpc_status internally
            // (e.g. 502->Internal, 504->DeadlineExceeded, 500->Internal).
            if send_grpc_trailers_only(session, ctx, &edgion_http.server_header_opts, code)
                .await
                .is_err()
            {
                // Delivery failed: the downstream connection is dead.
                ctx.add_error(EdgionStatus::ClientClosed);
            }
        } else {
            let mut resp = pingora_core::protocols::http::ServerSession::generate_error(code);
            edgion_http.server_header_opts.apply_to_response(&mut resp);
            if session
                .write_error_response(resp, bytes::Bytes::default())
                .await
                .is_err()
            {
                // Delivery failed: the downstream connection is dead.
                ctx.add_error(EdgionStatus::ClientClosed);
            }
        }
    }

    FailToProxy {
        error_code: code,
        can_reuse_downstream: matches!(code, 400 | 403 | 404 | 429),
    }
}
