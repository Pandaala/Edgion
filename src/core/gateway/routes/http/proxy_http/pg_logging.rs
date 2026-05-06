use super::EdgionHttpProxy;
use crate::core::gateway::cli::config::{get_backend_requests_config, get_request_duration_config};
use crate::core::gateway::observe::metrics::{
    global_metrics, record_backend_request, record_request_duration, status_group, TestData, TestType,
};
use crate::core::gateway::observe::AccessLogEntry;
use crate::types::EdgionHttpContext;
use pingora_core::Error as PingoraError;
use pingora_proxy::Session;

#[inline]
pub async fn logging(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    _e: Option<&PingoraError>,
    ctx: &mut EdgionHttpContext,
) {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.set_response_body_size(session.upstream_body_bytes_received());
        let wpt = session.upstream_write_pending_time();
        if !wpt.is_zero() {
            upstream.wpt = Some(wpt.as_millis() as u64);
        }
    }
    // Record proxied request bytes for bandwidth monitoring
    global_metrics().add_request_bytes(session.body_bytes_read() as u64);
    // Record proxied response bytes for bandwidth monitoring
    global_metrics().add_response_bytes(session.upstream_body_bytes_received() as u64);
    // g1-007: balance the LB runtime ledger using the explicit fields written by
    // `connected_to_upstream` (see `lb_runtime_cleanup` for the full contract).
    lb_runtime_cleanup(ctx);

    // Update response status from session
    if let Some(resp_header) = session.response_written() {
        ctx.request_info.status = Some(resp_header.status.as_u16());
    }

    // Record HTTP request metrics
    record_request_metrics(ctx, _e);

    // Create access log entry
    let entry = AccessLogEntry::from_context(ctx);
    let entry_json = entry.to_json();

    // Clone only when integration testing needs the log stored; production path is zero-copy.
    let entry_json_for_store = if crate::core::common::config::is_integration_testing_mode() {
        let should_store = session
            .req_header()
            .headers
            .get("access_log")
            .and_then(|v| v.to_str().ok())
            .map(|v| v == "test_store")
            .unwrap_or(false);
        if should_store {
            Some(entry_json.clone())
        } else {
            None
        }
    } else {
        None
    };

    edgion_http.access_logger.send(entry_json).await;

    // Defensive cleanup for mirror state. Dropping JoinHandle detaches task.
    if let Some(mirror_state) = ctx.mirror_state.take() {
        drop(mirror_state);
    }

    if let Some(json_for_store) = entry_json_for_store {
        let store = crate::core::gateway::observe::access_log_store::get_access_log_store();
        let trace_key = ctx
            .request_info
            .x_trace_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if let Err(e) = store.store(trace_key, json_for_store) {
            tracing::warn!(
                component = "access_log_store",
                error = %e,
                "Failed to store access log"
            );
        }
    }
}

/// Record HTTP request metrics for monitoring and testing
///
/// Records request information to Prometheus for each completed request.
/// Test fields (test_key, test_data) are only populated when --integration-testing-mode is enabled
/// AND the Gateway has the corresponding annotations set.
#[inline]
fn record_request_metrics(ctx: &EdgionHttpContext, error: Option<&PingoraError>) {
    // When route matching fails, gateway_info stays at Default (empty name).
    // Use "unknown" to stay consistent with route/backend fallback labels.
    let gateway_ns = match ctx.gateway_info.gateway_namespace() {
        "" => "unknown",
        ns => ns,
    };
    let gateway_name = match ctx.gateway_info.gateway_name() {
        "" => "unknown",
        name => name,
    };

    // Get matched route information (HTTP or gRPC)
    let (route_ns, route_name) = if let Some(ref route_unit) = ctx.route_unit {
        (
            route_unit.matched_info.rsc.ns.as_str(),
            route_unit.matched_info.rsc.name.as_str(),
        )
    } else if let Some(ref grpc_unit) = ctx.grpc_route_unit {
        (
            grpc_unit.matched_info.rsc.ns.as_str(),
            grpc_unit.matched_info.rsc.name.as_str(),
        )
    } else {
        ("unknown", "unknown")
    };

    // Get backend information. Bypass paths (DirectEndpoint / ExternalJump) have no rsc and
    // fall back to "unknown" — same fallback as when backend_context itself is missing.
    let (backend_ns, backend_name) = ctx
        .backend_context
        .as_ref()
        .and_then(|bc| bc.rsc.as_ref())
        .map(|rsc| (rsc.ns.as_str(), rsc.name.as_str()))
        .unwrap_or(("unknown", "unknown"));

    // Get protocol from discover_protocol (default "http", could be "grpc", "websocket", etc.)
    let protocol = ctx.request_info.discover_protocol.as_deref().unwrap_or("http");

    // Get test metrics only when integration_testing_mode is enabled
    // This prevents processing test annotations in production
    let (test_key, test_data) = if crate::core::common::config::is_integration_testing_mode() {
        let key = ctx.gateway_info.metrics_test_key.as_deref().unwrap_or("");
        let data = build_test_data(ctx, error);
        (key, data)
    } else {
        ("", String::new())
    };

    // Record the metric
    let status = status_group(ctx.request_info.status);
    record_backend_request(
        &get_backend_requests_config().labels,
        gateway_ns,
        gateway_name,
        route_ns,
        route_name,
        backend_ns,
        backend_name,
        protocol,
        status,
        test_key,
        &test_data,
    );

    // Record request duration histogram
    let duration_ms = ctx.start_time.elapsed().as_secs_f64() * 1000.0;
    record_request_duration(
        get_request_duration_config(),
        gateway_ns,
        gateway_name,
        route_ns,
        route_name,
        backend_ns,
        backend_name,
        protocol,
        status,
        duration_ms,
    );
}

