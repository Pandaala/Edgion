// AccessLogExtern Integration Test Suite
//
// `AccessLogExtern` is a declaration, not a plugin filter: it never executes in
// a stage. It is resolved once, at `logging()`, and projects operator-named
// fields into a nested `ext` block on the access log entry. See
// `docs/superpowers/specs/2026-07-16-access-log-extern-plugin-design.md`.
//
// The first four cases below are the reason this design exists: they pass ONLY
// because collection moved to `logging()`. Under a request-stage filter, all
// four would fail — see each case's doc comment for why.
//
// This suite brings its own private Gateway chain (EdgionGatewayConfig ->
// GatewayClass -> Gateway) rather than overlaying the shared, cluster-scoped
// "example-gateway" EdgionGatewayConfig or the shared "edgion-plugins-gateway"
// Gateway: EdgionGatewayConfig is cluster-scoped, and a full
// `run_integration.sh` run loads every conf directory in one process, so a
// second same-named overlay would silently clobber whichever one applies
// first (same failure class as HTTPRoute/Backend/DnsResolution's dns-res-egc
// fix). All requests in this suite go through port 31395 (suite key
// "EdgionPlugins/AccessLogExtern" in ports.json), not the shared 31180.
//
// Required config files (in
// edgion-tests/integration/conf/EdgionPlugins/AccessLogExtern/):
//   00_EdgionGatewayConfig_als-egc.yaml    (private EGC, Tier 1: globalPluginsRef)
//   00_GatewayClass_als-class.yaml         (private GatewayClass -> als-egc)
//   02_Gateway_edgion-test_als-gateway.yaml (private Gateway, port 31395, Tier 2 annotation)
//   03_EdgionPlugins_als-global.yaml        (Tier 1 declarations)
//   04_EdgionPlugins_als-gateway-tier.yaml   (Tier 2 declarations)
//   05-22_*                                  (per-case Tier 3 route fixtures)
//
// Case 6 (`whitelist_hit_and_miss`) is intentionally NOT implemented here — see
// the module-level comment below.
//
// `whitelist_hit_and_miss` needs `access_log_sensitive.mode: replace` on the
// gateway, but the integration baseline pins `mode: off` in
// `config/edgion-gateway.yaml` / `edgion-tests/k8s/kubernetes/gateway/configmap.yaml`
// precisely so every other suite can assert real (unmasked) values. That file is
// loaded once at gateway startup for the whole `run_integration.sh` invocation —
// there is no per-suite / per-scenario override of gateway-local config in this
// harness (unlike K8s resources, which are just conf-sync'd per suite). Writing
// a "whitelist miss is masked" assertion against a `mode: off` gateway would
// silently assert nothing true (under `off`, EVERY value — whitelisted or not —
// renders in clear; see `apply_sensitive`'s mode-0 branch). The whitelist
// decision itself is already covered by the Task 3/5 unit tests:
// `edgion-gateway/src/observe/access_log/log_ext.rs::masking_decision_table` and
// `edgion-resources/src/resources/edgion_gateway_config.rs::access_log_extern_policy_tests`.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use reqwest::Method;
use std::time::Instant;

pub struct AccessLogExternTestSuite;

// ── helpers ──────────────────────────────────────────────────────────────

fn gen_trace_id(case: &str) -> String {
    format!("test-als-{}-{}", case, uuid::Uuid::new_v4())
}

/// Read a string field out of the entry's `ext` block.
fn ext_get<'a>(entry: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    entry.get("ext").and_then(|e| e.get(key)).and_then(|v| v.as_str())
}

/// Whether `key` is present at all in the entry's `ext` block (any type).
fn ext_has(entry: &serde_json::Value, key: &str) -> bool {
    entry.get("ext").and_then(|e| e.get(key)).is_some()
}

/// Fetch the stored access log entry for `trace_id`, retrying briefly.
async fn fetch_entry(ctx: &TestContext, trace_id: &str) -> Result<serde_json::Value, String> {
    ctx.access_log_client()
        .get_access_log_with_retry(trace_id, 10, 200)
        .await
        .map(|e| e.data)
        .map_err(|e| format!("Failed to fetch access log: {}", e))
}

