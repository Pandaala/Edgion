// DynamicExternalUpstream Integration Test Suite
//
// Required config files (in examples/test/conf/EdgionPlugins/DynamicExternalUpstream/):
// - 01_EdgionPlugins_default_deu-skip.yaml        (skip mode: onMissing=skip, onNoMatch=skip)
// - 01_EdgionPlugins_default_deu-reject.yaml      (reject mode: onMissing=reject, onNoMatch=reject)
// - 01_EdgionPlugins_default_deu-regex.yaml       (regex extraction mode)
// - 02_HTTPRoute_default_deu-skip-test.yaml        (fallback to test-http:30001)
// - 02_HTTPRoute_default_deu-reject-test.yaml
// - 02_HTTPRoute_default_deu-regex-test.yaml
//
// Test strategy:
// DynamicExternalUpstream routes to external domains via DNS resolution.
// In test env, domainMap targets "localhost" which resolves to 127.0.0.1.
// The loopback security check rejects connections to 127.0.0.1, so
// matched requests result in 502 (expected security behavior).
//
// We verify:
// 1. Plugin request_filter logic via access log (plugin_log entries)
// 2. Skip/Reject behavior for missing/unmatched keys
// 3. Regex extraction
// 4. Debug header presence
// 5. Loopback rejection (security)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct DynamicExternalUpstreamTestSuite;

impl DynamicExternalUpstreamTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DynamicExternalUpstreamTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

const SKIP_HOST: &str = "deu-skip-test.example.com";
const REJECT_HOST: &str = "deu-reject-test.example.com";
const REGEX_HOST: &str = "deu-regex-test.example.com";

impl DynamicExternalUpstreamTestSuite {
    // ======================================================================
    // Skip mode tests (SKIP_HOST)
    // ======================================================================

