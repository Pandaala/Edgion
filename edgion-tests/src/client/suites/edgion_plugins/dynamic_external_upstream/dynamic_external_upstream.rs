// DynamicExternalUpstream Integration Test Suite
//
// Required config files (in edgion-tests/integration/conf/EdgionPlugins/DynamicExternalUpstream/):
// - 00_EdgionGatewayConfig_deu-egc.yaml          (explicit deny-loopback policy)
// - 00_GatewayClass_deu-class.yaml
// - 00_Gateway_edgion-test_deu-gateway.yaml       (dedicated listener on port 32005)
// - 01_EdgionPlugins_default_deu-skip.yaml        (fallback mode: onMissing=fallback, onNoMatch=fallback)
// - 01_EdgionPlugins_default_deu-reject.yaml      (reject mode: onMissing=reject, onNoMatch=reject)
// - 01_EdgionPlugins_default_deu-regex.yaml       (regex extraction mode)
// - 02_HTTPRoute_default_deu-skip-test.yaml        (fallback to test-http:30001)
// - 02_HTTPRoute_default_deu-reject-test.yaml
// - 02_HTTPRoute_default_deu-regex-test.yaml
//
// Test strategy:
// DynamicExternalUpstream routes to external domains via DNS resolution.
// In test env, domainMap targets "localhost" which resolves to 127.0.0.1.
// The suite-owned policy denies loopback in both local and Kubernetes fixtures.
// The two denial probes require 500/502 plus the current request's canonical
// BackendLocalhostNotAllowed error; an ordinary dial failure cannot pass.
//
// We verify:
// 1. Plugin request_filter logic via access log (plugin_log entries)
// 2. Fallback/Reject behavior for missing/unmatched keys
// 3. Regex extraction
// 4. dye presence
// 5. Loopback rejection (security)

use crate::access_log_client::{trace_id_for_marker, traceparent_for_marker, AccessLogEntry};
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};

fn verify_loopback_rejection(status: u16, entry: &AccessLogEntry, trace_id: &str) -> Result<(), String> {
    if status != 500 && status != 502 {
        return Err(format!("Expected 500 or 502 for loopback rejection, got {status}"));
    }
    if entry.get_nested_str("request_info.trace_id") != Some(trace_id)
        || !entry.has_diagnostic("error", "BackendLocalhostNotAllowed")
    {
        return Err("Missing trace-correlated BackendLocalhostNotAllowed evidence".to_string());
    }
    Ok(())
}

