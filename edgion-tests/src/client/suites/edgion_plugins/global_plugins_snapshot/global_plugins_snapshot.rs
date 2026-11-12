// EdgionPlugins GlobalPluginsSnapshot Test Suite
//
// Integration coverage for the unified per-request global plugin snapshot
// (EdgionGatewayConfig.globalPluginsRef -> EdgionHttpContext cache).
//
// Configs defined in edgion-tests/integration/conf/EdgionPlugins/GlobalPluginsSnapshot/:
//   01 EdgionPlugins gp-resp-header-modifier:
//        upstreamResponseFilterPlugins -> ResponseHeaderModifier add X-Global-Plugin: yes
//   02 EdgionGatewayConfig example-gateway override:
//        globalPluginsRef = [gp-resp-header-modifier]
//   03 HTTPRoute /headers -> test-http:30001 (no route-level filters)
//   04 Gateway global-plugins-missing (private port): edgion.io/edgion-plugins
//        names an EdgionPlugins that does not exist
//   05 HTTPRoute /headers on global-plugins-missing
//   06 Gateway global-plugins-malformed (private port): edgion.io/edgion-plugins
//        holds a malformed reference (invalid object-name characters)
//   07 HTTPRoute /headers on global-plugins-malformed
//
// Cases implemented:
//   * Response header injection - upstream_response_filter runs and headers land on the wire.
//   * Missing Gateway-level plugin target fails closed - 500 instead of serving the
//     request without the plugin (gw-core-02). A dangling global reference is not
//     placed on the shared example-gateway config because it would deny every suite.
//   * Malformed Gateway-level plugin reference fails closed - the invalid slot is kept
//     and denies with 500 instead of being skipped (gw-core-02b).
//
// Not yet covered: big-body invariance and onion ordering across route + global plugins
// (would require an additional EdgionPlugins resource with a route-level ExtensionRef).

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::port_config::{PortConfig, PortKind};
use std::time::Instant;

const HOST: &str = "global-plugins-snapshot.example.com";
const MISSING_GATEWAY_HOST: &str = "global-plugins-missing.example.com";
const MISSING_GATEWAY_PORT_KEY: &str = "EdgionPlugins/GlobalPluginsSnapshot/MissingGatewayPlugins";
const MALFORMED_GATEWAY_HOST: &str = "global-plugins-malformed.example.com";
const MALFORMED_GATEWAY_PORT_KEY: &str = "EdgionPlugins/GlobalPluginsSnapshot/MalformedGatewayPlugins";

pub struct GlobalPluginsSnapshotTestSuite;

