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
}

/// Record HTTP request metrics for monitoring and testing
///
/// Records request information to Prometheus for each completed request.
/// Test fields (test_key, test_data) are empty in production mode.
#[inline]
fn record_request_metrics(ctx: &EdgionHttpContext, error: Option<&PingoraError>) {
    // Get gateway information
    let gateway_ns = ctx.gateway_info.gateway_namespace();
    let gateway_name = ctx.gateway_info.gateway_name();

    // Get backend information
    let (backend_ns, backend_name) = ctx
        .backend_context
        .as_ref()
        .map(|bc| (bc.namespace.as_str(), bc.name.as_str()))
        .unwrap_or(("unknown", "unknown"));

    // Get protocol from discover_protocol (default "http", could be "grpc", "websocket", etc.)
    let protocol = ctx
        .request_info
        .discover_protocol
        .as_deref()
        .unwrap_or("http");

    // Get test metrics (empty strings if not in test mode)
    let test_key = ctx.gateway_info.metrics_test_key.as_deref().unwrap_or("");
    let test_data = build_test_data(ctx, error);

    // Record the metric
    record_backend_request(
        gateway_ns,
        gateway_name,
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
            // LB test: collect backend IP, port, and hash key
            if let Some(upstream) = ctx.get_current_upstream() {
                if let Some(addr) = &upstream.backend_addr {
                    if let Some(inet) = addr.as_inet() {
                        test_data.ip = Some(inet.ip().to_string());
                        test_data.port = Some(inet.port());
                    }
                }
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
