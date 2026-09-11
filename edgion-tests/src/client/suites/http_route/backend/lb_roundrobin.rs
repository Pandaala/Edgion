// RoundRobin integration tests use existing backend metrics to verify the exact
// fixture endpoints, request count, and distribution.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::LbDistributionAnalysis;
use async_trait::async_trait;
use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

const METRICS_TEST_KEY: &str = "lb-rr-test";

pub struct LBRoundRobinTestSuite;

fn resolve_expected_endpoints(
    k8s_mode: bool,
    k8s_endpoint_ips: Option<&str>,
    local_endpoints: &[&str],
    expected_count: usize,
) -> Result<Vec<String>, String> {
    let endpoints = if k8s_mode {
        k8s_endpoint_ips
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|ip| !ip.is_empty())
            .map(|ip| format!("{ip}:30001"))
            .collect::<Vec<_>>()
    } else {
        local_endpoints.iter().map(|endpoint| (*endpoint).to_string()).collect()
    };
    let unique: HashSet<&str> = endpoints.iter().map(String::as_str).collect();
    if endpoints.len() != expected_count || unique.len() != expected_count {
        return Err(format!(
            "Expected {expected_count} unique RoundRobin fixture endpoints, got {}: {endpoints:?}",
            unique.len()
        ));
    }
    Ok(endpoints)
}

fn distribution_delta(
    before: &LbDistributionAnalysis,
    after: &LbDistributionAnalysis,
    expected_endpoints: &[&str],
    expected_service: &str,
    expected_requests: u64,
) -> Result<BTreeMap<String, u64>, String> {
    let total_delta = after.total_requests.checked_sub(before.total_requests).ok_or_else(|| {
        format!(
            "Backend metric total counter reset: before={}, after={}",
            before.total_requests, after.total_requests
        )
    })?;
    if total_delta != expected_requests {
        return Err(format!(
            "Expected {expected_requests} total recorded requests, got {total_delta}"
        ));
    }

    let expected_service_after = after.by_service.get(expected_service).copied().unwrap_or(0);
    let expected_service_before = before.by_service.get(expected_service).copied().unwrap_or(0);
    let expected_service_delta = expected_service_after
        .checked_sub(expected_service_before)
        .ok_or_else(|| format!("Backend service metric counter reset for {expected_service}"))?;
    if expected_service_delta != expected_requests {
        return Err(format!(
            "Expected {expected_requests} requests for backend service {expected_service}, got {expected_service_delta}"
        ));
    }
    for (service, after_count) in &after.by_service {
        if service == expected_service {
            continue;
        }
        let before_count = before.by_service.get(service).copied().unwrap_or(0);
        let count = after_count.checked_sub(before_count).ok_or_else(|| {
            format!("Backend service metric counter reset for {service}: before={before_count}, after={after_count}")
        })?;
        if count > 0 {
            return Err(format!(
                "Observed {count} requests on unexpected backend service {service}"
            ));
        }
    }

    let expected: HashSet<&str> = expected_endpoints.iter().copied().collect();
    let mut delta = BTreeMap::new();
    for endpoint in expected_endpoints {
        let before_count = before.by_endpoint.get(*endpoint).copied().unwrap_or(0);
        let after_count = after.by_endpoint.get(*endpoint).copied().unwrap_or(0);
        let count = after_count.checked_sub(before_count).ok_or_else(|| {
            format!("Backend metric counter reset for {endpoint}: before={before_count}, after={after_count}")
        })?;
        delta.insert((*endpoint).to_string(), count);
    }
    for (endpoint, after_count) in &after.by_endpoint {
        if expected.contains(endpoint.as_str()) {
            continue;
        }
        let before_count = before.by_endpoint.get(endpoint).copied().unwrap_or(0);
        let count = after_count.checked_sub(before_count).ok_or_else(|| {
            format!("Backend metric counter reset for {endpoint}: before={before_count}, after={after_count}")
        })?;
        if count > 0 {
            return Err(format!("Observed {count} requests on unexpected endpoint {endpoint}"));
        }
    }

    let observed_requests: u64 = delta.values().sum();
    if observed_requests != expected_requests {
        return Err(format!(
            "Expected {expected_requests} endpoint-attributed requests, got {observed_requests}: {delta:?}"
        ));
    }
    if expected_endpoints.is_empty() || expected_requests % expected_endpoints.len() as u64 != 0 {
        return Err("RoundRobin test fixture must divide requests evenly across endpoints".to_string());
    }
    let expected_per_endpoint = expected_requests / expected_endpoints.len() as u64;
    if let Some((endpoint, count)) = delta.iter().find(|(_, count)| **count != expected_per_endpoint) {
        return Err(format!(
            "RoundRobin distribution is unbalanced: expected {expected_per_endpoint} requests per endpoint, {endpoint} got {count}; distribution={delta:?}"
        ));
    }
    Ok(delta)
}

