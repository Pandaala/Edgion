// Listener Hostname constraint test suite
//
// Required config files:
// - Gateway/ListenerHostname/Gateway.yaml               # Gateway with hostname constraints
// - Gateway/ListenerHostname/HTTPRoute_exact_match.yaml # Exact hostname match
// - Gateway/ListenerHostname/HTTPRoute_wildcard_match.yaml # Wildcard hostname match
// - Gateway/ListenerHostname/HTTPRoute_mismatch.yaml    # Hostname mismatch (negative test)
// - Gateway/ListenerHostname/HTTPRoute_no_restriction.yaml # No hostname restriction
// - Gateway/ListenerHostname/HTTPRoute_wildcard_root_mismatch.yaml # Wildcard doesn't match root

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct ListenerHostnameTestSuite;

impl ListenerHostnameTestSuite {
    /// Test exact hostname match: api.example.com matches listener hostname
    fn test_exact_hostname_match() -> TestCase {
        TestCase::new(
            "exact_hostname_match",
            "Test exact hostname match between Route and Listener",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31240/health");

                    let response = client
                        .get(&url)
                        .header("Host", "api.example.com")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("✓ Exact hostname match works: api.example.com (status: {})", resp.status()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 OK for matching hostname, got {}", resp.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test hostname mismatch: other.example.com doesn't match listener hostname api.example.com
    fn test_hostname_mismatch() -> TestCase {
        TestCase::new(
            "hostname_mismatch",
            "Test hostname mismatch returns 404 (negative test)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31240/health");

                    let response = client
                        .get(&url)
                        .header("Host", "other.example.com")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            if resp.status() == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Hostname mismatch correctly rejected with 404".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 404 for non-matching hostname, got {}", resp.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test wildcard hostname match: api.wildcard.example.com matches *.wildcard.example.com
    fn test_wildcard_hostname_match() -> TestCase {
        TestCase::new(
            "wildcard_hostname_match",
            "Test wildcard hostname match (*.wildcard.example.com)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    
                    // Test 1: api.wildcard.example.com should match
                    let url1 = format!("http://127.0.0.1:31241/health");
                    let response1 = client
                        .get(&url1)
                        .header("Host", "api.wildcard.example.com")
                        .send()
                        .await;

                    match response1 {
                        Ok(resp) => {
                            if !resp.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("api.wildcard.example.com should match *.wildcard.example.com, got {}", resp.status()),
                                );
                            }
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }

                    // Test 2: www.wildcard.example.com should also match
                    let url2 = format!("http://127.0.0.1:31241/health");
                    let response2 = client
                        .get(&url2)
                        .header("Host", "www.wildcard.example.com")
                        .send()
                        .await;

                    match response2 {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Wildcard hostname match works for api.* and www.*".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("www.wildcard.example.com should match, got {}", resp.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test wildcard doesn't match root domain: wildcard.example.com doesn't match *.wildcard.example.com
    fn test_wildcard_root_mismatch() -> TestCase {
        TestCase::new(
            "wildcard_root_mismatch",
            "Test wildcard doesn't match root domain (negative test)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31241/health");

                    let response = client
                        .get(&url)
                        .header("Host", "wildcard.example.com")  // Root domain
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            if resp.status() == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Wildcard correctly doesn't match root domain".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 404 for root domain with wildcard listener, got {}", resp.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test no hostname restriction: any hostname should work when listener has no hostname
    fn test_no_hostname_restriction() -> TestCase {
        TestCase::new(
            "no_hostname_restriction",
            "Test listener with no hostname restriction allows any hostname",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31242/health");

                    let response = client
                        .get(&url)
                        .header("Host", "any-domain.example.com")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Listener without hostname restriction allows any hostname".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 OK for listener without hostname restriction, got {}", resp.status()),
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

impl TestSuite for ListenerHostnameTestSuite {
    fn name(&self) -> &str {
        "Gateway Listener Hostname Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_exact_hostname_match(),
            Self::test_hostname_mismatch(),
            Self::test_wildcard_hostname_match(),
            Self::test_wildcard_root_mismatch(),
            Self::test_no_hostname_restriction(),
        ]
    }
}
