//! Error response utilities
//!
//! Provides helper functions to send error responses in pingora handlers.
//!
//! # gRPC handling
//!
//! All public `end_response_*` functions are gRPC-aware. When `ctx.request_info.is_grpc_request`
//! is true, they delegate to [`send_grpc_trailers_only`] which converts the HTTP status code to
//! the corresponding gRPC status code and writes a single-frame Trailers-Only gRPC error response.
//!
//! This means callers never need to check `is_grpc_request` themselves — the conversion is
//! handled here automatically and uniformly.

use crate::runtime::server::server_header::ServerHeaderOpts;
use crate::EdgionHttpContext;
use bytes::Bytes;
use edgion_resources::EdgionErrorCode;
use http::HeaderMap;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use pingora_core::modules::http::grpc_web::GrpcWebBridge;
use pingora_core::protocols::http::bridge::grpc_web::GrpcWebCtx;
use pingora_core::protocols::http::HttpTask;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

/// Conditionally shut down the session after sending an error response.
///
/// For HTTP/1.x: proceed with shutdown to close the TCP connection, since there is
/// no stream-level termination mechanism in HTTP/1. Pingora's H1 arm of
/// `HttpSession::shutdown` resolves to `underlying_stream.shutdown()`.
///
/// For HTTP/2: skip. The preceding `write_response_body(_, true)` already sent
/// END_STREAM on the H2 stream, which is the correct way to terminate it. The skip
/// documents that intent; it is not load-bearing. Pingora's H2 arm of
/// `HttpSession::shutdown` never touches TCP — it only sends
/// `RST_STREAM(INTERNAL_ERROR)`, and only when the stream has not already ended —
/// so on this path the call would be a no-op either way.
///
/// `PluginSession::shutdown` (`plugins/runtime/session_adapter.rs`) deliberately
/// does NOT mirror this `is_http2()` gate; see that function for why the divergence
/// is intentional.
async fn shutdown_after_error_response(session: &mut Session) {
    if !session.is_http2() {
        session.shutdown().await;
    }
}

// ─── gRPC error response (Trailers-Only single frame) ────────────────────────

/// Send a single-frame gRPC Trailers-Only response: one HEADERS frame with END_STREAM set,
/// carrying content-type (already on `resp`) plus grpc-status / grpc-message as ordinary
/// header fields. Uses `write_response_tasks` so the `end=true` flag reaches the H2 layer —
/// the plain `Session::write_response_header` drops `end_of_stream` before the H2 codec.
pub(crate) async fn send_grpc_trailers_only_frame(
    session: &mut Session,
    mut resp: ResponseHeader,
    grpc_status: u16,
    grpc_message: Option<&str>,
) -> pingora_core::Result<()> {
    // Terminal status fields belong exclusively to this writer. Queued plugin
    // values must not conflict with gRPC-Web's trailer frame or survive a
    // native trailers-only reply whose canonical message is absent.
    resp.remove_header("grpc-status");
    resp.remove_header("grpc-message");
    // Read-only check of the grpc-web bridge state. For native gRPC the bridge is never
    // init()'d, so it stays Disabled (!= Upgrade) and we take the unchanged native path
    // below. Reading via get_mut + == does not mutate the bridge; the borrow is dropped
    // before write_response_tasks.
    let grpc_web_active = session
        .downstream_modules_ctx
        .get_mut::<GrpcWebBridge>()
        .is_some_and(|b| **b == GrpcWebCtx::Upgrade);

    if grpc_web_active {
        // grpc-web: keep content-type = application/grpc on resp (the bridge's
        // response_header_filter converts it to grpc-web and sets chunked, Upgrade ->
        // Trailers). grpc-status / grpc-message go into the trailer, which the bridge's
        // response_trailer_filter encodes into a 0x80 body frame (Trailers -> Done).
        let mut trailers = HeaderMap::new();
        trailers.insert(
            "grpc-status",
            grpc_status
                .to_string()
                .parse()
                .expect("u16 decimal string is always a valid HeaderValue"),
        );
        if let Some(msg) = grpc_message {
            trailers.insert(
                "grpc-message",
                percent_encode_grpc_message(msg)
                    .parse()
                    .expect("percent-encoded grpc-message contains only ASCII, cannot fail"),
            );
        }
        session
            .write_response_tasks(vec![
                HttpTask::Header(Box::new(resp), false),
                HttpTask::Trailer(Some(Box::new(trailers))),
            ])
            .await?;
    } else {
        // Native gRPC (and non-grpc): unchanged single-frame Trailers-Only — grpc-status /
        // grpc-message as ordinary header fields in one HEADERS frame with END_STREAM=true.
        resp.insert_header("grpc-status", grpc_status.to_string())
            .expect("u16 decimal string is always a valid HeaderValue");
        if let Some(msg) = grpc_message {
            resp.insert_header("grpc-message", percent_encode_grpc_message(msg))
                .expect("percent-encoded grpc-message contains only ASCII, cannot fail");
        }
        session
            .write_response_tasks(vec![HttpTask::Header(Box::new(resp), true)])
            .await?;
    }
    Ok(())
}

