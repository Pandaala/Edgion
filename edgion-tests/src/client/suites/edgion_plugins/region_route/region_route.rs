// RegionRoute Integration Test Suite
//
// EdgionPlugins owns request extraction and routing rules. Its required clusterRegionRef
// selects the ClusterRegionSetting local identity and region table; unusable settings
// default to Continue without changing the request.
//
// Required conf files (edgion-tests/integration/conf/EdgionPlugins/RegionRoute/):
//   01_EdgionPlugins_region-route-main.yaml      — plugin config: clusterRegionRef, keyGet,
//                                                   hashKeyGet, hashCalc, routeRules,
//                                                   routeByKeyConfMatch (vip-va/vip-oh)
//   01_EdgionPlugins_region-route-norules.yaml    — inline config with no routeRules (no-op)
//   01_EdgionPlugins_region-route-myregion.yaml   — setting reference with local regionName and one-hop failover
//   01_EdgionPlugins_region-route-broken-ref.yaml — clusterRegionRef to nonexistent
//                                                   EdgionConfigData (setting_missing -> Continue)
//   02_HTTPRoute_region-route-main-test.yaml
//   02_HTTPRoute_region-route-norules-test.yaml
//   02_HTTPRoute_region-route-myregion-test.yaml
//   02_HTTPRoute_region-route-broken-ref-test.yaml
//
// Test strategy:
//   Backend endpoints 127.0.0.1:30001 (virginia) and 127.0.0.1:30002 (ohio) are
//   loopback addresses, so Pingora rejects connections to them with 502.
//   Correct routing behaviour is verified through the access log (plugin_log field).
//
// Tests that expect 200 use routes that pass through to the normal backend (test-http:30001).
// Tests that expect 502 verify the plugin correctly set ExternalJumpPreset (access log check).

use crate::access_log_client::AccessLogEntry;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use edgion_gateway::plugins::http::region_route::log_consts as log_t;
use std::time::Instant;

const MAIN_HOST: &str = "region-route-main.example.com";
const NORULES_HOST: &str = "region-route-norules.example.com";
const MYREGION_HOST: &str = "region-route-myregion.example.com";
const BROKEN_REF_HOST: &str = "region-route-broken-ref.example.com";

