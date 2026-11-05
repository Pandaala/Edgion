// HTTPRoute Delegation test suite (cascading match)
//
// Required config files (in edgion-tests/integration/conf/HTTPRoute/Delegation/):
// - Gateway.yaml                                     # delegation-gateway (port 31932)
// - HTTPRoute_edgion-test_delegation-parent.yaml     # root: /plain, wildcard delegation of
//                                                    # /api/v1/billing (+ mandatory header), own /api
// - HTTPRoute_edgion-deleg_billing-child.yaml        # child declaring the parent: /pay, /audit, /evil
// - EdgionPlugins_edgion-deleg_deleg-deny-*.yaml     # child plugins probing parent-first execution
// - HTTPRoute_edgion-test_precedence-root-{a,b}.yaml # per-level precedence (sibling roots, same host)
// - HTTPRoute_edgion-deleg_precedence-child.yaml
// - HTTPRoute_edgion-test_billing-v{1,2}-parent.yaml # two parents of one child
// - HTTPRoute_edgion-deleg_multi-child.yaml
// - HTTPRoute_edgion-test_chain-l1.yaml              # five-level chain root
// - HTTPRoute_edgion-deleg_chain-l{2..6}.yaml        # levels 2-5 served, level 6 DepthExceeded
// - Service/EndpointSlice for child-svc (edgion-deleg -> test_server 30001)
//
// Verifies the Gateway-side cascading match end-to-end (design
// tasks/todo/route-delegation-cascading-match-design.md §3, §6, §7, §8.2):
// - a child declaring its parent is reachable only through the parent
// - parent mandatory filters run before child plugins (chain order)
// - child paths are absolute; a disjoint child rule never matches through the parent
// - a child-subtree miss backtracks to the parent's next own rule
// - specificity is compared per level (a sibling root beats a deeper child path)
// - one child is shared by two parents, each applying only its own filter
// - a five-level chain is served, a sixth level is not and reports DepthExceeded
// - access-log `match_info` is an array ordered root -> leaf

use crate::access_log_client::{traceparent_for_marker, AccessLogEntry};
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers as status_helpers;
use async_trait::async_trait;
use serde_json::Value;
use std::time::{Duration, Instant};

const HOST: &str = "delegation.example.com";
const PRECEDENCE_HOST: &str = "precedence.delegation.example.com";
const MULTI_HOST: &str = "multi.delegation.example.com";
const CHAIN_HOST: &str = "chain.delegation.example.com";
const PARENT_NS: &str = "edgion-test";
const CHILD_NS: &str = "edgion-deleg";
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

