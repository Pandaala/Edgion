use pingora_core::Error as PingoraError;
use pingora_proxy::Session;
use crate::types::EdgionHttpContext;
use crate::core::observe::AccessLogEntry;
use super::EdgionHttp;

#[inline]
pub async fn logging(
    edgion_http: &EdgionHttp,
    session: &mut Session,
    _e: Option<&PingoraError>,
    ctx: &mut EdgionHttpContext
) {
    // Update LB metrics based on policy type
    if let Some(upstream) = ctx.get_current_upstream() {
        if let Some(addr) = &upstream.backend_addr {
            match &upstream.lb_policy {
                Some(crate::types::ParsedLBPolicy::LeastConn) => {
                    // Decrement connection count for LeastConnection LB
                    crate::core::lb::leastconn::decrement(addr);
                }
                Some(crate::types::ParsedLBPolicy::Ewma) => {
                    // Update EWMA with response latency
                    let latency_us = upstream.start_time.elapsed().as_micros() as u64;
                    crate::core::lb::ewma::update(addr, latency_us);
                }
                _ => {}
            }
        }
    }
    
    // Update response status from session
    if let Some(resp_header) = session.response_written() {
        ctx.request_info.status = Some(resp_header.status.as_u16());
    }

    // Create access log entry
    let entry = AccessLogEntry::from_context(ctx);

    // In DEBUG mode, print access log to terminal
    if tracing::level_filters::LevelFilter::current() >= tracing::level_filters::LevelFilter::DEBUG {
        tracing::debug!(
            access_log = %entry.to_json(),
            "Access log"
        );
    }

    // Send to access logger
    edgion_http.access_logger.send(entry.to_json()).await;
}

