// PerTryTimeout Test suite
//
// Tests GEP-1731 per-try timeout: when an upstream attempt exceeds
// timeouts.backendRequest and the rule has a retry policy, the gateway
// cancels the attempt and retries. Without a retry policy the timeout
// is final and the client gets 504.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};

pub struct PerTryTimeoutTestSuite;

impl PerTryTimeoutTestSuite {
    /// Test: first attempt sleeps 3s (> backendRequest 1s) — canceled and retried, 2nd attempt succeeds
    fn test_slow_attempt_is_retried() -> TestCase {
        TestCase::new(
            "slow_attempt_is_retried",
            "First attempt sleeps 3s, per-try timeout 1s cancels it — retry succeeds with 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_id = format!("ptt-retried-{}", uuid::Uuid::new_v4());

                    // Reset state
                    let _ = ctx
                        .http_client
                        .get(format!("{}/retry-sequence-reset", ctx.http_url()))
                        .header("Host", "per-try.example.com")
                        .send()
                        .await;

                    let req_start = Instant::now();
                    // First entry's status code is irrelevant — the attempt is
                    // canceled at the 1s backendRequest bound before it responds.
                    let response = ctx
                        .http_client
                        .get(format!("{}/retry-sequence/{}/200d3000,200", ctx.http_url(), test_id))
                        .header("Host", "per-try.example.com")
                        .header("x-trace-id", &test_id)
                        .header("access_log", "test_store")
                        .timeout(Duration::from_secs(15))
                        .send()
                        .await;
                    let elapsed = req_start.elapsed();

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let attempt = resp
                                .headers()
                                .get("x-attempt-number")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 200 after per-try timeout retry, got {} (elapsed {:?})",
                                        status, elapsed
                                    ),
                                );
                            }
                            if attempt != "2" {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected x-attempt-number=2 (retry served), got '{}'", attempt),
                                );
                            }
                            if elapsed >= Duration::from_millis(2900) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Elapsed {:?} >= 2900ms — first attempt was not canceled at ~1s per-try timeout",
                                        elapsed
                                    ),
                                );
                            }

                            // Verify the retry was Case 1.5 (per-try timeout), not some
                            // other retry path: the access-log error chain must carry
                            // BackendRequestTimeoutRetry for the canceled attempt.
                            let al_client = ctx.access_log_client();
                            if let Ok(entry) = al_client.get_access_log_with_retry(&test_id, 10, 200).await {
                                let has_marker = entry.data["errors"].as_array().is_some_and(|errs| {
                                    errs.iter().any(|e| e.as_str() == Some("BackendRequestTimeoutRetry"))
                                });
                                if !has_marker {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "Access log errors chain missing BackendRequestTimeoutRetry: {}",
                                            entry.data["errors"]
                                        ),
                                    );
                                }
                            }

                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!(
                                    "Got 200 on attempt 2 in {:?} — slow attempt canceled and retried (504_03 logged)",
                                    elapsed
                                ),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: same slow backend but route has no retry policy — per-try timeout is final 504
    fn test_no_retry_without_policy() -> TestCase {
        TestCase::new(
            "no_retry_without_policy",
            "No retry policy — backendRequest timeout yields final 504 at ~1s",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_id = format!("ptt-noretry-{}", uuid::Uuid::new_v4());

                    // Reset state
                    let _ = ctx
                        .http_client
                        .get(format!("{}/retry-sequence-reset", ctx.http_url()))
                        .header("Host", "per-try-noretry.example.com")
                        .send()
                        .await;

                    let req_start = Instant::now();
                    let response = ctx
                        .http_client
                        .get(format!("{}/retry-sequence/{}/200d3000,200", ctx.http_url(), test_id))
                        .header("Host", "per-try-noretry.example.com")
                        .header("x-trace-id", &test_id)
                        .timeout(Duration::from_secs(15))
                        .send()
                        .await;
                    let elapsed = req_start.elapsed();

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 504 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 504 (timeout final, no retry), got {} (elapsed {:?})",
                                        status, elapsed
                                    ),
                                );
                            }
                            if elapsed >= Duration::from_millis(2900) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Elapsed {:?} >= 2900ms — timeout should be final at ~1s without retry",
                                        elapsed
                                    ),
                                );
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Got 504 in {:?} — timeout final without retry policy", elapsed),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: every attempt is slow — retries exhausted (1 initial + 2 retries), final 504
    fn test_exhaustion_yields_504() -> TestCase {
        TestCase::new(
            "exhaustion_yields_504",
            "Every attempt sleeps 3s — 3 attempts each canceled at ~1s, final 504",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_id = format!("ptt-exhausted-{}", uuid::Uuid::new_v4());

                    // Reset state
                    let _ = ctx
                        .http_client
                        .get(format!("{}/retry-sequence-reset", ctx.http_url()))
                        .header("Host", "per-try.example.com")
                        .send()
                        .await;

                    let req_start = Instant::now();
                    let response = ctx
                        .http_client
                        .get(format!("{}/retry-sequence/{}/200d3000", ctx.http_url(), test_id))
                        .header("Host", "per-try.example.com")
                        .header("x-trace-id", &test_id)
                        .timeout(Duration::from_secs(15))
                        .send()
                        .await;
                    let elapsed = req_start.elapsed();

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 504 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 504 after retry exhaustion, got {} (elapsed {:?})",
                                        status, elapsed
                                    ),
                                );
                            }
                            if elapsed < Duration::from_millis(2700) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Elapsed {:?} < 2700ms — expected ~3 attempts x ~1s per-try timeout",
                                        elapsed
                                    ),
                                );
                            }
                            if elapsed >= Duration::from_secs(9) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Elapsed {:?} >= 9s — attempts were not canceled at the ~1s per-try timeout",
                                        elapsed
                                    ),
                                );
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Got 504 in {:?} — 3 attempts each canceled at ~1s", elapsed),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
    /// Test: client gives up at 500ms while every attempt sleeps 3s. Probed
    /// behavior (Pingora 0.8 h1): the downstream EOF is detected almost
    /// immediately — the gateway aborts the in-flight first attempt (status
    /// 499, single upstreams entry) instead of running the retry loop to
    /// exhaustion, and fail_to_proxy records the ClientClosed marker so the
    /// abandoned work is analyzable offline (log-only decision: the backoff
    /// TODO in pg_upstream_peer.rs is intentionally NOT implemented).
    fn test_client_disconnect_is_logged() -> TestCase {
        TestCase::new(
            "client_disconnect_is_logged",
            "Client disconnects at 500ms — access log records ClientClosed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_id = format!("ptt-clientclosed-{}", uuid::Uuid::new_v4());

                    // Reset state
                    let _ = ctx
                        .http_client
                        .get(format!("{}/retry-sequence-reset", ctx.http_url()))
                        .header("Host", "per-try.example.com")
                        .send()
                        .await;

                    // Every attempt sleeps 3s (per-try timeout would cancel each
                    // at ~1s); the client aborts at 500ms, before the first
                    // cancellation.
                    let response = ctx
                        .http_client
                        .get(format!("{}/retry-sequence/{}/200d3000", ctx.http_url(), test_id))
                        .header("Host", "per-try.example.com")
                        .header("x-trace-id", &test_id)
                        .header("access_log", "test_store")
                        .timeout(Duration::from_millis(500))
                        .send()
                        .await;

                    if response.is_ok() {
                        return TestResult::failed(
                            start.elapsed(),
                            "Expected the client-side 500ms timeout to abort the request".to_string(),
                        );
                    }

                    // The gateway aborts on the downstream EOF almost instantly,
                    // but poll generously in case a slow CI host delays the
                    // access-log store write.
                    let al_client = ctx.access_log_client();
                    let entry = match al_client.get_access_log_with_retry(&test_id, 60, 250).await {
                        Ok(entry) => entry,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Access log entry not found after client disconnect: {}", e),
                            )
                        }
                    };

                    let has_client_closed = entry.data["errors"]
                        .as_array()
                        .is_some_and(|errs| errs.iter().any(|v| v.as_str() == Some("ClientClosed")));
                    if !has_client_closed {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Access log errors missing ClientClosed: {}", entry.data),
                        );
                    }

                    let status = entry.data["request_info"]["status"].as_u64().unwrap_or(0);
                    if status != 499 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected logged status 499 (client closed), got {}", status),
                        );
                    }

                    let attempts = entry.data["backend_context"]["upstreams"]
                        .as_array()
                        .map(|u| u.len())
                        .unwrap_or(0);
                    if attempts == 0 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected at least one upstream attempt recorded: {}", entry.data),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "ClientClosed logged with status 499; {} upstream attempt(s) recorded",
                            attempts
                        ),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for PerTryTimeoutTestSuite {
    fn name(&self) -> &str {
        "PerTryTimeout"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/PerTryTimeout")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_slow_attempt_is_retried(),
            Self::test_no_retry_without_policy(),
            Self::test_exhaustion_yields_504(),
            Self::test_client_disconnect_is_logged(),
        ]
    }
}
