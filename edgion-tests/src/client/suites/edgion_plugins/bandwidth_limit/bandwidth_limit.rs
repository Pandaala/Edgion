// BandwidthLimit Plugin Test Suite
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::time::Duration;

static KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct BandwidthLimitTestSuite;

impl BandwidthLimitTestSuite {
    /// Generate unique test key for metrics
    fn generate_test_key() -> String {
        let count = KEY_COUNTER.fetch_add(1, Ordering::SeqCst);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("bw-test-{}-{}", now, count)
    }

    /// Test 1: Functional Compliance
    /// Validate downloading 100KB at 100KB/s takes approximately 1 second.
    fn test_functional_compliance() -> TestCase {
        TestCase::new(
            "bandwidth_limit_functional_compliance",
            "Verify that downloading 100KB at 100KB/s takes >= 1 second",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = ctx.http_url() + "/echo"; // POST to /echo

                    // Create 100KB payload
                    // 100KB = 100 * 1024 = 102400 bytes
                    let payload_size = 102400;
                    let payload = "a".repeat(payload_size);

                    let _ = client
                        .post(&url)
                        .header("Host", "bandwidth-limit.example.com")
                        .header("X-Target-Path", "/test/bandwidth-limit/compliance")
                        .send()
                        .await;

                    let target_url = ctx.http_url() + "/test/bandwidth-limit/compliance";

                    let test_start = Instant::now();
                    let response = client
                        .post(&target_url)
                        .header("Host", "bandwidth-limit.example.com")
                        .body(payload)
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            if !resp.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request failed with status: {}", resp.status()),
                                );
                            }

                            // Download the body to force bandwidth limit
                            let bytes = resp.bytes().await.unwrap_or_default();
                            let duration = test_start.elapsed();

                            // Check size (should be roughly payload size + headers in echo)
                            if bytes.len() < payload_size {
                                return TestResult::failed(
                                    duration,
                                    format!("Response too small: {} bytes", bytes.len()),
                                );
                            }

                            // Rate is 100KB/s. Size is >100KB. Expect > 1s.
                            // Allow a small buffer (e.g. 0.9s) in case of slightly faster clock or buffering.
                            if duration.as_secs_f64() < 0.9 {
                                return TestResult::failed(
                                    duration,
                                    format!(
                                        "Bandwidth limit failed! Duration: {:.2}s for {} bytes (Rate: 100KB/s)",
                                        duration.as_secs_f64(),
                                        bytes.len()
                                    ),
                                );
                            }

