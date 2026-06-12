// RegionRoute Integration Test Suite
//
// Tests the ServiceRegionRoute PluginMetaData → ClusterRegionRoute PluginMetaData → EdgionPlugins resolve flow.
//
// Required conf files (edgion-tests/conf/EdgionPlugins/RegionRoute/):
//   01_EdgionPlugins_region-route-main.yaml      — ref-only: serviceRegionRouteRef
//   01_EdgionPlugins_region-route-norules.yaml    — ref-only: serviceRegionRouteRef
//   01_EdgionPlugins_region-route-myregion.yaml   — ref-only: serviceRegionRouteRef
//   01_PluginMetaData_global-region-conf.yaml     — ClusterRegionRoute PluginMetaData with complete config:
//                                                   regions, keyGet, hashKeyGet, hashCalc, routeRules,
//                                                   routeByKeyConfMatch.matchMap (vip-va→virginia, vip-oh→ohio)
//   01_PluginMetaData_region-route-main.yaml      — ServiceRegionRoute PluginMetaData with clusterRef + regions
//   02_HTTPRoute_region-route-main-test.yaml
//   02_HTTPRoute_region-route-norules-test.yaml
//   02_HTTPRoute_region-route-myregion-test.yaml
//
// PluginMetaData resolve flow:
//   1. Controller processes EdgionPlugins "region-route-main"
//   2. Resolves serviceRegionRouteRef → loads ServiceRegionRoute PluginMetaData "region-route-main"
//   3. Follows ServiceRegionRoute PluginMetaData's clusterRef → loads ClusterRegionRoute PluginMetaData "global-region-conf"
//   4. Copies ALL config from ClusterRegionRoute PluginMetaData: regions, keyGet, hashCalc, routeRules
//   5. Injects routeByKeyConfMatch.matchMap into RouteByKeyConfMatch route rules
//   6. Overlays per-region failoverTo from ServiceRegionRoute PluginMetaData (none in this test)
//   7. Gateway receives the fully merged config
//
// Test strategy:
//   Backend endpoints 127.0.0.1:30001 (virginia) and 127.0.0.1:30002 (ohio) are
//   loopback addresses, so Pingora rejects connections to them with 502.
//   Correct routing behaviour is verified through the access log (plugin_log field).
//
// Tests that expect 200 use routes that pass through to the normal backend (test-http:30001).
// Tests that expect 502 verify the plugin correctly set ExternalJumpPreset (access log check).

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const MAIN_HOST: &str = "region-route-main.example.com";
const NORULES_HOST: &str = "region-route-norules.example.com";
const MYREGION_HOST: &str = "region-route-myregion.example.com";
const BROKEN_REF_HOST: &str = "region-route-broken-ref.example.com";

pub struct RegionRouteTestSuite;

impl RegionRouteTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RegionRouteTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl RegionRouteTestSuite {
    // =========================================================================
    // Pass-through tests (expect 200 from normal backend)
    // =========================================================================

