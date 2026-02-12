use super::EdgionHttp;
use crate::core::observe::metrics::{record_backend_request, status_group};
use crate::core::observe::test_metrics::{TestData, TestType};
use crate::core::observe::AccessLogEntry;
use crate::types::EdgionHttpContext;
use pingora_core::Error as PingoraError;
use pingora_proxy::Session;

#[inline]
pub async fn logging(
    edgion_http: &EdgionHttp,
    session: &mut Session,
    _e: Option<&PingoraError>,
    ctx: &mut EdgionHttpContext,
) {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.set_response_body_size(session.upstream_body_bytes_received());
    }
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

    // Record HTTP request metrics
    record_request_metrics(ctx, _e);

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

    // Store in Access Log Store when integration testing mode is enabled
    // Only store when request has "access_log: test_store" header to avoid
    // flooding the store during high-volume tests (e.g., LB distribution tests)
    if crate::core::cli::config::is_integration_testing_mode() {
        let should_store = session
            .req_header()
            .headers
            .get("access_log")
            .and_then(|v| v.to_str().ok())
            .map(|v| v == "test_store")
            .unwrap_or(false);

        if should_store {
            let store = crate::core::observe::access_log_store::get_access_log_store();
            // Use x_trace_id if available, otherwise generate a unique key
            let trace_key = ctx
                .request_info
                .x_trace_id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            if let Err(e) = store.store(trace_key, entry.to_json()) {
                tracing::warn!(
                    component = "access_log_store",
                    error = %e,
                    "Failed to store access log"
                );
            }
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
    // Get gateway information
    let gateway_ns = ctx.gateway_info.gateway_namespace();
    let gateway_name = ctx.gateway_info.gateway_name();

    // Get matched route information (HTTP or gRPC)
    let (route_ns, route_name) = if let Some(ref route_unit) = ctx.route_unit {
        (
            route_unit.matched_info.rns.as_str(),
            route_unit.matched_info.rn.as_str(),
        )
    } else if let Some(ref grpc_unit) = ctx.grpc_route_unit {
        (
            grpc_unit.matched_info.route_ns.as_str(),
            grpc_unit.matched_info.route_name.as_str(),
        )
    } else {
        ("unknown", "unknown")
    };

    // Get backend information
    let (backend_ns, backend_name) = ctx
        .backend_context
        .as_ref()
        .map(|bc| (bc.namespace.as_str(), bc.name.as_str()))
        .unwrap_or(("unknown", "unknown"));

    // Get protocol from discover_protocol (default "http", could be "grpc", "websocket", etc.)
    let protocol = ctx.request_info.discover_protocol.as_deref().unwrap_or("http");

    // Get test metrics only when integration_testing_mode is enabled
    // This prevents processing test annotations in production
    let (test_key, test_data) = if crate::core::cli::config::is_integration_testing_mode() {
        let key = ctx.gateway_info.metrics_test_key.as_deref().unwrap_or("");
        let data = build_test_data(ctx, error);
        (key, data)
    } else {
        ("", String::new())
    };

    // Record the metric
    record_backend_request(
        gateway_ns,
        gateway_name,
        route_ns,
        route_name,
        backend_ns,
        backend_name,
        protocol,
        status_group(ctx.request_info.status),
        test_key,
        &test_data,
    );
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
