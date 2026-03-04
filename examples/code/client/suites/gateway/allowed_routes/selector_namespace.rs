// AllowedRoutes Selector Namespace test suite
//
// Tests the Selector namespace policy behavior at the data-plane level.
//
// In FileSystem mode (used by integration tests), the NamespaceStore is empty
// (no K8s Namespace watcher), so the controller-side Selector evaluation denies
// all routes. However, routes are still synced to the Gateway data-plane which
// performs its own independent check. The data-plane falls back to Same policy
// for Selector (defense-in-depth):
//   - Same namespace route   → allowed (200) via Same fallback
//   - Cross namespace route  → denied  (404) via Same fallback
//
// This verifies the Selector code path is active and behaves differently from
// "All" (which would allow cross-namespace) while maintaining security.
//
// Required config files:
// - Gateway/AllowedRoutes/Selector/01_Gateway.yaml            # from: Selector with matchLabels
// - Gateway/AllowedRoutes/Selector/HTTPRoute_same_ns.yaml     # Same ns route (allowed via Same fallback)
// - Gateway/AllowedRoutes/Selector/HTTPRoute_cross_ns.yaml    # Cross ns route (denied via Same fallback)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct AllowedRoutesSelectorNamespaceTestSuite;

impl AllowedRoutesSelectorNamespaceTestSuite {
    /// Same-namespace route should be allowed because the data-plane Selector
    /// falls back to Same policy (defense-in-depth).
    fn test_same_namespace_allowed_via_fallback() -> TestCase {
        TestCase::new(
            "selector_same_ns_allowed",
            "Selector allows same-namespace route via Same fallback on data-plane",
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
                                    format!(
                                        "Selector Same-fallback correctly allowed same-ns route (status: {})",
                                        resp.status()
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 200 for same-ns route with Selector→Same fallback, got {}",
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

    /// Cross-namespace route should be denied because the data-plane Selector
    /// falls back to Same policy, and the route is in a different namespace.
    fn test_cross_namespace_denied_via_fallback() -> TestCase {
        TestCase::new(
            "selector_cross_ns_denied",
            "Selector denies cross-namespace route via Same fallback on data-plane",
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
                                    "Selector Same-fallback correctly denied cross-ns route".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 404 for cross-ns route with Selector→Same fallback, got {}",
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
        vec![
            Self::test_same_namespace_allowed_via_fallback(),
            Self::test_cross_namespace_denied_via_fallback(),
        ]
    }
}
