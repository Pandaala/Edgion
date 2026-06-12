// RetryOnStatus Test suite
//
// Tests retry-on-status-code feature: when upstream returns a status code
// listed in HTTPRouteRule.retry.codes, the gateway retries to another backend.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct RetryOnStatusTestSuite;

impl RetryOnStatusTestSuite {
    /// Test: backend returns 502 then 200 — retry should succeed
    fn test_retry_502_then_200() -> TestCase {
        TestCase::new(
            "retry_502_then_200",
            "Backend returns 502,200 — gateway retries and client gets 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_id = format!("retry-502-200-{}", uuid::Uuid::new_v4());

                    // Reset state
                    let _ = ctx
                        .http_client
                        .get(format!("{}/retry-sequence-reset", ctx.http_url()))
                        .header("Host", "retry-on-status.example.com")
                        .send()
                        .await;

                    let response = ctx
                        .http_client
                        .get(format!("{}/retry-sequence/{}/502,200", ctx.http_url(), test_id))
                        .header("Host", "retry-on-status.example.com")
                        .header("x-trace-id", &test_id)
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            // The retried attempt's per-upstream entry must carry the
                            // explicit reason label (not just the 502 status code).
                            let al_client = ctx.access_log_client();
                            let entry = match al_client.get_access_log_with_retry(&test_id, 10, 200).await {
                                Ok(entry) => entry,
                                Err(e) => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Access log fetch failed: {}", e),
                                    )
                                }
                            };
                            let has_label = entry.data["backend_context"]["upstreams"]
                                .as_array()
                                .is_some_and(|ups| {
                                    ups.iter().any(|u| {
                                        u["err"].as_array().is_some_and(|errs| {
                                            errs.iter().any(|v| v.as_str() == Some("RetryOnStatus"))
                                        })
                                    })
                                });
                            if !has_label {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Access log upstreams missing RetryOnStatus label: {}",
                                        entry.data["backend_context"]["upstreams"]
                                    ),
                                );
                            }

                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Got 200 after retry; RetryOnStatus label present in access log".to_string(),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: backend returns 503,503,200 — two retries then success
    fn test_retry_503_503_200() -> TestCase {
        TestCase::new(
            "retry_503_503_200",
            "Backend returns 503,503,200 — two retries then success",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_id = format!("retry-503x2-{}", uuid::Uuid::new_v4());

                    let _ = ctx
                        .http_client
                        .get(format!("{}/retry-sequence-reset", ctx.http_url()))
                        .header("Host", "retry-on-status.example.com")
                        .send()
                        .await;

                    let response = ctx
                        .http_client
                        .get(format!("{}/retry-sequence/{}/503,503,200", ctx.http_url(), test_id))
                        .header("Host", "retry-on-status.example.com")
                        .header("x-trace-id", &test_id)
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Got 200 after two retries".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: backend returns 200 on first try — no retry needed
    fn test_no_retry_on_200() -> TestCase {
        TestCase::new(
            "no_retry_on_200",
            "Backend returns 200 — no retry, direct success",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_id = format!("no-retry-{}", uuid::Uuid::new_v4());

                    let _ = ctx
                        .http_client
                        .get(format!("{}/retry-sequence-reset", ctx.http_url()))
                        .header("Host", "retry-on-status.example.com")
                        .send()
                        .await;

                    let response = ctx
                        .http_client
                        .get(format!("{}/retry-sequence/{}/200", ctx.http_url(), test_id))
                        .header("Host", "retry-on-status.example.com")
                        .header("x-trace-id", &test_id)
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Got 200 on first attempt, no retry".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: backend returns 502 x4 — exceeds attempts(3), should see error
    fn test_retry_exhausted() -> TestCase {
        TestCase::new(
            "retry_exhausted",
            "Backend returns 502 x4 — exceeds retry limit, client gets error",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_id = format!("exhausted-{}", uuid::Uuid::new_v4());

                    let _ = ctx
                        .http_client
                        .get(format!("{}/retry-sequence-reset", ctx.http_url()))
                        .header("Host", "retry-on-status.example.com")
                        .send()
                        .await;

                    let response = ctx
                        .http_client
                        .get(format!("{}/retry-sequence/{}/502,502,502,502", ctx.http_url(), test_id))
                        .header("Host", "retry-on-status.example.com")
                        .header("x-trace-id", &test_id)
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            // Should get 502 (last attempt's error propagated) or a gateway error
                            if status == 502 || status >= 500 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Got {} after retry exhaustion (expected 5xx)", status),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 5xx after retry exhaustion, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: backend returns 500 — not in retry codes list, should not retry
    fn test_no_retry_on_500() -> TestCase {
        TestCase::new(
            "no_retry_on_500",
            "Backend returns 500 — not in codes [502,503], no retry",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_id = format!("no-retry-500-{}", uuid::Uuid::new_v4());

                    let _ = ctx
                        .http_client
                        .get(format!("{}/retry-sequence-reset", ctx.http_url()))
                        .header("Host", "retry-on-status.example.com")
                        .send()
                        .await;

                    let response = ctx
                        .http_client
                        .get(format!("{}/retry-sequence/{}/500,200", ctx.http_url(), test_id))
                        .header("Host", "retry-on-status.example.com")
                        .header("x-trace-id", &test_id)
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 500 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Got 500 — correctly did NOT retry (500 not in codes)".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 500 (no retry), got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for RetryOnStatusTestSuite {
    fn name(&self) -> &str {
        "RetryOnStatus"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/RetryOnStatus")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_no_retry_on_200(),
            Self::test_retry_502_then_200(),
            Self::test_retry_503_503_200(),
            Self::test_retry_exhausted(),
            Self::test_no_retry_on_500(),
        ]
    }
}
