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
                    ctx.add_error(crate::types::EdgionStatus::BackendTlsSanValidationFailed);
                    return Err(pingora_core::Error::explain(
                        pingora_core::ErrorType::InvalidCert,
                        format!("Backend cert SAN mismatch: {}", san_result.reason),
                    ));
                }
            }
        }
    }

    // Record connection time (time from start_time to connection established).
    // P0-07: upstream borrow ends at block exit so we can write ctx fields after.
    // g1-007: also record the EWMA backend + attempt start so logging only
    // updates EWMA for attempts that actually completed `connected_to_upstream`.
    use std::sync::Arc;
    enum LbCleanup {
        LeastConn(Arc<crate::core::gateway::lb::EdgionBackend>),
        Ewma(Arc<crate::core::gateway::lb::EdgionBackend>, std::time::Instant),
    }
    let lb_cleanup = if let Some(upstream) = ctx.get_current_upstream_mut() {
        let ct = upstream.start_time.elapsed().as_millis() as u64;
        upstream.ct = Some(ct);

        match (&upstream.lb_edgion_backend, &upstream.lb_policy) {
            (Some(backend), Some(crate::types::ParsedLBPolicy::LeastConn)) => {
                backend.inc_conn();
                Some(LbCleanup::LeastConn(backend.clone()))
            }
            (Some(backend), Some(crate::types::ParsedLBPolicy::Ewma)) => {
                Some(LbCleanup::Ewma(backend.clone(), upstream.start_time))
            }
            _ => None,
        }
    } else {
        None
    };
    // upstream borrow dropped here — safe to write ctx
    match lb_cleanup {
        Some(LbCleanup::LeastConn(backend)) => {
            ctx.leastconn_incremented_backend = Some(backend);
        }
        Some(LbCleanup::Ewma(backend, start)) => {
            ctx.last_ewma_backend = Some((backend, start));
        }
        None => {}
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
