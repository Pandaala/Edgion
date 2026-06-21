// PerTryTimeout Test suite
//
// Tests GEP-1731 per-try timeout: when an upstream attempt exceeds
// timeouts.backendRequest and the rule has a retry policy, the gateway
// cancels the attempt and retries. Without a retry policy the timeout
// is final and the client gets 504.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};

/// Local mirror of the gateway's `generate_session_token`
/// (`edgion-gateway/src/routes/http/session_persistence.rs`): the 8-hex-char
/// lowercase CRC32 of `"ip:port"`. Duplicated here because `edgion-tests` does
/// NOT depend on `edgion-gateway` (it sidesteps the boringssl/rustls feature
/// dance). Keep in sync if the gateway ever changes the token formula.
fn sp_token(ip: &str, port: u64) -> String {
    format!("{:08x}", crc32fast::hash(format!("{}:{}", ip, port).as_bytes()))
}

fn is_k8s_mode() -> bool {
    std::env::var("EDGION_TEST_K8S_MODE")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

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
    /// Test: backend commits 200 + headers, then stalls mid-body. The per-try
    /// (backendRequest) timeout fires while the body is in flight, but because
    /// response headers are already written downstream the `response_written`
    /// guard forbids a retry — a retry would corrupt the half-sent response.
    /// Pins the only correctness-critical guard: client gets 200 with a
    /// truncated/errored body at ~1s, exactly one upstream attempt, and the
    /// `BackendRequestTimeoutRetry` marker is ABSENT (the discriminator vs. the
    /// pre-header timeout case in `slow_attempt_is_retried`).
    fn test_midbody_timeout_not_retried() -> TestCase {
        TestCase::new(
            "midbody_timeout_not_retried",
            "Mid-body per-try timeout: response_written guard forbids retry — 200 + truncated body, single attempt",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_id = format!("ptt-midbody-{}", uuid::Uuid::new_v4());

                    let req_start = Instant::now();
                    let response = ctx
                        .http_client
                        .get(format!("{}/stream-stall/{}", ctx.http_url(), test_id))
                        .header("Host", "per-try.example.com")
                        .header("x-trace-id", &test_id)
                        .header("access_log", "test_store")
                        .timeout(Duration::from_secs(15))
                        .send()
                        .await;

                    // Headers (200) are committed immediately, before the mid-body
                    // timeout — send() must succeed and report 200.
                    let resp = match response {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Expected 200 headers before mid-body timeout, send() failed: {}", e),
                            )
                        }
                    };
                    let status = resp.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected 200 (headers committed before mid-body timeout), got {}",
                                status
                            ),
                        );
                    }

                    // The gateway cuts the stalled body at ~backendRequest, so the
                    // chunked body never reaches its terminating chunk: the read errors.
                    let body_result = resp.bytes().await;
                    let elapsed = req_start.elapsed();
                    if body_result.is_ok() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected truncated/errored body (mid-body timeout), but body read completed in {:?}",
                                elapsed
                            ),
                        );
                    }
                    if elapsed >= Duration::from_millis(2900) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Body cut at {:?} >= 2900ms — attempt not bounded at ~1s backendRequest (or it was retried)",
                                elapsed
                            ),
                        );
                    }
                    if elapsed < Duration::from_millis(700) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Body cut at {:?} < 700ms — too early to be the ~1s backendRequest bound",
                                elapsed
                            ),
                        );
                    }

                    // No retry: exactly one upstream attempt, and the per-try retry
                    // marker must be ABSENT (the response_written guard discriminator).
                    let al_client = ctx.access_log_client();
                    let entry = match al_client.get_access_log_with_retry(&test_id, 20, 200).await {
                        Ok(e) => e,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Access log entry not found after mid-body timeout: {}", e),
                            )
                        }
                    };

                    let attempts = entry.data["backend_context"]["upstreams"]
                        .as_array()
                        .map(|u| u.len())
                        .unwrap_or(0);
                    if attempts != 1 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected exactly 1 upstream attempt (no retry after headers committed), got {}: {}",
                                attempts, entry.data["backend_context"]["upstreams"]
                            ),
                        );
                    }

                    let has_retry_marker = entry.data["errors"]
                        .as_array()
                        .is_some_and(|errs| errs.iter().any(|e| e.as_str() == Some("BackendRequestTimeoutRetry")));
                    if has_retry_marker {
                        return TestResult::failed(
                            start.elapsed(),
                            "BackendRequestTimeoutRetry present — response_written guard failed to block the mid-body retry"
                                .to_string(),
                        );
                    }

                    let logged_status = entry.data["request_info"]["status"].as_u64().unwrap_or(0);
                    if logged_status != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected logged status 200 (committed response), got {}", logged_status),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "200 committed, body cut at {:?}, single attempt, no retry marker — response_written guard holds",
                            elapsed
                        ),
                    )
                })
            },
        )
    }

    /// Test: a steady 500ms trickle that spans ~2s (> the 1s backendRequest bound)
    /// is NOT killed. Each inter-chunk gap stays under backendRequest, so the
    /// per-I/O timeout never fires and the full body is delivered with 200.
    /// Pins the "an attempt that keeps making progress is not killed" claim.
    fn test_trickle_progress_not_killed() -> TestCase {
        TestCase::new(
            "trickle_progress_not_killed",
            "Steady 500ms trickle over ~2s (> 1s backendRequest) is not killed — full 200 body delivered",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_id = format!("ptt-trickle-{}", uuid::Uuid::new_v4());

                    // 4 chunks, 500ms apart → ~2s total; each gap < 1s backendRequest.
                    let req_start = Instant::now();
                    let response = ctx
                        .http_client
                        .get(format!("{}/stream-trickle/{}/4/500", ctx.http_url(), test_id))
                        .header("Host", "per-try.example.com")
                        .header("x-trace-id", &test_id)
                        .timeout(Duration::from_secs(15))
                        .send()
                        .await;

                    let resp = match response {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = resp.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200 (steady progress not killed), got {}", status),
                        );
                    }

                    let body = match resp.bytes().await {
                        Ok(b) => b,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "Body should complete fully (progress not killed), but read errored: {}",
                                    e
                                ),
                            )
                        }
                    };
                    let elapsed = req_start.elapsed();

                    let text = String::from_utf8_lossy(&body);
                    let n = text.matches("trickle-chunk-").count();
                    if n != 4 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected 4 trickled chunks in the body, got {} (body cut?): {:?}",
                                n, text
                            ),
                        );
                    }
                    if elapsed < Duration::from_millis(1500) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Completed in {:?} < 1500ms — trickle did not actually span past backendRequest",
                                elapsed
                            ),
                        );
                    }
                    if elapsed >= Duration::from_secs(5) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Completed in {:?} >= 5s — unexpectedly slow", elapsed),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "Full 4-chunk body in {:?} (> 1s backendRequest) — steady progress not killed",
                            elapsed
                        ),
                    )
                })
            },
        )
    }

    /// Test: deadline-transition (GEP-1731). Route request=2s, backendRequest=1.5s,
    /// attempts=5, backend always slow (3s). Attempt 1's tightest bound is
    /// backendRequest → canceled and retried (BackendRequestTimeoutRetry). By
    /// attempt 2 only ~0.5s of the 2s request deadline remains, so RequestDeadline
    /// becomes the operative bound; a deadline-sourced timeout is never retried
    /// even though 3 retries of budget remain. Pins end-to-end that deadline
    /// timeouts do not consume retry budget: final 504 at ~2s with exactly 2
    /// upstream attempts (the [1.9s, 3s) elapsed window is the discriminator).
    fn test_deadline_transition_not_retried() -> TestCase {
        TestCase::new(
            "deadline_transition_not_retried",
            "Deadline-transition: attempt 2's RequestDeadline timeout is not retried despite budget — 504 at ~2s, 2 attempts",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_id = format!("ptt-deadline-{}", uuid::Uuid::new_v4());

                    // Reset state
                    let _ = ctx
                        .http_client
                        .get(format!("{}/retry-sequence-reset", ctx.http_url()))
                        .header("Host", "per-try-deadline.example.com")
                        .send()
                        .await;

                    let req_start = Instant::now();
                    let response = ctx
                        .http_client
                        .get(format!("{}/retry-sequence/{}/200d3000", ctx.http_url(), test_id))
                        .header("Host", "per-try-deadline.example.com")
                        .header("x-trace-id", &test_id)
                        .header("access_log", "test_store")
                        .timeout(Duration::from_secs(15))
                        .send()
                        .await;
                    let elapsed = req_start.elapsed();

                    let resp = match response {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = resp.status().as_u16();
                    if status != 504 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 504 (request deadline), got {} (elapsed {:?})", status, elapsed),
                        );
                    }

                    // Discriminator window: attempt 1 ~1.5s + attempt 2 ~0.5s = ~2s.
                    // < 1.9s would mean attempt 2 was not bounded by the 2s deadline;
                    // >= 3s would mean the deadline-sourced timeout was (wrongly) retried.
                    if elapsed < Duration::from_millis(1900) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("504 at {:?} < 1900ms — attempt 2 not bounded by the 2s request deadline", elapsed),
                        );
                    }
                    if elapsed >= Duration::from_secs(3) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("504 at {:?} >= 3s — deadline-sourced timeout was retried (must never be)", elapsed),
                        );
                    }

                    let al_client = ctx.access_log_client();
                    let entry = match al_client.get_access_log_with_retry(&test_id, 20, 200).await {
                        Ok(e) => e,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Access log entry not found after deadline 504: {}", e),
                            )
                        }
                    };

                    // Exactly 2 attempts: attempt 1 (backendRequest, retried) +
                    // attempt 2 (RequestDeadline, NOT retried despite attempts: 5).
                    let attempts = entry.data["backend_context"]["upstreams"]
                        .as_array()
                        .map(|u| u.len())
                        .unwrap_or(0);
                    if attempts != 2 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected exactly 2 upstream attempts (deadline timeout not retried), got {}: {}",
                                attempts, entry.data["backend_context"]["upstreams"]
                            ),
                        );
                    }

                    // Attempt 1's cancellation must be the per-try (backendRequest) retry.
                    let has_retry_marker = entry.data["errors"].as_array().is_some_and(|errs| {
                        errs.iter().any(|e| e.as_str() == Some("BackendRequestTimeoutRetry"))
                    });
                    if !has_retry_marker {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected BackendRequestTimeoutRetry (attempt 1 per-try cancel): {}",
                                entry.data["errors"]
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "504 at {:?}, 2 attempts — attempt 1 retried (backendRequest), attempt 2 deadline-bounded and not retried despite budget",
                            elapsed
                        ),
                    )
                })
            },
        )
    }

    /// Test: client gives up at 500ms while every attempt sleeps 3s. The
    /// downstream EOF is detected almost immediately — the gateway aborts the
    /// in-flight first attempt (status 499, single upstreams entry) instead of
    /// running the retry loop to exhaustion. fail_to_proxy records the ClientClosed
    /// marker; no early-abort backoff is applied.
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

    /// Scenario A (Bug 1): a sticky target hit by attempt 1 times out at the 1s
    /// per-try bound; the retry re-picks via plain LB on the other backend and
    /// the response must refresh the cookie to that serving backend.
    fn test_sticky_refresh_on_cross_attempt_retry() -> TestCase {
        TestCase::new(
            "sticky_refresh_on_cross_attempt_retry",
            "Sticky per-try timeout: cross-attempt retry refreshes the cookie to the serving backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let seq_id = format!("ptt-sp-refresh-{}", uuid::Uuid::new_v4());

                    let client = reqwest::Client::builder()
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .timeout(Duration::from_secs(15))
                        .build()
                        .expect("client");

                    // Request 1 (no cookie, non-sequence path): establish a sticky
                    // cookie that points at whichever backend served it.
                    let r1 = match client
                        .get(format!("{}/test", ctx.http_url()))
                        .header("Host", "per-try-sp.example.com")
                        .header("x-trace-id", format!("{}-init", seq_id))
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request 1 failed: {}", e)),
                    };
                    if r1.status().as_u16() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("request 1 expected 200, got {}", r1.status()),
                        );
                    }
                    let cookie1 = match r1.headers().get("set-cookie").and_then(|v| v.to_str().ok()) {
                        Some(sc) if sc.starts_with("edgion-sp-ptt=") => sc.split(';').next().unwrap_or(sc).to_string(),
                        Some(sc) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("request 1 Set-Cookie not 'edgion-sp-ptt=': {}", sc),
                            )
                        }
                        None => {
                            return TestResult::failed(start.elapsed(), "request 1 carried no Set-Cookie".to_string())
                        }
                    };
                    let token1 = cookie1.split('=').nth(1).unwrap_or("").to_string();

                    // Request 2 (with cookie1): attempt 1 routes to the sticky
                    // backend, is canceled at the 1s per-try timeout, retry
                    // re-picks via plain LB.
                    let req_start = Instant::now();
                    let r2 = match client
                        .get(format!("{}/retry-sequence/{}/200d3000,200", ctx.http_url(), seq_id))
                        .header("Host", "per-try-sp.example.com")
                        .header("cookie", &cookie1)
                        .header("x-trace-id", &seq_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request 2 failed: {}", e)),
                    };
                    let elapsed = req_start.elapsed();

                    if r2.status().as_u16() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("request 2 expected 200 after retry, got {}", r2.status()),
                        );
                    }
                    let cookie2 = match r2.headers().get("set-cookie").and_then(|v| v.to_str().ok()) {
                        Some(sc) if sc.starts_with("edgion-sp-ptt=") => sc.split(';').next().unwrap_or(sc).to_string(),
                        Some(sc) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("request 2 Set-Cookie not 'edgion-sp-ptt=': {}", sc),
                            )
                        }
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                "request 2 carried no Set-Cookie — cross-attempt refresh missing (bug)".to_string(),
                            )
                        }
                    };
                    let token2 = cookie2.split('=').nth(1).unwrap_or("").to_string();

                    // Timing: the full per-try window elapsed on the sticky backend
                    // (>= 950ms), but the attempt was canceled at ~1s, not the 3s
                    // server sleep (< 2900ms; no backoff configured).
                    if elapsed < Duration::from_millis(950) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "elapsed {:?} < 950ms — sticky attempt did not run the full per-try window",
                                elapsed
                            ),
                        );
                    }
                    if elapsed >= Duration::from_millis(2900) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "elapsed {:?} >= 2900ms — attempt was not canceled at the ~1s per-try timeout",
                                elapsed
                            ),
                        );
                    }

                    // Inspect the access log: stickiness engaged + refresh correct.
                    let al_client = ctx.access_log_client();
                    let entry = match al_client.get_access_log_with_retry(&seq_id, 20, 200).await {
                        Ok(e) => e,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("access log entry not found for {}: {}", seq_id, e),
                            )
                        }
                    };
                    let upstreams = match entry.data["backend_context"]["upstreams"].as_array() {
                        Some(u) if u.len() >= 2 => u,
                        other => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("expected >= 2 upstream attempts, got {:?}", other),
                            )
                        }
                    };

                    // Stickiness engaged: attempt 0 (the timed-out one) must encode
                    // the same backend as cookie1 — guards against an affinity-map
                    // miss (InjectNew) masking the sticky routing check.
                    let u0 = &upstreams[0];
                    let u0_ip = u0["ip"].as_str().unwrap_or("");
                    let u0_port = u0["port"].as_u64().unwrap_or(0);
                    if sp_token(u0_ip, u0_port) != token1 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "stickiness not engaged: attempt0 {}:{} token != cookie1 token {}",
                                u0_ip, u0_port, token1
                            ),
                        );
                    }

                    // Refresh correct: the last attempt served 200 and cookie2
                    // points at that serving backend.
                    let last = &upstreams[upstreams.len() - 1];
                    let last_ip = last["ip"].as_str().unwrap_or("");
                    let last_port = last["port"].as_u64().unwrap_or(0);
                    let last_status = last["status"].as_u64().unwrap_or(0);
                    if last_status != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("last upstream status expected 200, got {}", last_status),
                        );
                    }
                    if sp_token(last_ip, last_port) != token2 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "refresh incorrect: cookie2 token {} != serving backend {}:{} token {}",
                                token2,
                                last_ip,
                                last_port,
                                sp_token(last_ip, last_port)
                            ),
                        );
                    }

                    // Confirm the retry was the per-try timeout path (Case 1.5).
                    let has_marker = entry.data["errors"]
                        .as_array()
                        .is_some_and(|errs| errs.iter().any(|e| e.as_str() == Some("BackendRequestTimeoutRetry")));
                    if !has_marker {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "errors chain missing BackendRequestTimeoutRetry: {}",
                                entry.data["errors"]
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "cross-attempt refresh OK in {:?}: cookie1 {} -> cookie2 {} (served {}:{})",
                            elapsed, token1, token2, last_ip, last_port
                        ),
                    )
                })
            },
        )
    }

    /// Scenario C2 (strict re-pin): a sticky per-try timeout retry must re-pin to
    /// the SAME backend (not migrate via plain LB). Both upstream attempts share
    /// one ip:port; stickiness is proven by attempt-0's token matching the cookie.
    fn test_strict_repin_on_cross_attempt_retry() -> TestCase {
        TestCase::new(
            "strict_repin_on_cross_attempt_retry",
            "Strict SP per-try timeout: cross-attempt retry re-pins to the same backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if is_k8s_mode() {
                        return TestResult::passed_with_message(
                            start.elapsed(),
                            "Skipped in K8s mode: Gateway API HTTPRoute CRD rejects Edgion's non-standard sessionPersistence.strict extension".to_string(),
                        );
                    }

                    let seq_id = format!("ptt-sp-strict-{}", uuid::Uuid::new_v4());

                    let client = reqwest::Client::builder()
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .timeout(Duration::from_secs(15))
                        .build()
                        .expect("client");

                    let r1 = match client
                        .get(format!("{}/test", ctx.http_url()))
                        .header("Host", "strict-sp.example.com")
                        .header("x-trace-id", format!("{}-init", seq_id))
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request 1 failed: {}", e)),
                    };
                    if r1.status().as_u16() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("request 1 expected 200, got {}", r1.status()),
                        );
                    }
                    let cookie1 = match r1.headers().get("set-cookie").and_then(|v| v.to_str().ok()) {
                        Some(sc) if sc.starts_with("edgion-sp-strict=") => {
                            sc.split(';').next().unwrap_or(sc).to_string()
                        }
                        other => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("request 1 Set-Cookie not 'edgion-sp-strict=': {:?}", other),
                            )
                        }
                    };
                    let token1 = cookie1.split('=').nth(1).unwrap_or("").to_string();

                    let req_start = Instant::now();
                    let r2 = match client
                        .get(format!("{}/retry-sequence/{}/200d3000,200", ctx.http_url(), seq_id))
                        .header("Host", "strict-sp.example.com")
                        .header("cookie", &cookie1)
                        .header("x-trace-id", &seq_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request 2 failed: {}", e)),
                    };
                    let elapsed = req_start.elapsed();

                    if r2.status().as_u16() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("request 2 expected 200 after re-pin retry, got {}", r2.status()),
                        );
                    }
                    if elapsed < Duration::from_millis(950) || elapsed >= Duration::from_millis(2900) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "elapsed {:?} outside [950ms, 2900ms) — per-try window/cancel off",
                                elapsed
                            ),
                        );
                    }

                    let al_client = ctx.access_log_client();
                    let entry = match al_client.get_access_log_with_retry(&seq_id, 20, 200).await {
                        Ok(e) => e,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("access log entry not found for {}: {}", seq_id, e),
                            )
                        }
                    };
                    let upstreams = match entry.data["backend_context"]["upstreams"].as_array() {
                        Some(u) if u.len() >= 2 => u,
                        other => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("expected >= 2 upstream attempts, got {:?}", other),
                            )
                        }
                    };
                    let u0 = &upstreams[0];
                    let u0_ip = u0["ip"].as_str().unwrap_or("");
                    let u0_port = u0["port"].as_u64().unwrap_or(0);
                    let last = &upstreams[upstreams.len() - 1];
                    let last_ip = last["ip"].as_str().unwrap_or("");
                    let last_port = last["port"].as_u64().unwrap_or(0);

                    if sp_token(u0_ip, u0_port) != token1 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "stickiness not engaged: attempt0 {}:{} token != cookie1 token {}",
                                u0_ip, u0_port, token1
                            ),
                        );
                    }
                    if (u0_ip, u0_port) != (last_ip, last_port) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "strict re-pin failed: attempt0 {}:{} != retry {}:{} (migrated)",
                                u0_ip, u0_port, last_ip, last_port
                            ),
                        );
                    }
                    if last["status"].as_u64().unwrap_or(0) != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("last upstream status expected 200, got {}", last["status"]),
                        );
                    }
                    let has_marker = entry.data["errors"]
                        .as_array()
                        .is_some_and(|errs| errs.iter().any(|e| e.as_str() == Some("BackendRequestTimeoutRetry")));
                    if !has_marker {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "errors chain missing BackendRequestTimeoutRetry: {}",
                                entry.data["errors"]
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "strict re-pin OK in {:?}: both attempts on {}:{}",
                            elapsed, last_ip, last_port
                        ),
                    )
                })
            },
        )
    }

    /// Scenario C1 (strict in-attempt fail-close): a request whose cookie pins to
    /// an unhealthy (active-HC-down) backend must get 503, not migrate to the
    /// healthy backend. The cookie token is computed client-side for the known
    /// unreachable address (192.0.2.1) — its token is in the affinity map.
    fn test_strict_inattempt_fail_close_503() -> TestCase {
        TestCase::new(
            "strict_inattempt_fail_close_503",
            "Strict SP: cookie pinned to an unhealthy backend fails closed with 503",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if is_k8s_mode() {
                        return TestResult::passed_with_message(
                            start.elapsed(),
                            "Skipped in K8s mode: Gateway API HTTPRoute CRD rejects Edgion's non-standard sessionPersistence.strict extension".to_string(),
                        );
                    }

                    let client = reqwest::Client::builder()
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .timeout(Duration::from_secs(10))
                        .build()
                        .expect("client");

                    let token = sp_token("192.0.2.1", 30001);
                    let cookie = format!("edgion-sp-strict-hc={}", token);

                    let mut last = 0u16;
                    for _ in 0..20 {
                        let resp = client
                            .get(format!("{}/test", ctx.http_url()))
                            .header("Host", "strict-sp-hc.example.com")
                            .header("cookie", &cookie)
                            .send()
                            .await;
                        if let Ok(r) = resp {
                            last = r.status().as_u16();
                            if last == 503 {
                                return TestResult::passed_with_message(
                                    start.elapsed(),
                                    "strict in-attempt fail-close returned 503".to_string(),
                                );
                            }
                            if last == 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "strict pinned-to-unhealthy returned 200 — silently migrated (bug)".to_string(),
                                );
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    TestResult::failed(
                        start.elapsed(),
                        format!("expected 503 within poll window, last status {}", last),
                    )
                })
            },
        )
    }

    /// Scenario B (Bug 2): a first request (no cookie) whose attempt-1 response
    /// is a retryable 503 must carry Set-Cookie on the final retried 200 — the
    /// inject state must survive the discarded attempt-1 response.
    fn test_first_request_cookie_survives_status_retry() -> TestCase {
        TestCase::new(
            "first_request_cookie_survives_status_retry",
            "First request: status-code retry succeeds and the final 200 still carries Set-Cookie",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let seq_id = format!("ptt-sp-statusretry-{}", uuid::Uuid::new_v4());

                    let client = reqwest::Client::builder()
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .timeout(Duration::from_secs(15))
                        .build()
                        .expect("client");

                    // No cookie; attempt 1 = 503 (retryable), attempt 2 = 200.
                    let resp = match client
                        .get(format!("{}/retry-sequence/{}/503,200", ctx.http_url(), seq_id))
                        .header("Host", "per-try-sp.example.com")
                        .header("x-trace-id", &seq_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    };

                    if resp.status().as_u16() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected 200 after status retry, got {}", resp.status()),
                        );
                    }
                    let cookie = match resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()) {
                        Some(sc) if sc.starts_with("edgion-sp-ptt=") => sc.split(';').next().unwrap_or(sc).to_string(),
                        Some(sc) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Set-Cookie not 'edgion-sp-ptt=': {}", sc),
                            )
                        }
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                "final 200 carried no Set-Cookie — inject state lost on discarded retry (bug)"
                                    .to_string(),
                            )
                        }
                    };
                    let token = cookie.split('=').nth(1).unwrap_or("").to_string();

                    // The cookie must point at the backend that actually served 200.
                    let al_client = ctx.access_log_client();
                    let entry = match al_client.get_access_log_with_retry(&seq_id, 20, 200).await {
                        Ok(e) => e,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("access log entry not found for {}: {}", seq_id, e),
                            )
                        }
                    };
                    let upstreams = match entry.data["backend_context"]["upstreams"].as_array() {
                        Some(u) if !u.is_empty() => u,
                        other => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("expected >= 1 upstream attempt, got {:?}", other),
                            )
                        }
                    };
                    let last = &upstreams[upstreams.len() - 1];
                    let last_ip = last["ip"].as_str().unwrap_or("");
                    let last_port = last["port"].as_u64().unwrap_or(0);
                    if sp_token(last_ip, last_port) != token {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "cookie token {} != serving backend {}:{} token {}",
                                token,
                                last_ip,
                                last_port,
                                sp_token(last_ip, last_port)
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "status-retry first request kept Set-Cookie {} (served {}:{})",
                            token, last_ip, last_port
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
            Self::test_midbody_timeout_not_retried(),
            Self::test_trickle_progress_not_killed(),
            Self::test_deadline_transition_not_retried(),
            Self::test_client_disconnect_is_logged(),
            Self::test_sticky_refresh_on_cross_attempt_retry(),
            Self::test_strict_repin_on_cross_attempt_retry(),
            Self::test_strict_inattempt_fail_close_503(),
            Self::test_first_request_cookie_survives_status_retry(),
        ]
    }
}
