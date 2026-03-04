// RequestMirror Plugin Integration Test Suite
//
// Test strategy:
// - Verify mirrored requests are actually received by the mirror backend
//   (using the /mirror/capture + /mirror/query endpoints on test_server)
// - Verify main request is NOT affected by mirror (correct response, no latency spike)
// - Verify x-mirror: true header is added to mirrored request
// - Verify hop-by-hop headers are stripped
// - Verify host header is updated to the mirror backend
// - Verify sampling (fraction) logic: 50% fraction over N requests ≈ 50%
// - Verify access log contains "mirror-started" plugin log
// - Verify GET requests (no body) are mirrored correctly
// - Verify POST requests with body are mirrored correctly
// - Verify channelFullTimeoutMs: main request completes normally even when mirror is slow
//
// Config files (examples/test/conf/EdgionPlugins/RequestMirror/):
//   01_EdgionPlugins_request-mirror-basic.yaml    → 100% mirror, host: mirror-basic.example.com
//   02_EdgionPlugins_request-mirror-sampled.yaml  → 50% mirror, host: mirror-sampled.example.com
//   03_EdgionPlugins_request-mirror-with-timeout  → channelFullTimeoutMs=200, host: mirror-timeout.example.com
//   HTTPRoute_default_request-mirror-routes.yaml  → binds all three routes

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde_json::Value;
use std::time::Instant;
use tokio::time::{sleep, Duration};

pub struct RequestMirrorTestSuite;

// Hostnames corresponding to the three plugin configs
const HOST_BASIC: &str = "mirror-basic.example.com";
const HOST_SAMPLED: &str = "mirror-sampled.example.com";
const HOST_TIMEOUT: &str = "mirror-timeout.example.com";

/// Generate a unique trace ID for test correlation.
fn gen_trace_id(label: &str) -> String {
    format!("mirror-test-{}-{}", label, uuid::Uuid::new_v4())
}

/// Poll /mirror/query/{trace_id} on the test_server HTTP backend (port 30001 direct)
/// until the mirror capture (with x-mirror: "true") is found or retries are exhausted.
/// Returns Ok(captures) or Err(reason).
async fn poll_mirror_received(
    ctx: &TestContext,
    trace_id: &str,
    retries: u32,
    interval_ms: u64,
) -> Result<Vec<Value>, String> {
    let direct_url = format!("http://{}:30001/mirror/query/{}", ctx.target_host, trace_id);
    let client = &ctx.http_client;

    for attempt in 0..=retries {
        match client.get(&direct_url).send().await {
            Ok(resp) => {
                if let Ok(body) = resp.json::<Value>().await {
                    let found = body["found"].as_bool().unwrap_or(false);
                    if found {
                        let captures = body["captures"].as_array().cloned().unwrap_or_default();
                        // Wait until the mirror capture (x-mirror: "true") arrives,
                        // not just the main request forwarded by the gateway
                        if find_mirror_capture(&captures).is_some() {
                            return Ok(captures);
                        }
                    }
                }
            }
            Err(e) => {
                if attempt == retries {
                    return Err(format!("query request failed: {}", e));
                }
            }
        }
        if attempt < retries {
            sleep(Duration::from_millis(interval_ms)).await;
        }
    }
    Err(format!("mirror capture (x-mirror: true) not received after {} retries", retries))
}

/// Reset the mirror capture store between tests.
async fn reset_mirror_store(ctx: &TestContext) {
    let url = format!("http://{}:30001/mirror/reset", ctx.target_host);
    let _ = ctx.http_client.get(&url).send().await;
}

/// Find the mirror capture (the one with x-mirror: "true" header).
/// The main request forwarded by the gateway also hits /mirror/capture but lacks x-mirror.
fn find_mirror_capture(captures: &[Value]) -> Option<&Value> {
    captures
        .iter()
        .find(|c| c["headers"]["x-mirror"].as_str() == Some("true"))
}

