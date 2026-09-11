// ============================================================================
// LB LeastConn + retry-on-status Test Suite
// ============================================================================
//
// Setup: retry attempts=3, codes=[503], LeastConn LB, 2 reachable endpoints.
// All endpoints point at the same test_server (bound to 0.0.0.0:30001) but use
// 127.0.0.1 / 127.0.0.2 so the LeastConn picker sees two independent backends.
//
// Paths exercised:
//   1. 503 retry triggers begin_upstream_attempt cleanup (take
//      leastconn_incremented_backend) and the final logging cleanup; count is
//      neither over- nor under-decremented.
//   2. Happy-path traffic after a retry storm still distributes across both
//      endpoints, proving the LeastConn ledger was not biased by the retry path.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::MetricsClient;
use async_trait::async_trait;
use std::time::Instant;

pub struct LBLeastConnRetryOnStatusTestSuite;

impl LBLeastConnRetryOnStatusTestSuite {
    /// 8 concurrent requests to /status/503 trigger retry exhaust (status 503);
    /// then 8 /delay/1 happy-path requests verify LB still distributes properly.
    fn test_retry_storm_then_healthy_distribution() -> TestCase {
        TestCase::new(
            "leastconn_retry_storm_then_healthy",
            "LeastConn + retry-on-503 storm followed by healthy traffic still distributes across endpoints",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let port = 31124;

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(15))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    // ── Phase 1: retry storm ────────────────────────────────────
                    // /status/503 returns 503 on every attempt; after retry exhausts
                    // the client sees 503. This path repeatedly exercises
                    // begin_upstream_attempt's LeastConn cleanup.
                    let storm_count = 8u32;
                    let mut tasks = Vec::with_capacity(storm_count as usize);
                    for i in 0..storm_count {
                        let client = client.clone();
                        let url = format!("http://{}:{}/status/503", ctx.target_host, port);
                        let trace_id = format!("lc-rs-storm-{:04}", i);
                        tasks.push(tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-lc-rs.example.com")
                                .header(
                                    "traceparent",
                                    crate::access_log_client::traceparent_for_marker(&trace_id),
                                )
                                .send()
                                .await
                                .map(|r| r.status().as_u16())
                        }));
                    }

                    let mut storm_503 = 0u32;
                    let mut storm_other = Vec::new();
                    for t in tasks {
                        match t.await {
                            Ok(Ok(503)) => storm_503 += 1,
                            Ok(Ok(s)) => storm_other.push(s),
                            Ok(Err(e)) => {
                                return TestResult::failed(start.elapsed(), format!("storm req error: {}", e));
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("storm join error: {}", e));
                            }
                        }
                    }

                    if storm_503 == 0 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Phase1: expected all 503 after retry exhaust, none observed (other={:?})",
                                storm_other
                            ),
                        );
                    }
                    if !storm_other.is_empty() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Phase1: unexpected non-503 responses: {:?} (expected all 503 after retry)",
                                storm_other
                            ),
                        );
                    }

                    // ── Phase 2: wait for logging flush, then send happy-path ───
                    tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;

                    let happy_count = 8u32;
                    let mut h_tasks = Vec::with_capacity(happy_count as usize);
                    for i in 0..happy_count {
                        let client = client.clone();
                        let url = format!("http://{}:{}/delay/1", ctx.target_host, port);
                        let trace_id = format!("lc-rs-happy-{:04}", i);
                        h_tasks.push(tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-lc-rs.example.com")
                                .header(
                                    "traceparent",
                                    crate::access_log_client::traceparent_for_marker(&trace_id),
                                )
                                .send()
                                .await
                                .map(|r| r.status().as_u16())
                        }));
                    }

                    let mut happy_ok = 0u32;
                    for t in h_tasks {
                        match t.await {
                            Ok(Ok(s)) if (200..300).contains(&s) => happy_ok += 1,
                            Ok(Ok(s)) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Phase2: unexpected non-2xx status: {}", s),
                                );
                            }
                            Ok(Err(e)) => {
                                return TestResult::failed(start.elapsed(), format!("happy req error: {}", e));
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("happy join error: {}", e));
                            }
                        }
                    }

                    if happy_ok != happy_count {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Phase2: expected {} 2xx, got {}", happy_count, happy_ok),
                        );
                    }

                    // ── Metrics check: happy-path phase must hit at least 2 endpoints ──
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    let metrics = MetricsClient::from_host_port(&ctx.target_host, 12090);
                    let analysis = match metrics.analyze_lb_distribution("lb-lc-rs").await {
                        Ok(a) => a,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("metrics fetch failed: {}", e)),
                    };

                    let endpoint_count = analysis.by_endpoint.len();
                    let dist: Vec<String> = analysis
                        .by_endpoint
                        .iter()
                        .map(|(ep, c)| format!("{}={}", ep, c))
                        .collect();

                    if endpoint_count < 2 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "LB collapsed to single endpoint after retry storm — \
                                 likely g1-007 ledger drift. endpoints=[{}], total={}",
                                dist.join(", "),
                                analysis.total_requests
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "storm: {} × 503 (retry exhausted) | happy: {} × 2xx | dist=[{}]",
                            storm_503,
                            happy_ok,
                            dist.join(", ")
                        ),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for LBLeastConnRetryOnStatusTestSuite {
    fn name(&self) -> &str {
        "LB LeastConn + RetryOnStatus Tests"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/LBLeastConnRetryOnStatus")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_retry_storm_then_healthy_distribution()]
    }
}
