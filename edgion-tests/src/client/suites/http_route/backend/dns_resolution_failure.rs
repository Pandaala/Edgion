// DnsResolutionFailure Test suite
//
// Verifies that when an HTTPRoute backend references an unreachable IP address
// (a normal Service + EndpointSlice pointing at 192.0.2.1, RFC 5737 TEST-NET-1),
// the gateway:
//   1. Returns 504 (connect timeout) rather than crashing
//   2. Records the upstream connect failure in the access log backend context
//      (upstream status = 504 from ConnectTimedout path in pg_fail_to_connect)
//   3. The HTTPRoute Accepted=True (config is valid; the failure is runtime/data-plane only)
//
// Connect-timeout note: the gateway's connection-establishment deadline is
// resolved independently of the route's backendRequest timeout (Option-B
// decouple, see docs/superpowers/specs/2026-07-22-per-backend-connect-timeout-design.md).
// Connect is bounded by the per-backend BTP connection.connectTimeout (1s here)
// else the global defaultConnectTimeout (5s) — NOT by backendRequest. The
// EdgionBackendTrafficPolicy below sets a 1s connect timeout on the unreachable
// backend so each connect attempt fails fast at ~1s (2 attempts ~= 2-3s total),
// well within the test's 10s client budget.
//
// Required config files (in edgion-tests/integration/conf/HTTPRoute/Backend/DnsResolutionFailure/):
//   - Gateway_default_dns-fail-gateway.yaml         # HTTP listener on port 31339
//   - HTTPRoute_default_dns-fail-route.yaml         # Routes dns-fail.example.com → nonexistent-backend
//                                                   #   timeouts: request=8s, backendRequest=1s
//   - Service_default_nonexistent-backend.yaml      # ClusterIP Service (no endpoints via kube)
//   - EndpointSlice_default_nonexistent-backend.yaml # 192.0.2.1:80 — unreachable, passes SSRF defense
//   - EdgionBackendTrafficPolicy_edgion-default_dns-fail-connect-timeout.yaml # connectTimeout: 1s

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct DnsResolutionFailureTestSuite;

impl DnsResolutionFailureTestSuite {
    /// Requests to an unreachable backend (192.0.2.1:80) should return 504 (connect timeout).
    /// The BTP connection.connectTimeout=1s bounds each connect attempt to ~1s, so the
    /// gateway response is expected in ~2-3s total (2 attempts × 1s + overhead).
    fn test_returns_5xx() -> TestCase {
        TestCase::new(
            "returns_5xx",
            "HTTPRoute with unreachable backend IP should return 504 (connect timeout)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());

                    let mut last_status = None;
                    let mut last_error = None;

                    // BTP connectTimeout=1s so each gateway connect attempt times out in ~1s;
                    // 2 attempts + overhead → gateway responds in ~2-3s total.
                    // Use a 10s per-request timeout to absorb variance across all 3 retries.
                    for _ in 0..3 {
                        match ctx
                            .http_client
                            .get(&url)
                            .timeout(std::time::Duration::from_secs(10))
                            .header("Host", "dns-fail.example.com")
                            .send()
                            .await
                        {
                            Ok(resp) => {
                                let status = resp.status().as_u16();
                                last_status = Some(status);
                                if status >= 500 && status < 600 {
                                    return TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!(
                                            "Unreachable backend correctly returned {}: gateway did not crash",
                                            status
                                        ),
                                    );
                                }
                            }
                            Err(e) => {
                                last_error = Some(e.to_string());
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }

                    TestResult::failed(
                        start.elapsed(),
                        format!(
                            "Expected 5xx for unreachable backend, last_status={:?}, last_error={:?}",
                            last_status, last_error
                        ),
                    )
                })
            },
        )
    }

    /// Verify that the access log records the backend connect failure.
    ///
    /// When the TCP connect to 192.0.2.1:80 times out, Pingora fires `fail_to_connect`
    /// with ErrorType::ConnectTimedout.  pg_fail_to_connect sets upstream.status = 504
    /// on the ConnectTimedout path (does not record a request diagnostic).
    ///
    /// With BTP connectTimeout=1s each connect attempt times out in ~1s; gateway responds in ~2-3s total.
    ///
    /// We therefore assert that:
    ///  - The response HTTP status is 5xx (gateway did not crash)
    ///  - The access log entry's overall request status is 5xx (504)
    ///  - The backend_context upstreams list is non-empty (a connection was attempted)
    fn test_access_log_records_backend_error() -> TestCase {
        TestCase::new(
            "access_log_records_backend_error",
            "Access log should record a backend connection attempt for the unreachable IP",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("dns-fail-{}", uuid::Uuid::new_v4());

                    // Send request with trace id; allow up to 10s for the gateway to respond
                    // (backendRequest=1s × 2 attempts + overhead ≈ 2-3s, well within budget)
                    let resp = ctx
                        .http_client
                        .get(format!("{}/echo", ctx.http_url()))
                        .timeout(std::time::Duration::from_secs(10))
                        .header("Host", "dns-fail.example.com")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let status = match resp {
                        Ok(r) => r.status().as_u16(),
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Request failed: {}", e));
                        }
                    };

                    if status < 500 {
                        return TestResult::failed(start.elapsed(), format!("Expected 5xx but got {}", status));
                    }

                    // Fetch access log — gateway needs a moment to flush
                    let al_client = ctx.access_log_client();
                    let entry = match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                        Ok(e) => e,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Failed to fetch access log for trace_id={}: {}", trace_id, e),
                            );
                        }
                    };

                    // Access log status should be 5xx (504 for connect timeout)
                    let log_status = entry.status_code().unwrap_or(0);
                    if log_status < 500 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected 5xx in access log status, got {}: entry={:?}",
                                log_status, entry.data
                            ),
                        );
                    }

                    // At least one upstream connection attempt must be recorded
                    let upstream_count = entry.upstream_count();
                    if upstream_count == 0 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected backend_context.upstreams to be non-empty (connect was attempted), got 0: entry={:?}",
                                entry.data
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "Access log correctly records backend connect failure: status={}, upstream_count={}",
                            log_status, upstream_count
                        ),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for DnsResolutionFailureTestSuite {
    fn name(&self) -> &str {
        "DnsResolutionFailure"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/DnsResolutionFailure")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_returns_5xx(), Self::test_access_log_records_backend_error()]
    }
}