fn header_val(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn assert_header_eq(resp: &reqwest::Response, name: &str, expected: &str) -> Result<(), String> {
    match header_val(resp, name) {
        Some(v) if v == expected => Ok(()),
        Some(v) => Err(format!("Expected {}={}, got {}", name, expected, v)),
        None => Err(format!("Missing header {}, expected {}", name, expected)),
    }
}

impl GlobalPluginsSnapshotTestSuite {
    // Verify that global upstreamResponseFilterPlugins runs and the response header is set.
    fn test_global_response_header_injected() -> TestCase {
        TestCase::new(
            "global_plugins_snapshot_response_header_injected",
            "Global ResponseHeaderModifier adds X-Global-Plugin to the response",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .get(format!("{}/headers", ctx.http_url()))
                        .header("host", HOST)
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", resp.status()),
                                );
                            }
                            if let Err(e) = assert_header_eq(&resp, "x-global-plugin", "yes") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // gw-core-02: a Gateway-level edgion.io/edgion-plugins reference whose
    // EdgionPlugins target does not exist must fail closed (500) instead of
    // serving the request without the Gateway-level plugin list.
    fn test_missing_gateway_plugins_ref_fails_closed() -> TestCase {
        Self::gateway_plugins_ref_fails_closed(
            "global_plugins_snapshot_missing_gateway_ref_fails_closed",
            "Gateway-level plugin reference to a missing EdgionPlugins denies the request with 500",
            MISSING_GATEWAY_PORT_KEY,
            MISSING_GATEWAY_HOST,
        )
    }

    // gw-core-02b: a malformed Gateway-level reference is not dropped before
    // resolution; its slot fails closed exactly like a missing target.
    fn test_malformed_gateway_plugins_ref_fails_closed() -> TestCase {
        Self::gateway_plugins_ref_fails_closed(
            "global_plugins_snapshot_malformed_gateway_ref_fails_closed",
            "Malformed Gateway-level plugin reference denies the request with 500",
            MALFORMED_GATEWAY_PORT_KEY,
            MALFORMED_GATEWAY_HOST,
        )
    }

    // Shared assertion: a logged 500 with no upstream attempt, denied by the
    // configuration gate with the MissingEdgionPlugins token.
    fn gateway_plugins_ref_fails_closed(
        name: &'static str,
        description: &'static str,
        port_key: &'static str,
        host: &'static str,
    ) -> TestCase {
        TestCase::new(name, description, move |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let port = match PortConfig::load().and_then(|ports| ports.require_port(port_key, PortKind::Http)) {
                    Ok(port) => port,
                    Err(e) => return TestResult::failed(start.elapsed(), format!("Port lookup failed: {}", e)),
                };
                let marker = format!("gp-ref-{}", uuid::Uuid::new_v4());
                let resp = ctx
                    .http_client
                    .get(format!("http://{}:{port}/headers", ctx.target_host))
                    .header("host", host)
                    .header("access_log", "test_store")
                    .header("traceparent", crate::access_log_client::traceparent_for_marker(&marker))
                    .send()
                    .await;

                let status = match resp {
                    Ok(resp) => resp.status(),
                    Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                };
                if status != 500 {
                    return TestResult::failed(start.elapsed(), format!("Expected fail-closed 500, got {}", status));
                }
                // A bare 500 could also come from an upstream or backendRef error; the
                // access log proves the missing-reference gate denied before upstream.
                let log = match ctx
                    .access_log_client()
                    .get_access_log_with_retry(&marker, 15, 200)
                    .await
                {
                    Ok(log) => log,
                    Err(e) => return TestResult::failed(start.elapsed(), format!("Access log lookup failed: {}", e)),
                };
                if log.status_code() != Some(500) || log.upstream_count() != 0 {
                    return TestResult::failed(
                        start.elapsed(),
                        format!(
                            "Expected logged 500 with no upstream, got status {:?} upstreams {}",
                            log.status_code(),
                            log.upstream_count()
                        ),
                    );
                }
                let gate_tokens = log.data["stage_logs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|stage| stage["stage"] == "request_filters")
                    .filter_map(|stage| stage["filters"].as_array())
                    .flatten()
                    .find(|filter| filter["name"] == "RejectInvalidConfig")
                    .and_then(|filter| filter["log"].as_array());
                match gate_tokens {
                    Some(tokens) if tokens.iter().any(|token| token == "err: MissingEdgionPlugins") => {
                        TestResult::passed(start.elapsed())
                    }
                    _ => TestResult::failed(
                        start.elapsed(),
                        "Configuration gate did not record MissingEdgionPlugins".to_string(),
                    ),
                }
            })
        })
    }
}

impl TestSuite for GlobalPluginsSnapshotTestSuite {
    fn name(&self) -> &str {
        "GlobalPluginsSnapshot Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let mut cases = vec![Self::test_global_response_header_injected()];
        // The private fail-closed Gateways (04-07 fixtures, ports 32007/32008) exist
        // only in the local integration configuration, not in the Kubernetes fixtures.
        if std::env::var("EDGION_TEST_K8S_MODE").as_deref() != Ok("true") {
            cases.push(Self::test_missing_gateway_plugins_ref_fails_closed());
            cases.push(Self::test_malformed_gateway_plugins_ref_fails_closed());
        }
        cases
    }
}
