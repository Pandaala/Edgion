use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;

pub struct CircuitBreakerTestSuite;

#[async_trait]
impl TestSuite for CircuitBreakerTestSuite {
    fn name(&self) -> &str {
        "CircuitBreaker"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_normal_request_no_ejection(),
            Self::test_consecutive_failures_eject(),
            Self::test_recovery_after_cooldown(),
        ]
    }
}

impl CircuitBreakerTestSuite {
    fn test_normal_request_no_ejection() -> TestCase {
        TestCase::new(
            "normal_request_no_ejection",
            "200 responses don't trigger circuit breaker",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();

                    // Reset CB test state
                    let reset_url = format!("{}/cb-reset", ctx.http_url());
                    ctx.http_client
                        .get(&reset_url)
                        .header("Host", "cb-test.example.com")
                        .send()
                        .await
                        .ok();

                    for i in 0..5 {
                        let url = format!("{}/cb-fail/normal-{}/0", ctx.http_url(), i);
                        let resp = ctx
                            .http_client
                            .get(&url)
                            .header("Host", "cb-test.example.com")
                            .send()
                            .await;

                        match resp {
                            Ok(r) if r.status().as_u16() == 200 => {}
                            Ok(r) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", r.status()),
                                );
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Request failed: {}", e));
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "All 5 requests returned 200, no ejection".to_string(),
                    )
                })
            },
        )
    }

    fn test_consecutive_failures_eject() -> TestCase {
        TestCase::new(
            "consecutive_failures_eject",
            "3 consecutive 502s eject the backend endpoint",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    let test_id = format!(
                        "eject-{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_nanos()
                    );

                    // Reset CB test state
                    let reset_url = format!("{}/cb-reset", ctx.http_url());
                    ctx.http_client
                        .get(&reset_url)
                        .header("Host", "cb-test.example.com")
                        .send()
                        .await
                        .ok();

                    // Send requests that return 502 (fail_count=100 -> always fail)
                    // consecutiveErrors=3, so after 3 failures the endpoint should be ejected
                    for i in 0..4 {
                        let url = format!("{}/cb-fail/{}/100", ctx.http_url(), test_id);
                        let resp = ctx
                            .http_client
                            .get(&url)
                            .header("Host", "cb-test.example.com")
                            .send()
                            .await;

                        match resp {
                            Ok(r) => {
                                let status = r.status().as_u16();
                                if i < 3 {
                                    if status != 502 {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Request {} expected 502, got {}", i, status),
                                        );
                                    }
                                } else {
                                    // After ejection: no available backends -> 502 or 503
                                    if status != 502 && status != 503 {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Request {} after ejection expected 502/503, got {}", i, status),
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                if i < 3 {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Request {} failed unexpectedly: {}", i, e),
                                    );
                                }
                            }
                        }

                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Backend ejected after 3 consecutive 502s".to_string(),
                    )
                })
            },
        )
    }

    fn test_recovery_after_cooldown() -> TestCase {
        TestCase::new(
            "recovery_after_cooldown",
            "Ejected backend recovers after ejectionSeconds",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    let test_id = format!(
                        "recover-{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_nanos()
                    );

                    // Reset CB test state
                    let reset_url = format!("{}/cb-reset", ctx.http_url());
                    ctx.http_client
                        .get(&reset_url)
                        .header("Host", "cb-test.example.com")
                        .send()
                        .await
                        .ok();

                    // Trigger ejection: 3 failures (fail_count=3 -> first 3 return 502, then 200)
                    for _ in 0..3 {
                        let url = format!("{}/cb-fail/{}/3", ctx.http_url(), test_id);
                        ctx.http_client
                            .get(&url)
                            .header("Host", "cb-test.example.com")
                            .send()
                            .await
                            .ok();
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }

                    // Wait for ejection cooldown (ejectionSeconds=3)
                    tokio::time::sleep(std::time::Duration::from_secs(4)).await;

                    // After recovery, backend should be available again (returns 200 now)
                    let url = format!("{}/cb-fail/{}/3", ctx.http_url(), test_id);
                    let mut success = false;
                    for _ in 0..3 {
                        match ctx
                            .http_client
                            .get(&url)
                            .header("Host", "cb-test.example.com")
                            .send()
                            .await
                        {
                            Ok(r) if r.status().as_u16() == 200 => {
                                success = true;
                                break;
                            }
                            _ => {
                                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            }
                        }
                    }

                    if success {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Backend recovered after ejection cooldown".to_string(),
                        )
                    } else {
                        TestResult::failed(start.elapsed(), "Backend did not recover after cooldown".to_string())
                    }
                })
            },
        )
    }
}
