// ============================================================================
// LB EWMA + retry-on-status Test Suite — validates FinalizeMode::Discard behavior
// ============================================================================
//
// Setup: retry attempts=3, codes=[503], EWMA LB, 2 reachable endpoints.
//
// finalize_attempt(Discard) on LbCommit::Ewma must NOT call update_ewma on
// the abandoned backend. If it did, retried-away backends would accumulate
// inflated latency samples from partial connections, skewing subsequent EWMA
// selection. The test verifies that after a retry storm healthy traffic still
// reaches both endpoints (no score collapse to a single point).
//
// Gateway annotations enable test-mode metrics:
//   edgion.io/metrics-test-key: "lb-ewma-rs"
//   edgion.io/metrics-test-type: "lb"

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::MetricsClient;
use async_trait::async_trait;
use std::time::Instant;

pub struct LBEwmaRetryOnStatusTestSuite;

impl LBEwmaRetryOnStatusTestSuite {
    fn test_retry_storm_then_healthy_distribution() -> TestCase {
        TestCase::new(
            "ewma_retry_storm_then_healthy",
            "EWMA + retry-on-503 storm followed by healthy traffic still distributes across endpoints",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let port = 31126;

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(15))
                        .no_proxy()
                        .build()
                        .expect("Failed to create HTTP client");

                    // ── Phase 1: retry storm ────────────────────────────────────
                    // Each /status/503 response triggers up to 3 retries.
                    // begin_upstream_attempt calls finalize_attempt(Discard) on the
                    // previous attempt's LbCommit::Ewma — must NOT call update_ewma.
                    let storm_count = 8u32;
                    let mut tasks = Vec::with_capacity(storm_count as usize);
                    for i in 0..storm_count {
                        let client = client.clone();
                        let url = format!("http://{}:{}/status/503", ctx.target_host, port);
                        let trace_id = format!("ewma-rs-storm-{:04}", i);
                        tasks.push(tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-ewma-rs.example.com")
                                .header("x-trace-id", &trace_id)
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
                            format!("Phase1: unexpected non-503 responses: {:?}", storm_other),
                        );
                    }

                    // ── Phase 2: healthy traffic after the storm ────────────────
                    tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;

                    let happy_count = 8u32;
                    let mut h_tasks = Vec::with_capacity(happy_count as usize);
                    for i in 0..happy_count {
                        let client = client.clone();
                        let url = format!("http://{}:{}/delay/1", ctx.target_host, port);
                        let trace_id = format!("ewma-rs-happy-{:04}", i);
                        h_tasks.push(tokio::spawn(async move {
                            client
                                .get(&url)
                                .header("host", "lb-ewma-rs.example.com")
                                .header("x-trace-id", &trace_id)
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

                    // ── Metrics: healthy phase must hit >= 2 endpoints ──────────
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    let metrics = MetricsClient::from_host_port(&ctx.target_host, 12090);
                    let analysis = match metrics.analyze_lb_distribution("lb-ewma-rs").await {
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
                                "EWMA collapsed to single endpoint after retry storm — \
                                 possible FinalizeMode::Discard regression. endpoints=[{}], total={}",
                                dist.join(", "),
                                analysis.total_requests
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "storm: {} x 503 (retry exhausted) | happy: {} x 2xx | dist=[{}]",
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
impl TestSuite for LBEwmaRetryOnStatusTestSuite {
    fn name(&self) -> &str {
        "LB EWMA + RetryOnStatus Tests"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/LBEwmaRetryOnStatus")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_retry_storm_then_healthy_distribution()]
    }
}
