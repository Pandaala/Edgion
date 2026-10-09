// RateLimitRedis Plugin Test Suite
//
// Verifies real Redis-backed conditional composite counting and failure behavior.
//
// Config layout:
//   /test/rate-limit-redis/allow          -> rate-limit-redis-main  (directPeerIp + host + header)
//   /test/rate-limit-redis/missing-deny   -> rate-limit-redis-missing-deny (onKeyMiss: Deny, onRedisFailure: Allow)
//   /test/rate-limit-redis/redis-fail-open  -> rate-limit-redis-fail-open  (redisRef: default/redis-not-found, onRedisFailure: Allow)
//   /test/rate-limit-redis/redis-fail-close -> rate-limit-redis-fail-close (redisRef: default/redis-not-found, onRedisFailure: Deny)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct RateLimitRedisTestSuite;

impl RateLimitRedisTestSuite {
    fn test_independent_quotas_and_final_header_ownership() -> TestCase {
        TestCase::new(
            "redis_independent_tenant_user_quotas",
            "Independent identities, retained early debits and final denial headers",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let outcome: Result<(), String> = async {
                        let tenant = uuid::Uuid::new_v4().to_string();
                        for (user, status, limit, remaining) in [
                            ("a", 200, 2, 1),
                            ("a", 200, 2, 0),
                            ("a", 429, 2, 0),
                            ("b", 200, 2, 1),
                            ("b", 429, 4, 0),
                        ] {
                            let response = ctx
                                .http_client
                                .get(format!("{}/echo", ctx.http_url()))
                                .header("host", "redis-multi-quota.example.com")
                                .header("X-Tenant", &tenant)
                                .header("X-User", user)
                                .send()
                                .await
                                .map_err(|e| e.to_string())?;
                            let actual = response.status().as_u16();
                            let number = |name| {
                                response
                                    .headers()
                                    .get(name)
                                    .and_then(|v| v.to_str().ok())
                                    .and_then(|v| v.parse::<u64>().ok())
                            };
                            if actual != status
                                || number("x-ratelimit-limit") != Some(limit)
                                || number("x-ratelimit-remaining") != Some(remaining)
                            {
                                return Err(format!("Unexpected multi-quota headers/status for {user}: {actual}"));
                            }
                            if status == 429 && !response.headers().contains_key("retry-after") {
                                return Err("Missing retry advice".into());
                            }
                            let body = response.text().await.map_err(|e| e.to_string())?;
                            if status == 429 && body != r#"{"message":"Rate limit exceeded"}"# {
                                return Err("Unexpected quota rejection body".into());
                            }
                        }
                        // Observe the new header selection with fresh probe identities,
                        // then verify a previously exhausted named bucket survives reordering.
                        let original = include_str!(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/integration/conf/EdgionPlugins/RateLimitRedis/06_EdgionPlugins_multi-quota.yaml"
                        ))
                        .split("\n---\n")
                        .next()
                        .ok_or("Missing multi-quota resource")?;
                        let mut changed: serde_yaml::Value =
                            serde_yaml::from_str(original).map_err(|e| e.to_string())?;
                        let config = &mut changed["spec"]["requestPlugins"][0]["config"];
                        config["quotas"].as_sequence_mut().ok_or("Missing quotas")?.reverse();
                        config["responseQuota"] = serde_yaml::Value::String("tenant".into());
                        ctx.apply_yaml(&serde_yaml::to_string(&changed).map_err(|e| e.to_string())?)
                            .await?;
                        let checked: Result<(), String> = async {
                            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                            loop {
                                let probe = uuid::Uuid::new_v4().to_string();
                                let response = ctx
                                    .http_client
                                    .get(format!("{}/echo", ctx.http_url()))
                                    .header("host", "redis-multi-quota.example.com")
                                    .header("X-Tenant", probe)
                                    .header("X-User", "probe")
                                    .send()
                                    .await
                                    .map_err(|e| e.to_string())?;
                                if response.status().as_u16() == 200
                                    && response
                                        .headers()
                                        .get("x-ratelimit-limit")
                                        .and_then(|v| v.to_str().ok())
                                        == Some("4")
                                {
                                    break;
                                }
                                if std::time::Instant::now() >= deadline {
                                    return Err("Quota reorder did not converge".into());
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                            }
                            let response = ctx
                                .http_client
                                .get(format!("{}/echo", ctx.http_url()))
                                .header("host", "redis-multi-quota.example.com")
                                .header("X-Tenant", &tenant)
                                .header("X-User", "a")
                                .send()
                                .await
                                .map_err(|e| e.to_string())?;
                            if response.status().as_u16() != 429
                                || response
                                    .headers()
                                    .get("x-ratelimit-limit")
                                    .and_then(|v| v.to_str().ok())
                                    != Some("2")
                            {
                                return Err("Reordering reset or aliased a named Redis counter".into());
                            }
                            Ok(())
                        }
                        .await;
                        ctx.apply_yaml(original).await?;
                        checked?;
                        Ok(())
                    }
                    .await;
                    match outcome {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    // ==================== 1. conditional composite key with real Redis ====================
    fn test_conditional_composite_limit() -> TestCase {
        TestCase::new(
            "rate_limit_redis_conditional_composite",
            "conditions select traffic and directPeerIp + normalized host + header form an independent Redis bucket",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/rate-limit-redis/allow", ctx.target_host);
                    let rate_key = format!("composite-{}", uuid::Uuid::new_v4());

                    // Equivalent authority spellings must use one normalized Host slot.
                    let hosts = [
                        "rate-limit-redis.example.com",
                        "RATE-LIMIT-REDIS.EXAMPLE.COM.:31180",
                        "rate-limit-redis.example.com",
                    ];
                    for (index, host) in hosts.into_iter().enumerate() {
                        let response = client
                            .get(&url)
                            .header("host", host)
                            .header("X-Rate-Apply", "yes")
                            .header("X-Rate-Key", &rate_key)
                            .send()
                            .await;
                        match response {
                            Ok(response) if response.status().as_u16() == 200 => {}
                            Ok(response) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Composite request {} expected 200, got {}",
                                        index + 1,
                                        response.status().as_u16()
                                    ),
                                );
                            }
                            Err(error) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Composite request {} failed: {}", index + 1, error),
                                );
                            }
                        }
                    }

                    let rejected = client
                        .get(&url)
                        .header("host", "RATE-LIMIT-REDIS.EXAMPLE.COM.:31180")
                        .header("X-Rate-Apply", "yes")
                        .header("X-Rate-Key", &rate_key)
                        .send()
                        .await;
                    match rejected {
                        Ok(response) if response.status().as_u16() == 429 => {}
                        Ok(response) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "Fourth equivalent composite request expected 429, got {}",
                                    response.status().as_u16()
                                ),
                            );
                        }
                        Err(error) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Rejected composite request failed: {}", error),
                            );
                        }
                    }

                    // A different descriptor must retain its own quota.
                    let independent = client
                        .get(&url)
                        .header("host", "rate-limit-redis.example.com")
                        .header("X-Rate-Apply", "yes")
                        .header("X-Rate-Key", format!("independent-{}", uuid::Uuid::new_v4()))
                        .send()
                        .await;
                    match independent {
                        Ok(response) if response.status().as_u16() == 200 => {}
                        Ok(response) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "Independent composite descriptor expected 200, got {}",
                                    response.status().as_u16()
                                ),
                            );
                        }
                        Err(error) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Independent composite request failed: {}", error),
                            );
                        }
                    }

                    // The exhausted descriptor must be bypassed when the entry run condition is false.
                    let bypassed = client
                        .get(&url)
                        .header("host", "rate-limit-redis.example.com")
                        .header("X-Rate-Key", &rate_key)
                        .send()
                        .await;
                    match bypassed {
                        Ok(response) if response.status().as_u16() == 200 => TestResult::passed_with_message(
                            start.elapsed(),
                            "Composite bucket exhausted, independent descriptor allowed, and condition miss bypassed the limiter"
                                .to_string(),
                        ),
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!("Condition-bypass request expected 200, got {}", response.status().as_u16()),
                        ),
                        Err(error) => TestResult::failed(
                            start.elapsed(),
                            format!("Condition-bypass request failed: {}", error),
                        ),
                    }
                })
            },
        )
    }

    // ==================== 2. fail-open: Redis unavailable => allow ====================
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

    // ==================== 3. fail-close: Redis unavailable => deny ====================
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

    // ==================== 4. missing key => deny ====================
    fn test_missing_key_returns_429() -> TestCase {
        TestCase::new(
            "rate_limit_redis_missing_key_denied",
            "onKeyMiss: Deny — request without X-Rate-Key header is rejected with 429",
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
                                    format!("Got 429 as expected (onKeyMiss: Deny), body: {}", body),
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
            Self::test_conditional_composite_limit(),
            Self::test_independent_quotas_and_final_header_ownership(),
            Self::test_fail_open_allows_when_redis_unavailable(),
            Self::test_fail_close_denies_when_redis_unavailable(),
            Self::test_missing_key_returns_429(),
        ]
    }
}