async fn get_with_headers(
    ctx: &TestContext,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> reqwest::Result<(u16, String)> {
    let mut request = ctx
        .http_client
        .get(format!("{}{}", ctx.http_url(), path))
        .header("Host", host);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.send().await?;
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}

async fn get_via_gateway(ctx: &TestContext, path: &str) -> reqwest::Result<(u16, String)> {
    get_with_headers(ctx, HOST, path, &[]).await
}

/// test_server's catch-all echoes request headers as `name: value` lines.
fn echoed(body: &str, header: &str, value: &str) -> bool {
    body.to_lowercase().contains(&format!("{header}: {value}"))
}

/// Send one traced request and return its status, body and access-log entry.
/// The stored entry is deleted before returning.
async fn traced_get(ctx: &TestContext, host: &str, path: &str) -> Result<(u16, String, AccessLogEntry), String> {
    let marker = format!("http-delegation-{}", uuid::Uuid::new_v4());
    let traceparent = traceparent_for_marker(&marker);
    let (status, body) = get_with_headers(
        ctx,
        host,
        path,
        &[("traceparent", traceparent.as_str()), ("access_log", "test_store")],
    )
    .await
    .map_err(|e| format!("request failed: {e}"))?;
    let access_logs = ctx.access_log_client();
    let entry = access_logs
        .get_access_log_with_retry(&marker, 30, 100)
        .await
        .map_err(|e| format!("access log for {path} not found: {e}"))?;
    let _ = access_logs.delete_access_log(&marker).await;
    Ok((status, body, entry))
}

/// `(kind, ns, name, rule_id)` of every `match_info` element, root -> leaf.
fn chain_of(entry: &AccessLogEntry) -> Vec<(String, String, String, u64)> {
    entry
        .route_chain()
        .iter()
        .map(|level| {
            let rsc = &level["rsc"];
            (
                rsc["kind"].as_str().unwrap_or_default().to_string(),
                rsc["ns"].as_str().unwrap_or_default().to_string(),
                rsc["name"].as_str().unwrap_or_default().to_string(),
                level["rule_id"].as_u64().unwrap_or(u64::MAX),
            )
        })
        .collect()
}

fn expect_chain(entry: &AccessLogEntry, expected: &[(&str, &str, u64)]) -> Result<(), String> {
    let actual = chain_of(entry);
    let wanted: Vec<(String, String, String, u64)> = expected
        .iter()
        .map(|(ns, name, rule)| ("HTTPRoute".to_string(), ns.to_string(), name.to_string(), *rule))
        .collect();
    if actual == wanted {
        Ok(())
    } else {
        Err(format!(
            "match_info chain mismatch: expected {wanted:?}, got {actual:?} (raw: {})",
            entry.data.get("match_info").cloned().unwrap_or(Value::Null)
        ))
    }
}

fn done(start: Instant, result: Result<String, String>) -> TestResult {
    match result {
        Ok(message) => TestResult::passed_with_message(start.elapsed(), message),
        Err(error) => TestResult::failed(start.elapsed(), error),
    }
}

fn expect_status(path: &str, actual: u16, expected: u16) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{path}: expected {expected}, got {actual}"))
    }
}

pub struct DelegationTestSuite;

