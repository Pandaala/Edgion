// ============================================================================
// LB LeastConn Test Suite — regression for P0 g1-001
// ============================================================================
//
// Validates LBPolicyLeastConn distributes load across endpoints by sending
// concurrent slow requests and asserting the load lands on more than one
// backend. Regression for P0 g1-001 (issue: per-request LB metadata silently
// dropped on first attempt). Without that fix, runtime_state::increment is
// never called, all backends report count=0 forever, the LeastConn picker's
// min-heap tiebreak picks index 0, and 100% of traffic goes to a single
// endpoint — endpoint_count would be 1.
//
// Gateway annotations enable test-mode metrics:
//   edgion.io/metrics-test-key: "lb-lc-test"
//   edgion.io/metrics-test-type: "lb"

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::MetricsClient;
use async_trait::async_trait;
use std::time::Instant;

pub struct LBLeastConnTestSuite;

impl LBLeastConnTestSuite {
    /// 12 concurrent /delay/1 requests should distribute across multiple endpoints.
    /// Bug regression: with the bug, all 12 would land on a single endpoint.
    fn test_leastconn_distributes_concurrent() -> TestCase {
        TestCase::new(
            "leastconn_distributes_concurrent",
            "Concurrent slow requests to LeastConn route distribute across multiple endpoints",
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
                        let url = format!("http://{}:31122/delay/1", ctx.target_host);
                        let trace_id = format!("lc-{:04}", i);

                        let task = tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-lc-eps.example.com")
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

                    // Allow logging stage to flush metrics
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    let metrics_client = MetricsClient::from_host_port(&ctx.target_host, 12090);
                    match metrics_client.analyze_lb_distribution("lb-lc-test").await {
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

                            // The g1-001 bug fix is what allows distribution > 1 for
                            // LeastConn under concurrency. Pre-fix, all 12 requests
                            // would land on the first listed endpoint (counter stuck
                            // at 0 → min-heap tiebreak by index).
                            if endpoint_count < 2 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "LeastConn collapsed to a single endpoint (regression of P0 g1-001): \
                                         {} requests on [{}]",
                                        analysis.total_requests,
                                        distribution.join(", ")
                                    ),
                                );
                            }

                            let msg = format!(
                                "LeastConn: {} requests across {} endpoints [{}], balanced={}, variance={:.2}%",
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
impl TestSuite for LBLeastConnTestSuite {
    fn name(&self) -> &str {
        "LB LeastConn Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_leastconn_distributes_concurrent()]
    }
}
