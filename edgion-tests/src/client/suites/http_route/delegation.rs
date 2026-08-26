// HTTPRoute Delegation test suite
//
// Required config files (in edgion-tests/integration/conf/HTTPRoute/Delegation/):
// - Gateway.yaml                                     # delegation-gateway (port 31932)
// - HTTPRoute_edgion-test_delegation-parent.yaml     # root route: /plain + wildcard delegation of /api/v1/billing
// - HTTPRoute_edgion-deleg_billing-child.yaml        # child: /api/v1/billing/pay (valid) + /evil (out of prefix)
// - Service/EndpointSlice for child-svc (edgion-deleg → test_server 30001)
//
// Verifies the controller-side delegation flatten end-to-end:
// - delegated path routes through the child's backend
// - the parent's mandatory filter is inherited onto delegated traffic
// - out-of-prefix child rules are dropped (404)
// - the child is unreachable except through the delegation (no direct attach)
// - the parent's sibling plain rule keeps working

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

const HOST: &str = "delegation.example.com";

async fn get_via_gateway(ctx: &TestContext, path: &str) -> reqwest::Result<(u16, String)> {
    let response = ctx
        .http_client
        .get(format!("{}{}", ctx.http_url(), path))
        .header("Host", HOST)
        .send()
        .await?;
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}

pub struct DelegationTestSuite;

impl DelegationTestSuite {
    /// Delegated path routes to the child's backend and inherits the parent filter.
    fn test_delegated_path_routes_with_inherited_filter() -> TestCase {
        TestCase::new(
            "delegated_path_and_inherited_filter",
            "Delegated /api/v1/billing/pay reaches the child backend and carries the parent's mandatory header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match get_via_gateway(&ctx, "/api/v1/billing/pay").await {
                        Ok((200, body)) => {
                            // test_server's catch-all echoes request headers; the
                            // parent's RequestHeaderModifier must have set this.
                            if body.to_lowercase().contains("x-parent-filter: on") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "delegated path 200 with inherited x-parent-filter".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("parent filter header missing on delegated traffic; body: {body}"),
                                )
                            }
                        }
                        Ok((status, _)) => {
                            TestResult::failed(start.elapsed(), format!("expected 200 on delegated path, got {status}"))
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {e}")),
                    }
                })
            },
        )
    }

    /// The child's out-of-prefix rule (/evil) must be dropped by the flatten step.
    fn test_out_of_prefix_child_rule_dropped() -> TestCase {
        TestCase::new(
            "out_of_prefix_child_rule_dropped",
            "Child rule outside the delegated prefix is not routable (404)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match get_via_gateway(&ctx, "/evil").await {
                        Ok((404, _)) => TestResult::passed(start.elapsed()),
                        Ok((status, _)) => TestResult::failed(
                            start.elapsed(),
                            format!("expected 404 for out-of-prefix child rule, got {status}"),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {e}")),
                    }
                })
            },
        )
    }

    /// A path under the delegated prefix but matching no child rule stays 404.
    fn test_unmatched_delegated_path_404() -> TestCase {
        TestCase::new(
            "unmatched_delegated_path_404",
            "Delegated prefix without a matching child rule returns 404",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match get_via_gateway(&ctx, "/api/v1/billing/unmapped").await {
                        Ok((404, _)) => TestResult::passed(start.elapsed()),
                        Ok((status, _)) => TestResult::failed(
                            start.elapsed(),
                            format!("expected 404 for unmatched delegated path, got {status}"),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {e}")),
                    }
                })
            },
        )
    }

    /// The parent's non-delegating sibling rule keeps working.
    fn test_plain_sibling_rule_intact() -> TestCase {
        TestCase::new(
            "plain_sibling_rule_intact",
            "Parent's plain /plain rule still routes normally",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match get_via_gateway(&ctx, "/plain/anything").await {
                        Ok((200, body)) => {
                            // The plain rule must NOT inherit the delegating
                            // rule's filter (filters are per-rule).
                            if body.to_lowercase().contains("x-parent-filter: on") {
                                TestResult::failed(
                                    start.elapsed(),
                                    "plain rule unexpectedly carries the delegating rule's filter".to_string(),
                                )
                            } else {
                                TestResult::passed(start.elapsed())
                            }
                        }
                        Ok((status, _)) => {
                            TestResult::failed(start.elapsed(), format!("expected 200 on plain rule, got {status}"))
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {e}")),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for DelegationTestSuite {
    fn name(&self) -> &str {
        "HTTPRoute Delegation"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_delegated_path_routes_with_inherited_filter(),
            Self::test_out_of_prefix_child_rule_dropped(),
            Self::test_unmatched_delegated_path_404(),
            Self::test_plain_sibling_rule_intact(),
        ]
    }
}

// ============================================================
// Dynamic scenario phase suites (RouteDelegationDynamic)
// ============================================================

const DYN_HOST: &str = "delegation-dyn.example.com";
async fn dyn_status(ctx: &TestContext, path: &str) -> reqwest::Result<u16> {
    let url = format!("{}{}", ctx.http_url(), path);
    ctx.http_client
        .get(&url)
        .header("Host", DYN_HOST)
        .send()
        .await
        .map(|r| r.status().as_u16())
}

pub struct DelegationDynamicInitialTestSuite;

impl DelegationDynamicInitialTestSuite {
    fn pay_routes_pay2_missing() -> TestCase {
        TestCase::new(
            "delegation_dyn_initial",
            "[INITIAL] /api/v1/billing/pay → 200, /api/v1/billing/pay2 → 404",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match (
                        dyn_status(&ctx, "/api/v1/billing/pay").await,
                        dyn_status(&ctx, "/api/v1/billing/pay2").await,
                    ) {
                        (Ok(200), Ok(404)) => TestResult::passed(start.elapsed()),
                        (a, b) => {
                            TestResult::failed(start.elapsed(), format!("expected (200, 404), got ({a:?}, {b:?})"))
                        }
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for DelegationDynamicInitialTestSuite {
    fn name(&self) -> &str {
        "HTTPRoute Delegation Dynamic - Initial Phase"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::pay_routes_pay2_missing()]
    }
}

pub struct DelegationDynamicUpdateTestSuite;

impl DelegationDynamicUpdateTestSuite {
    fn pay2_routes_pay_missing() -> TestCase {
        TestCase::new(
            "delegation_dyn_update",
            "[UPDATE] child rule moved: /api/v1/billing/pay2 → 200, /api/v1/billing/pay → 404",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match (
                        dyn_status(&ctx, "/api/v1/billing/pay2").await,
                        dyn_status(&ctx, "/api/v1/billing/pay").await,
                    ) {
                        (Ok(200), Ok(404)) => TestResult::passed(start.elapsed()),
                        (a, b) => {
                            TestResult::failed(start.elapsed(), format!("expected (200, 404), got ({a:?}, {b:?})"))
                        }
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for DelegationDynamicUpdateTestSuite {
    fn name(&self) -> &str {
        "HTTPRoute Delegation Dynamic - Update Phase"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::pay2_routes_pay_missing()]
    }
}
