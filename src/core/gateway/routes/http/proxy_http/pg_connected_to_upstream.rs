use super::EdgionHttpProxy;
use crate::core::gateway::observe::AccessLogEntry;
use crate::core::gateway::tls::validation::backend_san::BackendSanValidationResult;
use crate::types::EdgionHttpContext;
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::Digest;
use pingora_proxy::Session;
use std::time::Instant;

#[inline]
pub async fn connected_to_upstream(
    edgion_http: &EdgionHttpProxy,
    _session: &mut Session,
    _reused: bool,
    _peer: &HttpPeer,
    #[cfg(unix)] _fd: std::os::unix::io::RawFd,
    #[cfg(windows)] _sock: std::os::windows::io::RawSocket,
    digest: Option<&Digest>,
    ctx: &mut EdgionHttpContext,
) -> pingora_core::Result<()> {
    // Check backend SAN validation result from TLS handshake hook.
    // Connection reuse safety: group_key isolation in create_tls_peer() ensures
    // reused connections always share the same SAN policy validation result.
    if let Some(digest) = digest {
        if let Some(ssl_digest) = &digest.ssl_digest {
            if let Some(san_result) = ssl_digest.extension.get::<BackendSanValidationResult>() {
                if !san_result.passed {
                    ctx.add_error(crate::types::EdgionStatus::BackendTlsSanValidationFailed);
                    return Err(pingora_core::Error::explain(
                        pingora_core::ErrorType::InvalidCert,
                        format!("Backend cert SAN mismatch: {}", san_result.reason),
                    ));
                }
            }
        }
    }

    // Record connection time and commit LB per-attempt state.
    // Both read and write are on the same upstream entry — no borrow escape needed.
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        let ct = Instant::now()
            .saturating_duration_since(upstream.start_time)
            .as_millis() as u64;
        upstream.ct = Some(ct);
        crate::core::gateway::lb::commit_attempt(upstream);
    }

    // For gRPC-Web or WebSocket, log connection establishment immediately
    if let Some(protocol) = &ctx.request_info.discover_protocol {
        if protocol == "grpc-web" || protocol == "websocket" {
            let mut entry = AccessLogEntry::from_context(ctx);
            entry.set_conn_est();

            // Send to access logger
            edgion_http.access_logger.send(entry.to_json()).await;
        }
    }

    Ok(())
}
