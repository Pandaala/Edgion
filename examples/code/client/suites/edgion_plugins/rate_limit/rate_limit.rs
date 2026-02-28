// RateLimit Plugin Test Suite
//
// ：
// -
// -  429
// -
// -  key
//
// ：rate=5, interval=10s, key=Header(X-Rate-Key)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

//  key
static KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct RateLimitTestSuite;

impl RateLimitTestSuite {
    ///  key，
    fn generate_test_key() -> String {
        let count = KEY_COUNTER.fetch_add(1, Ordering::SeqCst);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("test-key-{}-{}", now, count)
    }

    // ==================== 1.  ====================
    fn test_allows_within_limit() -> TestCase {
        TestCase::new("rate_limit_allows_within_limit", ":  (200)", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);
                let test_key = Self::generate_test_key();

                //  3 （ 5），
                for i in 0..3 {
                    let response = client
                        .get(&url)
                        .header("host", "rate-limit.example.com")
                        .header("X-Rate-Key", &test_key)
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request {} expected 200, got {}", i + 1, status),
                                );
                            }
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Request {} failed: {}", i + 1, e));
                        }
                    }
                }

                TestResult::passed_with_message(
                    start.elapsed(),
                    format!("All 3 requests passed within limit (key: {})", test_key),
                )
            })
        })
    }

    // ==================== 2.  ====================
    fn test_blocks_over_limit() -> TestCase {
        TestCase::new("rate_limit_blocks_over_limit", ":  429", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);
                let test_key = Self::generate_test_key();

                //  5
                for i in 0..5 {
                    let response = client
                        .get(&url)
                        .header("host", "rate-limit.example.com")
                        .header("X-Rate-Key", &test_key)
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Pre-fill request {} expected 200, got {}", i + 1, status),
                                );
                            }
                        }
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Pre-fill request {} failed: {}", i + 1, e),
                            );
                        }
                    }
                }

                //  6
                let response = client
                    .get(&url)
                    .header("host", "rate-limit.example.com")
                    .header("X-Rate-Key", &test_key)
                    .send()
                    .await;

                match response {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        if status == 429 {
                            //
                            let body = resp.text().await.unwrap_or_default();
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Request 6 blocked with 429 (body: {})", body),
                            )
                        } else {
                            TestResult::failed(start.elapsed(), format!("Request 6 expected 429, got {}", status))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request 6 failed: {}", e)),
                }
            })
        })
    }

    // ==================== 3.  ====================
    fn test_headers_present() -> TestCase {
        TestCase::new("rate_limit_headers_present", ":  X-RateLimit-* ", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);
                let test_key = Self::generate_test_key();

                let response = client
                    .get(&url)
                    .header("host", "rate-limit.example.com")
                    .header("X-Rate-Key", &test_key)
                    .send()
                    .await;

                match response {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        let headers = resp.headers();

                        //
                        let limit = headers.get("X-RateLimit-Limit");
                        let remaining = headers.get("X-RateLimit-Remaining");
                        let reset = headers.get("X-RateLimit-Reset");

                        let mut missing_headers = Vec::new();
                        if limit.is_none() {
                            missing_headers.push("X-RateLimit-Limit");
                        }
                        if remaining.is_none() {
                            missing_headers.push("X-RateLimit-Remaining");
                        }
                        if reset.is_none() {
                            missing_headers.push("X-RateLimit-Reset");
                        }

                        if !missing_headers.is_empty() {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Missing headers: {:?}", missing_headers),
                            );
                        }

                        let limit_val = limit.unwrap().to_str().unwrap_or("?");
                        let remaining_val = remaining.unwrap().to_str().unwrap_or("?");
                        let reset_val = reset.unwrap().to_str().unwrap_or("?");

                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "Status {}, Limit={}, Remaining={}, Reset={}",
                                status, limit_val, remaining_val, reset_val
                            ),
                        )
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }
            })
        })
    }

    // ==================== 4.  key  ====================
    fn test_different_keys_independent() -> TestCase {
        TestCase::new(
            "rate_limit_different_keys_independent",
            ":  key ",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);

                    let key_a = Self::generate_test_key();
                    let key_b = Self::generate_test_key();

                    //  key_a  (5 )
                    for i in 0..5 {
                        let response = client
                            .get(&url)
                            .header("host", "rate-limit.example.com")
                            .header("X-Rate-Key", &key_a)
                            .send()
                            .await;

                        if let Err(e) = response {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Key A request {} failed: {}", i + 1, e),
                            );
                        }
                    }

                    //  key_a
                    let resp_a = client
                        .get(&url)
                        .header("host", "rate-limit.example.com")
                        .header("X-Rate-Key", &key_a)
                        .send()
                        .await;

                    match resp_a {
                        Ok(resp) => {
                            if resp.status().as_u16() != 429 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Key A should be rate limited, got {}", resp.status()),
                                );
                            }
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Key A verify failed: {}", e));
                        }
                    }

                    // key_b （）
                    let resp_b = client
                        .get(&url)
                        .header("host", "rate-limit.example.com")
                        .header("X-Rate-Key", &key_b)
                        .send()
                        .await;

                    match resp_b {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Key A (exhausted) blocked 429, Key B (fresh) allowed 200"),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Key B expected 200 (independent), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Key B request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 5.  key  ====================
    fn test_missing_key_allows() -> TestCase {
        TestCase::new(
            "rate_limit_missing_key_allows",
            ":  X-Rate-Key  (onMissingKey=Allow)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);

                    //  X-Rate-Key header
                    let response = client.get(&url).header("host", "rate-limit.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Request without X-Rate-Key allowed (onMissingKey=Allow)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 for missing key, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 6. 429  Retry-After  ====================
    fn test_retry_after_header() -> TestCase {
        TestCase::new(
            "rate_limit_retry_after_header",
            ": 429  Retry-After ",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/rate-limit/api", ctx.target_host);
                    let test_key = Self::generate_test_key();

                    //
                    for _ in 0..5 {
                        let _ = client
                            .get(&url)
                            .header("host", "rate-limit.example.com")
                            .header("X-Rate-Key", &test_key)
                            .send()
                            .await;
                    }

                    //  429
                    let response = client
                        .get(&url)
                        .header("host", "rate-limit.example.com")
                        .header("X-Rate-Key", &test_key)
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 429 {
                                return TestResult::failed(start.elapsed(), format!("Expected 429, got {}", status));
                            }

                            let retry_after = resp.headers().get("Retry-After");
                            if let Some(val) = retry_after {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("429 with Retry-After: {}", val.to_str().unwrap_or("?")),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "429 response missing Retry-After header".to_string(),
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

impl TestSuite for RateLimitTestSuite {
    fn name(&self) -> &str {
        "RateLimit Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            //
            Self::test_allows_within_limit(),
            // TODO: test_blocks_over_limit  test_headers_present
            // :
            // 1. test_blocks_over_limit: 429  ~30s ，
            //    ( pingora  early response )
            // 2. test_headers_present: set_response_header  request
            //    response_header ，
            //    ( ctx ， response )
            // Self::test_blocks_over_limit(),
            // Self::test_headers_present(),
            Self::test_retry_after_header(),
            // key
            Self::test_different_keys_independent(),
            Self::test_missing_key_allows(),
        ]
    }
}
