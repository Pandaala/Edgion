use super::EdgionHttpProxy;
use crate::EdgionHttpContext;
use pingora_core::prelude::HttpPeer;
use pingora_core::{Error, ErrorType};
use pingora_proxy::Session;
use std::time::Instant;

#[inline]
pub fn fail_to_connect(
    edgion_http: &EdgionHttpProxy,
    _session: &mut Session,
    _peer: &HttpPeer,
    ctx: &mut EdgionHttpContext,
    mut e: Box<Error>,
) -> Box<Error> {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.set_response_body_size(0);
    }
    // Set status code based on error type: 504 for timeout, 503 for other errors
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        let status_code = match e.etype() {
            ErrorType::ConnectTimedout | ErrorType::TLSHandshakeTimedout => 504,
            _ => 503,
        };
        upstream.status = Some(status_code);
        // Record the failure reason for every attempt, timeouts included: a
        // silent 504 connect-timeout entry is indistinguishable from a per-try
        // timeout entry (also 504) when aggregating retries by reason.
        upstream.err.push(e.etype().as_str().to_string());
        upstream.et = Some(
            Instant::now()
                .saturating_duration_since(upstream.start_time)
                .as_millis() as u64,
        );
    }

    // Record circuit breaker failure
    if let Some(upstream) = ctx.get_current_upstream() {
        if let (Some(service_key), Some(addr)) = (&upstream.service_key, &upstream.lb_backend_addr) {
            // Connect failure / connect timeout / TLS-handshake timeout — the data plane
            // never got a usable HTTP response, so this is a local-origin failure.
            crate::backends::get_circuit_breaker_store().record_failure_by_addr(
                service_key,
                addr,
                crate::backends::FailureClass::LocalOrigin,
            );
        }
    }

    // Check request timeout dynamically before allowing retry
    // If timeout is exceeded, block all retries and return 504. Both the
    // route-level `request_timeout` and a client-supplied `grpc-timeout`
    // (which can be tighter than the route timeout) abort retries; either
    // deadline blown means a new LB pick / connection / circuit-breaker
    // counter increment would only burn resources the caller has given up on.
    if ctx.request_timeout_exceeded() || ctx.grpc_deadline_exceeded() {
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            upstream.status = Some(504);
        }
        // Do not set retry flag - this blocks any further retries
        return e;
    }

    // Determine max_retries: rule.retry.attempts > global (HTTP or gRPC route)
    let max_retries = ctx
        .retry_attempts()
        .unwrap_or(edgion_http.edgion_gateway_config.spec.max_retries);

    if ctx.try_cnt <= max_retries {
        e.set_retry(true);
    }

    e
}