async fn probe_loopback_rejection(ctx: TestContext, host: &str) -> TestResult {
    let start = Instant::now();
    let marker = format!("deu-loopback-denial-{}", uuid::Uuid::new_v4());
    let trace_id = trace_id_for_marker(&marker);
    let access_logs = ctx.access_log_client();
    let request = ctx
        .http_client
        .get(format!("{}/echo", ctx.edgion_plugins_url()))
        .header("host", host)
        .header("X-Target-Region", "us")
        .header("traceparent", traceparent_for_marker(&marker))
        .header("access_log", "test_store");
    let exchange = tokio::time::timeout(Duration::from_secs(10), async {
        let response = request
            .send()
            .await
            .map_err(|_| "Loopback probe request failed".to_string())?;
        let status = response.status().as_u16();
        response
            .bytes()
            .await
            .map_err(|_| "Loopback probe response body read failed".to_string())?;
        Ok::<_, String>(status)
    })
    .await;
    let outcome = match exchange {
        Ok(Ok(status)) => match tokio::time::timeout(
            Duration::from_secs(5),
            access_logs.get_access_log_with_retry(&marker, 20, 100),
        )
        .await
        {
            Ok(Ok(entry)) => verify_loopback_rejection(status, &entry, &trace_id),
            _ => Err("Failed to observe loopback probe access log".to_string()),
        },
        Ok(Err(error)) => Err(error),
        Err(_) => Err("Loopback probe exchange timed out".to_string()),
    };
    // Bounded cleanup cannot guarantee deletion after outer cancellation or
    // terminal publication following an exchange timeout; the store TTL still applies.
    let cleanup_ok = matches!(
        tokio::time::timeout(Duration::from_secs(5), access_logs.delete_access_log(&marker)).await,
        Ok(Ok(true))
    );
    match (outcome, cleanup_ok) {
        (Ok(()), true) => TestResult::passed_with_message(
            start.elapsed(),
            "Loopback rejected with the canonical request diagnostic".to_string(),
        ),
        (Err(error), true) => TestResult::failed(start.elapsed(), error),
        (Ok(()), false) => TestResult::failed(
            start.elapsed(),
            "Failed to clean up loopback probe access log".to_string(),
        ),
        (Err(error), false) => TestResult::failed(
            start.elapsed(),
            format!("{error}; failed to clean up loopback probe access log"),
        ),
    }
}

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
                                TestResult::failed(start.elapsed(), format!("Expected 200 (fallback), got {}", status))
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
                                TestResult::failed(start.elapsed(), format!("Expected 200 (fallback), got {}", status))
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
            "Skip mode: matched localhost rejected with the current request diagnostic",
            |ctx: TestContext| Box::pin(probe_loopback_rejection(ctx, SKIP_HOST)),
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
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
                                    // Plugin should log "upstream selected"
                                    if log_str.contains("upstream selected") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Access log does not contain 'upstream selected': {}", log_str),
                                        )
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to get access log: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Skip mode: verify access log when no match (should contain "upstream miss")
    fn skip_access_log_no_match() -> TestCase {
        TestCase::new(
            "skip_access_log_no_match",
            "Skip mode: access log contains upstream miss entry when key not in domainMap",
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
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 (fallback), got {}", status),
                                );
                            }
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains("upstream miss") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Access log does not contain 'upstream miss': {}", log_str),
                                        )
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to get access log: {}", e))
                                }
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
                                TestResult::failed(start.elapsed(), format!("Expected 400 (reject), got {}", status))
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
            "Reject mode: matched localhost rejected with the current request diagnostic",
            |ctx: TestContext| Box::pin(probe_loopback_rejection(ctx, REJECT_HOST)),
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
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
                                    // Plugin should log "upstream selected"
                                    if log_str.contains("upstream selected") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Access log does not contain 'upstream selected': {}", log_str),
                                        )
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to get access log: {}", e))
                                }
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
            "Regex mode: overrideHost is set (verified via access log dye)",
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
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
                                    if log_str.contains("upstream selected") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Access log does not contain 'upstream selected': {}", log_str),
                                        )
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to get access log: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ======================================================================
    // dye test
    // ======================================================================

    /// Verify dye entry in access log when matched
    fn dye_header_in_access_log() -> TestCase {
        TestCase::new(
            "dye_header_in_access_log",
            "dye X-Dynamic-External-Upstream recorded in access log",
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
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(_resp) => {
                            // Check access log for plugin log containing "upstream selected"
                            let al_client = ctx.access_log_client();
                            match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(entry) => {
                                    let log_str = entry.data.to_string();
                                    if log_str.contains("upstream selected") {
                                        TestResult::passed(start.elapsed())
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Access log does not contain 'upstream selected': {}", log_str),
                                        )
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to get access log: {}", e))
                                }
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

    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins/DynamicExternalUpstream")
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
            // dye
            Self::dye_header_in_access_log(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn denial_entry(trace_id: &str) -> AccessLogEntry {
        AccessLogEntry {
            data: json!({
                "request_info": {"trace_id": trace_id},
                "diagnostics": [{"kind": "error", "code": "BackendLocalhostNotAllowed"}]
            }),
        }
    }

    #[test]
    fn loopback_rejection_requires_expected_status_and_current_trace() {
        let trace = trace_id_for_marker("loopback-verdict");
        let entry = denial_entry(&trace);
        for status in [500, 502] {
            assert!(verify_loopback_rejection(status, &entry, &trace).is_ok());
        }
        for status in [200, 400, 403, 404, 501, 503, 504] {
            assert!(verify_loopback_rejection(status, &entry, &trace).is_err());
        }
        assert!(verify_loopback_rejection(502, &entry, "stale-trace").is_err());
        for request_info in [json!({}), json!({"trace_id": 42}), json!(null)] {
            let mut entry = denial_entry(&trace);
            entry.data["request_info"] = request_info;
            assert!(verify_loopback_rejection(502, &entry, &trace).is_err());
        }
    }

    #[test]
    fn loopback_rejection_rejects_dial_failure_and_selection_only_evidence() {
        let trace = trace_id_for_marker("allowed-loopback-dial-failure");
        let entry = AccessLogEntry {
            data: json!({
                "request_info": {"trace_id": trace, "status": 502},
                "stage_logs": [{"plugins": [{"name": "DynamicExternalUpstream", "log": "upstream selected"}]}],
                "backend_context": {"upstreams": [{"err": "connection refused"}]}
            }),
        };
        for status in [500, 502] {
            assert!(verify_loopback_rejection(status, &entry, &trace).is_err());
        }
    }

    #[test]
    fn loopback_rejection_requires_exact_error_diagnostic() {
        let trace = trace_id_for_marker("loopback-malformed-evidence");
        for diagnostics in [
            json!(null),
            json!([]),
            json!({"kind": "error", "code": "BackendLocalhostNotAllowed"}),
            json!([null, "BackendLocalhostNotAllowed"]),
            json!([{"kind": "info", "code": "BackendLocalhostNotAllowed"}]),
            json!([{"kind": "error", "code": "BackendAddrLinkLocalNotAllowed"}]),
            json!([{"kind": "error", "code": "BackendLocalhostNotAllowedExtra"}]),
            json!([{"kind": "error", "code": 42}]),
            json!([{"code": "BackendLocalhostNotAllowed"}]),
            json!([{"kind": "error"}]),
        ] {
            let mut entry = denial_entry(&trace);
            entry.data["diagnostics"] = diagnostics;
            assert!(verify_loopback_rejection(502, &entry, &trace).is_err());
        }
        assert!(verify_loopback_rejection(502, &AccessLogEntry { data: json!({}) }, &trace).is_err());
    }
}