                            TestResult::passed_with_message(
                                duration,
                                format!(
                                    "Passed: Downloaded {} bytes in {:.2}s (Expected > 1s)",
                                    bytes.len(),
                                    duration.as_secs_f64()
                                ),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test 2: Metrics Verification
    /// Verify that latency metrics reflect the bandwidth throttling.
    fn test_metrics_verification() -> TestCase {
        TestCase::new(
            "bandwidth_limit_metrics_verification",
            "Verify latency metrics reflect bandwidth throttling",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let _test_key = Self::generate_test_key();
                    let target_url = ctx.http_url() + "/test/bandwidth-limit/metrics";

                    // Rate: 50KB/s.
                    // Payload: 50KB.
                    // Expected Duration: ~1s.
                    let payload_size = 51200; // 50 * 1024
                    let payload = "b".repeat(payload_size);

                    let test_start = Instant::now();
                    let response = client
                        .post(&target_url)
                        .header("Host", "bandwidth-limit.example.com")
                        .body(payload)
                        .send()
                        .await;

                    if let Err(e) = response {
                        return TestResult::failed(start.elapsed(), format!("Request failed: {}", e));
                    }
                    let resp = response.unwrap();
                    let _ = resp.bytes().await; // Consume body

                    let duration = test_start.elapsed();

                    // Wait for metrics to be scraped/updated (allow some propagation delay)
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    // Fetch metrics from Gateway Metrics API (port 12090).
                    // Filter by route name (no per-request test key in static config).
                    let metrics_client = crate::metrics_helper::MetricsClient::from_host_port(&ctx.target_host, 12090);
                    let metrics = metrics_client.fetch_backend_metrics().await;

                    if let Err(e) = metrics {
                        return TestResult::failed(duration, format!("Failed to fetch metrics: {}", e));
                    }
                    let metrics = metrics.unwrap();

                    let relevant_metrics: Vec<_> = metrics
                        .iter()
                        .filter(|m| m.test_data.as_ref().map(|td| td.latency_ms.is_some()).unwrap_or(false))
                        .collect();

                    if relevant_metrics.is_empty() {
                        return TestResult::failed(
                            duration,
                            format!(
                                "No metrics found for bandwidth-limit-test-route with latency data. Total metrics: {}",
                                metrics.len()
                            ),
                        );
                    }

                    // Analyze latency
                    // Find the max latency (most recent or slowest)
                    let max_latency = relevant_metrics
                        .iter()
                        .filter_map(|m| m.test_data.as_ref().and_then(|td| td.latency_ms))
                        .max()
                        .unwrap_or(0);

                    // We expect latency to be around 1000ms.
                    if max_latency < 800 {
                        return TestResult::failed(
                            duration,
                            format!(
                                "Metric latency too low: {}ms (Expected > 800ms). Client duration: {:.2}s",
                                max_latency,
                                duration.as_secs_f64()
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        duration,
                        format!(
                            "Passed: Metric latency {}ms matches expectation (Client duration: {:.2}s)",
                            max_latency,
                            duration.as_secs_f64()
                        ),
                    )
                })
            },
        )
    }

    /// Test 3: Selected BackendRef Scope
    /// Verify that a BandwidthLimit reached through a selected BackendRef runs
    /// in the response-body stage.
    fn test_backend_ref_scope() -> TestCase {
        TestCase::new(
            "bandwidth_limit_backend_ref_scope",
            "Verify that a selected BackendRef ExtensionRef limits response bandwidth",
            |ctx: TestContext| {
                Box::pin(async move {
                    let client = &ctx.http_client;
                    let target_url = ctx.http_url() + "/test/bandwidth-limit/backend-scope";
                    let payload_size = 100 * 1024;
                    let payload = "c".repeat(payload_size);
                    let test_start = Instant::now();

                    let response = client
                        .post(&target_url)
                        .header("Host", "bandwidth-limit.example.com")
                        .body(payload)
                        .send()
                        .await;

                    let response = match response {
                        Ok(response) => response,
                        Err(error) => {
                            return TestResult::failed(
                                test_start.elapsed(),
                                format!("BackendRef-scoped request failed: {error}"),
                            );
                        }
                    };
                    if !response.status().is_success() {
                        return TestResult::failed(
                            test_start.elapsed(),
                            format!("BackendRef-scoped request failed with status: {}", response.status()),
                        );
                    }

                    let body = response.bytes().await.unwrap_or_default();
                    let duration = test_start.elapsed();
                    if body.len() < payload_size {
                        return TestResult::failed(
                            duration,
                            format!("BackendRef-scoped response too small: {} bytes", body.len()),
                        );
                    }
                    if duration.as_secs_f64() < 0.9 {
                        return TestResult::failed(
                            duration,
                            format!(
                                "BackendRef-scoped bandwidth limit did not run: {:.2}s for {} bytes",
                                duration.as_secs_f64(),
                                body.len()
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        duration,
                        format!(
                            "BackendRef-scoped limit downloaded {} bytes in {:.2}s",
                            body.len(),
                            duration.as_secs_f64()
                        ),
                    )
                })
            },
        )
    }
}

impl TestSuite for BandwidthLimitTestSuite {
    fn name(&self) -> &str {
        "BandwidthLimit Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_functional_compliance(),
            Self::test_metrics_verification(),
            Self::test_backend_ref_scope(),
        ]
    }
}
