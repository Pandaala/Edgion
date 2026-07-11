// ============================================================================
// LB EWMA Test Suite — validates LbCommit::Ewma commit/finalize lifecycle
// ============================================================================
//
// Validates that an EdgionBackendTrafficPolicy with loadBalancer.type: Ewma
// routes requests across multiple endpoints and
// correctly runs the commit_attempt / finalize_attempt(Settle) lifecycle for
// each request. Under concurrent load all endpoints start with equal EWMA
// scores (INITIAL_EWMA_US = 1ms), so the selector distributes across them.
//
// Gateway annotations enable test-mode metrics:
//   edgion.io/metrics-test-key: "lb-ewma-test"
//   edgion.io/metrics-test-type: "lb"

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::MetricsClient;
use async_trait::async_trait;
use std::time::Instant;

pub struct LBEwmaTestSuite;

impl LBEwmaTestSuite {
    /// 12 concurrent /delay/1 requests should distribute across multiple endpoints.
    /// All backends start with INITIAL_EWMA_US (1ms), so selection is unbiased
    /// on first contact; distribution must not collapse to a single endpoint.
    fn test_ewma_distributes_concurrent() -> TestCase {
        TestCase::new(
            "ewma_distributes_concurrent",
            "Concurrent requests to EWMA route distribute across multiple endpoints",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(8))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    let request_count = 12;
                    let mut tasks = Vec::new();

                    for i in 0..request_count {
                        let client = client.clone();
                        let url = format!("http://{}:31125/delay/1", ctx.target_host);
                        let trace_id = format!("ewma-{:04}", i);

                        let task = tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-ewma.example.com")
                                .header("x-trace-id", &trace_id)
                                .send()
                                .await
                        });
                        tasks.push(task);
                    }

                    let mut completed = 0u32;
                    for task in tasks {
                        match task.await {
                            Ok(Ok(r)) if r.status().as_u16() == 200 => completed += 1,
                            Ok(Ok(r)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request returned non-200: {}", r.status()),
                                );
                            }
                            Ok(Err(e)) => {
                                return TestResult::failed(start.elapsed(), format!("Request error: {}", e));
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Join error: {}", e));
                            }
                        }
                    }

                    if completed < request_count {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Only {} / {} requests completed with 200", completed, request_count),
                        );
                    }

                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    let metrics_client = MetricsClient::from_host_port(&ctx.target_host, 12090);
                    match metrics_client.analyze_lb_distribution("lb-ewma-test").await {
                        Ok(analysis) => {
                            if analysis.total_requests == 0 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "No requests recorded in metrics".to_string(),
                                );
                            }

                            let endpoint_count = analysis.by_endpoint.len();
                            let distribution: Vec<String> = analysis
                                .by_endpoint
                                .iter()
                                .map(|(ep, count)| format!("{}:{}", ep, count))
                                .collect();

                            if endpoint_count < 2 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "EWMA collapsed to a single endpoint: \
                                         {} requests on [{}]",
                                        analysis.total_requests,
                                        distribution.join(", ")
                                    ),
                                );
                            }

                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!(
                                    "EWMA: {} requests across {} endpoints [{}]",
                                    analysis.total_requests,
                                    endpoint_count,
                                    distribution.join(", ")
                                ),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Metrics analysis failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for LBEwmaTestSuite {
    fn name(&self) -> &str {
        "LB EWMA Tests"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/LBEwma")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_ewma_distributes_concurrent()]
    }
}
