// AllowedRoutes All Namespaces test suite
//
// Required config files:
// - Gateway/AllowedRoutes/All/Gateway.yaml               # Gateway with allowedRoutes.namespaces.from: All
// - Gateway/AllowedRoutes/All/HTTPRoute_same_ns.yaml # Route in same namespace
// - Gateway/AllowedRoutes/All/HTTPRoute_cross_ns.yaml # Route in different namespace

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct AllowedRoutesAllNamespacesTestSuite;

impl AllowedRoutesAllNamespacesTestSuite {
    /// Test All namespaces - Route in same namespace should be allowed
    fn test_same_namespace_allowed() -> TestCase {
        TestCase::new(
            "all_same_namespace_allowed",
            "Test Route in same namespace is allowed (with from: All)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31211/health");

                    let response = client
                        .get(&url)
                        .header("Host", "all-same-ns.example.com")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("✓ Same namespace route allowed with from: All (status: {})", resp.status()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 OK for same namespace route, got {}", resp.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test All namespaces - Route in different namespace should also be allowed
    fn test_cross_namespace_allowed() -> TestCase {
        TestCase::new(
            "all_cross_namespace_allowed",
            "Test Route in different namespace is allowed (with from: All)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31211/health");

                    let response = client
                        .get(&url)
                        .header("Host", "all-cross-ns.example.com")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("✓ Cross namespace route allowed with from: All (status: {})", resp.status()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 OK for cross namespace route with from: All, got {}", resp.status()),
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

impl TestSuite for AllowedRoutesAllNamespacesTestSuite {
    fn name(&self) -> &str {
        "Gateway AllowedRoutes All Namespaces Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_same_namespace_allowed(),
            Self::test_cross_namespace_allowed(),
        ]
    }
}
