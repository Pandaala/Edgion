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
use crate::types::EdgionHttpContext;
use bytes::Bytes;
use http::{HeaderMap, HeaderValue};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

// ─── gRPC trailers-only response ─────────────────────────────────────────────

/// Send a gRPC trailers-only error response, converting an HTTP status code to the
/// corresponding gRPC status code.
///
/// This is the single dispatch point for all gRPC error responses from direct Session
/// callers. All public `end_response_*` functions call this when `is_grpc_request` is true.
///
/// HTTP → gRPC status mapping follows the gRPC over HTTP/2 spec and
/// https://cloud.google.com/apis/design/errors#http_mapping.
async fn send_grpc_trailers_only(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
    http_status: u16,
) -> pingora_core::Result<()> {
    let (grpc_status, grpc_message) = http_to_grpc_status(http_status);

    ctx.request_info.status = Some(200);

    // Step 1: Send response headers with :status 200 and content-type.
    // grpc-status and grpc-message must NOT go here — they must be in HTTP/2 trailers.
    // Pingora ignores end_of_stream on write_response_header for H2 (always sends without
    // END_STREAM), so we use a separate trailers frame to properly close the stream.
    let mut resp = ResponseHeader::build(200, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("content-type", "application/grpc").unwrap();
    session.write_response_header(Box::new(resp), false).await?;

    // Step 2: Send grpc-status and grpc-message as HTTP/2 trailers (HEADERS frame with
    // END_STREAM). Per gRPC-over-HTTP/2 spec, the status must be in trailers, not in
    // the initial response headers, unless the response is a trailers-only frame
    // (HEADERS + END_STREAM). Since Pingora cannot set END_STREAM on response headers,
    // we use the two-frame pattern: response headers + trailer headers with END_STREAM.
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

    ctx.request_info.status = Some(400);

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
    session.shutdown().await;

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

    ctx.request_info.status = Some(403);

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
    session.shutdown().await;

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

    ctx.request_info.status = Some(421);

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
    session.shutdown().await;

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

    ctx.request_info.status = Some(404);

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
    session.shutdown().await;

    Ok(())
}

/// Send gRPC UNAVAILABLE (status 14) error response for backend errors.
///
/// Used when no healthy upstream is available for a gRPC request.
pub async fn end_response_grpc_unavailable(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    ctx.request_info.status = Some(200);

    let mut resp = ResponseHeader::build(200, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "application/grpc").unwrap();
    resp.insert_header("grpc-status", "14").unwrap();
    resp.insert_header("grpc-message", "no healthy upstream").unwrap();

    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, true).await?;

    Ok(())
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

    ctx.request_info.status = Some(500);

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
    session.shutdown().await;

    Ok(())
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

    ctx.request_info.status = Some(503);

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
    session.shutdown().await;

    Ok(())
}