    /// No routeRules config → plugin is a no-op → normal backend responds 200
    fn no_route_rules_passes_through() -> TestCase {
        TestCase::new(
            "no_route_rules_passes_through",
            "No routeRules: plugin is no-op, normal backend returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", NORULES_HOST).send().await {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// serviceRegionRouteRef points at a PluginMetaData that does not resolve →
    /// region_route's resolve_config returns BrokenRef → data plane responds 500.
    fn broken_ref_returns_500() -> TestCase {
        TestCase::new(
            "broken_ref_returns_500",
            "Broken serviceRegionRouteRef: resolve_config returns BrokenRef, request gets 500",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", BROKEN_REF_HOST).send().await {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 500 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 500 (BrokenRef), got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// No keyGet value, no hashKeyGet value → plugin passes through → normal backend 200
    fn missing_key_passes_through() -> TestCase {
        TestCase::new(
            "missing_key_passes_through",
            "No keyGet/hashKeyGet value: RouteByHashRange passes through, normal backend 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MAIN_HOST)
                        // No X-Account-Id header, no account_id cookie, no x_region_hash
                        // No X-Direct-Region, no matchMap key
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 (pass-through), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // =========================================================================
    // Hash routing via access log (expect 502 + access log entry)
    // =========================================================================

    /// X-Account-Id header → plugin computes hash → routes to one of the two regions.
    /// The jump log (→virginia/ohio) is truncated by the 100-byte LogBuffer when
    /// match-miss(32B) + key(32B) + wb(28B) already fills 92 bytes.
    /// We verify via "key=hdr:X-Account-Id" (hash computed) and "wb→" (write-back ran,
    /// proving routing happened — write_back only runs when a hash is successfully computed).
    fn hash_routing_logged() -> TestCase {
        TestCase::new(
            "hash_routing_logged",
            "X-Account-Id header → hash computed and write-back triggered (access log)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-hash-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MAIN_HOST)
                        .header("X-Account-Id", "acct-001")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_resp) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    let has_hash = log_str.contains("key=hdr:X-Account-Id");
                                    let has_wb = log_str.contains("wb→res_hdr:X-Region-Hash");
                                    if has_hash && has_wb {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Access log missing expected entries. has_hash={}, has_wb={}. log: {}",
                                                has_hash, has_wb, log_str
                                            ),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Same account ID sent twice → same region both times (hash determinism)
    fn hash_routing_deterministic() -> TestCase {
        TestCase::new(
            "hash_routing_deterministic",
            "Same X-Account-Id always routes to same region (determinism)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let account_id = "acct-determinism-test";

                    let send_and_get_region = |ctx: TestContext, trace_id: String| async move {
                        let url = format!("{}/echo", ctx.edgion_plugins_url());
                        let _ = ctx
                            .http_client
                            .get(&url)
                            .header("host", MAIN_HOST)
                            .header("X-Account-Id", account_id)
                            .header("x-trace-id", &trace_id)
                            .header("access_log", "test_store")
                            .send()
                            .await;
                        let al_client = ctx.access_log_client();
                        al_client
                            .get_access_log_with_retry(&trace_id, 10, 200)
                            .await
                            .map(|e| e.data.to_string())
                    };

                    let trace1 = format!("rr-determ-1-{}", uuid::Uuid::new_v4());
                    let trace2 = format!("rr-determ-2-{}", uuid::Uuid::new_v4());

                    let log1 = send_and_get_region(ctx.clone(), trace1).await;
                    let log2 = send_and_get_region(ctx.clone(), trace2).await;

                    // Extract "hash=N" value from log entry like "key=hdr:X-Account-Id hash=N"
                    // The value ends at the first non-digit (quote/comma in JSON serialized form).
                    let extract_hash = |log: &str| -> Option<String> {
                        let start = log.find("hash=")? + 5;
                        let rest = &log[start..];
                        let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
                        if end == 0 {
                            return None;
                        }
                        Some(rest[..end].to_string())
                    };

                    match (log1, log2) {
                        (Ok(l1), Ok(l2)) => {
                            let h1 = extract_hash(&l1);
                            let h2 = extract_hash(&l2);
                            match (h1, h2) {
                                (Some(v1), Some(v2)) if v1 == v2 => TestResult::passed(start.elapsed()),
                                (Some(v1), Some(v2)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Hash not deterministic: {} vs {} (different routes)", v1, v2),
                                ),
                                _ => TestResult::failed(
                                    start.elapsed(),
                                    format!("Could not extract hash from logs: l1={} l2={}", l1, l2),
                                ),
                            }
                        }
                        (Err(e), _) | (_, Err(e)) => {
                            TestResult::failed(start.elapsed(), format!("Access log error: {}", e))
                        }
                    }
                })
            },
        )
    }

    // =========================================================================
    // RouteByDirectRegion override (expect 502 + access log entry)
    // =========================================================================

    /// X-Direct-Region: virginia → access log shows "direct=virginia" and "→virginia"
    fn direct_region_virginia() -> TestCase {
        TestCase::new(
            "direct_region_virginia",
            "X-Direct-Region: virginia → access log shows direct routing to virginia",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-direct-va-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MAIN_HOST)
                        .header("X-Direct-Region", "virginia")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains("direct=virginia") && log_str.contains("→virginia") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected 'direct=virginia' and '→virginia' in log: {}", log_str),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// X-Direct-Region: ohio → access log shows "direct=ohio" and "→ohio"
    fn direct_region_ohio() -> TestCase {
        TestCase::new(
            "direct_region_ohio",
            "X-Direct-Region: ohio → access log shows direct routing to ohio",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-direct-oh-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MAIN_HOST)
                        .header("X-Direct-Region", "ohio")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains("direct=ohio") && log_str.contains("→ohio") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected 'direct=ohio' and '→ohio' in log: {}", log_str),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// X-Direct-Region: invalid → unknown region, falls through → RouteByKeyConfMatch misses
    /// → RouteByHashRange computes hash from X-Account-Id
    fn direct_region_unknown_falls_through() -> TestCase {
        TestCase::new(
            "direct_region_unknown_falls_through",
            "X-Direct-Region with unknown region name falls through to subsequent rules",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-direct-unk-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MAIN_HOST)
                        .header("X-Direct-Region", "invalid")
                        .header("X-Account-Id", "acct-fallthrough")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    // Verify: unknown direct region falls through, RouteByKeyConfMatch
                                    // misses, then RouteByHashRange routes.
                                    // LogBuffer is 100 bytes. After fallthrough(38B) + match-miss(31B)
                                    // + key(31B) = 100B exactly, the wb→ entry (26B) doesn't fit.
                                    // So we verify hash computation via "key=hdr:X-Account-Id hash="
                                    // rather than "wb→res_hdr:X-Region-Hash" which is truncated.
                                    let fell_through = log_str.contains("fallthrough");
                                    let match_miss = log_str.contains("match=hdr:X-Account-Id no conf");
                                    let hash_computed = log_str.contains("key=hdr:X-Account-Id hash=");
                                    if fell_through && match_miss && hash_computed {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected fallthrough+match-miss+hash. fell_through={}, match_miss={}, hash_computed={}. log: {}",
                                                fell_through, match_miss, hash_computed, log_str
                                            ),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // =========================================================================
    // RouteByKeyConfMatch (expect 502 + access log entry)
    // =========================================================================

    /// X-Account-Id: vip-va → matchMap hit → virginia
    fn key_conf_match_virginia() -> TestCase {
        TestCase::new(
            "key_conf_match_virginia",
            "X-Account-Id vip-va → matchMap routes to virginia",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-match-va-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MAIN_HOST)
                        .header("X-Account-Id", "vip-va")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    let has_match = log_str.contains("match=hdr:X-Account-Id→virginia");
                                    let has_jump = log_str.contains("→virginia");
                                    if has_match && has_jump {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected match→virginia and →virginia. has_match={}, has_jump={}. log: {}",
                                                has_match, has_jump, log_str
                                            ),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// X-Account-Id: vip-oh → matchMap hit → ohio
    fn key_conf_match_ohio() -> TestCase {
        TestCase::new(
            "key_conf_match_ohio",
            "X-Account-Id vip-oh → matchMap routes to ohio",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-match-oh-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MAIN_HOST)
                        .header("X-Account-Id", "vip-oh")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    let has_match = log_str.contains("match=hdr:X-Account-Id→ohio");
                                    let has_jump = log_str.contains("→ohio");
                                    if has_match && has_jump {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected match→ohio and →ohio. has_match={}, has_jump={}. log: {}",
                                                has_match, has_jump, log_str
                                            ),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// X-Account-Id value not in matchMap → "no conf" → falls to RouteByHashRange
    fn key_conf_match_miss_falls_to_hash() -> TestCase {
        TestCase::new(
            "key_conf_match_miss_falls_to_hash",
            "X-Account-Id not in matchMap → no conf, falls through to RouteByHashRange",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-match-miss-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MAIN_HOST)
                        .header("X-Account-Id", "not-in-map")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    let has_no_conf = log_str.contains("match=hdr:X-Account-Id no conf");
                                    let has_hash = log_str.contains("key=hdr:X-Account-Id hash=");
                                    if has_no_conf && has_hash {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected no-conf then hash. has_no_conf={}, has_hash={}. log: {}",
                                                has_no_conf, has_hash, log_str
                                            ),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // =========================================================================
    // hashKeyGet passthrough (expect 502 + access log entry)
    // =========================================================================

    /// Cookie x_region_hash=100 → falls in virginia [0,600] → access log "hash_pass=100"
    fn hash_passthrough_virginia() -> TestCase {
        TestCase::new(
            "hash_passthrough_virginia",
            "Pre-computed hash cookie 100 → virginia range [0,600], access log shows hash_pass",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-pass-va-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MAIN_HOST)
                        // x_region_hash=100 → falls in [0,600] → virginia
                        .header("Cookie", "x_region_hash=100")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains("hash_pass=100") && log_str.contains("→virginia") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected 'hash_pass=100' and '→virginia': {}", log_str),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Cookie x_region_hash=800 → falls in ohio [601,1000] → access log "hash_pass=800"
    fn hash_passthrough_ohio() -> TestCase {
        TestCase::new(
            "hash_passthrough_ohio",
            "Pre-computed hash cookie 800 → ohio range [601,1000], access log shows hash_pass",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-pass-oh-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MAIN_HOST)
                        // x_region_hash=800 → falls in [601,1000] → ohio
                        .header("Cookie", "x_region_hash=800")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains("hash_pass=800") && log_str.contains("→ohio") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected 'hash_pass=800' and '→ohio': {}", log_str),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// x_region_hash=1001 (>= modulo=1001) is out of range → ignored → falls back to hashCalc
    fn hash_passthrough_out_of_range_falls_to_compute() -> TestCase {
        TestCase::new(
            "hash_passthrough_out_of_range_falls_to_compute",
            "hash passthrough >= modulo is out-of-range, falls back to hashCalc computation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-pass-oor-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MAIN_HOST)
                        .header("Cookie", "x_region_hash=1001") // invalid: >= modulo → ignored
                        .header("X-Account-Id", "acct-oor")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    // Should NOT use passthrough; should compute hash from X-Account-Id
                                    let used_passthrough = log_str.contains("hash_pass=1001");
                                    let used_key = log_str.contains("key=hdr:X-Account-Id");
                                    if !used_passthrough && used_key {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected fallback to hashCalc. used_passthrough={}, used_key={}. log: {}",
                                                used_passthrough, used_key, log_str
                                            ),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // =========================================================================
    // myRegion + failoverTo tests (MYREGION_HOST)
    // =========================================================================

    /// X-Direct-Region: virginia → myRegion match → local pass-through → 200
    fn my_region_direct_local() -> TestCase {
        TestCase::new(
            "my_region_direct_local",
            "X-Direct-Region: virginia (=myRegion) → local pass-through, backend returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MYREGION_HOST)
                        .header("X-Direct-Region", "virginia")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 (local pass-through), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// X-Direct-Region: ohio (failoverTo=virginia=myRegion) → redirect ohio→virginia → local pass → 200
    fn my_region_failover_to_local() -> TestCase {
        TestCase::new(
            "my_region_failover_to_local",
            "X-Direct-Region: ohio (failoverTo=virginia=myRegion) → redirect→local pass, 200 + log check",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-mr-ftlocal-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MYREGION_HOST)
                        .header("X-Direct-Region", "ohio")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    let has_redirect = log_str.contains("redirect ohio→virginia");
                                    let has_local = log_str.contains("local, pass");
                                    if has_redirect && has_local {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected 'redirect ohio→virginia' and 'local, pass'. has_redirect={}, has_local={}. log: {}",
                                                has_redirect, has_local, log_str
                                            ),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// X-Direct-Region: europe (failoverTo=ohio, ohio≠myRegion) → redirect europe→ohio → jump → 502
    fn my_region_failover_to_nonlocal() -> TestCase {
        TestCase::new(
            "my_region_failover_to_nonlocal",
            "X-Direct-Region: europe (failoverTo=ohio≠myRegion) → redirect→jump to loopback → 502 + log check",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-mr-ftnl-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MYREGION_HOST)
                        .header("X-Direct-Region", "europe")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            // Pingora returns 502 or 500 for loopback connection errors
                            let status = resp.status().as_u16();
                            if status != 502 && status != 500 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 502/500 (jump to loopback), got {}", status),
                                );
                            }
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    let has_redirect = log_str.contains("redirect europe→ohio");
                                    let has_jump = log_str.contains("→ohio");
                                    if has_redirect && has_jump {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected 'redirect europe→ohio' and '→ohio'. has_redirect={}, has_jump={}. log: {}",
                                                has_redirect, has_jump, log_str
                                            ),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // =========================================================================
    // hashCalc writeBack (check response header)
    // =========================================================================

    /// X-Account-Id: acct-wb → hash computed → write-back logged in access log.
    /// Note: 502 (loopback backend) responses don't carry RequestFilter-phase response headers,
    /// so we verify write-back via access log entry "wb→res_hdr:X-Region-Hash".
    fn hash_writeback_logged() -> TestCase {
        TestCase::new(
            "hash_writeback_logged",
            "hashCalc.writeBack: write-back target logged in access log (wb→res_hdr:X-Region-Hash)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-wb-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", MAIN_HOST)
                        .header("X-Account-Id", "acct-wb")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    // Plugin logs "wb→res_hdr:X-Region-Hash; " after computing hash
                                    if log_str.contains("wb→res_hdr:X-Region-Hash") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected 'wb→res_hdr:X-Region-Hash' in log: {}", log_str),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Access log error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for RegionRouteTestSuite {
    fn name(&self) -> &str {
        "RegionRoute"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Pass-through (200)
            Self::no_route_rules_passes_through(),
            Self::missing_key_passes_through(),
            // Broken ref (500)
            Self::broken_ref_returns_500(),
            // Hash routing (502 + access log)
            Self::hash_routing_logged(),
            Self::hash_routing_deterministic(),
            // RouteByDirectRegion override (502 + access log)
            Self::direct_region_virginia(),
            Self::direct_region_ohio(),
            Self::direct_region_unknown_falls_through(),
            // RouteByKeyConfMatch (502 + access log)
            Self::key_conf_match_virginia(),
            Self::key_conf_match_ohio(),
            Self::key_conf_match_miss_falls_to_hash(),
            // hashKeyGet passthrough (502 + access log)
            Self::hash_passthrough_virginia(),
            Self::hash_passthrough_ohio(),
            Self::hash_passthrough_out_of_range_falls_to_compute(),
            // hashCalc writeBack (response header)
            Self::hash_writeback_logged(),
            // myRegion + failoverTo (MYREGION_HOST)
            Self::my_region_direct_local(),
            Self::my_region_failover_to_local(),
            Self::my_region_failover_to_nonlocal(),
        ]
    }
}
