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
    // filter-04b: the sentinel error from a written local reply is intentional,
    // not an upstream failure. Skip body-size accounting and circuit-breaker
    // failure recording, and leave the error non-retryable.
    if ctx.local_reply_written {
        return e.more_context(format!("Peer: {}", peer));
    }

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
        .retry_attempts()
        .unwrap_or(edgion_http.edgion_gateway_config.spec.max_retries);

    // Either route `request_timeout` or client `grpc-timeout` having
    // expired disqualifies further retries.
    let deadline_exceeded = ctx.request_timeout_exceeded() || ctx.grpc_deadline_exceeded();

    let can_retry = client_reused && buffer_ok && ctx.try_cnt <= max_retries && !deadline_exceeded;

    e.retry.decide_reuse(can_retry);

    e
}
