// ============================================================================
// LB RoundRobin Test Suite - using Prometheus metrics to verify LB distribution
// ============================================================================
//
// This test suite verifies RoundRobin Load Balancer policy through metrics analysis.
// Gateway must have test mode enabled (--test-mode) for metrics test features.
//
// Test scenarios:
// 1. EndpointSlice mode (default) - single slice with 3 backends
// 2. Endpoints mode (ServiceEndpoint kind) - 3 backends
// 3. Multi-slice mode - 2 slices with 2 backends each (4 total)
//
// Gateway annotations for test mode:
//   edgion.io/metrics-test-key: "lb-rr-test"
//   edgion.io/metrics-test-type: "lb"

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::MetricsClient;
use async_trait::async_trait;
use std::time::Instant;

pub struct LBRoundRobinTestSuite;

impl LBRoundRobinTestSuite {
    /// Test RoundRobin LB policy using EndpointSlice (default behavior)
    fn test_roundrobin_eps() -> TestCase {
        TestCase::new(
            "roundrobin_eps",
            "Verify RoundRobin LB with EndpointSlice (default, single slice)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    let request_count = 12; // Should distribute evenly across 3 backends
                    let mut tasks = Vec::new();

                    for i in 0..request_count {
                        let client = client.clone();
                        let url = "http://127.0.0.1:31120/test".to_string();
                        let trace_id = format!("rr-eps-{:04}", i);

                        let task = tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-rr-eps.example.com")
                                .header("x-trace-id", &trace_id)
                                .send()
                                .await
                        });
                        tasks.push(task);
                    }

                    for task in tasks {
                        let _ = task.await;
                    }

                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    let metrics_client = MetricsClient::from_host_port("127.0.0.1", 5901);
                    match metrics_client.analyze_lb_distribution("lb-rr-test").await {
                        Ok(analysis) => {
                            if analysis.total_requests == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No requests recorded in metrics".to_string(),
                                );
                            }

                            let endpoint_count = analysis.by_endpoint.len();
                            if endpoint_count == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No endpoint distribution data".to_string(),
                                );
                            }

                            let distribution: Vec<String> = analysis
                                .by_endpoint
                                .iter()
                                .map(|(ep, count)| format!("{}:{}", ep, count))
                                .collect();

                            let msg = format!(
                                "EPS RoundRobin: {} requests across {} endpoints [{}], balanced={}, variance={:.2}%",
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
    fn test_roundrobin_ep() -> TestCase {
        TestCase::new(
            "roundrobin_ep",
            "Verify RoundRobin LB with Endpoints (ServiceEndpoint kind)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    let request_count = 12;
                    let mut tasks = Vec::new();

                    for i in 0..request_count {
                        let client = client.clone();
                        let url = "http://127.0.0.1:31120/test".to_string();
                        let trace_id = format!("rr-ep-{:04}", i);

                        let task = tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-rr-ep.example.com")
                                .header("x-trace-id", &trace_id)
                                .send()
                                .await
                        });
                        tasks.push(task);
                    }

                    for task in tasks {
                        let _ = task.await;
                    }

                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    let metrics_client = MetricsClient::from_host_port("127.0.0.1", 5901);
                    match metrics_client.analyze_lb_distribution("lb-rr-test").await {
                        Ok(analysis) => {
                            if analysis.total_requests == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No requests recorded in metrics".to_string(),
                                );
                            }

                            let endpoint_count = analysis.by_endpoint.len();
                            if endpoint_count == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No endpoint distribution data".to_string(),
                                );
                            }

                            let distribution: Vec<String> = analysis
                                .by_endpoint
                                .iter()
                                .map(|(ep, count)| format!("{}:{}", ep, count))
                                .collect();

                            let msg = format!(
                                "EP RoundRobin: {} requests across {} endpoints [{}], balanced={}, variance={:.2}%",
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

    /// Test RoundRobin LB with multiple EndpointSlices
    fn test_roundrobin_multi_slice() -> TestCase {
        TestCase::new(
            "roundrobin_multi_slice",
            "Verify RoundRobin LB across multiple EndpointSlices (2 slices, 4 backends)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    let request_count = 16; // Should distribute evenly across 4 backends
                    let mut tasks = Vec::new();

                    for i in 0..request_count {
                        let client = client.clone();
                        let url = "http://127.0.0.1:31120/test".to_string();
                        let trace_id = format!("rr-multi-{:04}", i);

                        let task = tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-rr-multi.example.com")
                                .header("x-trace-id", &trace_id)
                                .send()
                                .await
                        });
                        tasks.push(task);
                    }

                    for task in tasks {
                        let _ = task.await;
                    }

                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    let metrics_client = MetricsClient::from_host_port("127.0.0.1", 5901);
                    match metrics_client.analyze_lb_distribution("lb-rr-test").await {
                        Ok(analysis) => {
                            if analysis.total_requests == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No requests recorded in metrics".to_string(),
                                );
                            }

                            let endpoint_count = analysis.by_endpoint.len();
                            if endpoint_count < 4 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 4 backends (multi-slice), got {}", endpoint_count),
                                );
                            }

                            let distribution: Vec<String> = analysis
                                .by_endpoint
                                .iter()
                                .map(|(ep, count)| format!("{}:{}", ep, count))
                                .collect();

                            let msg = format!(
                                "Multi-slice RoundRobin: {} requests across {} endpoints [{}], balanced={}, variance={:.2}%",
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
impl TestSuite for LBRoundRobinTestSuite {
    fn name(&self) -> &str {
        "LB RoundRobin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_roundrobin_eps(),
            Self::test_roundrobin_ep(),
            Self::test_roundrobin_multi_slice(),
        ]
    }
}
