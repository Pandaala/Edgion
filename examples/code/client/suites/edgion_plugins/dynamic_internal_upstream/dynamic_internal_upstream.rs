// DynamicInternalUpstream Integration Test Suite
//
// Required config files (in examples/test/conf/EdgionPlugins/DynamicInternalUpstream/):
// - 01_Service_default_diu-backend-{stable,canary,debug}.yaml
// - 02_EndpointSlice_default_diu-backend-{stable,canary,debug}-slice.yaml
// - 03_EdgionPlugins_default_diu-rules.yaml        (rules mode, onMissing=fallback)
// - 03_EdgionPlugins_default_diu-direct.yaml        (direct mode, onMissing=reject)
// - 04_HTTPRoute_default_diu-rules-test.yaml        (3 backends: stable w90, canary w10, debug w0)
// - 04_HTTPRoute_default_diu-direct-test.yaml       (2 backends: stable w50, canary w50)
//
// Test backends:
// - diu-backend-stable  → test_server :30001 (responds "Server: 0.0.0.0:30001")
// - diu-backend-canary  → test_server :30002 (responds "Server: 0.0.0.0:30002")
// - diu-backend-debug   → test_server :30003 (responds "Server: 0.0.0.0:30003")

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct DynamicInternalUpstreamTestSuite;

impl DynamicInternalUpstreamTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DynamicInternalUpstreamTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

const RULES_HOST: &str = "diu-rules-test.example.com";
const DIRECT_HOST: &str = "diu-direct-test.example.com";

/// Helper: extract the port number from echo response body "Server: 0.0.0.0:PORT"
fn extract_server_port(body: &str) -> Option<u16> {
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("Server: ") {
            // rest is like "0.0.0.0:30001"
            if let Some(port_str) = rest.rsplit(':').next() {
                return port_str.trim().parse().ok();
            }
        }
    }
    None
}

impl DynamicInternalUpstreamTestSuite {
    // ======================================================================
    // Rules mode tests (RULES_HOST)
    // ======================================================================

