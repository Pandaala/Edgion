use super::EdgionHttpProxy;
use crate::core::gateway::observe::AccessLogEntry;
use crate::core::gateway::tls::validation::backend_san::BackendSanValidationResult;
use crate::types::EdgionHttpContext;
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::Digest;
use pingora_proxy::Session;

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
                    tracing::error!(
                        reason = %san_result.reason,
                        "Backend TLS certificate SAN validation failed, rejecting connection"
                    );
                    ctx.add_error(crate::types::EdgionStatus::BackendTlsSanValidationFailed);
                    return Err(pingora_core::Error::explain(
                        pingora_core::ErrorType::InvalidCert,
                        format!("Backend cert SAN mismatch: {}", san_result.reason),
                    ));
                }
                tracing::debug!("Backend TLS certificate SAN validation passed");
            }
        }
    }

    // Record connection time (time from start_time to connection established)
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        let ct = upstream.start_time.elapsed().as_millis() as u64;
        upstream.ct = Some(ct);

        if let (Some(service_key), Some(addr), Some(crate::types::ParsedLBPolicy::LeastConn)) = (
            upstream.service_key.as_deref(),
            upstream.lb_backend_addr.as_ref(),
            &upstream.lb_policy,
        ) {
            crate::core::gateway::lb::runtime_state::increment(service_key, addr);
        }
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