impl DelegationTestSuite {
    /// existing_billing: the child declares its parent; the delegated path reaches
    /// the child's backend with the parent's mandatory header, and the access log
    /// records the chain root -> leaf.
    fn test_delegated_path_routes_with_inherited_filter() -> TestCase {
        TestCase::new(
            "delegated_path_and_inherited_filter",
            "Delegated /api/v1/billing/pay reaches the child backend, carries the parent's header, logs root -> leaf",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        let path = "/api/v1/billing/pay";
                        let (status, body, entry) = traced_get(&ctx, HOST, path).await?;
                        expect_status(path, status, 200)?;
                        if !echoed(&body, "x-parent-filter", "on") {
                            return Err(format!(
                                "parent filter header missing on delegated traffic; body: {body}"
                            ));
                        }
                        if echoed(&body, "x-served-by", "platform-api") {
                            return Err("delegated request was served by the platform /api rule".to_string());
                        }
                        expect_chain(
                            &entry,
                            &[(PARENT_NS, "delegation-parent", 1), (CHILD_NS, "billing-child", 0)],
                        )?;
                        if entry.route_name() != Some("billing-child") {
                            return Err(format!("leaf route_name() = {:?}", entry.route_name()));
                        }
                        Ok(
                            "200 via child with inherited x-parent-filter; match_info [parent r1, child r0]"
                                .to_string(),
                        )
                    }
                    .await;
                    done(start, result)
                })
            },
        )
    }

    /// Child status: one Accepted=True / DelegationAccepted entry for its parentRef.
    fn test_child_status_accepted() -> TestCase {
        TestCase::new(
            "child_status_delegation_accepted",
            "billing-child has Accepted=True / DelegationAccepted for its HTTPRoute parentRef",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = status_helpers::wait_resource_status(
                        &ctx,
                        "HTTPRoute",
                        CHILD_NS,
                        "billing-child",
                        STATUS_TIMEOUT,
                        |route| {
                            status_helpers::parent_entry_has(
                                route,
                                PARENT_NS,
                                "delegation-parent",
                                "Accepted",
                                "True",
                                "DelegationAccepted",
                            )
                        },
                    )
                    .await
                    .map(|_| "Accepted=True / DelegationAccepted".to_string());
                    done(start, result)
                })
            },
        )
    }

    /// chain_filters_parent_first: the client sends `x-parent-filter: off`; the
    /// parent overwrites it with `on` before the child plugin (which denies `off`)
    /// runs. A sibling child rule whose plugin denies `on` returns 403, proving the
    /// child plugin is live and sees the parent's write.
    fn test_chain_filters_parent_first() -> TestCase {
        TestCase::new(
            "chain_filters_parent_first",
            "Parent header modifier runs before the child plugin and is visible upstream",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        let forged = [("x-parent-filter", "off")];
                        let path = "/api/v1/billing/pay";
                        let (status, body) = get_with_headers(&ctx, HOST, path, &forged)
                            .await
                            .map_err(|e| format!("request failed: {e}"))?;
                        expect_status(path, status, 200).map_err(|e| {
                            format!("{e}: the child plugin saw the client header, so it ran before the parent filter")
                        })?;
                        if !echoed(&body, "x-parent-filter", "on") || echoed(&body, "x-parent-filter", "off") {
                            return Err(format!("upstream did not see only the parent's value; body: {body}"));
                        }
                        let path = "/api/v1/billing/audit";
                        let (status, _) = get_with_headers(&ctx, HOST, path, &forged)
                            .await
                            .map_err(|e| format!("request failed: {e}"))?;
                        expect_status(path, status, 403)
                            .map_err(|e| format!("{e}: the child plugin did not observe the parent's header"))?;
                        Ok("parent filter first: forged header overwritten (200), child plugin sees parent value (403)"
                            .to_string())
                    }
                    .await;
                    done(start, result)
                })
            },
        )
    }

    /// A child rule disjoint from the parent scope (/evil) never matches.
    fn test_disjoint_child_rule_unreachable() -> TestCase {
        TestCase::new(
            "disjoint_child_rule_unreachable",
            "Child rule outside the parent's delegated prefix is not routable (404)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        let (status, _) = get_via_gateway(&ctx, "/evil")
                            .await
                            .map_err(|e| format!("request failed: {e}"))?;
                        expect_status("/evil", status, 404)?;
                        Ok("404".to_string())
                    }
                    .await;
                    done(start, result)
                })
            },
        )
    }

    /// backtrack_to_platform_rule: the child subtree misses, so the matcher
    /// continues with the parent's own /api rule.
    fn test_backtrack_to_platform_rule() -> TestCase {
        TestCase::new(
            "backtrack_to_platform_rule",
            "Child-subtree miss under /api/v1/billing backtracks to the parent's own /api rule",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        let path = "/api/v1/billing/other";
                        let (status, body, entry) = traced_get(&ctx, HOST, path).await?;
                        expect_status(path, status, 200)?;
                        if !echoed(&body, "x-served-by", "platform-api") {
                            return Err(format!("not served by the platform /api rule; body: {body}"));
                        }
                        if echoed(&body, "x-parent-filter", "on") {
                            return Err("backtracked request still carries the delegating rule's filter".to_string());
                        }
                        expect_chain(&entry, &[(PARENT_NS, "delegation-parent", 2)])?;
                        Ok("served by parent rule 2 with a one-element match_info".to_string())
                    }
                    .await;
                    done(start, result)
                })
            },
        )
    }

    /// The parent's non-delegating sibling rule keeps working and logs a
    /// one-element `match_info` array.
    fn test_plain_sibling_rule_intact() -> TestCase {
        TestCase::new(
            "plain_sibling_rule_intact",
            "Parent's plain /plain rule still routes normally",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        let path = "/plain/anything";
                        let (status, body, entry) = traced_get(&ctx, HOST, path).await?;
                        expect_status(path, status, 200)?;
                        // Filters are per rule: the plain rule must not carry the
                        // delegating rule's header.
                        if echoed(&body, "x-parent-filter", "on") {
                            return Err("plain rule unexpectedly carries the delegating rule's filter".to_string());
                        }
                        expect_chain(&entry, &[(PARENT_NS, "delegation-parent", 0)])?;
                        Ok("200 with a one-element match_info".to_string())
                    }
                    .await;
                    done(start, result)
                })
            },
        )
    }

    /// per_level_precedence: root B `PathPrefix /api/users` beats root A's child
    /// `Exact /api/users` because each level compares only its own rules.
    fn test_per_level_precedence() -> TestCase {
        TestCase::new(
            "per_level_precedence",
            "Sibling root PathPrefix /api/users wins over a delegated child's Exact /api/users",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        let path = "/api/users";
                        let (status, body, entry) = traced_get(&ctx, PRECEDENCE_HOST, path).await?;
                        expect_status(path, status, 200)?;
                        if !echoed(&body, "x-served-by", "root-b") {
                            return Err(format!("{path} was not served by root B; body: {body}"));
                        }
                        expect_chain(&entry, &[(PARENT_NS, "precedence-root-b", 0)])?;
                        let path = "/api/orders";
                        let (status, body, entry) = traced_get(&ctx, PRECEDENCE_HOST, path).await?;
                        expect_status(path, status, 200)?;
                        if !echoed(&body, "x-served-by", "precedence-child") {
                            return Err(format!("{path} was not served by the delegated child; body: {body}"));
                        }
                        expect_chain(
                            &entry,
                            &[(PARENT_NS, "precedence-root-a", 0), (CHILD_NS, "precedence-child", 0)],
                        )?;
                        Ok("/api/users -> root B; /api/orders -> root A -> child".to_string())
                    }
                    .await;
                    done(start, result)
                })
            },
        )
    }

    /// multi_parent_child: one child under /v1/billing and /v2/billing parents.
    fn test_multi_parent_child() -> TestCase {
        TestCase::new(
            "multi_parent_child",
            "One child is served through two parents, each applying only its own filter",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        for (version, other) in [("v1", "v2"), ("v2", "v1")] {
                            let path = format!("/{version}/billing/invoice");
                            let (status, body, entry) = traced_get(&ctx, MULTI_HOST, &path).await?;
                            expect_status(&path, status, 200)?;
                            if !echoed(&body, "x-parent", version) || echoed(&body, "x-parent", other) {
                                return Err(format!("{path}: expected only x-parent: {version}; body: {body}"));
                            }
                            let parent = format!("billing-{version}-parent");
                            expect_chain(&entry, &[(PARENT_NS, &parent, 0), (CHILD_NS, "multi-child", 0)])?;
                        }
                        // No backtrack target under the v2 parent: a subtree miss is 404.
                        let (status, _) = get_with_headers(&ctx, MULTI_HOST, "/v2/billing/other", &[])
                            .await
                            .map_err(|e| format!("request failed: {e}"))?;
                        expect_status("/v2/billing/other", status, 404)?;
                        status_helpers::wait_resource_status(
                            &ctx,
                            "HTTPRoute",
                            CHILD_NS,
                            "multi-child",
                            STATUS_TIMEOUT,
                            |route| {
                                ["billing-v1-parent", "billing-v2-parent"].iter().all(|parent| {
                                    status_helpers::parent_entry_has(
                                        route,
                                        PARENT_NS,
                                        parent,
                                        "Accepted",
                                        "True",
                                        "DelegationAccepted",
                                    )
                                })
                            },
                        )
                        .await
                        .map_err(|e| format!("multi-child status: {e}"))?;
                        Ok("v1 and v2 parents both serve the child; two Accepted status entries".to_string())
                    }
                    .await;
                    done(start, result)
                })
            },
        )
    }

    /// five_level_chain: the level-5 leaf is served with a five-element chain;
    /// the level-6 route is never served and reports DepthExceeded.
    fn test_five_level_chain() -> TestCase {
        TestCase::new(
            "five_level_chain",
            "Level-5 leaf is served; a level-6 route is not and reports DepthExceeded",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        let path = "/chain/l2/l3/l4/l5/leaf";
                        let (status, _, entry) = traced_get(&ctx, CHAIN_HOST, path).await?;
                        expect_status(path, status, 200)?;
                        expect_chain(
                            &entry,
                            &[
                                (PARENT_NS, "chain-l1", 0),
                                (CHILD_NS, "chain-l2", 0),
                                (CHILD_NS, "chain-l3", 0),
                                (CHILD_NS, "chain-l4", 0),
                                (CHILD_NS, "chain-l5", 0),
                            ],
                        )?;
                        let path = "/chain/l2/l3/l4/l6";
                        let (status, _) = get_with_headers(&ctx, CHAIN_HOST, path, &[])
                            .await
                            .map_err(|e| format!("request failed: {e}"))?;
                        expect_status(path, status, 404)?;
                        status_helpers::wait_resource_status(
                            &ctx,
                            "HTTPRoute",
                            CHILD_NS,
                            "chain-l5",
                            STATUS_TIMEOUT,
                            |route| {
                                status_helpers::parent_entry_has(
                                    route,
                                    CHILD_NS,
                                    "chain-l4",
                                    "Accepted",
                                    "True",
                                    "DelegationAccepted",
                                )
                            },
                        )
                        .await
                        .map_err(|e| format!("chain-l5 status: {e}"))?;
                        status_helpers::wait_resource_status(
                            &ctx,
                            "HTTPRoute",
                            CHILD_NS,
                            "chain-l6",
                            STATUS_TIMEOUT,
                            |route| {
                                status_helpers::parent_entry_has(
                                    route,
                                    CHILD_NS,
                                    "chain-l5",
                                    "Accepted",
                                    "False",
                                    "DepthExceeded",
                                )
                            },
                        )
                        .await
                        .map_err(|e| format!("chain-l6 status: {e}"))?;
                        Ok("five-level match_info root -> leaf; level 6 404 + DepthExceeded".to_string())
                    }
                    .await;
                    done(start, result)
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
            Self::test_child_status_accepted(),
            Self::test_chain_filters_parent_first(),
            Self::test_disjoint_child_rule_unreachable(),
            Self::test_backtrack_to_platform_rule(),
            Self::test_plain_sibling_rule_intact(),
            Self::test_per_level_precedence(),
            Self::test_multi_parent_child(),
            Self::test_five_level_chain(),
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
        vec![Self::pay_routes_pay2_missing(), delegated_regex_scope_case(false)]
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
        vec![Self::pay2_routes_pay_missing(), delegated_regex_scope_case(true)]
    }
}

fn delegated_regex_scope_case(updated: bool) -> TestCase {
    TestCase::new(
        "delegated_regex_parent_boundary",
        "Regex candidates stay inside the current parent prefix",
        move |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let suffix = if updated { "2" } else { "" };
                let old_suffix = if updated { "" } else { "2" };
                for (path, expected) in [
                    (format!("/api/v1/billing/guard{suffix}"), 200),
                    (format!("/api/v1/billingguard{suffix}"), 404),
                    (format!("/api/v1/billing/guard{old_suffix}"), 404),
                ] {
                    match dyn_status(&ctx, &path).await {
                        Ok(actual) if actual == expected => {}
                        result => {
                            return TestResult::failed(
                                Instant::now().saturating_duration_since(start),
                                format!("{path}: expected {expected}, got {result:?}"),
                            )
                        }
                    }
                }
                TestResult::passed(Instant::now().saturating_duration_since(start))
            })
        },
    )
}
