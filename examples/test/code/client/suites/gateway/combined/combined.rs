// Combined scenarios test suite
//
// Required config files:
// - Gateway/Combined/Gateway.yaml                                  # Gateway with combined constraints
// - Gateway/Combined/HTTPRoute_hostname_same_ns_match.yaml        # Hostname + Same NS match
// - Gateway/Combined/HTTPRoute_hostname_diff_ns.yaml              # Hostname match but different NS
// - Gateway/Combined/HTTPRoute_hostname_mismatch_same_ns.yaml     # Same NS but hostname mismatch
// - Gateway/Combined/HTTPRoute_section_hostname_match.yaml        # sectionName + hostname match
// - Gateway/Combined/HTTPRoute_section_hostname_mismatch.yaml     # sectionName + hostname mismatch

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct CombinedScenariosTestSuite;

impl CombinedScenariosTestSuite {
    /// Test hostname match + same namespace (both pass)
    fn test_hostname_and_same_ns_match() -> TestCase {
        TestCase::new(
            "hostname_and_same_ns_match",
            "Test hostname matches and same namespace (both constraints pass)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31230/health");

                    let response = client.get(&url).header("Host", "api.combined.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Both hostname and same namespace constraints pass".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 OK when both constraints pass, got {}", resp.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test hostname match but different namespace (namespace constraint fails)
    fn test_hostname_match_diff_ns() -> TestCase {
        TestCase::new(
            "hostname_match_diff_ns",
            "Test hostname matches but different namespace (should deny)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31230/health");

                    let response = client.get(&url).header("Host", "www.combined.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            if resp.status() == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Correctly denied when hostname matches but namespace differs".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 404 when namespace constraint fails, got {}", resp.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test same namespace but hostname mismatch (hostname constraint fails)
    fn test_same_ns_hostname_mismatch() -> TestCase {
        TestCase::new(
            "same_ns_hostname_mismatch",
            "Test same namespace but hostname mismatch (should deny)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31230/health");

                    let response = client.get(&url).header("Host", "other.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            if resp.status() == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Correctly denied when namespace matches but hostname differs".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 404 when hostname constraint fails, got {}", resp.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test sectionName + hostname match
    fn test_section_and_hostname_match() -> TestCase {
        TestCase::new(
            "section_and_hostname_match",
            "Test sectionName and hostname both match",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31231/health");

                    let response = client
                        .get(&url)
                        .header("Host", "secure.combined.example.com")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Both sectionName and hostname constraints pass".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 OK when both constraints pass, got {}", resp.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test sectionName match but hostname mismatch
    fn test_section_match_hostname_mismatch() -> TestCase {
        TestCase::new(
            "section_match_hostname_mismatch",
            "Test sectionName matches but hostname mismatch (should deny)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31231/health");

                    let response = client
                        .get(&url)
                        .header("Host", "other.combined.example.com")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            if resp.status() == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Correctly denied when sectionName matches but hostname differs".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 404 when hostname constraint fails, got {}", resp.status()),
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

impl TestSuite for CombinedScenariosTestSuite {
    fn name(&self) -> &str {
        "Gateway Combined Scenarios Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_hostname_and_same_ns_match(),
            Self::test_hostname_match_diff_ns(),
            Self::test_same_ns_hostname_mismatch(),
            Self::test_section_and_hostname_match(),
            Self::test_section_match_hostname_mismatch(),
        ]
    }
}
