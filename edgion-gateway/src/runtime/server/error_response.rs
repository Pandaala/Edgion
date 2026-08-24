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
    resp: ResponseHeader,
    grpc_status: u16,
    grpc_message: Option<&str>,
) -> pingora_core::Result<()> {
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
        let mut resp = resp;
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

    ctx.request_info.grpc_status = Some(grpc_status);

    let mut resp = ResponseHeader::build(200, None)?;
    server_header_opts.apply_to_response(&mut resp);
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
/// Keeps ASCII visible characters as-is except `%` and space (0x20); percent-encodes
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

/// Shared nginx-style HTML error response writer used by every `end_response_<code>` helper.
///
/// Claims final-response ownership, records the selected status, builds the
/// response header with the standard `Content-Type: text/html`, writes the nginx-style
/// body (only `title` varies between status codes), and tears down the session.
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
    title: &str,
) -> pingora_core::Result<()> {
    if !ctx.claim_final_response(status) {
        return Ok(());
    }

    let mut resp = ResponseHeader::build(status, None)?;
    server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html")
        .expect("static ASCII header literal, cannot fail");

    let body = format!(
        "<html>\n<head><title>{title}</title></head>\n<body>\n<center><h1>{title}</h1></center>\n<hr><center>edgion</center>\n</body>\n</html>"
    );

    let resp_box = Box::new(resp);
    session.write_response_header(resp_box, false).await?;
    session.write_response_body(Some(Bytes::from(body)), true).await?;
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
    send_html_error(session, ctx, server_header_opts, 400, "400 Bad Request").await
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
    send_html_error(session, ctx, server_header_opts, 421, "421 Misdirected Request").await
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
    send_html_error(session, ctx, server_header_opts, 404, "404 Not Found").await
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
    send_html_error(session, ctx, server_header_opts, 500, "500 Internal Server Error").await
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
    send_html_error(session, ctx, server_header_opts, 502, "502 Bad Gateway").await
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
/// The `let _ =` at call sites is intentional: a write error on a broken downstream
/// socket is not actionable. Pingora surfaces it via connection state.
pub async fn terminate_with_fallback(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    server_header_opts: &ServerHeaderOpts,
) -> pingora_core::Result<()> {
    if ctx.response_write_started() {
        return Ok(());
    }
    ctx.record_error(EdgionErrorCode::PluginTerminateWithoutResponse);
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
    send_html_error(
        session,
        ctx,
        server_header_opts,
        503,
        "503 Service Temporarily Unavailable",
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EdgionHttpContext;
    use edgion_resources::EdgionErrorCode;

    /// When response ownership is claimed, `terminate_with_fallback` must be a no-op:
    /// it must NOT record `PluginTerminateWithoutResponse` in diagnostics.
    /// This test validates the guard branch directly without needing a live Session.
    #[test]
    fn terminate_with_fallback_noop_when_response_write_started() {
        let mut ctx = EdgionHttpContext::new();
        assert!(ctx.claim_final_response(204));

        // Pre-condition: no errors recorded yet
        assert!(ctx.diagnostics.is_empty());

        // Simulate the guard that terminate_with_fallback checks first
        if !ctx.response_write_started() {
            ctx.record_error(EdgionErrorCode::PluginTerminateWithoutResponse);
        }

        // Because response ownership was claimed, no error should have been added.
        assert!(
            ctx.diagnostics.is_empty(),
            "terminate_with_fallback must not add errors after response ownership is claimed"
        );
        assert!(
            !ctx.diagnostics
                .contains_error(EdgionErrorCode::PluginTerminateWithoutResponse),
            "PluginTerminateWithoutResponse must not appear after response write ownership is claimed"
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

    /// ASCII messages (all current http_to_grpc_status values) must pass through
    /// percent_encode_grpc_message unchanged — no spurious encoding on the happy path.
    #[test]
    fn percent_encode_grpc_message_ascii_passthrough() {
        for msg in &[
            "invalid argument",
            "unauthenticated",
            "permission denied",
            "not found",
            "resource exhausted",
            "cancelled",
            "unimplemented",
            "unavailable",
            "deadline exceeded",
            "internal",
            "unknown",
        ] {
            assert_eq!(
                percent_encode_grpc_message(msg),
                *msg,
                "ASCII grpc-message must not be modified by percent-encoding"
            );
        }
    }

    /// Non-ASCII bytes must be percent-encoded per gRPC spec (RFC 3986 §2.1).
    /// Uses latin-1 chars and a multi-byte CJK character to exercise both
    /// two-byte and three-byte UTF-8 encoding paths.
    #[test]
    fn percent_encode_grpc_message_non_ascii_encoded() {
        // é = U+00E9 → UTF-8 0xC3 0xA9 → "%C3%A9"
        assert_eq!(percent_encode_grpc_message("caf\u{00E9}"), "caf%C3%A9");
        // π = U+03C0 → UTF-8 0xCF 0x80 → "%CF%80"
        assert_eq!(percent_encode_grpc_message("\u{03C0}"), "%CF%80");
        // Mixed: ASCII space kept, non-ASCII encoded
        assert_eq!(
            percent_encode_grpc_message("op\u{00E9}ration invalid"),
            "op%C3%A9ration invalid"
        );
    }

    /// Control characters must be percent-encoded.
    #[test]
    fn percent_encode_grpc_message_control_chars_encoded() {
        assert_eq!(percent_encode_grpc_message("line\nbreak"), "line%0Abreak");
        assert_eq!(percent_encode_grpc_message("tab\there"), "tab%09here");
    }

    /// The percent sign itself (0x25) is the escape introducer and is excluded from the
    /// gRPC unencoded byte set, so it must be encoded as `%25` (matches Envoy's behavior).
    #[test]
    fn percent_encode_grpc_message_percent_encoded() {
        assert_eq!(
            percent_encode_grpc_message("quota 50% exceeded"),
            "quota 50%25 exceeded"
        );
        // A literal percent-escape in the input must be double-encoded, not passed through.
        assert_eq!(percent_encode_grpc_message("%41"), "%2541");
    }
}
