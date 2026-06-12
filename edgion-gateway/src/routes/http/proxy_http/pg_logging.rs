use super::EdgionHttpProxy;
use crate::cli::config::{get_backend_requests_config, get_request_duration_config};
use crate::observe::metrics::{
    global_metrics, record_backend_request, record_request_duration, status_group, TestData, TestType,
};
use crate::observe::AccessLogEntry;
use crate::EdgionHttpContext;
use pingora_core::Error as PingoraError;
use pingora_proxy::Session;
use std::time::Instant;

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
    let entry_json_for_store = if edgion_common::test_mode::is_integration_testing_mode() {
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
        let store = crate::observe::access_log_store::get_access_log_store();
        let trace_key = ctx
            .request_info
            .x_trace_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if let Err(e) = store.store(trace_key, json_for_store) {
            tracing::warn!( // tracing-allow: access-log store send failure (registered exception, see 01-log-safety.md)
                component = "access_log_store",
                error = %e,
                "Failed to store access log"
            );
            global_metrics().access_log_dropped();
        }
    }
}

#[inline]
fn record_request_metrics(ctx: &EdgionHttpContext, error: Option<&PingoraError>) {
    let duration_ms = Instant::now().saturating_duration_since(ctx.start_time).as_secs_f64() * 1000.0;

    if try_record_via_handles(ctx, duration_ms) {
        return;
    }

    // Fallback: integration-testing mode, route miss, bypass path,
    // gateway not in the route's attached set, or unknown protocol.
    record_request_metrics_fallback(ctx, error, duration_ms);
}

#[inline]
fn try_record_via_handles(ctx: &EdgionHttpContext, duration_ms: f64) -> bool {
    use crate::observe::metrics::route_handles::{protocol_to_idx, ps_index, status_to_idx, RouteMetricHandles};

    if edgion_common::test_mode::is_integration_testing_mode() {
        return false;
    }

    let handles: &RouteMetricHandles = match (&ctx.route_unit, &ctx.grpc_route_unit) {
        (Some(r), _) => &r.metric_handles,
        (None, Some(g)) => &g.metric_handles,
        (None, None) => return false,
    };

    let entry = match handles.lookup(ctx.gateway_info.gateway_namespace(), ctx.gateway_info.gateway_name()) {
        Some(e) => e,
        None => return false,
    };

    let bc = match ctx.backend_context.as_ref() {
        Some(bc) => bc,
        None => return false,
    };
    let b_idx = match bc.backend_ref_idx {
        Some(i) => i,
        None => return false,
    };

    let protocol = ctx.request_info.discover_protocol.as_deref().unwrap_or("http");
    let p_idx = match protocol_to_idx(protocol) {
        Some(p) => p,
        None => return false,
    };
    let s_idx = status_to_idx(ctx.request_info.status);
    let idx = ps_index(p_idx, s_idx);

    if b_idx >= entry.counters.len() {
        debug_assert!(
            false,
            "backend_ref_idx out of bounds — RouteMetricHandles built against stale backend_refs"
        );
        return false;
    }
    entry.counters[b_idx][idx].increment(1);
    if let Some(hs) = entry.histograms.as_ref() {
        hs[b_idx][idx].record(duration_ms);
    }
    true
}

