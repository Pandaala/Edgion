use super::EdgionHttp;
use crate::core::services::acme::gw::challenge_store::get_global_challenge_store;
use crate::types::EdgionHttpContext;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;

/// ACME HTTP-01 challenge path prefix
const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

#[inline]
pub async fn early_request_filter(
    edgion_http: &EdgionHttp,
    session: &mut Session,
    _ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // ACME HTTP-01 challenge check (fast bypass when no active challenges)
    try_serve_acme_challenge(session).await?;

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
        tracing::debug!("Process shutting down, disabling keepalive for new request");
    } else {
        session.set_keepalive(Some(client_timeout.keepalive_timeout));
    }

    Ok(())
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

    tracing::info!(token = token, host = host, "Serving ACME HTTP-01 challenge response");

    let mut resp = ResponseHeader::build(200, None)?;
    resp.insert_header("Content-Type", "text/plain")?;
    resp.insert_header("Content-Length", key_auth.len().to_string())?;

    session.write_response_header(Box::new(resp), false).await?;
    session
        .write_response_body(Some(bytes::Bytes::from(key_auth)), true)
        .await?;

    // Short-circuit: response already sent, no further proxy processing needed
    Err(pingora_core::Error::explain(
        pingora_core::ErrorType::HTTPStatus(200),
        "ACME challenge response sent",
    ))
}