// Observe selection and terminal rejection independently: the target is logged before
// the jump setter runs, while an arbitrary 500/502 does not prove the loopback guard.
fn observed_nonlocal_failover(entry: &AccessLogEntry) -> bool {
    if entry.route_name() != Some("region-route-myregion-test")
        || !entry.has_diagnostic("error", "BackendLocalhostNotAllowed")
    {
        return false;
    }
    let Some(stages) = entry.data["stage_logs"].as_array() else {
        return false;
    };
    let mut executions = stages
        .iter()
        .filter_map(|stage| stage["edgion_plugins"].as_array())
        .flatten()
        .filter(|group| group["name"] == "region-route-myregion")
        .filter_map(|group| group["logs"].as_array())
        .flatten()
        .filter(|plugin| plugin["name"] == "RegionRoute");
    let Some(execution) = executions.next() else {
        return false;
    };
    if executions.next().is_some() {
        return false;
    }
    let Some(messages) = execution["log"].as_array() else {
        return false;
    };
    let redirect = format!("{}europe→ohio", log_t::FAILOVER_REDIRECT);
    messages
        .iter()
        .any(|message| message.as_str() == Some(redirect.as_str()))
        && messages.iter().filter_map(|message| message.as_str()).any(|message| {
            // Include the delimiter so another port cannot match by prefix.
            message.starts_with("→ohio 127.0.0.1:30002 ")
        })
}

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

    /// A missing clusterRegionRef target triggers Continue and normal proxying.
    /// The original backend returns 200 without RegionRoute request changes.
    /// Access log must contain "setting_missing" confirming the signal was recorded.
    fn broken_setting_ref_continues() -> TestCase {
        TestCase::new(
            "broken_setting_ref_continues",
            "Broken clusterRegionRef (nonexistent EdgionConfigData): setting_missing -> continue normal proxy -> 200; access log contains setting_missing",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-broken-ref-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", BROKEN_REF_HOST)
                        .header("X-Direct-Region", "virginia")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            // An unavailable setting leaves normal backend selection unchanged.
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected normal proxy response 200, got {}", status),
                                );
                            }
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains(log_t::SETTING_MISSING) {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("setting_missing logged as expected; status={}", status),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected 'setting_missing' in access log. log: {}", log_str),
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_resp) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    // Plugin emits: "hash key=<key> src=hdr:X-Account-Id val=<n>"
                                    // and "hash_wb→res_hdr:X-Region-Hash".
                                    let has_hash = log_str.contains(log_t::HASH_KEY)
                                        && log_str.contains("src=hdr:X-Account-Id val=");
                                    let has_wb =
                                        log_str.contains(log_t::HASH_WB) && log_str.contains("res_hdr:X-Region-Hash");
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
                            .header(
                                "traceparent",
                                crate::access_log_client::traceparent_for_marker(&trace_id),
                            )
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

                    // Extract the hash value from a log entry like
                    // `hash key=<key> src=hdr:X-Account-Id val=<N>`. The value ends at the
                    // first non-digit (quote/comma in JSON serialized form).
                    let extract_hash = |log: &str| -> Option<String> {
                        let start = log.find("val=")? + 4;
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains(log_t::DIRECT) && log_str.contains("→virginia") {
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains(log_t::DIRECT) && log_str.contains("→ohio") {
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
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
                                    // misses, then RouteByHashRange routes. Plugin emits:
                                    //   `direct=<r> unknown, fallthrough`
                                    //   `conf_match key=<v> src=hdr:X-Account-Id no_entry, fallthrough`
                                    //   `hash key=<k> src=hdr:X-Account-Id val=<n>`
                                    let fell_through = log_str.contains("fallthrough");
                                    let match_miss = log_str.contains("src=hdr:X-Account-Id")
                                        && log_str.contains(log_t::CONF_MATCH_NO_ENTRY);
                                    let hash_computed = log_str.contains(log_t::HASH_KEY)
                                        && log_str.contains("src=hdr:X-Account-Id val=");
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    // Plugin emits `conf_match key=<v> src=hdr:X-Account-Id →virginia`
                                    // (note the space before the arrow).
                                    let has_match = log_str.contains(log_t::CONF_MATCH_KEY)
                                        && log_str.contains("src=hdr:X-Account-Id →virginia");
                                    if has_match {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected conf_match key=...→virginia. log: {}", log_str),
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    // Plugin emits `conf_match key=<v> src=hdr:X-Account-Id →ohio`.
                                    let has_match = log_str.contains(log_t::CONF_MATCH_KEY)
                                        && log_str.contains("src=hdr:X-Account-Id →ohio");
                                    if has_match {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected conf_match key=...→ohio. log: {}", log_str),
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    // Plugin: `conf_match key=<v> src=hdr:X-Account-Id no_entry, fallthrough`
                                    // then `hash key=<k> src=hdr:X-Account-Id val=<n>`.
                                    let has_no_conf = log_str.contains("src=hdr:X-Account-Id")
                                        && log_str.contains(log_t::CONF_MATCH_NO_ENTRY);
                                    let has_hash = log_str.contains(log_t::HASH_KEY)
                                        && log_str.contains("src=hdr:X-Account-Id val=");
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains(&format!("{}100", log_t::HASH_PASS))
                                        && log_str.contains("→virginia")
                                    {
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_) => {
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains(&format!("{}800", log_t::HASH_PASS))
                                        && log_str.contains("→ohio")
                                    {
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
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
                                    // Plugin: `hash_pass=<v> src=...` only fires on in-range passthrough;
                                    // fallback emits `hash key=<k> src=hdr:X-Account-Id val=<n>`.
                                    let used_passthrough = log_str.contains(&format!("{}1001", log_t::HASH_PASS));
                                    let used_key = log_str.contains(log_t::HASH_KEY)
                                        && log_str.contains("src=hdr:X-Account-Id val=");
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
    // local regionName + failoverTo tests (MYREGION_HOST)
    // =========================================================================

    /// X-Direct-Region: virginia → local regionName match → local pass-through → 200
    fn my_region_direct_local() -> TestCase {
        TestCase::new(
            "my_region_direct_local",
            "X-Direct-Region: virginia (=local regionName) → local pass-through, backend returns 200",
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

    /// Local-region pass-through still dyes: the request was routed to a
    /// region (virginia == local regionName), so X-Region must reach the backend.
    fn myregion_local_pass_sets_dye_header() -> TestCase {
        TestCase::new(
            "myregion_local_pass_sets_dye_header",
            "Local-region pass-through: dye X-Region carries 'virginia'",
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
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            let body = resp.text().await.unwrap_or_default();
                            if body.contains("x-region: virginia") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("x-region: virginia not found in echo body: {}", body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// X-Direct-Region: ohio (failoverTo=virginia=local regionName) → redirect ohio→virginia → local pass → 200
    fn my_region_failover_to_local() -> TestCase {
        TestCase::new(
            "my_region_failover_to_local",
            "X-Direct-Region: ohio (failoverTo=virginia=local regionName) → redirect→local pass, 200 + log check",
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
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
                                    // Plugin emits `failover <region>→<target>` then `local_region, pass`.
                                    let has_redirect =
                                        log_str.contains(log_t::FAILOVER_REDIRECT) && log_str.contains("ohio→virginia");
                                    let has_local = log_str.contains(log_t::LOCAL_REGION_PASS);
                                    if has_redirect && has_local {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected 'failover ohio→virginia' and 'local_region, pass'. has_redirect={}, has_local={}. log: {}",
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

    /// X-Direct-Region: europe (failoverTo=ohio, ohio≠local regionName) → redirect europe→ohio → jump → 502
    fn my_region_failover_to_nonlocal() -> TestCase {
        TestCase::new(
            "my_region_failover_to_nonlocal",
            "X-Direct-Region: europe (failoverTo=ohio≠local regionName) → redirect→jump to loopback → 502 + log check",
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
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
                                    if observed_nonlocal_failover(&entry) {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected route region-route-myregion-test, one RegionRoute execution in region-route-myregion with 'failover europe→ohio' and target '→ohio 127.0.0.1:30002', and error BackendLocalhostNotAllowed. log: {}",
                                                entry.data
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
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
                                    if log_str.contains(log_t::HASH_WB) && log_str.contains("res_hdr:X-Region-Hash") {
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
            // Missing setting -> Continue (200 + access log)
            Self::broken_setting_ref_continues(),
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
            // local regionName + failoverTo (MYREGION_HOST)
            Self::my_region_direct_local(),
            Self::myregion_local_pass_sets_dye_header(),
            Self::my_region_failover_to_local(),
            Self::my_region_failover_to_nonlocal(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::{observed_nonlocal_failover, AccessLogEntry};
    use serde_json::json;

    fn nonlocal_failover() -> AccessLogEntry {
        AccessLogEntry {
            data: json!({
                "match_info": {"rsc": {"name": "region-route-myregion-test"}},
                "stage_logs": [{"edgion_plugins": [{
                    "name": "region-route-myregion",
                    "logs": [{"name": "RegionRoute", "log": [
                        "failover europe→ohio",
                        "→ohio 127.0.0.1:30002 sni=region-route-myregion.example.com"
                    ]}]
                }]}],
                "diagnostics": [{"kind": "error", "code": "BackendLocalhostNotAllowed"}]
            }),
        }
    }

    #[test]
    fn nonlocal_failover_accepts_independent_selection_and_rejection() {
        assert!(observed_nonlocal_failover(&nonlocal_failover()));
    }

    #[test]
    fn nonlocal_failover_rejects_incomplete_or_wrong_selection() {
        for messages in [
            json!(["failover europe→ohio"]),
            json!(["→ohio 127.0.0.1:30002 sni=example.com"]),
            json!(["failover europe→ohio", "→ohio 127.0.0.1:30003 sni=example.com"]),
            json!(["failover europe→ohio", "→ohio 127.0.0.1:300020 sni=example.com"]),
            json!(["failover europe→ohio", "→virginia 127.0.0.1:30001 sni=example.com"]),
            json!([
                "unrelated failover europe→ohio",
                "→ohio 127.0.0.1:30002 sni=example.com"
            ]),
            json!("failover europe→ohio; →ohio 127.0.0.1:30002 sni=example.com"),
            json!(null),
        ] {
            let mut entry = nonlocal_failover();
            entry.data["stage_logs"][0]["edgion_plugins"][0]["logs"][0]["log"] = messages;
            assert!(!observed_nonlocal_failover(&entry), "{}", entry.data);
        }
    }

    #[test]
    fn nonlocal_failover_requires_typed_terminal_rejection() {
        for diagnostics in [
            json!([]),
            json!(null),
            json!([{"kind": "error", "code": "BackendConnectError"}]),
            json!([{"kind": "info", "code": "BackendLocalhostNotAllowed"}]),
        ] {
            let mut entry = nonlocal_failover();
            entry.data["diagnostics"] = diagnostics;
            assert!(!observed_nonlocal_failover(&entry), "{}", entry.data);
        }
    }

    #[test]
    fn nonlocal_failover_requires_expected_scope_and_unique_execution() {
        for path in [
            "/match_info/rsc/name",
            "/stage_logs/0/edgion_plugins/0/name",
            "/stage_logs/0/edgion_plugins/0/logs/0/name",
        ] {
            let mut entry = nonlocal_failover();
            *entry.data.pointer_mut(path).unwrap() = json!("unrelated");
            assert!(!observed_nonlocal_failover(&entry), "{path}");
        }
        for path in [
            "/stage_logs",
            "/stage_logs/0/edgion_plugins",
            "/stage_logs/0/edgion_plugins/0/logs",
        ] {
            for replacement in [json!(null), json!([]), json!({})] {
                let mut entry = nonlocal_failover();
                *entry.data.pointer_mut(path).unwrap() = replacement;
                assert!(!observed_nonlocal_failover(&entry), "{path}");
            }
        }
        let mut entry = nonlocal_failover();
        let duplicate = entry.data["stage_logs"][0].clone();
        entry.data["stage_logs"].as_array_mut().unwrap().push(duplicate);
        assert!(!observed_nonlocal_failover(&entry));
    }

    #[test]
    fn nonlocal_failover_ignores_messages_outside_execution() {
        let mut entry = nonlocal_failover();
        let messages = entry.data["stage_logs"][0]["edgion_plugins"][0]["logs"][0]["log"].take();
        entry.data["stage_logs"][0]["edgion_plugins"][0]["logs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name": "OtherPlugin", "log": messages.clone()}));
        entry.data["unrelated_metadata"] = messages;
        assert!(!observed_nonlocal_failover(&entry));
    }
}