impl AccessLogExternTestSuite {
    // ==================== 1. ext_present_on_404 ====================
    //
    // Requesting an unmatched host never reaches route matching successfully:
    // `ctx.route_unit` and `ctx.gateway_info` are both left at their defaults
    // (see pg_request_filter.rs — `ctx.gateway_info` is only assigned on the
    // `Ok(result)` branch of `match_route`). So only the GLOBAL tier
    // (Tier 1, GatewayClass-wide `globalPluginsRef`) can possibly resolve here
    // — the Gateway-annotation tier (Tier 2) reads `ctx.gateway_info`, which is
    // empty on a 404, and there is no route for the Tier 3 declarations to
    // live on at all. Under a request-stage filter, a 404 short-circuits before
    // any stage runs and NOTHING would be collected.
    fn test_ext_present_on_404() -> TestCase {
        TestCase::new(
            "ext_present_on_404",
            "AccessLogExtern: the global tier's field is in `ext` on a 404 (unmatched host)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("404");
                    let url = format!("{}/anything", ctx.http_url());

                    let resp = ctx
                        .http_client
                        .get(&url)
                        .header("host", "accesslogextern-404.example.com")
                        .header("x-tenant", "acme-global-404")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let resp = match resp {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    if resp.status().as_u16() != 404 {
                        return TestResult::failed(start.elapsed(), format!("Expected 404, got {}", resp.status()));
                    }

                    let entry = match fetch_entry(&ctx, &trace_id).await {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    match ext_get(&entry, "global_tenant") {
                        Some("acme-global-404") => TestResult::passed_with_message(
                            start.elapsed(),
                            "global tier field present on 404 entry".to_string(),
                        ),
                        other => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected ext.global_tenant='acme-global-404', got {:?}. Entry: {}",
                                other, entry
                            ),
                        ),
                    }
                })
            },
        )
    }

    // ==================== 2. ext_present_when_deny_terminates ====================
    //
    // KeyAuth with no key source found (no `X-API-Key` header sent) denies with
    // 401 (`ErrTerminateRequest`) inside `run_request_plugins`, which `break`s
    // the plugin loop for the route. But route matching already ran before Step
    // 2, so `ctx.route_unit` is populated and the route-tier declaration below
    // still resolves at logging(). Under a request-stage filter, the Deny would
    // have prevented that filter from ever running, and the field would be
    // missing.
    //
    // Note: the plugin brief describes this status as 403; KeyAuth's actual
    // deny path (`unauthorized()` in key_auth/plugin.rs) sends 401. This test
    // asserts the real behavior.
    fn test_ext_present_when_deny_terminates() -> TestCase {
        TestCase::new(
            "ext_present_when_deny_terminates",
            "AccessLogExtern: ext still carries route fields when an earlier plugin denies (401)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("deny");
                    let url = format!("{}/anything", ctx.http_url());

                    let resp = ctx
                        .http_client
                        .get(&url)
                        .header("host", "accesslogextern-deny.example.com")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let resp = match resp {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    if resp.status().as_u16() != 401 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 401 (KeyAuth deny, no key sent), got {}", resp.status()),
                        );
                    }

                    let entry = match fetch_entry(&ctx, &trace_id).await {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    match ext_get(&entry, "deny_field") {
                        Some("deny-team") => TestResult::passed_with_message(
                            start.elapsed(),
                            "route field present in ext despite an earlier Deny terminating the request".to_string(),
                        ),
                        other => TestResult::failed(
                            start.elapsed(),
                            format!("Expected ext.deny_field='deny-team', got {:?}. Entry: {}", other, entry),
                        ),
                    }
                })
            },
        )
    }

    // ==================== 3. ext_present_on_cors_preflight ====================
    //
    // An OPTIONS preflight (Origin + Access-Control-Request-Method) is handled
    // by the global preflight handler BEFORE Step 2 plugin execution
    // (pg_request_filter.rs Step 1.5), short-circuiting with 204 without ever
    // running the route's requestPlugins. Route matching runs before the
    // preflight check, though, so `ctx.route_unit` is set and the route-tier
    // declaration resolves at logging(). Under a request-stage filter, nothing
    // would have executed and the field would be missing.
    fn test_ext_present_on_cors_preflight() -> TestCase {
        TestCase::new(
            "ext_present_on_cors_preflight",
            "AccessLogExtern: ext is present on a short-circuited CORS preflight (204)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("cors");
                    let url = format!("{}/test", ctx.http_url());

                    let resp = ctx
                        .http_client
                        .request(Method::OPTIONS, &url)
                        .header("host", "accesslogextern-cors.example.com")
                        .header("origin", "https://preflight-client.example.com")
                        .header("access-control-request-method", "GET")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let resp = match resp {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    if resp.status().as_u16() != 204 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 204 (preflight), got {}", resp.status()),
                        );
                    }

                    let entry = match fetch_entry(&ctx, &trace_id).await {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    match ext_get(&entry, "preflight_field") {
                        Some("gold") => TestResult::passed_with_message(
                            start.elapsed(),
                            "route field present in ext on the short-circuited preflight".to_string(),
                        ),
                        other => TestResult::failed(
                            start.elapsed(),
                            format!("Expected ext.preflight_field='gold', got {:?}. Entry: {}", other, entry),
                        ),
                    }
                })
            },
        )
    }

    // ==================== 4. global_ctx_reads_route_stage_ctxset ====================
    //
    // The GLOBAL tier declares `from: ctx name: tenant_id` (see
    // 03_EdgionPlugins_als-global.yaml); a ROUTE-stage CtxSet plugin writes that
    // ctx var during request processing (09_EdgionPlugins_route-ctxset.yaml).
    // collect() evaluates the global tier FIRST in source order, but collection
    // itself only happens once, at logging() — after every stage has already
    // run — so it sees the final ctx state regardless of which tier declared
    // the field. Under a request-stage filter, the global tier's filter would
    // run before route-stage plugins and would read `tenant_id` before CtxSet
    // ever wrote it.
    fn test_global_ctx_reads_route_stage_ctxset() -> TestCase {
        TestCase::new(
            "global_ctx_reads_route_stage_ctxset",
            "AccessLogExtern: global tier `from: ctx` reads a var written by a route-stage CtxSet",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("globalctx");
                    let url = format!("{}/ping", ctx.http_url());

                    let resp = ctx
                        .http_client
                        .get(&url)
                        .header("host", "accesslogextern-globalctx.example.com")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let resp = match resp {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    if resp.status().as_u16() != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", resp.status()));
                    }

                    let entry = match fetch_entry(&ctx, &trace_id).await {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    match ext_get(&entry, "tenant") {
                        Some("acme-tenant-42") => TestResult::passed_with_message(
                            start.elapsed(),
                            "global tier correctly read the route-stage ctx var".to_string(),
                        ),
                        other => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected ext.tenant='acme-tenant-42', got {:?}. Entry: {}",
                                other, entry
                            ),
                        ),
                    }
                })
            },
        )
    }

    // ==================== 5. sources_resolve ====================
    //
    // One accessLogExtern field per `from` source, all declared on a single
    // route (11_EdgionPlugins_sources.yaml). Exercises every branch of
    // `resolve_field` in one request.
    fn test_sources_resolve() -> TestCase {
        TestCase::new(
            "sources_resolve",
            "AccessLogExtern: every `from` source resolves to the expected value",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("sources");
                    let url = format!("{}/probe?srcq=query-value", ctx.http_url());

                    let resp = ctx
                        .http_client
                        .get(&url)
                        .header("host", "accesslogextern-sources.example.com")
                        .header("x-src-header", "header-value")
                        .header("cookie", "srccookie=cookie-value")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let resp = match resp {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    if resp.status().as_u16() != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", resp.status()));
                    }

                    let entry = match fetch_entry(&ctx, &trace_id).await {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    let expected: &[(&str, &str)] = &[
                        ("src_label", "label-value"),
                        ("src_annotation", "annotation-value"),
                        ("src_header", "header-value"),
                        ("src_query", "query-value"),
                        ("src_cookie", "cookie-value"),
                        ("src_resp_header", "resp-value"),
                        ("src_ctx", "ctx-value"),
                    ];

                    let mut mismatches = Vec::new();
                    for (key, want) in expected {
                        match ext_get(&entry, key) {
                            Some(got) if got == *want => {}
                            other => mismatches.push(format!("{}: expected {:?}, got {:?}", key, want, other)),
                        }
                    }

                    if mismatches.is_empty() {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "all 7 sources resolved to their expected values".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Mismatches: {}. Entry: {}", mismatches.join("; "), entry),
                        )
                    }
                })
            },
        )
    }

    // ==================== 5b. query_source_reads_entry_not_rewritten_uri ====================
    //
    // `from: query` must resolve against the entry-time raw query
    // (`ctx.entry_query()`), never a post-rewrite re-parse of the
    // live session query — see `resolve_field`'s `LogExtSource::Query` arm and
    // `ctx.entry_query_param`'s doc comment. ProxyRewrite injects a brand-new query
    // param (`rewritten_only`) that the client never sent; it appears only in
    // the upstream-bound URI. If the source were ever changed to re-parse the
    // post-rewrite query, this field would incorrectly appear.
    fn test_query_source_reads_entry_not_rewritten_uri() -> TestCase {
        TestCase::new(
            "query_source_reads_entry_not_rewritten_uri",
            "AccessLogExtern: `from: query` reads the entry-time query, not the post-rewrite URI",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("queryrewrite");
                    let url = format!("{}/somepath?orig_param=entry-value", ctx.http_url());

                    let resp = ctx
                        .http_client
                        .get(&url)
                        .header("host", "accesslogextern-queryrewrite.example.com")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let resp = match resp {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    if resp.status().as_u16() != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", resp.status()));
                    }

                    let entry = match fetch_entry(&ctx, &trace_id).await {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    if ext_get(&entry, "src_orig_q") != Some("entry-value") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected ext.src_orig_q='entry-value', got {:?}. Entry: {}",
                                ext_get(&entry, "src_orig_q"),
                                entry
                            ),
                        );
                    }
                    if ext_has(&entry, "src_rewritten_q") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "ext.src_rewritten_q must be absent (the client never sent `rewritten_only`; it only \
                                 exists in the post-rewrite upstream URI) — regression: query source is reading \
                                 post-rewrite state. Entry: {}",
                                entry
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "query source resolved the entry-time value and ignored the rewrite-injected param".to_string(),
                    )
                })
            },
        )
    }

    // ==================== 7. resp_header_absent_on_upstream_timeout ====================
    //
    // The route's rule sets a 1s `timeouts.request`; /delay/2 exceeds it, so the
    // gateway returns 504 before the backend's response (and the
    // upstream-response-stage ResponseHeaderModifier) is ever seen.
    fn test_resp_header_absent_on_upstream_timeout() -> TestCase {
        TestCase::new(
            "resp_header_absent_on_upstream_timeout",
            "AccessLogExtern: respHeader is absent on an upstream timeout; request-side fields survive",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("timeout");
                    let url = format!("{}/delay/2", ctx.http_url());

                    let resp = ctx
                        .http_client
                        .get(&url)
                        .header("host", "accesslogextern-timeout.example.com")
                        .header("x-req-marker", "req-value")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let resp = match resp {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    if resp.status().as_u16() != 504 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 504 (1s timeout vs 2s delay), got {}", resp.status()),
                        );
                    }

                    let entry = match fetch_entry(&ctx, &trace_id).await {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    if ext_get(&entry, "req_field") != Some("req-value") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected ext.req_field='req-value' (request-side, must survive), got {:?}. Entry: {}",
                                ext_get(&entry, "req_field"),
                                entry
                            ),
                        );
                    }
                    if ext_has(&entry, "resp_field") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "ext.resp_field must be absent — the backend's response was never seen. Entry: {}",
                                entry
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "request-side field survived the timeout; respHeader field correctly absent".to_string(),
                    )
                })
            },
        )
    }

    // ==================== 8. three_tier_collision_route_wins ====================
    //
    // The same `key` ("collide") is declared at all three tiers, each sourced
    // from a distinct header so the three values are distinguishable.
    // collect() applies tiers in order global -> gateway -> route, and
    // `LogExt::insert` overwrites an existing key in place — the route tier
    // must win. This is the only remaining coverage that swapping the
    // collector's tier loops would break (the prior unit test on this was
    // deleted as vacuous — it exercised `LogExt::insert` directly, not the
    // tier ordering in `collect()`).
    fn test_three_tier_collision_route_wins() -> TestCase {
        TestCase::new(
            "three_tier_collision_route_wins",
            "AccessLogExtern: with the same key declared at all 3 tiers, the route tier wins",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("collision");
                    let url = format!("{}/x", ctx.http_url());

                    let resp = ctx
                        .http_client
                        .get(&url)
                        .header("host", "accesslogextern-collision.example.com")
                        .header("x-global-val", "global-value")
                        .header("x-gateway-val", "gateway-value")
                        .header("x-route-val", "route-value")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let resp = match resp {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    if resp.status().as_u16() != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", resp.status()));
                    }

                    let entry = match fetch_entry(&ctx, &trace_id).await {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    match ext_get(&entry, "collide") {
                        Some("route-value") => TestResult::passed_with_message(
                            start.elapsed(),
                            "route tier's value won the 3-way collision".to_string(),
                        ),
                        other => TestResult::failed(
                            start.elapsed(),
                            format!("Expected ext.collide='route-value', got {:?}. Entry: {}", other, entry),
                        ),
                    }
                })
            },
        )
    }

    // ==================== 9. value_truncated_at_cap ====================
    fn test_value_truncated_at_cap() -> TestCase {
        TestCase::new(
            "value_truncated_at_cap",
            "AccessLogExtern: a 400-byte header value is truncated to 256 bytes in ext",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("truncate");
                    let url = format!("{}/x", ctx.http_url());
                    let big_value = "A".repeat(400);

                    let resp = ctx
                        .http_client
                        .get(&url)
                        .header("host", "accesslogextern-truncate.example.com")
                        .header("x-big-header", &big_value)
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let resp = match resp {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    if resp.status().as_u16() != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", resp.status()));
                    }

                    let entry = match fetch_entry(&ctx, &trace_id).await {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };

                    match ext_get(&entry, "big_field") {
                        Some(v) if v.len() == 256 && v == "A".repeat(256) => TestResult::passed_with_message(
                            start.elapsed(),
                            "400-byte value correctly truncated to 256 bytes".to_string(),
                        ),
                        Some(v) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected a 256-byte value of 'A's, got {} bytes: {:?}", v.len(), v),
                        ),
                        None => TestResult::failed(
                            start.elapsed(),
                            format!("ext.big_field missing entirely. Entry: {}", entry),
                        ),
                    }
                })
            },
        )
    }

    // ==================== 10. attacker_header_cannot_forge_fields ====================
    fn test_attacker_header_cannot_forge_fields() -> TestCase {
        TestCase::new(
            "attacker_header_cannot_forge_fields",
            "AccessLogExtern: an attacker-controlled header value cannot break out of its JSON string",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("forge");
                    let url = format!("{}/x", ctx.http_url());
                    let payload = r#"a","status":999,"x":"b"#;

                    let resp = ctx
                        .http_client
                        .get(&url)
                        .header("host", "accesslogextern-forge.example.com")
                        .header("user-agent", payload)
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let resp = match resp {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    if resp.status().as_u16() != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", resp.status()));
                    }

                    // `entry` is only reachable at all if `.json()` inside fetch_entry
                    // already parsed the response as valid JSON — a forged/broken
                    // entry would have failed to parse there.
                    let entry = match fetch_entry(&ctx, &trace_id).await {
                        Ok(e) => e,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Entry did not parse as valid JSON (log injection?): {}", e),
                            )
                        }
                    };

                    if entry.get("status").is_some() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Forged top-level 'status' field found. Entry: {}", entry),
                        );
                    }

                    let real_status = entry
                        .get("request_info")
                        .and_then(|ri| ri.get("status"))
                        .and_then(|s| s.as_u64());
                    if real_status != Some(200) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected request_info.status=200 (real status), got {:?}. Entry: {}",
                                real_status, entry
                            ),
                        );
                    }

                    if ext_get(&entry, "ua_field") != Some(payload) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected ext.ua_field={:?} (faithfully escaped), got {:?}. Entry: {}",
                                payload,
                                ext_get(&entry, "ua_field"),
                                entry
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "attacker-controlled value round-tripped safely; no fields forged".to_string(),
                    )
                })
            },
        )
    }
}

impl TestSuite for AccessLogExternTestSuite {
    fn name(&self) -> &str {
        "AccessLogExtern Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_ext_present_on_404(),
            Self::test_ext_present_when_deny_terminates(),
            Self::test_ext_present_on_cors_preflight(),
            Self::test_global_ctx_reads_route_stage_ctxset(),
            Self::test_sources_resolve(),
            Self::test_query_source_reads_entry_not_rewritten_uri(),
            Self::test_resp_header_absent_on_upstream_timeout(),
            Self::test_three_tier_collision_route_wins(),
            Self::test_value_truncated_at_cap(),
            Self::test_attacker_header_cannot_forge_fields(),
        ]
    }
}
