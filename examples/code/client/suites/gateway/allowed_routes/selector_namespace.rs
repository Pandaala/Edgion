// AllowedRoutes Selector Namespace test suite
//
// Tests the Selector namespace policy behavior.
//
// This suite validates Selector namespace policy in both execution modes:
// - FileSystem mode: missing namespace labels fall back to Same
// - K8s mode: namespace labels are evaluated by the controller
//
// The shared expectation remains the same:
//   - Same namespace route   → Accepted=True, compiled, allowed (200)
//   - Cross namespace route  → Accepted=False, not compiled, denied (404)
//
// This verifies the Selector code path is active and behaves differently from
// "All" (which would allow cross-namespace) while maintaining security.
//
// Required config files:
// - Gateway/AllowedRoutes/Selector/01_Gateway.yaml            # from: Selector with matchLabels
// - Gateway/AllowedRoutes/Selector/HTTPRoute_same_ns.yaml     # Same ns route (allowed)
// - Gateway/AllowedRoutes/Selector/HTTPRoute_cross_ns.yaml    # Cross ns route (denied)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct AllowedRoutesSelectorNamespaceTestSuite;

impl AllowedRoutesSelectorNamespaceTestSuite {
    /// Same-namespace route should be allowed, either via Selector label match
    /// in K8s mode or Same fallback when labels are unavailable.
    fn test_same_namespace_allowed() -> TestCase {
        TestCase::new(
            "selector_same_ns_allowed",
            "Selector allows same-namespace route on data-plane",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();
                    let url = format!("http://{}:{}/health", ctx.target_host, ctx.http_port);

                    let response = client
                        .get(&url)
                        .header("Host", "selector-same-ns.example.com")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Selector correctly allowed same-ns route (status: {})", resp.status()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 200 for same-ns route under Selector policy, got {}",
                                        resp.status()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Cross-namespace route should be denied when it does not match the
    /// Selector policy. In FileSystem mode this is enforced by Same fallback.
    fn test_cross_namespace_denied() -> TestCase {
        TestCase::new(
            "selector_cross_ns_denied",
            "Selector denies cross-namespace route on data-plane",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();
                    let url = format!("http://{}:{}/health", ctx.target_host, ctx.http_port);

                    let response = client
                        .get(&url)
                        .header("Host", "selector-cross-ns.example.com")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            if resp.status() == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Selector correctly denied cross-ns route".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 404 for cross-ns route under Selector policy, got {}",
                                        resp.status()
                                    ),
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

impl TestSuite for AllowedRoutesSelectorNamespaceTestSuite {
    fn name(&self) -> &str {
        "Gateway AllowedRoutes Selector Namespace Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_same_namespace_allowed(), Self::test_cross_namespace_denied()]
    }
}
