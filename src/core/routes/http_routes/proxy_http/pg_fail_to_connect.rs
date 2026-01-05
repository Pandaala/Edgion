use super::EdgionHttp;
use crate::types::EdgionHttpContext;
use pingora_core::prelude::HttpPeer;
use pingora_core::{Error, ErrorType};
use pingora_proxy::Session;

/// fail_to_connect - called when connection to upstream fails
#[inline]
pub fn fail_to_connect(
    edgion_http: &EdgionHttp,
    _session: &mut Session,
    _peer: &HttpPeer,
    ctx: &mut EdgionHttpContext,
    mut e: Box<Error>,
) -> Box<Error> {
    // Set status code based on error type: 504 for timeout, 503 for other errors
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        let status_code = match e.etype() {
            ErrorType::ConnectTimedout | ErrorType::TLSHandshakeTimedout => 504,
            _ => 503,
        };
        upstream.status = Some(status_code);
        // Only add error message for non-timeout errors (503)
        if status_code != 504 {
            upstream.err.push(e.etype().as_str().to_string());
        }
        upstream.et = Some(upstream.start_time.elapsed().as_millis() as u64);
    }

    // Check request timeout dynamically before allowing retry
    // If timeout is exceeded, block all retries and return 504
    let request_timeout = ctx
        .route_unit
        .as_ref()
        .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
        .and_then(|timeouts| timeouts.request_timeout)
        .or_else(|| {
            ctx.grpc_route_unit
                .as_ref()
                .and_then(|unit| unit.rule.parsed_timeouts.as_ref())
                .and_then(|timeouts| timeouts.request_timeout)
        });

    if let Some(timeout) = request_timeout {
        let elapsed = ctx.start_time.elapsed();
        if elapsed >= timeout {
            tracing::warn!(
                total_attempts = ctx.try_cnt,
                elapsed_secs = elapsed.as_secs_f64(),
                timeout_secs = timeout.as_secs_f64(),
                "Request timeout exceeded, blocking retry"
            );
            // Set 504 status for timeout
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.status = Some(504);
            }
            // Do not set retry flag - this blocks any further retries
            return e;
        }
    }

    // Determine max_retries: route annotation > global config
    let max_retries = ctx
        .route_unit
        .as_ref()
        .and_then(|unit| unit.rule.parsed_max_retries)
        .unwrap_or(edgion_http.edgion_gateway_config.spec.max_retries);

    if ctx.try_cnt <= max_retries {
        e.set_retry(true);
    }

    e
}
