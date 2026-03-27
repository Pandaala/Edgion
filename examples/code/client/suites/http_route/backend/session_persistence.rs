// ============================================================================
// Session Persistence Test Suite
// ============================================================================
//
// Verifies Gateway API Session Persistence (Cookie and Header modes).
//
// Test scenarios:
// 1. Cookie persistence - first request gets Set-Cookie, subsequent requests
//    with that cookie are routed successfully
// 2. Header persistence - first request gets session header in response,
//    subsequent requests with that header are routed successfully

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct SessionPersistenceTestSuite;

impl SessionPersistenceTestSuite {
    /// Test Cookie-based session persistence
    fn test_cookie_persistence() -> TestCase {
        TestCase::new(
            "cookie_persistence",
            "Verify Cookie session persistence - Set-Cookie on first request, routing with cookie",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("Failed to create HTTP client");

                    // Step 1: Send initial request WITHOUT cookie
                    let url = format!("http://{}:31313/test", ctx.target_host);
                    let resp = match client
                        .get(&url)
                        .header("host", "sp-cookie.example.com")
                        .header("x-trace-id", "sp-cookie-init")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Initial request failed: {}", e));
                        }
                    };

                    if resp.status() != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", resp.status()));
                    }

                    // Step 2: Verify Set-Cookie header
                    let set_cookie = resp
                        .headers()
                        .get("set-cookie")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());

                    let cookie_header = match &set_cookie {
                        Some(sc) if sc.starts_with("edgion-sp-test=") => sc.clone(),
                        Some(sc) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Set-Cookie does not start with 'edgion-sp-test=': {}", sc),
                            );
                        }
                        None => {
                            return TestResult::failed(start.elapsed(), "No Set-Cookie header in response".to_string());
                        }
                    };

                    // Step 3: Verify cookie attributes
                    let mut missing_attrs = Vec::new();
                    if !cookie_header.contains("Path=/") {
                        missing_attrs.push("Path=/");
                    }
                    if !cookie_header.contains("Max-Age=") {
                        missing_attrs.push("Max-Age=");
                    }
                    if !cookie_header.contains("HttpOnly") {
                        missing_attrs.push("HttpOnly");
                    }
                    if !missing_attrs.is_empty() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Set-Cookie missing attributes {:?}: {}", missing_attrs, cookie_header),
                        );
                    }

                    // Step 4: Extract cookie name=value for subsequent requests
                    let cookie_value = cookie_header.split(';').next().unwrap_or(&cookie_header).to_string();

                    // Step 5: Send 5 requests WITH the cookie, verify all succeed
                    for i in 0..5 {
                        let resp = client
                            .get(&url)
                            .header("host", "sp-cookie.example.com")
                            .header("cookie", &cookie_value)
                            .header("x-trace-id", format!("sp-cookie-{}", i))
                            .send()
                            .await;

                        match resp {
                            Ok(r) if r.status() == 200 => {}
                            Ok(r) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Cookie request {} got status {}", i, r.status()),
                                );
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Cookie request {} failed: {}", i, e),
                                );
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "Cookie persistence OK: Set-Cookie={}, 5 subsequent requests succeeded",
                            cookie_value
                        ),
                    )
                })
            },
        )
    }

    /// Test Header-based session persistence
    fn test_header_persistence() -> TestCase {
        TestCase::new(
            "header_persistence",
            "Verify Header session persistence - response header on first request, routing with header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("Failed to create HTTP client");

                    // Step 1: Send initial request WITHOUT session header
                    let url = format!("http://{}:31313/test", ctx.target_host);
                    let resp = match client
                        .get(&url)
                        .header("host", "sp-header.example.com")
                        .header("x-trace-id", "sp-header-init")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Initial request failed: {}", e));
                        }
                    };

                    if resp.status() != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", resp.status()));
                    }

                    // Step 2: Verify x-session-id header in response
                    let token = match resp
                        .headers()
                        .get("x-session-id")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string())
                    {
                        Some(t) if !t.is_empty() => t,
                        Some(_) => {
                            return TestResult::failed(start.elapsed(), "x-session-id header is empty".to_string());
                        }
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                "No x-session-id header in response".to_string(),
                            );
                        }
                    };

                    // Step 3: Send 5 requests WITH the session header, verify all succeed
                    for i in 0..5 {
                        let resp = client
                            .get(&url)
                            .header("host", "sp-header.example.com")
                            .header("x-session-id", &token)
                            .header("x-trace-id", format!("sp-header-{}", i))
                            .send()
                            .await;

                        match resp {
                            Ok(r) if r.status() == 200 => {}
                            Ok(r) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Header request {} got status {}", i, r.status()),
                                );
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Header request {} failed: {}", i, e),
                                );
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "Header persistence OK: token={}, 5 subsequent requests succeeded",
                            token
                        ),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for SessionPersistenceTestSuite {
    fn name(&self) -> &str {
        "Session Persistence Tests"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/SessionPersistence")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_cookie_persistence(), Self::test_header_persistence()]
    }
}
