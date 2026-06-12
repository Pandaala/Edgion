use super::EdgionHttpProxy;
use crate::send_grpc_trailers_only;
use crate::EdgionHttpContext;
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
    // does not consult this field and tears the downstream connection down anyway.
    // So in practice the connection is CLOSED after the complete, correct reply.
    // Known perf-only divergence from Envoy, which keeps the downstream reusable
    // for a clean local reply. See
    // tasks/todo/filter-04c-local-reply-header-carryover.md (#3).
    if ctx.local_reply_written {
        return FailToProxy {
            error_code: ctx.request_info.status.unwrap_or(500),
            can_reuse_downstream: true,
        };
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
