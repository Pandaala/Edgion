use super::EdgionHttpProxy;
use crate::core::gateway::cli::config::get_path_normalization_config;
use crate::core::gateway::path_normalization::{contains_escaped_slash, has_invalid_percent_encoding};
use crate::core::gateway::services::acme::challenge_store::get_global_challenge_store;
use crate::types::EdgionHttpContext;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

/// ACME HTTP-01 challenge path prefix
const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

#[inline]
pub async fn early_request_filter(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // ACME HTTP-01 challenge check (fast bypass when no active challenges)
    try_serve_acme_challenge(session).await?;

    // Path normalization entry (spec §3.1):
    // 1. Snapshot raw path and query from the incoming URI
    // 2. Apply reject_* policy checks (independent of normalization)
    // 3. Freeze entry view (entry_raw_path / entry_raw_query)
    // 4. Compute normalized current path cache
    let raw_path = session.req_header().uri.path().to_string();
    let raw_query: Option<String> = session.req_header().uri.query().map(str::to_string);

    let path_norm_cfg = get_path_normalization_config();

    if path_norm_cfg.reject_escaped_slashes && contains_escaped_slash(&raw_path) {
        return reject_400(session, edgion_http, "escaped slash in path rejected").await;
    }

    if path_norm_cfg.reject_invalid_percent_encoding && has_invalid_percent_encoding(&raw_path) {
        return reject_400(session, edgion_http, "invalid percent-encoding in path rejected").await;
    }

    ctx.request_info.entry_raw_path = raw_path;
    ctx.request_info.entry_raw_query = raw_query;
    ctx.refresh_normalized_path(session.req_header(), path_norm_cfg);

    // Set client timeouts from pre-parsed config (no runtime overhead)
    let client_timeout = &edgion_http.parsed_timeouts.client;
    session.set_read_timeout(Some(client_timeout.read_timeout));
    session.set_write_timeout(Some(client_timeout.write_timeout));

    // Handle graceful shutdown:
    // If the process is shutting down, disable keepalive for this connection.
    // This prevents the client from trying to reuse this connection for subsequent requests,
    // which helps in draining traffic away from this instance.
    if session.is_process_shutting_down() {
        session.set_keepalive(None);
    } else {
        session.set_keepalive(Some(client_timeout.keepalive_timeout));
    }

    Ok(())
}

/// Send a 400 Bad Request response and short-circuit the pipeline.
///
/// Used when path normalization reject policies are triggered
/// (`reject_escaped_slashes`, `reject_invalid_percent_encoding`).
/// Mirrors the ACME short-circuit pattern: write the response body then
/// return `Err(HTTPStatus(400))` so Pingora stops further processing.
async fn reject_400(
    session: &mut Session,
    edgion_http: &EdgionHttpProxy,
    reason: &'static str,
) -> pingora_core::Result<()> {
    let mut resp = ResponseHeader::build(400, None)?;
    edgion_http.server_header_opts.apply_to_response(&mut resp);
    resp.insert_header("Content-Type", "text/html").unwrap();

    let body = r#"<html>
<head><title>400 Bad Request</title></head>
<body>
<center><h1>400 Bad Request</h1></center>
<hr><center>edgion</center>
</body>
</html>"#;

    session.write_response_header(Box::new(resp), false).await?;
    session
        .write_response_body(Some(bytes::Bytes::from(body)), true)
        .await?;

    Err(pingora_core::Error::explain(
        pingora_core::ErrorType::HTTPStatus(400),
        reason,
    ))
}

/// Serve ACME HTTP-01 challenge response if there is an active challenge matching the request.
///
/// Fast path: `is_empty()` check on the global store ensures zero overhead when
/// no certificate issuance/renewal is in progress (99.99% of the time).
/// Only when the Controller pushes challenge tokens via gRPC will this path activate.
///
/// Returns `Err` with `HTTPStatus(200)` to short-circuit the proxy pipeline after
/// sending the challenge response directly to the ACME server.
#[inline]
async fn try_serve_acme_challenge(session: &mut Session) -> pingora_core::Result<()> {
    let store = get_global_challenge_store();
    if store.is_empty() {
        return Ok(());
    }

    let path = session.req_header().uri.path();
    let token = match path.strip_prefix(ACME_CHALLENGE_PREFIX) {
        Some(t) if !t.is_empty() => t,
        _ => return Ok(()),
    };

    // Extract Host header (strip port) for domain validation
    let host = session
        .req_header()
        .headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");

    let key_auth = match store.lookup(token, host) {
        Some(v) => v,
        None => return Ok(()),
    };

    let mut resp = ResponseHeader::build(200, None)?;
    resp.insert_header("Content-Type", "text/plain")?;
    resp.insert_header("Content-Length", key_auth.len().to_string())?;

    session.write_response_header(Box::new(resp), false).await?;
    session.write_response_body(Some(key_auth), true).await?;

    // Short-circuit: response already sent, no further proxy processing needed
    Err(pingora_core::Error::explain(
        pingora_core::ErrorType::HTTPStatus(200),
        "ACME challenge response sent",
    ))
}