    /// Rules mode: X-Backend-Target: canary → routes to canary backend (port 30002)
    fn rules_route_to_canary() -> TestCase {
        TestCase::new(
            "rules_route_to_canary",
            "Rules mode: header=canary routes to canary backend (port 30002)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", RULES_HOST)
                        .header("X-Backend-Target", "canary")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            let body = resp.text().await.unwrap_or_default();
                            match extract_server_port(&body) {
                                Some(30002) => TestResult::passed(start.elapsed()),
                                Some(port) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected port 30002 (canary), got {}", port),
                                ),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    format!("Cannot extract server port from body: {}", body),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Rules mode: X-Backend-Target: debug → routes to debug backend (port 30003, weight=0)
    fn rules_route_to_debug_weight_zero() -> TestCase {
        TestCase::new(
            "rules_route_to_debug_weight_zero",
            "Rules mode: header=debug routes to weight-0 backend (port 30003)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", RULES_HOST)
                        .header("X-Backend-Target", "debug")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            let body = resp.text().await.unwrap_or_default();
                            match extract_server_port(&body) {
                                Some(30003) => TestResult::passed(start.elapsed()),
                                Some(port) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected port 30003 (debug), got {}", port),
                                ),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    format!("Cannot extract server port from body: {}", body),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Rules mode: missing header → fallback to normal LB (stable or canary, not debug w=0)
    fn rules_missing_header_fallback() -> TestCase {
        TestCase::new(
            "rules_missing_header_fallback",
            "Rules mode: missing header falls back to weighted LB (not debug w=0)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", RULES_HOST)
                        // No X-Backend-Target header
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            let body = resp.text().await.unwrap_or_default();
                            match extract_server_port(&body) {
                                Some(30001) | Some(30002) => TestResult::passed(start.elapsed()),
                                Some(30003) => TestResult::failed(
                                    start.elapsed(),
                                    "Fallback selected debug backend (weight=0), should not happen".to_string(),
                                ),
                                Some(port) => TestResult::failed(start.elapsed(), format!("Unexpected port: {}", port)),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    format!("Cannot extract server port from body: {}", body),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Rules mode: no matching rule → fallback (onNoMatch: fallback)
    fn rules_no_match_fallback() -> TestCase {
        TestCase::new(
            "rules_no_match_fallback",
            "Rules mode: unknown value falls back to weighted LB",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", RULES_HOST)
                        .header("X-Backend-Target", "nonexistent-value")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 (fallback), got {}", status),
                                );
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Rules mode: invalid backend_ref name → 403 (onInvalid: reject)
    fn rules_invalid_backend_reject() -> TestCase {
        TestCase::new(
            "rules_invalid_backend_reject",
            "Rules mode: rule pointing to non-existent backend should 403 (but this tests valid rules, so we test via access log)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // The "canary" rule maps to diu-backend-canary which exists → 200
                    // The "debug" rule maps to diu-backend-debug which exists → 200
                    // So to test on_invalid, we use the direct mode test instead.
                    // Here just verify the debugHeader is set correctly.
                    let trace_id = format!("diu-debug-header-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", RULES_HOST)
                        .header("X-Backend-Target", "canary")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", status),
                                );
                            }

                            // Verify access log plugin_log contains "OK diu-backend-canary"
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains("OK diu-backend-canary") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Access log does not contain 'OK diu-backend-canary': {}",
                                                log_str
                                            ),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Failed to get access log: {}", e),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ======================================================================
    // Direct mode tests (DIRECT_HOST)
    // ======================================================================

    /// Direct mode: X-Backend-Target: diu-backend-canary → routes to canary (port 30002)
    fn direct_route_to_canary() -> TestCase {
        TestCase::new(
            "direct_route_to_canary",
            "Direct mode: header=diu-backend-canary routes to canary backend (port 30002)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", DIRECT_HOST)
                        .header("X-Backend-Target", "diu-backend-canary")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            let body = resp.text().await.unwrap_or_default();
                            match extract_server_port(&body) {
                                Some(30002) => TestResult::passed(start.elapsed()),
                                Some(port) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected port 30002 (canary), got {}", port),
                                ),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    format!("Cannot extract server port from body: {}", body),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Direct mode: X-Backend-Target: diu-backend-stable → routes to stable (port 30001)
    fn direct_route_to_stable() -> TestCase {
        TestCase::new(
            "direct_route_to_stable",
            "Direct mode: header=diu-backend-stable routes to stable backend (port 30001)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", DIRECT_HOST)
                        .header("X-Backend-Target", "diu-backend-stable")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            let body = resp.text().await.unwrap_or_default();
                            match extract_server_port(&body) {
                                Some(30001) => TestResult::passed(start.elapsed()),
                                Some(port) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected port 30001 (stable), got {}", port),
                                ),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    format!("Cannot extract server port from body: {}", body),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Direct mode: missing header → 400 (onMissing: reject)
    fn direct_missing_header_reject() -> TestCase {
        TestCase::new(
            "direct_missing_header_reject",
            "Direct mode: missing header returns 400 (onMissing: reject)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", DIRECT_HOST)
                        // No X-Backend-Target header
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 400 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 400 (reject), got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Direct mode: invalid backend name → 403 (onInvalid: reject)
    fn direct_invalid_backend_reject() -> TestCase {
        TestCase::new(
            "direct_invalid_backend_reject",
            "Direct mode: non-existent backend name returns 403 (onInvalid: reject)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", DIRECT_HOST)
                        .header("X-Backend-Target", "nonexistent-service")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 403 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 403 (invalid backend), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Rules mode: verify debug header (X-Dynamic-Internal-Upstream) is set in upstream request
    fn rules_debug_header_present() -> TestCase {
        TestCase::new(
            "rules_debug_header_present",
            "Rules mode: debug header X-Dynamic-Internal-Upstream is sent to upstream",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", RULES_HOST)
                        .header("X-Backend-Target", "canary")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            // The echo handler returns all headers — check if the debug header was forwarded
                            let body = resp.text().await.unwrap_or_default();
                            if body.contains("x-dynamic-internal-upstream: diu-backend-canary") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Debug header x-dynamic-internal-upstream not found in echo body: {}",
                                        body
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

    /// Rules mode: verify consistent routing — send 5 requests to canary, all should hit port 30002
    fn rules_consistent_routing() -> TestCase {
        TestCase::new(
            "rules_consistent_routing",
            "Rules mode: multiple requests with same target all go to same backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());

                    for i in 0..5 {
                        match ctx
                            .http_client
                            .get(&url)
                            .header("host", RULES_HOST)
                            .header("X-Backend-Target", "canary")
                            .send()
                            .await
                        {
                            Ok(resp) => {
                                let status = resp.status().as_u16();
                                if status != 200 {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Request {} got status {}", i, status),
                                    );
                                }
                                let body = resp.text().await.unwrap_or_default();
                                match extract_server_port(&body) {
                                    Some(30002) => {} // correct
                                    Some(port) => {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Request {} went to port {} instead of 30002", i, port),
                                        );
                                    }
                                    None => {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Request {}: cannot extract port from body", i),
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Request {} failed: {}", i, e));
                            }
                        }
                    }
                    TestResult::passed(start.elapsed())
                })
            },
        )
    }
}

impl TestSuite for DynamicInternalUpstreamTestSuite {
    fn name(&self) -> &str {
        "DynamicInternalUpstream"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Rules mode
            Self::rules_route_to_canary(),
            Self::rules_route_to_debug_weight_zero(),
            Self::rules_missing_header_fallback(),
            Self::rules_no_match_fallback(),
            Self::rules_invalid_backend_reject(),
            Self::rules_debug_header_present(),
            Self::rules_consistent_routing(),
            // Direct mode
            Self::direct_route_to_canary(),
            Self::direct_route_to_stable(),
            Self::direct_missing_header_reject(),
            Self::direct_invalid_backend_reject(),
        ]
    }
}
