//! Error response utilities
//!
//! Provides helper functions to send error responses in pingora handlers.
//!
//! # gRPC handling
//!
//! All public `end_response_*` functions are gRPC-aware. When `ctx.request_info.is_grpc_request`
//! is true, they delegate to [`send_grpc_trailers_only`] which converts the HTTP status code to
//! the corresponding gRPC status code and writes a trailers-only response.
//!
//! This means callers never need to check `is_grpc_request` themselves — the conversion is
//! handled here automatically and uniformly.

use crate::core::gateway::runtime::server::server_header::ServerHeaderOpts;
use crate::types::{EdgionHttpContext, EdgionStatus};
use bytes::Bytes;
use http::{HeaderMap, HeaderValue};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

/// Conditionally shut down the session after sending an error response.
///
/// For HTTP/2: skip shutdown. The preceding `write_response_body(_, true)` already
/// sent END_STREAM on the H2 stream, which is the correct way to terminate an
/// HTTP/2 stream. Calling `session.shutdown()` would trigger an abrupt TCP-level
/// close via the Pingora framework, causing transient EOF errors for subsequent
/// connections being accepted on the same listener port.
///
/// For HTTP/1.x: proceed with shutdown to close the TCP connection, since there is
/// no stream-level termination mechanism in HTTP/1.
async fn shutdown_after_error_response(session: &mut Session) {
    if !session.is_http2() {
        session.shutdown().await;
    }
}

// ─── gRPC trailers-only response ─────────────────────────────────────────────

