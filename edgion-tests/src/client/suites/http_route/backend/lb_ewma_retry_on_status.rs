// ============================================================================
// LB EWMA + retry-on-status Test Suite
// ============================================================================
//
// Setup: retry attempts=3, codes=[503], EWMA LB, 2 reachable endpoints.
//
// This integration test verifies the observable retry contract: three retries
// produce four EWMA upstream attempts, the abandoned attempts carry the
// RetryOnStatus reason, and the exhausted request returns the final 503.
// A retried attempt now KEEPS its latency sample (recorded at response headers)
// while its connection slot is released; a 5xx is recorded peak-only so failing
// fast cannot lower a backend's score. Classification lives in `lb/sample.rs`;
// the response-header call site is asserted by
// `pg_upstream_response_filter::lb_sample_tests`. Endpoint distribution is
// covered by the retry-free LBEwma suite.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct LBEwmaRetryOnStatusTestSuite;

impl LBEwmaRetryOnStatusTestSuite {
    fn test_retry_exhaustion_records_all_attempts() -> TestCase {
        TestCase::new(
            "ewma_retry_exhaustion_records_all_attempts",
            "EWMA retry-on-503 records three discarded attempts and one final attempt",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let port = 31126;
                    let trace_id = format!("ewma-rs-exhausted-{}", uuid::Uuid::new_v4());
                    let response = ctx
                        .http_client
                        .get(format!("http://{}:{port}/status/503", ctx.target_host))
                        .header("host", "lb-ewma-rs.example.com")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let response = match response {
                        Ok(response) => response,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Request failed: {error}"));
                        }
                    };
                    if response.status().as_u16() != 503 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected final status 503, got {}", response.status()),
                        );
                    }

                    let entry = match ctx
                        .access_log_client()
                        .get_access_log_with_retry(&trace_id, 10, 200)
                        .await
                    {
                        Ok(entry) => entry,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("Access log fetch failed: {error}"));
                        }
                    };
                    let Some(upstreams) = entry.data["backend_context"]["upstreams"].as_array() else {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Access log has no upstream attempts: {}", entry.data["backend_context"]),
                        );
                    };
                    if upstreams.len() != 4 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 4 upstream attempts, got {}: {upstreams:?}", upstreams.len()),
                        );
                    }

                    for (index, upstream) in upstreams.iter().enumerate() {
                        if upstream["status"].as_u64() != Some(503) || upstream["lb_policy"].as_str() != Some("Ewma") {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Unexpected upstream attempt {index}: {upstream}"),
                            );
                        }
                        let retried = upstream["err"]
                            .as_array()
                            .is_some_and(|errors| errors.iter().any(|error| error.as_str() == Some("RetryOnStatus")));
                        if retried != (index < 3) {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Unexpected retry marker on upstream attempt {index}: {upstream}"),
                            );
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Final 503 recorded after three RetryOnStatus attempts".to_string(),
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
        vec![Self::test_retry_exhaustion_records_all_attempts()]
    }
}
