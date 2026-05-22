// RateLimitRedis Plugin Test Suite
//
// Verifies that the RateLimitRedis plugin degrades correctly when Redis is
// unavailable (redisRef pointing to a non-existent Secret).
//
// Config layout:
//   /test/rate-limit-redis/allow          -> rate-limit-redis-main  (redisRef: default/redis-test, onRedisFailure: Allow)
//   /test/rate-limit-redis/missing-deny   -> rate-limit-redis-missing-deny (onMissingKey: Deny, onRedisFailure: Allow)
//   /test/rate-limit-redis/redis-fail-open  -> rate-limit-redis-fail-open  (redisRef: default/redis-not-found, onRedisFailure: Allow)
//   /test/rate-limit-redis/redis-fail-close -> rate-limit-redis-fail-close (redisRef: default/redis-not-found, onRedisFailure: Deny)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct RateLimitRedisTestSuite;

impl RateLimitRedisTestSuite {
    // ==================== 1. fail-open: Redis unavailable => allow ====================
    fn test_fail_open_allows_when_redis_unavailable() -> TestCase {
        TestCase::new(
            "rate_limit_redis_fail_open_allows",
            "fail-open: all requests pass (200) when Redis is unreachable (onRedisFailure: Allow)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/rate-limit-redis/redis-fail-open", ctx.target_host);

                    for i in 0..5 {
                        let resp = client
                            .get(&url)
                            .header("host", "rate-limit-redis.example.com")
                            .header("X-Rate-Key", "fail-open-test-key")
                            .send()
                            .await;

                        match resp {
                            Ok(r) => {
                                let status = r.status().as_u16();
                                if status != 200 {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Request {} expected 200 (fail-open), got {}", i + 1, status),
                                    );
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request {} network error: {}", i + 1, e),
                                );
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "All 5 requests returned 200 with Redis unavailable (onRedisFailure: Allow)".to_string(),
                    )
                })
            },
        )
    }

    // ==================== 2. fail-close: Redis unavailable => deny ====================
    fn test_fail_close_denies_when_redis_unavailable() -> TestCase {
        TestCase::new(
            "rate_limit_redis_fail_close_denies",
            "fail-close: all requests blocked (429) when Redis is unreachable (onRedisFailure: Deny)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!(
                        "http://{}:31180/test/rate-limit-redis/redis-fail-close",
                        ctx.target_host
                    );

                    for i in 0..5 {
                        let resp = client
                            .get(&url)
                            .header("host", "rate-limit-redis.example.com")
                            .header("X-Rate-Key", "fail-close-test-key")
                            .send()
                            .await;

                        match resp {
                            Ok(r) => {
                                let status = r.status().as_u16();
                                if status != 429 {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Request {} expected 429 (fail-close), got {}", i + 1, status),
                                    );
                                }
                                // Verify the reject message is present in the body
                                let body = r.text().await.unwrap_or_default();
                                if !body.contains("Redis unavailable") {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "Request {} got 429 but body does not contain 'Redis unavailable': {}",
                                            i + 1,
                                            body
                                        ),
                                    );
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request {} network error: {}", i + 1, e),
                                );
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "All 5 requests returned 429 with body 'Redis unavailable (deny)' (onRedisFailure: Deny)"
                            .to_string(),
                    )
                })
            },
        )
    }

    // ==================== 3. missing key => deny ====================
    fn test_missing_key_returns_429() -> TestCase {
        TestCase::new(
            "rate_limit_redis_missing_key_denied",
            "onMissingKey: Deny — request without X-Rate-Key header is rejected with 429",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/rate-limit-redis/missing-deny", ctx.target_host);

                    // Send request without X-Rate-Key header
                    let resp = client
                        .get(&url)
                        .header("host", "rate-limit-redis.example.com")
                        .send()
                        .await;

                    match resp {
                        Ok(r) => {
                            let status = r.status().as_u16();
                            if status == 429 {
                                let body = r.text().await.unwrap_or_default();
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Got 429 as expected (onMissingKey: Deny), body: {}", body),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 429 for missing key, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for RateLimitRedisTestSuite {
    fn name(&self) -> &str {
        "RateLimitRedis Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_fail_open_allows_when_redis_unavailable(),
            Self::test_fail_close_denies_when_redis_unavailable(),
            Self::test_missing_key_returns_429(),
        ]
    }
}