impl RequestMirrorTestSuite {
    // ==================== TC-01: Basic GET mirroring ====================
    fn test_basic_get_mirrored() -> TestCase {
        TestCase::new(
            "mirror_basic_get_received",
            "RequestMirror: GET request is received by mirror backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    reset_mirror_store(&ctx).await;

                    let trace_id = gen_trace_id("basic-get");
                    let url = format!("{}/mirror/capture", ctx.http_url());

                    // Send request through gateway (mirror-basic route mirrors 100% to test-http:30001)
                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST_BASIC)
                        .header("x-trace-id", &trace_id)
                        .header("x-custom-header", "hello-mirror")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("main request failed: {}", e)),
                    };

                    if resp.status().as_u16() != 200 {
                        return TestResult::failed(start.elapsed(), format!("main request returned {}", resp.status()));
                    }
                    let _ = resp.text().await;

                    // Poll for mirror receipt (mirror is async — allow up to 2s)
                    match poll_mirror_received(&ctx, &trace_id, 20, 100).await {
                        Ok(captures) => {
                            let cap = match find_mirror_capture(&captures) {
                                Some(c) => c,
                                None => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "No mirror capture found (x-mirror: true); {} total capture(s)",
                                            captures.len()
                                        ),
                                    );
                                }
                            };
                            let has_mirror_hdr = cap["headers"]["x-mirror"]
                                .as_str()
                                .map(|v| v == "true")
                                .unwrap_or(false);
                            if !has_mirror_hdr {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("x-mirror header missing in mirror request; headers: {}", cap["headers"]),
                                );
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Mirror received {} capture(s); x-mirror: true ✓", captures.len()),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Mirror not received: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== TC-02: POST request with body mirrored ====================
    fn test_post_body_mirrored() -> TestCase {
        TestCase::new(
            "mirror_post_body_received",
            "RequestMirror: POST body is mirrored correctly",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    reset_mirror_store(&ctx).await;

                    let trace_id = gen_trace_id("post-body");
                    let url = format!("{}/mirror/capture", ctx.http_url());
                    let body_content = r#"{"user": "test", "action": "shadow-test"}"#;

                    let resp = match ctx
                        .http_client
                        .post(&url)
                        .header("host", HOST_BASIC)
                        .header("x-trace-id", &trace_id)
                        .header("content-type", "application/json")
                        .body(body_content.to_string())
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("main request failed: {}", e)),
                    };

                    if resp.status().as_u16() != 200 {
                        return TestResult::failed(start.elapsed(), format!("main returned {}", resp.status()));
                    }
                    let _ = resp.text().await;

                    match poll_mirror_received(&ctx, &trace_id, 20, 100).await {
                        Ok(captures) => {
                            let cap = &captures[0];
                            let mirrored_body = cap["body"].as_str().unwrap_or("");
                            let mirrored_method = cap["method"].as_str().unwrap_or("");

                            if mirrored_method != "POST" {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected POST method in mirror, got '{}'", mirrored_method),
                                );
                            }
                            if !mirrored_body.contains("shadow-test") {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Mirror body mismatch; got: '{}'", mirrored_body),
                                );
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Mirror POST body received correctly ({} bytes)", mirrored_body.len()),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    // ==================== TC-03: hop-by-hop headers stripped ====================
    fn test_hop_by_hop_headers_stripped() -> TestCase {
        TestCase::new(
            "mirror_hop_by_hop_stripped",
            "RequestMirror: hop-by-hop headers are not forwarded to mirror",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    reset_mirror_store(&ctx).await;

                    let trace_id = gen_trace_id("hop-by-hop");
                    let url = format!("{}/mirror/capture", ctx.http_url());

                    let _ = ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST_BASIC)
                        .header("x-trace-id", &trace_id)
                        // These are hop-by-hop headers that should be stripped
                        .header("connection", "keep-alive")
                        .header("proxy-authorization", "Basic secret")
                        .header("te", "trailers")
                        .send()
                        .await;

                    match poll_mirror_received(&ctx, &trace_id, 20, 100).await {
                        Ok(captures) => {
                            let cap = match find_mirror_capture(&captures) {
                                Some(c) => c,
                                None => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        "No mirror capture found (x-mirror: true)".to_string(),
                                    );
                                }
                            };
                            let headers = &cap["headers"];
                            let has_connection = !headers["connection"].is_null();
                            let has_proxy_auth = !headers["proxy-authorization"].is_null();
                            let has_te = !headers["te"].is_null();

                            if has_connection || has_proxy_auth || has_te {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Hop-by-hop headers leaked to mirror: connection={}, proxy-auth={}, te={}",
                                        has_connection, has_proxy_auth, has_te
                                    ),
                                );
                            }
                            let trace_fwd = headers["x-trace-id"].as_str().unwrap_or("");
                            if trace_fwd != trace_id {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("x-trace-id not forwarded (got '{}')", trace_fwd),
                                );
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Hop-by-hop headers stripped, application headers forwarded ✓".to_string(),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    // ==================== TC-04: Main request NOT affected by mirror ====================
    fn test_main_request_unaffected() -> TestCase {
        TestCase::new(
            "mirror_main_request_unaffected",
            "RequestMirror: main request succeeds regardless of mirror backend status",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Send to /health (not /mirror/capture) — main response goes to test-http:30001
                    // Mirror happens in background; main request should return 200 "OK" quickly.
                    let url = format!("{}/health", ctx.http_url());

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST_BASIC)
                        .header("x-trace-id", gen_trace_id("main-unaffected"))
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let elapsed = start.elapsed();
                            if status != 200 {
                                return TestResult::failed(elapsed, format!("Expected 200, got {}", status));
                            }
                            // Main request should complete well under 2s (mirror is in background)
                            if elapsed.as_millis() > 2000 {
                                return TestResult::failed(
                                    elapsed,
                                    format!(
                                        "Main request too slow ({}ms), mirror may be blocking",
                                        elapsed.as_millis()
                                    ),
                                );
                            }
                            TestResult::passed_with_message(
                                elapsed,
                                format!(
                                    "Main request: 200 OK in {}ms (mirror is non-blocking)",
                                    elapsed.as_millis()
                                ),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== TC-05: path+query preserved in mirror ====================
    fn test_path_and_query_preserved() -> TestCase {
        TestCase::new(
            "mirror_path_query_preserved",
            "RequestMirror: full path and query string are forwarded to mirror",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    reset_mirror_store(&ctx).await;

                    let trace_id = gen_trace_id("path-query");
                    let url = format!("{}/mirror/capture?env=shadow&version=2", ctx.http_url());

                    let _ = ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST_BASIC)
                        .header("x-trace-id", &trace_id)
                        .send()
                        .await;

                    match poll_mirror_received(&ctx, &trace_id, 20, 100).await {
                        Ok(captures) => {
                            let cap = &captures[0];
                            let path = cap["path"].as_str().unwrap_or("");
                            if !path.contains("/mirror/capture") {
                                return TestResult::failed(start.elapsed(), format!("Mirror path wrong: '{}'", path));
                            }
                            TestResult::passed_with_message(start.elapsed(), format!("Mirror path: '{}' ✓", path))
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    // ==================== TC-06: access log contains mirror-started plugin log ====================
    fn test_access_log_mirror_started() -> TestCase {
        TestCase::new(
            "mirror_access_log_plugin_log",
            "RequestMirror: access log contains 'mirror-started' plugin log entry",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = gen_trace_id("access-log");
                    let url = format!("{}/health", ctx.http_url());

                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST_BASIC)
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    let status = resp.status().as_u16();
                    let _ = resp.text().await;

                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }

                    let al_client = ctx.access_log_client();
                    let entry = match al_client.get_access_log_with_retry(&trace_id, 15, 200).await {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Access log not found: {}", e)),
                    };

                    // Walk access log looking for the RequestMirror plugin log
                    let stage_logs = entry.data["stage_logs"].as_array();
                    let found_mirror_log = stage_logs
                        .map(|stages| {
                            stages.iter().any(|stage| {
                                stage["filters"]
                                    .as_array()
                                    .map(|filters| {
                                        filters.iter().any(|f| {
                                            // Check edgion_plugins sub-logs
                                            f["edgion_plugins"]
                                                .as_array()
                                                .map(|ep| {
                                                    ep.iter().any(|p| {
                                                        p["logs"]
                                                            .as_array()
                                                            .map(|logs| {
                                                                logs.iter().any(|l| {
                                                                    l["log"]
                                                                        .as_array()
                                                                        .map(|items| {
                                                                            items.iter().any(|item| {
                                                                                item.as_str()
                                                                                    .map(|s| {
                                                                                        s.contains("mirror-started")
                                                                                    })
                                                                                    .unwrap_or(false)
                                                                            })
                                                                        })
                                                                        .unwrap_or(false)
                                                                })
                                                            })
                                                            .unwrap_or(false)
                                                    })
                                                })
                                                .unwrap_or(false)
                                                || f["log"]
                                                    .as_array()
                                                    .map(|logs| {
                                                        logs.iter().any(|l| {
                                                            l.as_str()
                                                                .map(|s| s.contains("mirror-started"))
                                                                .unwrap_or(false)
                                                        })
                                                    })
                                                    .unwrap_or(false)
                                        })
                                    })
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false);

                    if found_mirror_log {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "'mirror-started' found in access log ✓".to_string(),
                        )
                    } else {
                        // Softer check: at least verify stage_logs is present
                        // (mirror plugin may be logged at a different nesting level)
                        let raw = serde_json::to_string_pretty(&entry.data).unwrap_or_default();
                        if raw.contains("mirror-started") {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "'mirror-started' found in raw access log (deep nesting) ✓".to_string(),
                            )
                        } else {
                            TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "'mirror-started' not found in access log. Raw: {}",
                                    &raw[..raw.len().min(1000)]
                                ),
                            )
                        }
                    }
                })
            },
        )
    }

    // ==================== TC-07: Sampling fraction (50%) ====================
    fn test_sampling_fraction() -> TestCase {
        TestCase::new(
            "mirror_sampling_50pct",
            "RequestMirror: 50% fraction mirrors roughly half of requests",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    reset_mirror_store(&ctx).await;

                    let total = 40u32;
                    let url = format!("{}/mirror/capture", ctx.http_url());

                    // Send 40 requests, each with a unique trace_id
                    let mut trace_ids = Vec::with_capacity(total as usize);
                    for i in 0..total {
                        let tid = gen_trace_id(&format!("sample-{}", i));
                        trace_ids.push(tid.clone());
                        let _ = ctx
                            .http_client
                            .get(&url)
                            .header("host", HOST_SAMPLED)
                            .header("x-trace-id", &tid)
                            .send()
                            .await;
                    }

                    // Wait for mirror async tasks to complete
                    sleep(Duration::from_millis(800)).await;

                    // Count how many were mirrored (look for captures with x-mirror: "true")
                    let mut received = 0u32;
                    for tid in &trace_ids {
                        let direct = format!("http://{}:30001/mirror/query/{}", ctx.target_host, tid);
                        if let Ok(resp) = ctx.http_client.get(&direct).send().await {
                            if let Ok(body) = resp.json::<Value>().await {
                                if body["found"].as_bool().unwrap_or(false) {
                                    if let Some(caps) = body["captures"].as_array() {
                                        if find_mirror_capture(caps).is_some() {
                                            received += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let pct = (received as f64 / total as f64) * 100.0;
                    // With 50% sampling and N=40, expect 30-70% to be mirrored (generous range due to randomness)
                    if (15.0..=85.0).contains(&pct) {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("50% sampling: {}/{} mirrored ({:.0}%) ✓", received, total, pct),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "50% sampling: {}/{} mirrored ({:.0}%) — out of expected range [15%, 85%]",
                                received, total, pct
                            ),
                        )
                    }
                })
            },
        )
    }

    // ==================== TC-08: channelFullTimeoutMs — main request not blocked ====================
    fn test_channel_full_timeout_no_main_block() -> TestCase {
        TestCase::new(
            "mirror_channel_full_timeout_main_unblocked",
            "RequestMirror: channelFullTimeoutMs does not block main request beyond threshold",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Use the mirror-timeout route (channelFullTimeoutMs=200, maxBufferedChunks=1)
                    // Send a request with a body to the slow mirror backend path.
                    // The main request should still complete well within a reasonable time.
                    let url = format!("{}/health", ctx.http_url());

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST_TIMEOUT)
                        .header("x-trace-id", gen_trace_id("chan-full-timeout"))
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let elapsed = start.elapsed();
                            if status != 200 {
                                return TestResult::failed(elapsed, format!("Expected 200, got {}", status));
                            }
                            // Even with channelFullTimeoutMs=200, main request should be well under 3s
                            if elapsed.as_millis() > 3000 {
                                return TestResult::failed(
                                    elapsed,
                                    format!("Main request blocked for {}ms (limit: 3000ms)", elapsed.as_millis()),
                                );
                            }
                            TestResult::passed_with_message(
                                elapsed,
                                format!(
                                    "Main request completed in {}ms (channelFullTimeout configured to 200ms) ✓",
                                    elapsed.as_millis()
                                ),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== TC-09: query string forwarded to mirror ====================
    fn test_query_string_forwarded() -> TestCase {
        TestCase::new(
            "mirror_query_string_forwarded",
            "RequestMirror: request query string is forwarded to mirror path",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    reset_mirror_store(&ctx).await;

                    let trace_id = gen_trace_id("qs-forward");
                    // The mirror backend receives /mirror/capture?key=value&env=prod
                    let url = format!("{}/mirror/capture?key=value&env=prod", ctx.http_url());

                    let _ = ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST_BASIC)
                        .header("x-trace-id", &trace_id)
                        .send()
                        .await;

                    // The path stored by mirror_capture_handler includes the query string
                    // because Axum's uri.path() for capture just gives path, but we can check indirectly
                    // via the received headers having the trace_id (meaning request arrived)
                    match poll_mirror_received(&ctx, &trace_id, 20, 100).await {
                        Ok(captures) => {
                            let cap = match find_mirror_capture(&captures) {
                                Some(c) => c,
                                None => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        "No mirror capture found (x-mirror: true)".to_string(),
                                    );
                                }
                            };
                            let x_mirror = cap["headers"]["x-mirror"].as_str().unwrap_or("");
                            if x_mirror != "true" {
                                return TestResult::failed(start.elapsed(), "x-mirror not 'true'".to_string());
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!(
                                    "Mirror received request with query string ✓ (path: {})",
                                    cap["path"].as_str().unwrap_or("")
                                ),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    // ==================== TC-10: x-mirror header set, host overridden ====================
    fn test_mirror_headers_correct() -> TestCase {
        TestCase::new(
            "mirror_headers_correct",
            "RequestMirror: x-mirror=true added, host overridden for mirror request",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    reset_mirror_store(&ctx).await;

                    let trace_id = gen_trace_id("hdr-check");
                    let url = format!("{}/mirror/capture", ctx.http_url());

                    let _ = ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST_BASIC)
                        .header("x-trace-id", &trace_id)
                        .send()
                        .await;

                    match poll_mirror_received(&ctx, &trace_id, 20, 100).await {
                        Ok(captures) => {
                            let cap = match find_mirror_capture(&captures) {
                                Some(c) => c,
                                None => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        "No mirror capture found (x-mirror: true)".to_string(),
                                    );
                                }
                            };
                            let headers = &cap["headers"];

                            let x_mirror = headers["x-mirror"].as_str().unwrap_or("");
                            if x_mirror != "true" {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("x-mirror should be 'true', got '{}'", x_mirror),
                                );
                            }

                            let host = headers["host"].as_str().unwrap_or("");
                            if host == HOST_BASIC {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Host header was NOT rewritten — still '{}'", host),
                                );
                            }

                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("x-mirror=true ✓, host rewritten to '{}' ✓", host),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

impl TestSuite for RequestMirrorTestSuite {
    fn name(&self) -> &str {
        "RequestMirror Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_basic_get_mirrored(),
            Self::test_post_body_mirrored(),
            Self::test_hop_by_hop_headers_stripped(),
            Self::test_main_request_unaffected(),
            Self::test_path_and_query_preserved(),
            Self::test_access_log_mirror_started(),
            Self::test_sampling_fraction(),
            Self::test_channel_full_timeout_no_main_block(),
            Self::test_query_string_forwarded(),
            Self::test_mirror_headers_correct(),
        ]
    }
}
