// AllowedRoutes Same Namespace test suite
//
// Required config files:
// - Gateway/AllowedRoutes/Same/Gateway.yaml               # Gateway with allowedRoutes.namespaces.from: Same
// - Gateway/AllowedRoutes/Same/HTTPRoute_same_namespace.yaml # Route in same namespace (allowed)
// - Gateway/AllowedRoutes/Same/HTTPRoute_diff_namespace.yaml # Route in different namespace (denied)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct AllowedRoutesSameNamespaceTestSuite;

impl AllowedRoutesSameNamespaceTestSuite {
    /// Test Same namespace - Route in same namespace should be allowed
    fn test_same_namespace_allowed() -> TestCase {
        TestCase::new(
            "same_namespace_allowed",
            "Test Route in same namespace is allowed",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31210/health");

                    let response = client.get(&url).header("Host", "same-ns.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("✓ Route in same namespace allowed (status: {})", resp.status()),
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

    /// Test different namespace - Route in different namespace should be denied
    fn test_diff_namespace_denied() -> TestCase {
        TestCase::new(
            "diff_namespace_denied",
            "Test Route in different namespace is denied (negative test)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::new();
                    let url = format!("http://127.0.0.1:31210/health");

                    let response = client.get(&url).header("Host", "diff-ns.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            if resp.status() == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Route in different namespace correctly denied with 404".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 404 for different namespace route, got {}", resp.status()),
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

impl TestSuite for AllowedRoutesSameNamespaceTestSuite {
    fn name(&self) -> &str {
        "Gateway AllowedRoutes Same Namespace Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_same_namespace_allowed(), Self::test_diff_namespace_denied()]
    }
}