/// Send a gRPC error response as a single-frame Trailers-Only response (HEADERS+END_STREAM),
/// converting an HTTP status code to the corresponding gRPC status code.
///
/// This is the single dispatch point for all gRPC error responses from direct Session callers.
/// All public `end_response_*` functions call this when `is_grpc_request` is true.
///
/// HTTP -> gRPC status mapping follows the gRPC over HTTP/2 spec and
/// https://cloud.google.com/apis/design/errors#http_mapping.
pub(crate) async fn send_grpc_trailers_only(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
    http_status: u16,
) -> pingora_core::Result<()> {
    let (grpc_status, grpc_message) = http_to_grpc_status(http_status);
    send_grpc_status_trailers_only(session, ctx, server_header_opts, grpc_status, Some(grpc_message)).await
}

/// Same wire shape as [`send_grpc_trailers_only`], but the gRPC status is given
/// directly instead of being derived from an HTTP status.
///
/// Used by the response-stage denial path: a plugin's `ErrGrpcError` names the
/// gRPC code itself, and there is no HTTP status that round-trips through
/// [`http_to_grpc_status`] back to an arbitrary code — routing it through one
/// would silently flatten, say, `RESOURCE_EXHAUSTED` into `INTERNAL`.
pub(crate) async fn send_grpc_status_trailers_only(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
    grpc_status: u16,
    grpc_message: Option<&str>,
) -> pingora_core::Result<()> {
    if !ctx.claim_final_response(200) {
        return Ok(());
    }

    ctx.request_info.set_grpc_status(grpc_status);

    let mut resp = ResponseHeader::build(200, None)?;
    server_header_opts.apply_to_response(&mut resp);
    // Response-stage plugin mutations target this fresh local reply, not the
    // discarded upstream response. Apply them after gateway defaults so
    // conditional operations observe the reply's real initial header set.
    ctx.drain_pending_response_header_ops(&mut resp);
    ctx.request_info.trace_id.reconcile_response(&mut resp);
    // The local-reply queue is shared by all plugins. Reassert the mandatory
    // trailers-only framing and media type even if a non-ExtProc producer
    // queued an unsafe operation.
    resp.remove_header(&http::header::CONTENT_LENGTH);
    resp.remove_header(&http::header::TRANSFER_ENCODING);
    resp.insert_header("content-type", "application/grpc")
        .expect("static ASCII header literal, cannot fail");
    send_grpc_trailers_only_frame(session, resp, grpc_status, grpc_message).await
}

/// ASCII byte set for gRPC `grpc-message` percent-encoding.
///
/// The gRPC over HTTP/2 spec only permits unencoded bytes `%x20-%x24` and `%x26-%x7E`.
/// `%x25` (`%`) is the percent-encoding escape introducer (RFC 3986 §2.1) and is excluded
/// from the unencoded set, so it must itself be encoded as `%25` — otherwise a message such
/// as `quota 50% exceeded` becomes ambiguous on the wire.
///
/// This set governs only ASCII bytes: `CONTROLS` (C0 controls 0x00-0x1F and DEL 0x7F) plus
/// `%`. Non-ASCII bytes (0x80-0xFF) are always encoded by `utf8_percent_encode` regardless of
/// the set. The result keeps exactly `%x20-%x24` and `%x26-%x7E` unencoded — matching the gRPC
/// grammar (and Envoy's behavior).
const GRPC_MESSAGE_ASCII_SET: &AsciiSet = &CONTROLS.add(b'%');

