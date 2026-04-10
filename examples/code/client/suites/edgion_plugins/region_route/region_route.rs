// RegionRoute Integration Test Suite
//
// Required conf files (examples/test/conf/EdgionPlugins/RegionRoute/):
//   01_EdgionPlugins_region-route-hash.yaml   — hash routing, direct_region, region_key_hash
//   01_EdgionPlugins_region-route-nokey.yaml  — empty region_key (no-op)
//   02_HTTPRoute_region-route-hash-test.yaml
//   02_HTTPRoute_region-route-nokey-test.yaml
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

const HASH_HOST: &str = "region-route-hash.example.com";
const NOKEY_HOST: &str = "region-route-nokey.example.com";

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

    /// Empty region_key config → plugin is a no-op → normal backend responds 200
    fn no_region_key_passes_through() -> TestCase {
        TestCase::new(
            "no_region_key_passes_through",
            "Empty region_key: plugin is no-op, normal backend returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", NOKEY_HOST).send().await {
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

    /// region_key configured but header absent → plugin passes through → normal backend 200
    fn missing_key_passes_through() -> TestCase {
        TestCase::new(
            "missing_key_passes_through",
            "region_key configured but header absent: plugin passes through, normal backend 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", HASH_HOST)
                        // No X-Account-Id header, no account_id cookie
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200 (pass-through), got {}", status))
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

    /// X-Account-Id header → plugin computes hash → routes to one of the two regions
    /// Verify access log contains "key=hdr(X-Account-Id)(...) hash=N"
    fn hash_routing_logged() -> TestCase {
        TestCase::new(
            "hash_routing_logged",
            "X-Account-Id header → hash computed, region jump logged in access log",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-hash-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", HASH_HOST)
                        .header("X-Account-Id", "acct-test-001")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_resp) => {
                            // Response is 502 (loopback rejected), check access log instead
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    // Plugin log should contain hash computation
                                    // and a region jump entry (→virginia or →ohio)
                                    let has_hash = log_str.contains("key=hdr(X-Account-Id)");
                                    let has_jump = log_str.contains("→virginia") || log_str.contains("→ohio");
                                    if has_hash && has_jump {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Access log missing expected entries. has_hash={}, has_jump={}. log: {}",
                                                has_hash, has_jump, log_str
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
                            .header("host", HASH_HOST)
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

                    match (log1, log2) {
                        (Ok(l1), Ok(l2)) => {
                            let region1 = if l1.contains("→virginia") {
                                "virginia"
                            } else if l1.contains("→ohio") {
                                "ohio"
                            } else {
                                "unknown"
                            };
                            let region2 = if l2.contains("→virginia") {
                                "virginia"
                            } else if l2.contains("→ohio") {
                                "ohio"
                            } else {
                                "unknown"
                            };
                            if region1 == region2 && region1 != "unknown" {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Same key routed differently: {} vs {}", region1, region2),
                                )
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
    // direct_region override (expect 502 + access log entry)
    // =========================================================================

    /// X-Direct-Region: virginia → access log shows "direct=virginia"
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
                        .header("host", HASH_HOST)
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

    /// X-Direct-Region: ohio → access log shows "direct=ohio"
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
                        .header("host", HASH_HOST)
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

    /// X-Direct-Region: unknown-region → falls through to hash routing
    fn direct_region_unknown_falls_through() -> TestCase {
        TestCase::new(
            "direct_region_unknown_falls_through",
            "X-Direct-Region with unknown region name falls through to hash routing",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-direct-unk-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", HASH_HOST)
                        .header("X-Direct-Region", "not-a-region")
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
                                    // Should log "direct unknown 'not-a-region', fallthrough"
                                    // then route by hash
                                    let fell_through = log_str.contains("fallthrough");
                                    let routed_by_hash =
                                        log_str.contains("key=hdr(X-Account-Id)") && (log_str.contains("→virginia") || log_str.contains("→ohio"));
                                    if fell_through && routed_by_hash {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected fallthrough then hash routing. fell_through={}, routed_by_hash={}. log: {}",
                                                fell_through, routed_by_hash, log_str
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
    // region_key_hash passthrough
    // =========================================================================

    /// Cookie x_region_hash=100 → falls in virginia [0,600] → access log "hash_pass=100"
    fn region_key_hash_passthrough_virginia() -> TestCase {
        TestCase::new(
            "region_key_hash_passthrough_virginia",
            "Pre-computed hash cookie 100 → virginia range [0,600], access log shows hash_pass",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-pass-va-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", HASH_HOST)
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
    fn region_key_hash_passthrough_ohio() -> TestCase {
        TestCase::new(
            "region_key_hash_passthrough_ohio",
            "Pre-computed hash cookie 800 → ohio range [601,1000], access log shows hash_pass",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-pass-oh-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", HASH_HOST)
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

    /// Out-of-range passthrough (x_region_hash=1001) is ignored → falls back to region_key computation
    fn region_key_hash_out_of_range_ignored() -> TestCase {
        TestCase::new(
            "region_key_hash_out_of_range_ignored",
            "hash passthrough > 1000 is ignored, falls back to region_key computation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("rr-pass-oor-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", HASH_HOST)
                        .header("Cookie", "x_region_hash=1001")  // invalid → ignored
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
                                    let used_key = log_str.contains("key=hdr(X-Account-Id)");
                                    if !used_passthrough && used_key {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected fallback to region_key. used_passthrough={}, used_key={}. log: {}",
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
}

impl TestSuite for RegionRouteTestSuite {
    fn name(&self) -> &str {
        "RegionRoute"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Pass-through (200)
            Self::no_region_key_passes_through(),
            Self::missing_key_passes_through(),
            // Hash routing (502 + access log)
            Self::hash_routing_logged(),
            Self::hash_routing_deterministic(),
            // direct_region override (502 + access log)
            Self::direct_region_virginia(),
            Self::direct_region_ohio(),
            Self::direct_region_unknown_falls_through(),
            // region_key_hash passthrough (502 + access log)
            Self::region_key_hash_passthrough_virginia(),
            Self::region_key_hash_passthrough_ohio(),
            Self::region_key_hash_out_of_range_ignored(),
        ]
    }
}