/// Dynamic fallback path. Same code as the previous `record_request_metrics`
/// body — kept verbatim so route-miss / bypass / integration-testing-mode
/// requests continue to emit the same time series.
#[inline]
fn record_request_metrics_fallback(ctx: &EdgionHttpContext, error: Option<&PingoraError>, duration_ms: f64) {
    let gateway_ns = match ctx.gateway_info.gateway_namespace() {
        "" => "unknown",
        ns => ns,
    };
    let gateway_name = match ctx.gateway_info.gateway_name() {
        "" => "unknown",
        name => name,
    };

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

    let (backend_ns, backend_name) = ctx
        .backend_context
        .as_ref()
        .and_then(|bc| bc.rsc.as_ref())
        .map(|rsc| (rsc.ns.as_str(), rsc.name.as_str()))
        .unwrap_or(("unknown", "unknown"));

    let protocol = ctx.request_info.discover_protocol.as_deref().unwrap_or("http");

    let (test_key, test_data) = if edgion_common::test_mode::is_integration_testing_mode() {
        let key = ctx.gateway_info.metrics_test_key.as_deref().unwrap_or("");
        let data = build_test_data(ctx, error);
        (key, data)
    } else {
        ("", String::new())
    };

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

/// Settle the final attempt's LB commit at request end.
/// Earlier attempts' commits were discarded by `begin_upstream_attempt` on each retry,
/// so only the last upstream entry can still hold a pending commit here.
/// Idempotent: a second call sees `None` from `take()` inside `finalize_attempt`.
#[inline]
fn lb_runtime_cleanup(ctx: &mut EdgionHttpContext) {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        crate::lb::finalize_attempt(upstream, crate::lb::FinalizeMode::Settle);
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
                test_data.latency_ms = Some(Instant::now().saturating_duration_since(start).as_millis() as u64);
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
    use crate::lb::edgion_backend::EdgionBackend;
    use crate::lb::LbCommit;
    use crate::BackendContext;
    use pingora_core::protocols::l4::socket::SocketAddr;
    use pingora_load_balancing::Backend;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn make_backend(ip: [u8; 4], port: u16) -> Arc<EdgionBackend> {
        Arc::new(EdgionBackend::new(Backend {
            addr: SocketAddr::Inet(std::net::SocketAddr::from((ip, port))),
            weight: 1,
            ext: Default::default(),
        }))
    }

    /// Push a new upstream entry with lb_edgion_backend and lb_committed pre-set,
    /// simulating the state that `connected_to_upstream` / `commit_attempt` would
    /// produce. Does NOT call inc_conn/update_ewma — callers do that explicitly
    /// so tests remain clear about which side-effects they are exercising.
    fn setup_committed_attempt(ctx: &mut EdgionHttpContext, backend: Arc<EdgionBackend>, commit: LbCommit) {
        if ctx.backend_context.is_none() {
            ctx.backend_context = Some(BackendContext {
                source: None,
                rsc: None,
                service_key: None,
                service_kind: None,
                upstreams: Vec::new(),
                current_upstream_id: None,
                backend_ref_idx: None,
            });
        }
        let bc = ctx.backend_context.as_mut().unwrap();
        bc.upstreams.push(crate::UpstreamInfo {
            ip: None,
            port: None,
            status: None,
            ct: None,
            ht: None,
            bt: None,
            et: None,
            wpt: None,
            response_body_size: None,
            start_time: Instant::now(),
            err: Vec::new(),
            lb_backend_addr: None,
            lb_edgion_backend: Some(backend),
            service_key: None,
            lb_policy: None,
            tls: None,
            lb_committed: Some(commit),
        });
        bc.current_upstream_id = Some(bc.upstreams.len() - 1);
    }

    #[test]
    fn cleanup_no_increment_does_not_touch_other_inflight() {
        let a = make_backend([127, 0, 0, 1], 30001);
        // Another request has an in-flight on A.
        a.inc_conn();
        assert_eq!(a.conn(), 1);

        // This request: an upstream entry exists but connect failed — lb_committed never set.
        let mut ctx = EdgionHttpContext::new();
        ctx.backend_context = Some(BackendContext {
            source: None,
            rsc: None,
            service_key: None,
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: None,
        });
        ctx.push_upstream(None, None);
        // lb_committed stays None (default from push_upstream)

        lb_runtime_cleanup(&mut ctx);

        assert_eq!(
            a.conn(),
            1,
            "cleanup with lb_committed=None must not touch another in-flight count"
        );
    }

    #[test]
    fn cleanup_after_connected_balances_to_zero() {
        let a = make_backend([127, 0, 0, 2], 30001);

        // Simulate connected_to_upstream: inc_conn then record commit.
        a.inc_conn();
        let mut ctx = EdgionHttpContext::new();
        setup_committed_attempt(&mut ctx, a.clone(), LbCommit::LeastConn);
        assert_eq!(a.conn(), 1);

        lb_runtime_cleanup(&mut ctx);

        assert_eq!(a.conn(), 0);
        assert!(
            ctx.get_current_upstream().unwrap().lb_committed.is_none(),
            "lb_committed must be taken by cleanup"
        );
    }

    #[test]
    fn retry_then_connected_then_logging_balances_both() {
        use crate::routes::http::proxy_http::pg_upstream_peer::begin_upstream_attempt;

        let a = make_backend([127, 0, 0, 3], 30001);
        let b = make_backend([127, 0, 0, 4], 30001);

        let mut ctx = EdgionHttpContext::new();

        // Attempt 1: connected to A.
        a.inc_conn();
        setup_committed_attempt(&mut ctx, a.clone(), LbCommit::LeastConn);
        assert_eq!(a.conn(), 1);

        // Retry: begin_upstream_attempt finalizes attempt 1 with Discard (dec A),
        // then pushes an empty slot for attempt 2.
        begin_upstream_attempt(&mut ctx);
        assert_eq!(a.conn(), 0);
        assert!(
            ctx.backend_context.as_ref().unwrap().upstreams[0]
                .lb_committed
                .is_none(),
            "begin_upstream_attempt must consume upstream 0's lb_committed"
        );

        // Attempt 2: connected to B.
        b.inc_conn();
        if let Some(up) = ctx.get_current_upstream_mut() {
            up.lb_edgion_backend = Some(b.clone());
            up.lb_committed = Some(LbCommit::LeastConn);
        }
        assert_eq!(b.conn(), 1);

        // Logging cleanup: settles attempt 2 (dec B).
        lb_runtime_cleanup(&mut ctx);

        assert_eq!(a.conn(), 0);
        assert_eq!(b.conn(), 0);
    }

    #[test]
    fn ewma_no_pollution_when_final_attempt_failed_to_connect() {
        use crate::routes::http::proxy_http::pg_upstream_peer::begin_upstream_attempt;

        let a = make_backend([127, 0, 0, 5], 30001);
        let initial = a.ewma();

        let mut ctx = EdgionHttpContext::new();

        // Attempt 1: connected to A — EWMA commit recorded.
        setup_committed_attempt(
            &mut ctx,
            a.clone(),
            LbCommit::Ewma {
                started: Instant::now(),
            },
        );

        // Retry: Discard-mode finalize drops the EWMA sample (no update_ewma).
        begin_upstream_attempt(&mut ctx);
        assert!(
            ctx.backend_context.as_ref().unwrap().upstreams[0]
                .lb_committed
                .is_none(),
            "retry must discard upstream 0's EWMA commit"
        );

        // Attempt 2: connect failed — lb_committed on upstream 1 stays None.
        lb_runtime_cleanup(&mut ctx);

        assert_eq!(
            a.ewma(),
            initial,
            "neither Discard-mode finalize nor a None-commit must update A's EWMA"
        );
    }

    #[test]
    fn ewma_uses_attempt_start_not_request_start() {
        let a = make_backend([127, 0, 0, 6], 30001);

        // Upstream entry whose start_time (= attempt start) is ~50ms in the past.
        let attempt_start = Instant::now() - Duration::from_millis(50);
        let mut ctx = EdgionHttpContext::new();
        ctx.backend_context = Some(BackendContext {
            source: None,
            rsc: None,
            service_key: None,
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: None,
        });
        ctx.push_upstream(None, None);
        if let Some(up) = ctx.get_current_upstream_mut() {
            up.lb_edgion_backend = Some(a.clone());
            up.lb_committed = Some(LbCommit::Ewma { started: attempt_start });
        }

        lb_runtime_cleanup(&mut ctx);

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
        setup_committed_attempt(&mut ctx, a.clone(), LbCommit::LeastConn);
        assert_eq!(a.conn(), 1);

        lb_runtime_cleanup(&mut ctx);
        assert_eq!(a.conn(), 0);

        // Simulate a separate concurrent in-flight on A.
        a.inc_conn();
        assert_eq!(a.conn(), 1);

        // Second cleanup: lb_committed was taken; must be a no-op.
        lb_runtime_cleanup(&mut ctx);
        assert_eq!(a.conn(), 1, "second cleanup must not decrement again");
    }
}
