use super::EdgionHttpProxy;
use crate::EdgionHttpContext;
use edgion_resources::EdgionStatus;
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

    // Record outlier detection failure
    if !matches!(
        ctx.selected_backend
            .as_ref()
            .and_then(crate::backends::SelectedHttpBackend::routed),
        Some(crate::backends::SelectedLogicalBackend::A2a(_) | crate::backends::SelectedLogicalBackend::Mcp(_))
    ) {
        if let Some(upstream) = ctx.get_current_upstream() {
            if let (Some(service_key), Some(addr)) = (&upstream.service_key, &upstream.lb_backend_addr) {
                // Connect failure / connect timeout / TLS-handshake timeout — the data plane
                // never got a usable HTTP response, so this is a local-origin failure.
                crate::backends::get_outlier_detection_store().record_failure_by_addr(
                    service_key,
                    addr,
                    crate::backends::FailureClass::LocalOrigin,
                );
            }
        }
    }

    // MCP is non-replayable even when Pingora handed this callback a retry
    // decision already set by the transport. Clamp it before every early
    // return and before any retry-budget or retry-reason side effect.
    let mcp_no_replay = ctx.force_mcp_no_replay(&mut e.retry);

    // Check request timeout dynamically before allowing retry
    // If timeout is exceeded, block all retries and return 504. Both the
    // route-level `request_timeout` and a client-supplied `grpc-timeout`
    // (which can be tighter than the route timeout) abort retries; either
    // deadline blown means a new LB pick / connection / outlier-detection
    // counter increment would only burn resources the caller has given up on.
    if ctx.request_timeout_exceeded() || ctx.grpc_deadline_exceeded() {
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            upstream.status = Some(504);
        }
        // The MCP clamp above also clears a transport-preselected retry.
        return e;
    }

    if mcp_no_replay {
        return e;
    }

    if ctx.ai_attempt_plan.is_some() {
        ctx.ai_attempt_runtime.finish_predispatch();
        // The typed predispatch hook consumes no scheduler dispatch slot,
        // failure-ledger entry, or retry-budget numerator. The bounded Redis
        // release is drained before the next selection or at final logging.
        if crate::ai::dispatch::handle_ai_predispatch_failure(ctx, true) {
            e.set_retry(true);
        }
        return e;
    }

    // Determine max_retries: rule.retry.attempts > global (HTTP or gRPC route)
    let max_retries = ctx
        .retry_attempts()
        .unwrap_or(edgion_http.edgion_gateway_config.spec.max_retries);

    if ctx.try_cnt <= max_retries {
        // Retry budget (retryConstraint): composes AFTER the maxRetries/deadline
        // checks above (D3) — both gates must pass. On refusal the retry flag is
        // NOT set; instead the client gets a deterministic 503, delivered by
        // swapping the error's ErrorType (pg_fail_to_proxy renders the status from
        // the error, never from ctx.request_info.status). The per-upstream entry
        // keeps the backend's real observed status (set above); the 503 is a
        // request-level outcome carried by the error.
        if let Some(budget) = super::pg_upstream_peer::retry_budget_for(ctx) {
            if !budget.try_retry() {
                crate::observe::metrics::record_retry_budget_denied();
                ctx.add_error(EdgionStatus::RetryBudgetExhausted);
                return Error::explain(ErrorType::HTTPStatus(503), "retry budget exhausted");
            }
        }
        e.set_retry(true);
    }

    e
}
