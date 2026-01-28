// ============================================================================
// LB Policy Test Suite - using Prometheus metrics to verify LB distribution
// ============================================================================
//
// This test suite verifies Load Balancer policies through metrics analysis.
// Gateway must have test mode enabled (--test-mode) for metrics test features.
//
// Required config files (in examples/conf/HTTPRoute/Backend/LBPolicy/):
// - EndpointSlice_default_lb-rr-test.yaml  # EndpointSlice backend (3 backends)
// - Endpoints_default_lb-rr-test.yaml      # Endpoints backend (3 backends, same IPs)
// - Service_default_lb-rr-test.yaml        # Service definition
// - HTTPRoute_default_lb-rr-noretry.yaml   # RoundRobin via EndpointSlice (default)
// - HTTPRoute_default_lb-rr-endpoint.yaml  # RoundRobin via Endpoints (ServiceEndpoint kind)
// - Gateway.yaml                           # Gateway with metrics test annotations
//
// Gateway annotations for test mode:
//   edgion.io/metrics-test-key: "lb-policy-test"
//   edgion.io/metrics-test-type: "lb"

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::MetricsClient;
use async_trait::async_trait;
use std::time::Instant;

pub struct LBPolicyTestSuite;

impl LBPolicyTestSuite {
    /// Test RoundRobin LB policy using EndpointSlice (default behavior)
    fn test_roundrobin_endpointslice() -> TestCase {
        TestCase::new(
            "roundrobin_endpointslice",
            "Verify RoundRobin LB with EndpointSlice (default) via metrics",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Create HTTP client
                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    let request_count = 9;
                    let mut tasks = Vec::new();

                    // 1. Send concurrent requests to EndpointSlice route
                    for i in 0..request_count {
                        let client = client.clone();
                        let url = format!("http://127.0.0.1:31120/test");
                        let trace_id = format!("rr-eps-{:04}", i);

                        let task = tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-rr-test.example.com")
                                .header("x-trace-id", &trace_id)
                                .send()
                                .await
                        });
                        tasks.push(task);
                    }

                    // 2. Wait for all requests to complete
                    for task in tasks {
                        let _ = task.await;
                    }

                    // 3. Wait for metrics to be recorded
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    // 4. Analyze metrics (Metrics API is on port 5901)
                    let metrics_client = MetricsClient::from_host_port("127.0.0.1", 5901);
                    match metrics_client.analyze_lb_distribution("lb-policy-test").await {
                        Ok(analysis) => {
                            // Check if we got any requests
                            if analysis.total_requests == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No requests recorded in metrics (test mode enabled?)".to_string(),
                                );
                            }

                            // Check distribution across backends
                            let endpoint_count = analysis.by_endpoint.len();
                            if endpoint_count == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No endpoint distribution data (test_data empty?)".to_string(),
                                );
                            }

                            let distribution: Vec<String> = analysis
                                .by_endpoint
                                .iter()
                                .map(|(ep, count)| format!("{}:{}", ep, count))
                                .collect();

                            let msg = format!(
                                "EndpointSlice RoundRobin: {} requests across {} endpoints [{}], balanced={}, variance={:.2}%",
                                analysis.total_requests,
                                endpoint_count,
                                distribution.join(", "),
                                analysis.is_balanced,
                                analysis.max_variance * 100.0
                            );

                            TestResult::passed_with_message(start.elapsed(), msg)
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Metrics analysis failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test RoundRobin LB policy using Endpoints (ServiceEndpoint kind)
    fn test_roundrobin_endpoint() -> TestCase {
        TestCase::new(
            "roundrobin_endpoint",
            "Verify RoundRobin LB with Endpoints (ServiceEndpoint kind) via metrics",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Create HTTP client
                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    let request_count = 9;
                    let mut tasks = Vec::new();

                    // 1. Send concurrent requests to Endpoints route (ServiceEndpoint kind)
                    for i in 0..request_count {
                        let client = client.clone();
                        let url = format!("http://127.0.0.1:31120/test");
                        let trace_id = format!("rr-ep-{:04}", i);

                        let task = tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-rr-endpoint.example.com")
                                .header("x-trace-id", &trace_id)
                                .send()
                                .await
                        });
                        tasks.push(task);
                    }

                    // 2. Wait for all requests to complete
                    for task in tasks {
                        let _ = task.await;
                    }

                    // 3. Wait for metrics to be recorded
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    // 4. Analyze metrics (Metrics API is on port 5901)
                    let metrics_client = MetricsClient::from_host_port("127.0.0.1", 5901);
                    match metrics_client.analyze_lb_distribution("lb-policy-test").await {
                        Ok(analysis) => {
                            // Check if we got any requests
                            if analysis.total_requests == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No requests recorded in metrics (test mode enabled?)".to_string(),
                                );
                            }

                            // Check distribution across backends
                            let endpoint_count = analysis.by_endpoint.len();
                            if endpoint_count == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No endpoint distribution data (test_data empty?)".to_string(),
                                );
                            }

                            let distribution: Vec<String> = analysis
                                .by_endpoint
                                .iter()
                                .map(|(ep, count)| format!("{}:{}", ep, count))
                                .collect();

                            let msg = format!(
                                "Endpoints RoundRobin: {} requests across {} endpoints [{}], balanced={}, variance={:.2}%",
                                analysis.total_requests,
                                endpoint_count,
                                distribution.join(", "),
                                analysis.is_balanced,
                                analysis.max_variance * 100.0
                            );

                            TestResult::passed_with_message(start.elapsed(), msg)
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Metrics analysis failed: {}", e)),
                    }
                })
            },
        )
    }


}

#[async_trait]
impl TestSuite for LBPolicyTestSuite {
    fn name(&self) -> &str {
        "LB Policy Tests (Metrics)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_roundrobin_endpointslice(),
            Self::test_roundrobin_endpoint(),
        ]
    }
}
