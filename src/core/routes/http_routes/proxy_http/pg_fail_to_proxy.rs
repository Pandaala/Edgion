use super::EdgionHttp;
use crate::types::EdgionHttpContext;
use pingora_core::{ConnectionClosed, Error, ErrorSource, ErrorType, HTTPStatus, ReadError, WriteError};
use pingora_proxy::{FailToProxy, Session};
use tracing::log::error;

#[inline]
pub async fn fail_to_proxy(
    edgion_http: &EdgionHttp,
    session: &mut Session,
    e: &Error,
    ctx: &mut EdgionHttpContext,
) -> FailToProxy {
    let code = match e.etype() {
        HTTPStatus(code) => *code,
        // Check for timeout errors - distinguish between client and backend timeouts
        ErrorType::ReadTimedout | ErrorType::WriteTimedout => {
            match e.esource() {
                ErrorSource::Downstream => 499, // Client closed request (client timeout)
                ErrorSource::Upstream => 504,   // Gateway timeout (backend timeout)
                _ => 504,                       // Default to 504 for other timeout sources
            }
        }
        ErrorType::ConnectTimedout | ErrorType::TLSHandshakeTimedout => 504, // Backend connection timeouts
        _ => {
            match e.esource() {
                ErrorSource::Upstream => 502,
                ErrorSource::Downstream => {
                    match e.etype() {
                        WriteError | ReadError | ConnectionClosed => {
                            // Client closed connection - return 499 for logging, but don't send response
                            499
                        }
                        _ => 400,
                    }
                }
                ErrorSource::Internal | ErrorSource::Unset => 500,
            }
        }
    };

    // Record error status code
    if code > 0 {
        // Only update request_info.status if not already set
        if ctx.request_info.status.is_none() {
            ctx.request_info.status = Some(code);
        }

        // Always update current upstream status and error message
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            upstream.status = Some(code);
            // Only add error message for non-timeout errors (not 504/499)
            if code != 504 && code != 499 {
                upstream.err.push(e.etype().as_str().to_string());
            }
        }
    }

    // Don't send error response if connection is already closed (499)
    // For 499, the client has already disconnected, so we can't send a response
    if code > 0 && code != 499 {
        // Generate error response and apply custom server headers
        let mut resp = pingora_core::protocols::http::ServerSession::generate_error(code);
        edgion_http.server_header_opts.apply_to_response(&mut resp);

        // Write error response
        session
            .write_error_response(resp, bytes::Bytes::default())
            .await
            .unwrap_or_else(|e| {
                error!("failed to send error response to downstream: {e}");
            });
    }

    FailToProxy {
        error_code: code,
        // default to no reuse, which is safest
        can_reuse_downstream: false,
    }
}