/// Percent-encode a gRPC message string per gRPC over HTTP/2 spec (RFC 3986 §2.1).
///
/// Keeps ASCII visible characters and space (0x20) as-is except `%`; percent-encodes
/// control characters, `%`, and non-ASCII bytes. Matches the encoding used by
/// `session_adapter::send_grpc_error_direct` for consistency across gRPC error paths.
pub(crate) fn percent_encode_grpc_message(s: &str) -> String {
    utf8_percent_encode(s, GRPC_MESSAGE_ASCII_SET).to_string()
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

// These fixed rejection pages are compiled once rather than formatted per response.
macro_rules! html_error_body {
    ($title:literal) => {
        concat!(
            "<html>\n<head><title>",
            $title,
            "</title></head>\n<body>\n<center><h1>",
            $title,
            "</h1></center>\n<hr><center>edgion</center>\n</body>\n</html>"
        )
        .as_bytes()
    };
}

/// Shared nginx-style HTML error response writer used by every `end_response_<code>` helper.
///
/// Claims final-response ownership, records the selected status, builds the
/// response header with the standard `Content-Type: text/html`, writes the nginx-style
/// static body, and tears down the session.
///
/// gRPC-aware delegation is intentionally NOT performed here: each caller checks
/// `ctx.request_info.is_grpc_request` first and routes to [`send_grpc_trailers_only`]
/// before reaching this helper. Keeping the gRPC branch at the call site preserves the
/// per-helper doc comments that describe the gRPC mapping for each HTTP status.
async fn send_html_error(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
    status: u16,
    body: &'static [u8],
) -> pingora_core::Result<()> {
    if !ctx.claim_final_response(status) {
        return Ok(());
    }

    let mut resp = ResponseHeader::build(status, None)?;
    server_header_opts.apply_to_response(&mut resp);
    ctx.drain_pending_response_header_ops(&mut resp);
    ctx.request_info.trace_id.reconcile_response(&mut resp);
    resp.remove_header(&http::header::CONTENT_LENGTH);
    resp.remove_header(&http::header::TRANSFER_ENCODING);
    resp.insert_header("Content-Type", "text/html")
        .expect("static ASCII header literal, cannot fail");
    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session
        .write_response_body(Some(Bytes::from_static(body)), true)
        .await?;
    shutdown_after_error_response(session).await;

    Ok(())
}

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
    send_html_error(
        session,
        ctx,
        server_header_opts,
        400,
        html_error_body!("400 Bad Request"),
    )
    .await
}

/// Send 421 Misdirected Request error response (RFC 7540).
/// Used when SNI and Host header mismatch for HTTPS requests.
/// For gRPC requests, returns gRPC UNKNOWN (status 2) — 421 has no direct gRPC equivalent.
pub async fn end_response_421(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    if ctx.request_info.is_grpc_request {
        return send_grpc_trailers_only(session, ctx, server_header_opts, 421).await;
    }
    send_html_error(
        session,
        ctx,
        server_header_opts,
        421,
        html_error_body!("421 Misdirected Request"),
    )
    .await
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
    send_html_error(session, ctx, server_header_opts, 404, html_error_body!("404 Not Found")).await
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
    send_html_error(
        session,
        ctx,
        server_header_opts,
        500,
        html_error_body!("500 Internal Server Error"),
    )
    .await
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
    send_html_error(
        session,
        ctx,
        server_header_opts,
        502,
        html_error_body!("502 Bad Gateway"),
    )
    .await
}

/// Fail-safe for plugins that returned
/// [`ErrTerminateRequest`](edgion_resources::filters::PluginRunningResult::ErrTerminateRequest)
/// without writing a response.
///
/// - If final-response ownership is already claimed (spec-compliant plugin wrote its own
///   response), this is a no-op — the plugin's response stands.
/// - Otherwise: adds [`EdgionErrorCode::PluginTerminateWithoutResponse`] to the access
///   log and writes a 502 (HTTP) / gRPC INTERNAL=13 fallback response.
///
/// Call sites may discard the returned write error because this function records
/// the independent downstream delivery failure before returning it.
pub async fn terminate_with_fallback(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    if ctx.response_write_started() {
        return Ok(());
    }
    ctx.record_error(EdgionErrorCode::PluginTerminateWithoutResponse);
    let result = end_response_502(session, ctx, server_header_opts).await;
    if result.is_err() {
        ctx.record_error(EdgionErrorCode::ClientClosed);
    }
    result
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
    send_html_error(
        session,
        ctx,
        server_header_opts,
        503,
        html_error_body!("503 Service Temporarily Unavailable"),
    )
    .await
}

#[cfg(test)]
#[path = "../../../tests/unit/runtime/server/error_response_test.rs"]
mod tests;
