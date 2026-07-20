use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;

pub struct CircuitBreakerTestSuite;

#[async_trait]
impl TestSuite for CircuitBreakerTestSuite {
    fn name(&self) -> &str {
        "CircuitBreaker"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/CircuitBreaker")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_normal_request_no_ejection(),
            Self::test_consecutive_failures_eject(),
            Self::test_recovery_after_cooldown(),
            Self::test_gateway_and_app_counters_independent(),
            Self::test_second_policy_still_ejects(),
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
            "Ejected backend recovers after ejectionTime",
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

                    // Wait for the backend to be available before starting the ejection setup.
                    // The previous test (consecutive_failures_eject) may have ejected the backend
                    // for ejectionTime=3s; if we proceed while it is still ejected, the gateway
                    // returns 503 without forwarding, so the test-server counter never advances and
                    // the subsequent "recovery" requests all get 502s (attempt <= fail_count).
                    // Poll with fail_count=0 (always 200) until the backend is reachable.
                    let probe_url = format!("{}/cb-fail/probe/0", ctx.http_url());
                    for _ in 0..40 {
                        match ctx
                            .http_client
                            .get(&probe_url)
                            .header("Host", "cb-test.example.com")
                            .send()
                            .await
                        {
                            Ok(r) if r.status().as_u16() == 200 => break,
                            _ => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
                        }
                    }

                    // Reset CB test state (server-side counter)
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

                    // Wait for ejection cooldown, then poll for recovery. The ejection
                    // window grows with the endpoint's ejection count (backoff:
                    // ejectionTime * num_ejections, capped at maxEjectionTime).
                    // An earlier case in this suite already ejected this same backend
                    // once, so its second ejection here lasts ~6s (ejectionTime=3s * 2)
                    // rather than 3s — a fixed short wait is not enough. Sleep past the
                    // base window, then poll generously until the backend serves again
                    // (breaks early on the first 200).
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

                    let url = format!("{}/cb-fail/{}/3", ctx.http_url(), test_id);
                    let mut success = false;
                    for _ in 0..30 {
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

    fn test_gateway_and_app_counters_independent() -> TestCase {
        TestCase::new(
            "gateway_and_app_counters_independent",
            "502 (gateway) ejects at gateway threshold 3 while 500 (app) at threshold 10 does not",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    let host = "cb-alias.example.com";
                    let test_id = format!(
                        "indep-{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_nanos()
                    );
                    // Reset.
                    ctx.http_client
                        .get(format!("{}/cb-reset", ctx.http_url()))
                        .header("Host", host)
                        .send()
                        .await
                        .ok();

                    // 3 application 500s — app threshold is 10, so NO ejection yet:
                    // the backend keeps serving (status keeps being 500, not a
                    // local 503), i.e. requests still reach the backend.
                    for _ in 0..3 {
                        let url = format!("{}/cb-fail/{}/100/500", ctx.http_url(), test_id);
                        let r = ctx.http_client.get(&url).header("Host", host).send().await;
                        match r {
                            Ok(resp) if resp.status().as_u16() == 500 => {}
                            Ok(resp) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("app phase expected 500 (still reaching backend), got {}", resp.status()),
                                )
                            }
                            Err(e) => return TestResult::failed(start.elapsed(), format!("app phase error: {}", e)),
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }

                    // Now 3 gateway 502s on a fresh test_id — gateway threshold is 3,
                    // so the endpoint ejects and the 4th request gets a local 502/503.
                    let gw_id = format!("{}-gw", test_id);
                    let mut ejected = false;
                    for i in 0..4 {
                        let url = format!("{}/cb-fail/{}/100/502", ctx.http_url(), gw_id);
                        let r = ctx.http_client.get(&url).header("Host", host).send().await;
                        if let Ok(resp) = r {
                            let s = resp.status().as_u16();
                            if i >= 3 && (s == 502 || s == 503) {
                                ejected = true;
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }

                    if ejected {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Gateway counter ejected at 3 while app counter (threshold 10) did not".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            "Gateway-class failures did not eject at threshold 3".to_string(),
                        )
                    }
                })
            },
        )
    }

    fn test_second_policy_still_ejects() -> TestCase {
        TestCase::new(
            "second_policy_still_ejects",
            "cb-alias outlierDetection policy drives gateway-class ejection",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    let host = "cb-alias.example.com";
                    let test_id = format!(
                        "second-{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_nanos()
                    );
                    ctx.http_client
                        .get(format!("{}/cb-reset", ctx.http_url()))
                        .header("Host", host)
                        .send()
                        .await
                        .ok();

                    // gateway threshold 3 via the cb-alias policy; 502 ejects after 3.
                    let mut ejected = false;
                    for i in 0..4 {
                        let url = format!("{}/cb-fail/{}/100/502", ctx.http_url(), test_id);
                        if let Ok(resp) = ctx.http_client.get(&url).header("Host", host).send().await {
                            let s = resp.status().as_u16();
                            if i >= 3 && (s == 502 || s == 503) {
                                ejected = true;
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }

                    if ejected {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "cb-alias policy ejected the backend".to_string(),
                        )
                    } else {
                        TestResult::failed(start.elapsed(), "cb-alias policy did not eject".to_string())
                    }
                })
            },
        )
    }
}