/// g1-007: balance the LB runtime ledger at request end.
///
/// Reads only the explicit fields written by `connected_to_upstream`; does not
/// fall back on `current_upstream.lb_backend_addr` (that path drifts the ledger
/// on attempts where peer was selected but connect failed).
/// - `leastconn_incremented_backend` is set only when this attempt actually
///   called `backend.inc_conn()`; the take-then-decrement here pairs with that
///   increment so the ledger stays balanced.
/// - `last_ewma_backend` is the same shape: only attempts that connected
///   contribute an EWMA sample, computed from the attempt's own start so
///   retries do not pollute the previous backend's sample with the retry's
///   elapsed time.
///
/// Idempotent: a second call sees `None` from both `take()` calls and does
/// not double-decrement.
#[inline]
fn lb_runtime_cleanup(ctx: &mut EdgionHttpContext) {
    if let Some(backend) = ctx.leastconn_incremented_backend.take() {
        backend.dec_conn();
    }
    if let Some((backend, attempt_start)) = ctx.last_ewma_backend.take() {
        let latency_us = attempt_start.elapsed().as_micros() as u64;
        backend.update_ewma(latency_us);
    }
}

/// Build test data based on test type from Gateway annotations
///
/// Returns empty string if test mode is not enabled.
/// All data is collected from ctx at logging stage.
#[inline]
fn build_test_data(ctx: &EdgionHttpContext, error: Option<&PingoraError>) -> String {
    let Some(test_type) = &ctx.gateway_info.metrics_test_type else {
        return String::new();
    };

    let mut test_data = TestData::new();

    match test_type {
        TestType::Lb => {
            // LB test: collect backend IP, port from UpstreamInfo (saved by push_upstream)
            if let Some(upstream) = ctx.get_current_upstream() {
                test_data.ip = upstream.ip.clone();
                test_data.port = upstream.port;
            }
            test_data.hash_key = ctx.hash_key.clone();
        }
        TestType::Retry => {
            // Retry test: collect try count and error
            test_data.try_count = Some(ctx.try_cnt);
            test_data.error = error.map(|e| e.to_string());
        }
        TestType::Latency => {
            // Latency test: collect upstream latency
            if let Some(start) = ctx.upstream_start_time {
                test_data.latency_ms = Some(start.elapsed().as_millis() as u64);
            }
        }
        TestType::None => {
            return String::new();
        }
    }

    test_data.to_json()
}

#[cfg(test)]
mod tests {
    //! Unit tests for the g1-007 LB runtime ledger cleanup contract.
    //!
    //! These tests verify the invariants of [`lb_runtime_cleanup`] without
    //! standing up a Pingora session: by directly mutating `EdgionHttpContext`
    //! fields and calling per-backend atomic methods we reproduce the exact
    //! stage sequence that the proxy would emit.
    use super::*;
    use crate::core::gateway::lb::edgion_backend::EdgionBackend;
    use pingora_core::protocols::l4::socket::SocketAddr;
    use pingora_load_balancing::Backend;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn make_backend(ip4: [u8; 4], port: u16) -> Arc<EdgionBackend> {
        Arc::new(EdgionBackend::new(Backend {
            addr: SocketAddr::Inet(std::net::SocketAddr::from((ip4, port))),
            weight: 1,
            ext: Default::default(),
        }))
    }

    /// Key regression: when a backend already has an in-flight count,
    /// cleanup of an attempt that never called `inc_conn` must not
    /// erroneously decrement that other in-flight count.
    #[test]
    fn cleanup_no_increment_does_not_touch_other_inflight() {
        let a = make_backend([127, 0, 0, 1], 30001);
        // Simulate an in-flight on A: connected, not yet finished.
        a.inc_conn();
        assert_eq!(a.conn(), 1);

        // A separate attempt picked A but connect failed; `inc_conn` was never
        // called, so `ctx.leastconn_incremented_backend` stays None.
        let mut ctx = EdgionHttpContext::new();
        assert!(ctx.leastconn_incremented_backend.is_none());

        lb_runtime_cleanup(&mut ctx);

        // Critical: A's in-flight count must still be 1, not erroneously decremented.
        assert_eq!(
            a.conn(),
            1,
            "cleanup of an attempt that never incremented must not decrement another in-flight backend"
        );
    }

