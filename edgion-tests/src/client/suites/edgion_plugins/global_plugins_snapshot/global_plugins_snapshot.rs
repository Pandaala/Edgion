// EdgionPlugins GlobalPluginsSnapshot Test Suite
//
// Integration coverage for the unified per-request global plugin snapshot
// (EdgionGatewayConfig.globalPluginsRef -> EdgionHttpContext cache).
//
// Configs defined in examples/test/conf/EdgionPlugins/GlobalPluginsSnapshot/:
//   01 EdgionPlugins gp-resp-header-modifier:
//        upstreamResponseFilterPlugins -> ResponseHeaderModifier add X-Global-Plugin: yes
//   02 EdgionGatewayConfig example-gateway override:
//        globalPluginsRef = [gp-resp-header-modifier, ghost (dangling)]
//   03 HTTPRoute /headers -> test-http:30001 (no route-level filters)
//
// Cases implemented:
//   * D3-A response header injection - the new sync upstream_response_filter
//     invocation actually runs and headers land on the wire.
//   * Dangling global ref tolerated - 200 still served, X-Global-Plugin still
//     applied; the missing "ghost" ref is logged once via tracing::warn! by
//     the resolver (verified via gateway logs externally; this case asserts
//     graceful behavior end-to-end).
//
// Not yet covered (deferred): big-body invariance and onion ordering across
// route + global plugins. Adding those would require additional EdgionPlugins
// resources and a route-level ExtensionRef wired up; out of scope for this
// minimum-viable scenario.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const HOST: &str = "global-plugins-snapshot.example.com";

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
    // D3-A: global EdgionPlugins.upstreamResponseFilterPlugins runs and sets
    // a response header. Pre-fix this header was silently dropped because
    // pg_upstream_response_filter never invoked global plugins at all.
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

    // Task 14: dangling globalPluginsRef entry. The "ghost" plugin name does
    // not exist; the resolver should warn once and continue. The valid
    // "gp-resp-header-modifier" ref must still take effect, so we assert both
    // 200 and the X-Global-Plugin header present.
    fn test_dangling_global_ref_tolerated() -> TestCase {
        TestCase::new(
            "global_plugins_snapshot_dangling_ref_tolerated",
            "Request with dangling globalPluginsRef still succeeds and surviving plugins still apply",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .get(format!("{}/headers", ctx.http_url()))
                        .header("host", HOST)
                        .header("x-test-trace", "dangling-ref")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            let status = resp.status();
                            if status.is_server_error() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Dangling ref produced 5xx: {}", status),
                                );
                            }
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
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
}

impl TestSuite for GlobalPluginsSnapshotTestSuite {
    fn name(&self) -> &str {
        "GlobalPluginsSnapshot Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_global_response_header_injected(),
            Self::test_dangling_global_ref_tolerated(),
        ]
    }
}
