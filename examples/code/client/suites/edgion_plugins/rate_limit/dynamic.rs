// RateLimit dynamic hot-reload test suites.
//
// Initial (onMissingKey=Allow): request without X-Rate-Key → 200
// Update  (onMissingKey=Deny):  request without X-Rate-Key → 429

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const HOST: &str = "rate-limit-dynamic.example.com";

async fn do_request_without_key(ctx: &TestContext) -> reqwest::Result<u16> {
    let url = format!("{}/test/rate-limit-dynamic", ctx.http_url());
    ctx.http_client
        .get(&url)
        .header("host", HOST)
        .send()
        .await
        .map(|r| r.status().as_u16())
}

// ============================================================
// Initial: Allow → 200
// ============================================================

pub struct RateLimitDynamicInitialTestSuite;

impl RateLimitDynamicInitialTestSuite {
    fn missing_key_allow_200() -> TestCase {
        TestCase::new(
            "rate_limit_dyn_initial_missing_key_200",
            "[INITIAL] missing X-Rate-Key with onMissingKey=Allow → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match do_request_without_key(&ctx).await {
                        Ok(200) => TestResult::passed(start.elapsed()),
                        Ok(s) => TestResult::failed(start.elapsed(), format!("expected 200 got {}", s)),
                        Err(e) => TestResult::failed(start.elapsed(), format!("req err: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait::async_trait]
impl TestSuite for RateLimitDynamicInitialTestSuite {
    fn name(&self) -> &str {
        "RateLimit Dynamic - Initial Phase"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::missing_key_allow_200()]
    }
}

// ============================================================
// Update: Deny → 429
// ============================================================

pub struct RateLimitDynamicUpdateTestSuite;

impl RateLimitDynamicUpdateTestSuite {
    fn missing_key_deny_429() -> TestCase {
        TestCase::new(
            "rate_limit_dyn_update_missing_key_429",
            "[UPDATE] missing X-Rate-Key with onMissingKey=Deny → 429",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match do_request_without_key(&ctx).await {
                        Ok(429) => TestResult::passed(start.elapsed()),
                        Ok(s) => TestResult::failed(start.elapsed(), format!("expected 429 got {}", s)),
                        Err(e) => TestResult::failed(start.elapsed(), format!("req err: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait::async_trait]
impl TestSuite for RateLimitDynamicUpdateTestSuite {
    fn name(&self) -> &str {
        "RateLimit Dynamic - Update Phase"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::missing_key_deny_429()]
    }
}