    #[test]
    fn cleanup_after_connected_balances_to_zero() {
        let a = make_backend([127, 0, 0, 2], 30001);

        // Simulate `connected_to_upstream`: inc_conn + set the ledger field.
        a.inc_conn();
        let mut ctx = EdgionHttpContext::new();
        ctx.leastconn_incremented_backend = Some(a.clone());
        assert_eq!(a.conn(), 1);

        lb_runtime_cleanup(&mut ctx);

        assert_eq!(a.conn(), 0);
        assert!(
            ctx.leastconn_incremented_backend.is_none(),
            "cleanup must take the field"
        );
    }

    /// Full retry path: A connects → `begin_upstream_attempt` decrements A → B connects → logging decrements B.
    #[test]
    fn retry_then_connected_then_logging_balances_both() {
        use crate::core::gateway::routes::http::proxy_http::pg_upstream_peer::begin_upstream_attempt;

        let a = make_backend([127, 0, 0, 3], 30001);
        let b = make_backend([127, 0, 0, 4], 30001);

        let mut ctx = EdgionHttpContext::new();

        // attempt 1 connects to A.
        a.inc_conn();
        ctx.leastconn_incremented_backend = Some(a.clone());
        assert_eq!(a.conn(), 1);

        // retry triggers begin_upstream_attempt: take + decrement A.
        begin_upstream_attempt(&mut ctx);
        assert_eq!(a.conn(), 0);
        assert!(ctx.leastconn_incremented_backend.is_none());

        // attempt 2 connects to B.
        b.inc_conn();
        ctx.leastconn_incremented_backend = Some(b.clone());
        assert_eq!(b.conn(), 1);

        // logging cleanup.
        lb_runtime_cleanup(&mut ctx);

        assert_eq!(a.conn(), 0);
        assert_eq!(b.conn(), 0);
    }

    #[test]
    fn ewma_no_pollution_when_final_attempt_failed_to_connect() {
        use crate::core::gateway::routes::http::proxy_http::pg_upstream_peer::begin_upstream_attempt;

        let a = make_backend([127, 0, 0, 5], 30001);

        // Default initial EWMA value.
        let initial = a.ewma();

        let mut ctx = EdgionHttpContext::new();

        // attempt 1 connects to A, records the EWMA backend.
        ctx.last_ewma_backend = Some((a.clone(), Instant::now()));

        // retry → begin_upstream_attempt clears the EWMA record.
        begin_upstream_attempt(&mut ctx);
        assert!(ctx.last_ewma_backend.is_none(), "retry must clear last_ewma_backend");

        // attempt 2 fails to connect, so connected_to_upstream never runs and
        // last_ewma_backend stays None. Logging cleanup must not call update_ewma.
        lb_runtime_cleanup(&mut ctx);

        assert_eq!(
            a.ewma(),
            initial,
            "an attempt that never connected must not pollute A's EWMA sample"
        );
    }

    #[test]
    fn ewma_uses_attempt_start_not_request_start() {
        let a = make_backend([127, 0, 0, 6], 30001);

        // Set attempt_start ~50ms in the past so update_ewma sees a measurable sample.
        let attempt_start = Instant::now() - Duration::from_millis(50);
        let mut ctx = EdgionHttpContext::new();
        ctx.last_ewma_backend = Some((a.clone(), attempt_start));

        lb_runtime_cleanup(&mut ctx);

        // update_ewma is alpha-smoothed: new = alpha*sample + (1-alpha)*current.
        // Without depending on the specific alpha, just assert the EWMA moved
        // away from INITIAL_EWMA_US (1_000us == 1ms).
        let after = a.ewma();
        assert!(
            after > 1_000,
            "EWMA should have moved past the 1ms initial after a ~50ms sample (got {}us)",
            after
        );
    }

    #[test]
    fn cleanup_idempotent_on_double_call() {
        let a = make_backend([127, 0, 0, 7], 30001);

        a.inc_conn();
        let mut ctx = EdgionHttpContext::new();
        ctx.leastconn_incremented_backend = Some(a.clone());
        assert_eq!(a.conn(), 1);

        lb_runtime_cleanup(&mut ctx);
        assert_eq!(a.conn(), 0);

        // Manually re-increment to simulate a separate concurrent in-flight,
        // then call cleanup again — second call must take None and not double-decrement.
        a.inc_conn();
        assert_eq!(a.conn(), 1);

        lb_runtime_cleanup(&mut ctx);

        assert_eq!(
            a.conn(),
            1,
            "cleanup must be idempotent; the second call must not decrement again"
        );
    }
}