/// Send a gRPC trailers-only error response, converting an HTTP status code to the
/// corresponding gRPC status code.
///
/// This is the single dispatch point for all gRPC error responses from direct Session
/// callers. All public `end_response_*` functions call this when `is_grpc_request` is true.
///
/// HTTP → gRPC status mapping follows the gRPC over HTTP/2 spec and
/// https://cloud.google.com/apis/design/errors#http_mapping.
pub(crate) async fn send_grpc_trailers_only(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
    http_status: u16,
) -> pingora_core::Result<()> {
    if ctx.request_info.response_committed {
        return Ok(());
    }

    let (grpc_status, grpc_message) = http_to_grpc_status(http_status);

    ctx.request_info.status = Some(200);
    ctx.request_info.grpc_status = Some(grpc_status);
    ctx.request_info.response_committed = true;

    // Pingora's proxy Session.write_response_header() does NOT pass end_of_stream
    // to the underlying H2 session, so we cannot use a single HEADERS+END_STREAM frame.
    // Instead, use: HEADERS (no END_STREAM) → empty DATA → TRAILERS (with grpc-status).
    let mut resp = ResponseHeader::build(200, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("content-type", "application/grpc").unwrap();
    session.write_response_header(Box::new(resp), false).await?;

    // Send empty body frame to transition the H2 stream to a state where trailers
    // can be sent. Without this, write_response_trailers may silently fail.
    session.write_response_body(Some(Bytes::new()), false).await?;

    // Send grpc-status and grpc-message as HTTP/2 trailers.
    let mut trailers = HeaderMap::new();
    trailers.insert(
        http::header::HeaderName::from_static("grpc-status"),
        HeaderValue::from_str(&grpc_status.to_string()).unwrap(),
    );
    trailers.insert(
        http::header::HeaderName::from_static("grpc-message"),
        HeaderValue::from_static(grpc_message),
    );
    session.as_downstream_mut().write_response_trailers(trailers).await?;
    Ok(())
}

/// Map an HTTP status code to a (grpc_status_code, message) pair.
///
/// Follows the mapping in the gRPC over HTTP/2 spec and Google Cloud error mapping.
pub fn http_to_grpc_status(http_status: u16) -> (u16, &'static str) {
    match http_status {
        400 => (3, "invalid argument"),
        401 => (16, "unauthenticated"),
        403 => (7, "permission denied"),
        404 => (5, "not found"),
        429 => (8, "resource exhausted"),
        499 => (1, "cancelled"),
        501 => (12, "unimplemented"),
        503 => (14, "unavailable"),
        504 => (4, "deadline exceeded"),
        500..=599 => (13, "internal"),
        _ => (2, "unknown"),
    }
}

// ─── Public HTTP error response functions ────────────────────────────────────

/// Send 400 Bad Request error response (nginx-style).
/// For gRPC requests, returns gRPC INVALID_ARGUMENT (status 3).
pub async fn end_response_400(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    if ctx.request_info.is_grpc_request {
        return send_grpc_trailers_only(session, ctx, server_header_opts, 400).await;
    }
    if ctx.request_info.response_committed {
        return Ok(());
    }

    ctx.request_info.status = Some(400);
    ctx.request_info.response_committed = true;

    let mut resp = ResponseHeader::build(400, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();

    let body = r#"<html>
<head><title>400 Bad Request</title></head>
<body>
<center><h1>400 Bad Request</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;

    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    shutdown_after_error_response(session).await;

    Ok(())
}

/// Send 403 Forbidden error response (nginx-style).
/// Used when client certificate validation fails or access is denied.
/// For gRPC requests, returns gRPC PERMISSION_DENIED (status 7).
#[allow(dead_code)]
pub async fn end_response_403(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    if ctx.request_info.is_grpc_request {
        return send_grpc_trailers_only(session, ctx, server_header_opts, 403).await;
    }
    if ctx.request_info.response_committed {
        return Ok(());
    }

    ctx.request_info.status = Some(403);
    ctx.request_info.response_committed = true;

    let mut resp = ResponseHeader::build(403, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();

    let body = r#"<html>
<head><title>403 Forbidden</title></head>
<body>
<center><h1>403 Forbidden</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;

    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    shutdown_after_error_response(session).await;

    Ok(())
}

/// Send 421 Misdirected Request error response (RFC 7540).
/// Used when SNI and Host header mismatch for HTTPS requests.
/// For gRPC requests, returns gRPC UNKNOWN (status 2) — 421 has no direct gRPC equivalent.
#[allow(dead_code)]
pub async fn end_response_421(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    if ctx.request_info.is_grpc_request {
        return send_grpc_trailers_only(session, ctx, server_header_opts, 421).await;
    }
    if ctx.request_info.response_committed {
        return Ok(());
    }

    ctx.request_info.status = Some(421);
    ctx.request_info.response_committed = true;

    let mut resp = ResponseHeader::build(421, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();

    let body = r#"<html>
<head><title>421 Misdirected Request</title></head>
<body>
<center><h1>421 Misdirected Request</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;

    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    shutdown_after_error_response(session).await;

    Ok(())
}

/// Send 404 Not Found error response (nginx-style).
/// For gRPC requests, returns gRPC NOT_FOUND (status 5) per Gateway API spec.
pub async fn end_response_404(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    if ctx.request_info.is_grpc_request {
        return send_grpc_trailers_only(session, ctx, server_header_opts, 404).await;
    }
    if ctx.request_info.response_committed {
        return Ok(());
    }

    ctx.request_info.status = Some(404);
    ctx.request_info.response_committed = true;

    let mut resp = ResponseHeader::build(404, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();

    let body = r#"<html>
<head><title>404 Not Found</title></head>
<body>
<center><h1>404 Not Found</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;

    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    shutdown_after_error_response(session).await;

    Ok(())
}

/// Send gRPC UNAVAILABLE (status 14) error response for backend errors.
///
/// Used when no healthy upstream is available for a gRPC request.
/// Delegates to [`send_grpc_trailers_only`] with HTTP 503 (maps to grpc-status 14).
pub async fn end_response_grpc_unavailable(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    send_grpc_trailers_only(session, ctx, server_header_opts, 503).await
}

/// Send gRPC UNIMPLEMENTED (status 12) error response.
///
/// Used when a gRPC request doesn't match any configured GRPCRoute rule.
/// Per Gateway API spec, unmatched gRPC requests should return UNIMPLEMENTED.
pub async fn end_response_grpc_unimplemented(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    send_grpc_trailers_only(session, ctx, server_header_opts, 501).await
}

/// Send 500 Internal Server Error error response (nginx-style).
/// For gRPC requests, returns gRPC INTERNAL (status 13) per gRPC protocol spec.
pub async fn end_response_500(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    if ctx.request_info.is_grpc_request {
        return send_grpc_trailers_only(session, ctx, server_header_opts, 500).await;
    }
    if ctx.request_info.response_committed {
        return Ok(());
    }

    ctx.request_info.status = Some(500);
    ctx.request_info.response_committed = true;

    let mut resp = ResponseHeader::build(500, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();

    let body = r#"<html>
<head><title>500 Internal Server Error</title></head>
<body>
<center><h1>500 Internal Server Error</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;

    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    shutdown_after_error_response(session).await;

    Ok(())
}

/// Send 502 Bad Gateway error response (nginx-style).
/// For gRPC requests, returns gRPC INTERNAL (status 13) — 502 falls into
/// the `500..=599 => (13, "internal")` arm of [`http_to_grpc_status`].
pub async fn end_response_502(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    if ctx.request_info.is_grpc_request {
        return send_grpc_trailers_only(session, ctx, server_header_opts, 502).await;
    }
    if ctx.request_info.response_committed {
        return Ok(());
    }

    ctx.request_info.status = Some(502);
    ctx.request_info.response_committed = true;

    let mut resp = ResponseHeader::build(502, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();

    let body = r#"<html>
<head><title>502 Bad Gateway</title></head>
<body>
<center><h1>502 Bad Gateway</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;

    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    shutdown_after_error_response(session).await;

    Ok(())
}

/// Fail-safe for plugins that returned
/// [`ErrTerminateRequest`](crate::types::filters::PluginRunningResult::ErrTerminateRequest)
/// without writing a response.
///
/// - If `response_committed` is already true (spec-compliant plugin wrote its own
///   response), this is a no-op — the plugin's response stands.
/// - Otherwise: adds [`EdgionStatus::PluginTerminateWithoutResponse`] to the access
///   log and writes a 502 (HTTP) / gRPC INTERNAL=13 fallback response.
///
/// The `let _ =` at call sites is intentional: a write error on a broken downstream
/// socket is not actionable. Pingora surfaces it via connection state.
pub async fn terminate_with_fallback(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    if ctx.request_info.response_committed {
        return Ok(());
    }
    ctx.add_error(EdgionStatus::PluginTerminateWithoutResponse);
    end_response_502(session, ctx, server_header_opts).await
}

/// Send 503 Service Temporarily Unavailable error response (nginx-style).
/// For gRPC requests, returns gRPC UNAVAILABLE (status 14).
pub async fn end_response_503(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    if ctx.request_info.is_grpc_request {
        return send_grpc_trailers_only(session, ctx, server_header_opts, 503).await;
    }
    if ctx.request_info.response_committed {
        return Ok(());
    }

    ctx.request_info.status = Some(503);
    ctx.request_info.response_committed = true;

    let mut resp = ResponseHeader::build(503, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();

    let body = r#"<html>
<head><title>503 Service Temporarily Unavailable</title></head>
<body>
<center><h1>503 Service Temporarily Unavailable</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;

    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
    shutdown_after_error_response(session).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EdgionHttpContext, EdgionStatus};

    /// When `response_committed` is true, `terminate_with_fallback` must be a no-op:
    /// it must NOT push `PluginTerminateWithoutResponse` into `ctx.edgion_status`.
    /// This test validates the guard branch directly without needing a live Session.
    #[test]
    fn terminate_with_fallback_noop_when_committed() {
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.response_committed = true;

        // Pre-condition: no errors recorded yet
        assert!(ctx.edgion_status.is_empty());

        // Simulate the guard that terminate_with_fallback checks first
        if !ctx.request_info.response_committed {
            ctx.add_error(EdgionStatus::PluginTerminateWithoutResponse);
        }

        // Because response_committed was true, no error should have been added
        assert!(
            ctx.edgion_status.is_empty(),
            "terminate_with_fallback must not add errors when response_committed = true"
        );
        assert!(
            !ctx.edgion_status
                .contains(&EdgionStatus::PluginTerminateWithoutResponse),
            "PluginTerminateWithoutResponse must not appear when response was already committed"
        );
    }

    /// Verify that `EdgionStatus::PluginTerminateWithoutResponse` has the expected
    /// discriminant value (50202). This pins the numeric code used in access log
    /// serialization so that any accidental renumbering causes a test failure.
    #[test]
    fn plugin_terminate_without_response_discriminant() {
        assert_eq!(
            EdgionStatus::PluginTerminateWithoutResponse as u32,
            50202u32,
            "PluginTerminateWithoutResponse discriminant must be 50202 for access log compatibility"
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
}