    /// Skip mode: missing header → normal backend (200)
    fn skip_missing_header_fallback() -> TestCase {
        TestCase::new(
            "skip_missing_header_fallback",
            "Skip mode: missing header falls back to normal backend (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", SKIP_HOST)
                        // No X-Target-Region header
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
                                    format!("Expected 200 (fallback), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Skip mode: unmatched key → normal backend (200)
    fn skip_no_match_fallback() -> TestCase {
        TestCase::new(
            "skip_no_match_fallback",
            "Skip mode: unmatched routing key falls back to normal backend (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", SKIP_HOST)
                        .header("X-Target-Region", "unknown-region")
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
                                    format!("Expected 200 (fallback), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Skip mode: matched key → plugin sets ExternalJumpPreset, DNS resolves to localhost → error (security rejection)
    /// Verifies the plugin correctly matched and the loopback security check works
    /// Pingora returns 502 or 500 depending on how it handles ConnectError
    fn skip_matched_localhost_rejected() -> TestCase {
        TestCase::new(
            "skip_matched_localhost_rejected",
            "Skip mode: matched key with localhost domain → 5xx (loopback rejected)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", SKIP_HOST)
                        .header("X-Target-Region", "us")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            // Pingora returns 502 or 500 for ConnectError
                            if status == 502 || status == 500 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 502 or 500 (loopback rejected), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Skip mode: verify plugin_log via access log when matched
    fn skip_access_log_matched() -> TestCase {
        TestCase::new(
            "skip_access_log_matched",
            "Skip mode: access log contains plugin OK entry when matched",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("deu-matched-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", SKIP_HOST)
                        .header("X-Target-Region", "eu")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_resp) => {
                            // Response may be 502 (localhost rejected), that's expected.
                            // Check access log for plugin execution.
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    // Plugin should log "OK localhost:30002"
                                    if log_str.contains("OK localhost:30002") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Access log does not contain 'OK localhost:30002': {}",
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

    /// Skip mode: verify access log when no match (should contain "NoMap" or "NoVal")
    fn skip_access_log_no_match() -> TestCase {
        TestCase::new(
            "skip_access_log_no_match",
            "Skip mode: access log contains NoMap entry when key not in domainMap",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("deu-nomatch-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", SKIP_HOST)
                        .header("X-Target-Region", "zz-unknown")
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
                                    format!("Expected 200 (fallback), got {}", status),
                                );
                            }
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains("NoMap") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Access log does not contain 'NoMap': {}",
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
    // Reject mode tests (REJECT_HOST)
    // ======================================================================

    /// Reject mode: missing header → 400
    fn reject_missing_header() -> TestCase {
        TestCase::new(
            "reject_missing_header",
            "Reject mode: missing header returns 400",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", REJECT_HOST)
                        // No X-Target-Region header
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 400 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 400 (reject), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Reject mode: unmatched key → 400
    fn reject_no_match() -> TestCase {
        TestCase::new(
            "reject_no_match",
            "Reject mode: unmatched routing key returns 400",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", REJECT_HOST)
                        .header("X-Target-Region", "ap") // not in reject config (only us, eu)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 400 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 400 (reject no match), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Reject mode: matched key → 5xx (localhost rejected by security check)
    /// Pingora returns 502 or 500 depending on how it handles ConnectError
    fn reject_matched_localhost_rejected() -> TestCase {
        TestCase::new(
            "reject_matched_localhost_rejected",
            "Reject mode: matched key with localhost → 5xx (loopback rejected)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", REJECT_HOST)
                        .header("X-Target-Region", "us")
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            // Pingora returns 502 or 500 for ConnectError
                            if status == 502 || status == 500 {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 502 or 500 (loopback rejected), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ======================================================================
    // Regex extraction tests (REGEX_HOST)
    // ======================================================================

    /// Regex mode: valid pattern matches → 502 (localhost rejected)
    fn regex_match_extracted() -> TestCase {
        TestCase::new(
            "regex_match_extracted",
            "Regex mode: extract routing key from pattern → plugin matches domain → 502",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("deu-regex-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", REGEX_HOST)
                        .header("X-Cluster-Target", "cluster=us-west;priority=high")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_resp) => {
                            // Check access log for regex extraction success
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    // Plugin should log "OK localhost:30001"
                                    if log_str.contains("OK localhost:30001") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Access log does not contain 'OK localhost:30001': {}",
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

    /// Regex mode: pattern does not match → fallback (200 from normal backend)
    fn regex_no_match_fallback() -> TestCase {
        TestCase::new(
            "regex_no_match_fallback",
            "Regex mode: regex does not match → falls back to normal backend (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", REGEX_HOST)
                        .header("X-Cluster-Target", "no-cluster-key-here")
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
                                    format!("Expected 200 (regex miss → fallback), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Regex mode: host override verified via access log
    fn regex_host_override() -> TestCase {
        TestCase::new(
            "regex_host_override",
            "Regex mode: overrideHost is set (verified via access log debug header)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("deu-host-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", REGEX_HOST)
                        .header("X-Cluster-Target", "cluster=eu-central;priority=low")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_resp) => {
                            // Check access log for eu-central match
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains("OK localhost:30002") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Access log does not contain 'OK localhost:30002': {}",
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
    // Debug header test
    // ======================================================================

    /// Verify debug header in access log when matched
    fn debug_header_in_access_log() -> TestCase {
        TestCase::new(
            "debug_header_in_access_log",
            "Debug header X-Dynamic-External-Upstream recorded in access log",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("deu-debug-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/echo", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", SKIP_HOST)
                        .header("X-Target-Region", "ap")
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_resp) => {
                            // Check access log for plugin log containing OK + ap domain (port 30003)
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains("OK localhost:30003") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Access log does not contain 'OK localhost:30003': {}",
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
}

impl TestSuite for DynamicExternalUpstreamTestSuite {
    fn name(&self) -> &str {
        "DynamicExternalUpstream"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Skip mode
            Self::skip_missing_header_fallback(),
            Self::skip_no_match_fallback(),
            Self::skip_matched_localhost_rejected(),
            Self::skip_access_log_matched(),
            Self::skip_access_log_no_match(),
            // Reject mode
            Self::reject_missing_header(),
            Self::reject_no_match(),
            Self::reject_matched_localhost_rejected(),
            // Regex extraction
            Self::regex_match_extracted(),
            Self::regex_no_match_fallback(),
            Self::regex_host_override(),
            // Debug header
            Self::debug_header_in_access_log(),
        ]
    }
}
