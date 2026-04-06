use super::EdgionHttpProxy;
use crate::types::EdgionHttpContext;
use pingora_core::prelude::HttpPeer;
use pingora_core::Error;
use pingora_proxy::Session;

#[inline]
pub fn error_while_proxy(
    edgion_http: &EdgionHttpProxy,
    peer: &HttpPeer,
    session: &mut Session,
    e: Box<Error>,
    ctx: &mut EdgionHttpContext,
    client_reused: bool,
) -> Box<Error> {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.set_response_body_size(session.upstream_body_bytes_received());
    }
    let mut e = e.more_context(format!("Peer: {}", peer));

    // Record circuit breaker failure (for non-status-retry errors only)
    if !ctx.retry_on_status {
        if let Some(upstream) = ctx.get_current_upstream() {
            if let (Some(service_key), Some(addr)) = (&upstream.service_key, &upstream.lb_backend_addr) {
                crate::core::gateway::backends::get_circuit_breaker_store().record_failure_by_addr(service_key, addr);
            }
        }
    }

    let buffer_ok = !session.as_ref().retry_buffer_truncated();

    // Case 1: status code triggered retry (set by upstream_response_filter)
    // Retry budget and timeout already checked in upstream_response_filter
    if ctx.retry_on_status && buffer_ok {
        ctx.retry_on_status = false; // reset for next attempt
        e.set_retry(true);
        return e;
    }

    // Case 2: connection reuse retry (Pingora built-in: stale keepalive connection)
    // Add max_retries and timeout guard
    let max_retries = ctx
        .route_unit
        .as_ref()
        .and_then(|unit| unit.rule.parsed_retry.as_ref().and_then(|r| r.attempts))
        .or_else(|| {
            ctx.grpc_route_unit
                .as_ref()
                .and_then(|unit| unit.rule.parsed_retry.as_ref().and_then(|r| r.attempts))
        })
        .unwrap_or(edgion_http.edgion_gateway_config.spec.max_retries);

    let timeout_exceeded = ctx
        .route_unit
        .as_ref()
        .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
        .or_else(|| {
            ctx.grpc_route_unit
                .as_ref()
                .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
        })
        .and_then(|t| t.request_timeout.as_duration())
        .map(|timeout| ctx.start_time.elapsed() >= timeout)
        .unwrap_or(false);

    let can_retry = client_reused && buffer_ok && ctx.try_cnt <= max_retries && !timeout_exceeded;

    e.retry.decide_reuse(can_retry);

    e
}
