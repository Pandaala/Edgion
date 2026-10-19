use super::EdgionHttpProxy;
use crate::ai::http::attempt;
use crate::EdgionHttpContext;
use edgion_resources::EdgionErrorCode;
use pingora_core::prelude::HttpPeer;
use pingora_core::{Error, ErrorType};
use pingora_proxy::Session;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpiredConnectDeadline {
    Request,
    Grpc,
}

fn select_expired_connect_deadline(
    request_deadline_exceeded: bool,
    grpc_deadline_exceeded: bool,
) -> Option<ExpiredConnectDeadline> {
    if request_deadline_exceeded {
        Some(ExpiredConnectDeadline::Request)
    } else if grpc_deadline_exceeded {
        Some(ExpiredConnectDeadline::Grpc)
    } else {
        None
    }
}

fn connect_timeout_deadline(
    error: &ErrorType,
    source: Option<crate::ctx::ConnectTimeoutSource>,
    request_remaining: Option<std::time::Duration>,
    grpc_remaining: Option<std::time::Duration>,
) -> Option<ExpiredConnectDeadline> {
    use crate::ctx::ConnectTimeoutSource;
    // Pingora's timer resolution can fire just before Instant reaches the
    // deadline. The OS also maps ETIMEDOUT to ConnectTimedout, so the configured
    // timer owner is insufficient: its budget must be within the timer's
    // sub-millisecond early-firing window. Other failures remain ordinary errors.
    if !matches!(error, ErrorType::ConnectTimedout | ErrorType::TLSHandshakeTimedout) {
        return None;
    }
    let (remaining, deadline) = match source {
        Some(ConnectTimeoutSource::RequestDeadline) => (request_remaining?, ExpiredConnectDeadline::Request),
        Some(ConnectTimeoutSource::GrpcDeadline) => (grpc_remaining?, ExpiredConnectDeadline::Grpc),
        Some(ConnectTimeoutSource::Configured) | None => return None,
    };
    (remaining <= std::time::Duration::from_millis(1)).then_some(deadline)
}

fn finish_expired_connect_deadline(
    ctx: &mut EdgionHttpContext,
    deadline: ExpiredConnectDeadline,
    error: Box<Error>,
) -> Box<Error> {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.status = Some(504);
    }
    ctx.record_error(match deadline {
        ExpiredConnectDeadline::Request => EdgionErrorCode::RequestTimeoutExceeded,
        ExpiredConnectDeadline::Grpc => EdgionErrorCode::GrpcClientDeadlineExceeded,
    });

    let mut timeout_error = Error::because(
        ErrorType::HTTPStatus(504),
        "end-to-end deadline exhausted during upstream connection establishment",
        error,
    );
    timeout_error.set_retry(false);
    timeout_error
}

#[inline]
pub fn fail_to_connect(
    _edgion_http: &EdgionHttpProxy,
    session: &mut Session,
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
        upstream.err.push(e.etype().as_str().into());
        upstream.et = Some(
            Instant::now()
                .saturating_duration_since(upstream.start_time)
                .as_millis() as u64,
        );
    }

    // Record outlier detection failure
    if let Some(upstream) = ctx.get_current_upstream() {
        if let (Some(backend), Some(token)) = (upstream.lb_backend.as_deref(), upstream.lb_outlier_completion.as_ref())
        {
            // Connect failure / connect timeout / TLS-handshake timeout — the data plane
            // never got a usable HTTP response, so this is a local-origin failure.
            crate::backends::get_outlier_detection_store().record_failure_for_backend_with_token(
                backend,
                token,
                crate::backends::FailureClass::LocalOrigin,
            );
        }
    }

    // Check request timeout dynamically before allowing retry
    // If timeout is exceeded, block all retries and return 504. Both the
    // route-level `request_timeout` and a client-supplied `grpc-timeout`
    // (which can be tighter than the route timeout) abort retries; either
    // deadline blown means a new LB pick / connection / outlier-detection
    // counter increment would only burn resources the caller has given up on.
    let now = Instant::now();
    let request_remaining = ctx
        .request_timeout()
        .map(|budget| budget.saturating_sub(now.saturating_duration_since(ctx.start_time)));
    let grpc_remaining = ctx
        .request_info
        .grpc_client_deadline()
        .map(|deadline| deadline.saturating_duration_since(now));
    if let Some(deadline) = select_expired_connect_deadline(
        request_remaining.is_some_and(|remaining| remaining.is_zero()),
        grpc_remaining.is_some_and(|remaining| remaining.is_zero()),
    )
    .or_else(|| connect_timeout_deadline(e.etype(), ctx.connect_timeout_source, request_remaining, grpc_remaining))
    {
        return finish_expired_connect_deadline(ctx, deadline, e);
    }

    let relay_allows_retry = session.request_relay_retry_state().can_start_next_attempt();

    if ctx.has_ai_attempt_plan() {
        attempt::on_connect_failure(ctx, relay_allows_retry, &mut e);
        return e;
    }

    // A retry Pingora cannot start must not consume the ordinary budget.
    if !relay_allows_retry {
        return e;
    }

    // Determine retry attempts: rule.retry.attempts > global (HTTP or gRPC route)
    let retry_attempts = ctx.retry_attempts().unwrap_or(ctx.egc.retry_attempts);

    if super::retry_budget_allows_next_attempt(ctx.try_cnt, retry_attempts) {
        // Retry budget (retryConstraint): composes AFTER the attempts/deadline
        // checks above (D3) — both gates must pass. On refusal the retry flag is
        // NOT set; instead the client gets a deterministic 503, delivered by
        // swapping the error's ErrorType (pg_fail_to_proxy renders the status from
        // the error, never from ctx.request_info.status). The per-upstream entry
        // keeps the backend's real observed status (set above); the 503 is a
        // request-level outcome carried by the error.
        if let Some(budget) = super::pg_upstream_peer::retry_budget_for(ctx) {
            if !budget.try_retry() {
                crate::observe::metrics::record_retry_budget_denied();
                ctx.record_error(EdgionErrorCode::RetryBudgetExhausted);
                return Error::explain(ErrorType::HTTPStatus(503), "retry budget exhausted");
            }
        }
        e.set_retry(true);
    }

    e
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_fail_to_connect_test.rs"]
mod tests;