async fn send_successful_requests(
    ctx: &TestContext,
    host: &str,
    trace_prefix: &str,
    request_count: u64,
    start: Instant,
) -> Result<(), TestResult> {
    for index in 0..request_count {
        let trace_id = format!("{trace_prefix}-{index:04}-{}", uuid::Uuid::new_v4());
        let response = ctx
            .http_client
            .get(format!("{}/test", ctx.http_url()))
            .header("host", host)
            .header(
                "traceparent",
                crate::access_log_client::traceparent_for_marker(trace_id),
            )
            .send()
            .await
            .map_err(|error| {
                TestResult::failed(
                    start.elapsed(),
                    format!("RoundRobin request {index}/{request_count} failed: {error}"),
                )
            })?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(TestResult::failed(
                start.elapsed(),
                format!(
                    "RoundRobin request {index}/{request_count} returned HTTP {} instead of 200",
                    response.status()
                ),
            ));
        }
        response.bytes().await.map_err(|error| {
            TestResult::failed(
                start.elapsed(),
                format!("RoundRobin request {index}/{request_count} response body failed: {error}"),
            )
        })?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_round_robin_case(
    ctx: TestContext,
    host: &str,
    trace_prefix: &str,
    request_count: u64,
    local_endpoints: &[&str],
    k8s_endpoints_env: &str,
    expected_endpoint_count: usize,
    expected_service: &str,
) -> TestResult {
    let start = Instant::now();
    let k8s_mode = std::env::var("EDGION_TEST_K8S_MODE").is_ok_and(|value| value == "true");
    let k8s_endpoint_ips = std::env::var(k8s_endpoints_env).ok();
    let expected_endpoints = match resolve_expected_endpoints(
        k8s_mode,
        k8s_endpoint_ips.as_deref(),
        local_endpoints,
        expected_endpoint_count,
    ) {
        Ok(endpoints) => endpoints,
        Err(error) => return TestResult::failed(start.elapsed(), error),
    };
    let expected_endpoint_refs: Vec<&str> = expected_endpoints.iter().map(String::as_str).collect();

    let before_warmup = match ctx.analyze_lb_distribution(METRICS_TEST_KEY).await {
        Ok(analysis) => analysis,
        Err(error) => {
            return TestResult::failed(start.elapsed(), format!("Initial metrics snapshot failed: {error}"));
        }
    };
    if let Err(result) = send_successful_requests(
        &ctx,
        host,
        &format!("{trace_prefix}-warmup"),
        expected_endpoint_count as u64,
        start,
    )
    .await
    {
        return result;
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let before = match ctx.analyze_lb_distribution(METRICS_TEST_KEY).await {
        Ok(analysis) => analysis,
        Err(error) => {
            return TestResult::failed(start.elapsed(), format!("Warmup metrics snapshot failed: {error}"));
        }
    };
    if let Err(error) = distribution_delta(
        &before_warmup,
        &before,
        &expected_endpoint_refs,
        expected_service,
        expected_endpoint_count as u64,
    ) {
        return TestResult::failed(start.elapsed(), format!("RoundRobin warmup failed: {error}"));
    }

    if let Err(result) = send_successful_requests(&ctx, host, trace_prefix, request_count, start).await {
        return result;
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    let after = match ctx.analyze_lb_distribution(METRICS_TEST_KEY).await {
        Ok(analysis) => analysis,
        Err(error) => {
            return TestResult::failed(start.elapsed(), format!("Final metrics snapshot failed: {error}"));
        }
    };

    match distribution_delta(
        &before,
        &after,
        &expected_endpoint_refs,
        expected_service,
        request_count,
    ) {
        Ok(distribution) => TestResult::passed_with_message(
            start.elapsed(),
            format!("RoundRobin distributed {request_count} successful sequential requests exactly: {distribution:?}"),
        ),
        Err(error) => TestResult::failed(start.elapsed(), error),
    }
}

impl LBRoundRobinTestSuite {
    fn test_roundrobin_eps() -> TestCase {
        TestCase::new(
            "roundrobin_eps",
            "Verify RoundRobin LB with EndpointSlice (default, single slice)",
            |ctx| {
                Box::pin(run_round_robin_case(
                    ctx,
                    "lb-rr-eps.example.com",
                    "rr-eps",
                    12,
                    &["127.0.0.1:30001", "127.0.0.2:30001", "127.0.0.3:30001"],
                    "EDGION_TEST_LB_RR_EPS_ENDPOINT_IPS",
                    3,
                    "edgion-default/lb-rr-service",
                ))
            },
        )
    }

    fn test_roundrobin_multi_slice() -> TestCase {
        TestCase::new(
            "roundrobin_multi_slice",
            "Verify RoundRobin LB across multiple EndpointSlices (2 slices, 4 backends)",
            |ctx| {
                Box::pin(run_round_robin_case(
                    ctx,
                    "lb-rr-multi.example.com",
                    "rr-multi",
                    16,
                    &[
                        "127.0.0.1:30001",
                        "127.0.0.2:30001",
                        "127.0.0.3:30001",
                        "127.0.0.4:30001",
                    ],
                    "EDGION_TEST_LB_RR_MULTI_ENDPOINT_IPS",
                    4,
                    "edgion-default/lb-rr-multi-service",
                ))
            },
        )
    }
}

#[async_trait]
impl TestSuite for LBRoundRobinTestSuite {
    fn name(&self) -> &str {
        "LB RoundRobin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_roundrobin_eps(), Self::test_roundrobin_multi_slice()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn analysis(service: &str, counts: &[(&str, u64)]) -> LbDistributionAnalysis {
        let total = counts.iter().map(|(_, count)| count).sum();
        LbDistributionAnalysis {
            total_requests: total,
            by_endpoint: counts.iter().map(|(key, count)| ((*key).to_string(), *count)).collect(),
            by_service: [(service.to_string(), total)].into_iter().collect(),
            ratio_by_endpoint: HashMap::new(),
            is_balanced: false,
            max_variance: 0.0,
        }
    }

    #[test]
    fn exact_counter_delta_passes() {
        let before = analysis("ns/svc", &[("a:1", 50), ("b:1", 40), ("c:1", 30)]);
        let after = analysis("ns/svc", &[("a:1", 54), ("b:1", 44), ("c:1", 34)]);
        let delta = distribution_delta(&before, &after, &["a:1", "b:1", "c:1"], "ns/svc", 12).unwrap();
        assert_eq!(delta.values().copied().collect::<Vec<_>>(), vec![4, 4, 4]);
    }

    #[test]
    fn missing_unbalanced_and_unexpected_traffic_fail() {
        let before = analysis("ns/svc", &[]);
        let missing = analysis("ns/svc", &[("a:1", 4), ("b:1", 4), ("c:1", 3)]);
        assert!(distribution_delta(&before, &missing, &["a:1", "b:1", "c:1"], "ns/svc", 12).is_err());
        let unbalanced = analysis("ns/svc", &[("a:1", 8), ("b:1", 2), ("c:1", 2)]);
        assert!(distribution_delta(&before, &unbalanced, &["a:1", "b:1", "c:1"], "ns/svc", 12).is_err());
        let unexpected = analysis("ns/svc", &[("a:1", 4), ("b:1", 4), ("c:1", 3), ("other:1", 1)]);
        assert!(distribution_delta(&before, &unexpected, &["a:1", "b:1", "c:1"], "ns/svc", 12).is_err());
    }

    #[test]
    fn wrong_service_and_unattributed_metric_fail() {
        let before = analysis("ns/svc", &[]);
        let wrong = analysis("ns/other", &[("a:1", 4), ("b:1", 4), ("c:1", 4)]);
        assert!(distribution_delta(&before, &wrong, &["a:1", "b:1", "c:1"], "ns/svc", 12).is_err());
        let mut unattributed = analysis("ns/svc", &[("a:1", 4), ("b:1", 4), ("c:1", 4)]);
        unattributed.total_requests = 13;
        assert!(distribution_delta(&before, &unattributed, &["a:1", "b:1", "c:1"], "ns/svc", 12).is_err());
    }

    #[test]
    fn endpoint_resolution_is_topology_aware_and_fail_closed() {
        assert_eq!(
            resolve_expected_endpoints(false, None, &["a:1", "b:1"], 2).unwrap(),
            vec!["a:1".to_string(), "b:1".to_string()]
        );
        assert_eq!(
            resolve_expected_endpoints(true, Some("10.0.0.2,10.0.0.3"), &[], 2).unwrap(),
            vec!["10.0.0.2:30001".to_string(), "10.0.0.3:30001".to_string()]
        );
        assert!(resolve_expected_endpoints(true, None, &[], 3).is_err());
        assert!(resolve_expected_endpoints(true, Some("10.0.0.2,10.0.0.2"), &[], 2).is_err());
    }

    #[test]
    fn integration_gateway_configs_enable_backend_identity_metrics() {
        let local_starter = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/utils/start_all_with_conf.sh"
        ));
        let k8s_config = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/k8s/kubernetes/gateway/configmap.yaml"
        ));

        assert!(local_starter.contains("backend_requests:\\n    labels:\\n      backend: true"));
        assert!(k8s_config.contains("backend_requests:\n        labels:\n          backend: true"));
    }
}
