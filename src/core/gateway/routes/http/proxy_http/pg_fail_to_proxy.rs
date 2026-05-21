use super::EdgionHttpProxy;
use crate::core::gateway::send_grpc_error_three_frame;
use crate::types::EdgionHttpContext;
use pingora_core::{ConnectionClosed, Error, ErrorSource, ErrorType, HTTPStatus, ReadError, WriteError};
use pingora_proxy::{FailToProxy, Session};
#[inline]
pub async fn fail_to_proxy(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    e: &Error,
    ctx: &mut EdgionHttpContext,
) -> FailToProxy {
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
            // send_grpc_error_three_frame uses http_to_grpc_status internally
            // (e.g. 502->Internal, 504->DeadlineExceeded, 500->Internal).
            let _ = send_grpc_error_three_frame(session, ctx, &edgion_http.server_header_opts, code).await;
        } else {
            let mut resp = pingora_core::protocols::http::ServerSession::generate_error(code);
            edgion_http.server_header_opts.apply_to_response(&mut resp);
            session
                .write_error_response(resp, bytes::Bytes::default())
                .await
                .unwrap_or_else(|_| {});
        }
    }

    FailToProxy {
        error_code: code,
        can_reuse_downstream: matches!(code, 400 | 403 | 404 | 429),
    }
}
