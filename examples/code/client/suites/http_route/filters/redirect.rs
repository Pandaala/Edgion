// HTTP to HTTPS Redirect Test suite
//
// Test Gateway annotation edgion.io/http-to-https-redirect feature
//
// Required config files (in examples/conf/):
// - Gateway_edge_redirect-gateway.yaml   # Gateway with redirect enabled (port 10081)
//
// Test scenarios:
// 1. Simple path redirect
// 2. Redirect with query params
// 3. Verify 301 status code
// 4. Verify Location header format

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use reqwest::redirect::Policy;
use std::time::Instant;

// HTTP redirect test port loaded from ports.json， ctx.http_port

pub struct HttpRedirectTestSuite;

impl HttpRedirectTestSuite {
    /// Test simple path redirect
    fn test_simple_redirect() -> TestCase {
        TestCase::new(
            "http_redirect_simple",
            "Test simple HTTP->HTTPS redirect",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Create client without auto-redirect
                    let client = reqwest::Client::builder()
                        .redirect(Policy::none())
                        .danger_accept_invalid_certs(true)
                        .no_proxy()
                        .build()
                        .unwrap();

                    let url = format!("http://{}:{}/health", ctx.target_host, ctx.http_port);

                    match client.get(&url).header("Host", "test.example.com").send().await {
                        Ok(response) => {
                            let status = response.status();

                            // Verify 301 redirect
                            if status.as_u16() != 301 {
                                return TestResult::failed(start.elapsed(), format!("Expected 301, got {}", status));
                            }

                            // Verify Location header
                            match response.headers().get("location") {
                                Some(location) => {
                                    let location_str = location.to_str().unwrap_or("");

                                    // Verify redirect URL format (https scheme, correct host and path)
                                    if location_str.starts_with("https://test.example.com:")
                                        && location_str.ends_with("/health")
                                    {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("301 Redirect to: {}", location_str),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Invalid redirect URL format: {}", location_str),
                                        )
                                    }
                                }
                                None => TestResult::failed(start.elapsed(), "Missing Location header".to_string()),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test redirect with query params
    fn test_redirect_with_query() -> TestCase {
        TestCase::new(
            "http_redirect_with_query",
            "Test redirect with query params",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .redirect(Policy::none())
                        .danger_accept_invalid_certs(true)
                        .no_proxy()
                        .build()
                        .unwrap();

                    let url = format!("http://{}:{}/api/users?page=1&limit=10", ctx.target_host, ctx.http_port);

                    match client.get(&url).header("Host", "api.example.com").send().await {
                        Ok(response) => {
                            let status = response.status();

                            if status.as_u16() != 301 {
                                return TestResult::failed(start.elapsed(), format!("Expected 301, got {}", status));
                            }

                            match response.headers().get("location") {
                                Some(location) => {
                                    let location_str = location.to_str().unwrap_or("");

                                    // Verify query params preserved
                                    if location_str.contains("page=1") && location_str.contains("limit=10") {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("Query params preserved: {}", location_str),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Query params not preserved in: {}", location_str),
                                        )
                                    }
                                }
                                None => TestResult::failed(start.elapsed(), "Missing Location header".to_string()),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test redirect uses correct HTTPS scheme
    fn test_redirect_https_scheme() -> TestCase {
        TestCase::new(
            "http_redirect_https_scheme",
            "Verify redirect URL uses HTTPS scheme",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .redirect(Policy::none())
                        .danger_accept_invalid_certs(true)
                        .no_proxy()
                        .build()
                        .unwrap();

                    let url = format!("http://{}:{}/test", ctx.target_host, ctx.http_port);

                    match client.get(&url).header("Host", "secure.example.com").send().await {
                        Ok(response) => {
                            if response.status().as_u16() != 301 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 301, got {}", response.status()),
                                );
                            }

                            match response.headers().get("location") {
                                Some(location) => {
                                    let location_str = location.to_str().unwrap_or("");

                                    if location_str.starts_with("https://") {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("HTTPS scheme verified: {}", location_str),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected https:// scheme, got: {}", location_str),
                                        )
                                    }
                                }
                                None => TestResult::failed(start.elapsed(), "Missing Location header".to_string()),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test POST request also redirected
    fn test_redirect_post_request() -> TestCase {
        TestCase::new(
            "http_redirect_post",
            "Test POST request returns redirect",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .redirect(Policy::none())
                        .danger_accept_invalid_certs(true)
                        .no_proxy()
                        .build()
                        .unwrap();

                    let url = format!("http://{}:{}/api/create", ctx.target_host, ctx.http_port);

                    match client
                        .post(&url)
                        .header("Host", "api.example.com")
                        .body("test data")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status();

                            if status.as_u16() == 301 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("POST request redirected with {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 301 for POST, got {}", status))
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
impl TestSuite for HttpRedirectTestSuite {
    fn name(&self) -> &str {
        "HTTP Redirect"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_simple_redirect(),
            Self::test_redirect_with_query(),
            Self::test_redirect_https_scheme(),
            Self::test_redirect_post_request(),
        ]
    }
}
