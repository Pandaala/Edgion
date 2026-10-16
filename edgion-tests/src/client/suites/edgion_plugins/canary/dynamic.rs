// Canary hot-swap dynamic phase suites.
//
// Initial: activeProfile=stable, no Selector override applied yet (Selector.active=stable) → routes to port 30001.
// Final:   Selector.active flipped to "canary" (only the EdgionConfigData changes) → routes to port 30002.
//
// Required conf files (edgion-tests/integration/conf/EdgionPlugins/Canary/HotSwap/DynamicTest/):
//   initial/Service_default_canary-hotswap-{v1,v2}.yaml
//   initial/EndpointSlice_default_canary-hotswap-{v1,v2}-slice.yaml
//   initial/EdgionConfigData_default_canary-hotswap-selector.yaml
//   initial/EdgionPlugins_default_canary-hotswap-switch.yaml
//   initial/HTTPRoute_default_canary-hotswap-test.yaml
//   updates/flip_to_v2/EdgionConfigData_default_canary-hotswap-selector.yaml
//   updates/flip_to_v2/EdgionPlugins_default_canary-hotswap-switch.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const CANARY_HOST: &str = "canary-hotswap-test.example.com";

fn extract_server_port(body: &str) -> Option<u16> {
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("Server: ") {
            if let Some(port_str) = rest.rsplit(':').next() {
                return port_str.trim().parse().ok();
            }
        }
    }
    None
}

/// Send one request to the hot-swap route and check the routed backend port.
/// `expected_port` is passed explicitly by each non-generic wrapper below. That
/// shape predates `TestCase::new` accepting capturing closures; it is kept as-is
/// because the wrappers are also the suite's test-case names.
async fn check_routes_to(ctx: TestContext, expected_port: u16) -> TestResult {
    let start = Instant::now();
    let client = &ctx.http_client;
    let url = format!("{}/canary/api", ctx.http_url());
    let request = client.get(&url).header("host", CANARY_HOST);
    match request.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            if status != 200 {
                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
            }
            let body = response.text().await.unwrap_or_default();
            match extract_server_port(&body) {
                Some(p) if p == expected_port => {
                    TestResult::passed_with_message(start.elapsed(), format!("Routed to port {}", p))
                }
                Some(other) => TestResult::failed(
                    start.elapsed(),
                    format!("Expected port {}, got {}", expected_port, other),
                ),
                None => TestResult::failed(start.elapsed(), "Could not parse backend port".to_string()),
            }
        }
        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
    }
}

pub struct CanaryHotSwapInitialTestSuite;

impl CanaryHotSwapInitialTestSuite {
    fn hotswap_initial_stable() -> TestCase {
        TestCase::new(
            "hotswap_initial_stable",
            "Initial: routes to stable (30001)",
            |ctx: TestContext| Box::pin(check_routes_to(ctx, 30001)),
        )
    }
}

impl TestSuite for CanaryHotSwapInitialTestSuite {
    fn name(&self) -> &str {
        "Canary HotSwap (initial)"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::hotswap_initial_stable(),
            TestCase::new(
                "hotswap_initial_dye",
                "Original dye is restored without the release-track header",
                |ctx: TestContext| {
                    Box::pin(async move {
                        let start = Instant::now();
                        match ctx
                            .http_client
                            .get(format!("{}/echo", ctx.http_url()))
                            .header("host", CANARY_HOST)
                            .send()
                            .await
                        {
                            Ok(response) => {
                                let status = response.status().as_u16();
                                match response.text().await {
                                    Ok(body) if status == 200 && original_dye(&body) => {
                                        TestResult::passed(start.elapsed())
                                    }
                                    _ => TestResult::failed(
                                        start.elapsed(),
                                        "Original stable dye missing or release-track dye remains".to_string(),
                                    ),
                                }
                            }
                            Err(error) => {
                                TestResult::failed(start.elapsed(), format!("Original dye request failed: {error}"))
                            }
                        }
                    })
                },
            ),
        ]
    }
}

pub struct CanaryHotSwapFinalTestSuite;

impl CanaryHotSwapFinalTestSuite {
    fn hotswap_final_canary() -> TestCase {
        TestCase::new(
            "hotswap_final_canary",
            "Final: Selector flipped, routes to canary (30002)",
            |ctx: TestContext| Box::pin(check_routes_to(ctx, 30002)),
        )
    }

    /// After the hot swap, the updated dye (new X-Release-Track entry)
    /// must be live and carry the flipped profile name.
    ///
    /// Uses /echo rather than /canary/api for the same reason as the static
    /// Canary suite's dye tests: the test-server route for /canary/api falls
    /// through to catch_all_handler, which title-cases header names, defeating
    /// a lowercase substring check. The HotSwap route has no explicit path
    /// match (defaults to "/" prefix), so /echo reaches the same backend.
    fn hotswap_final_dye_header() -> TestCase {
        TestCase::new(
            "hotswap_final_dye_header",
            "Final: hot-reloaded dye adds X-Release-Track carrying 'canary'",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());
                    match ctx.http_client.get(&url).header("host", CANARY_HOST).send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            let body = response.text().await.unwrap_or_default();
                            if body.contains("x-release-track: canary") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("x-release-track: canary not found in echo body: {}", body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for CanaryHotSwapFinalTestSuite {
    fn name(&self) -> &str {
        "Canary HotSwap (final)"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::hotswap_final_canary(), Self::hotswap_final_dye_header()]
    }
}

fn original_dye(body: &str) -> bool {
    let mut stable = false;
    for line in body.lines() {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("x-release-track") {
                return false;
            }
            if name.trim().eq_ignore_ascii_case("x-canary-active") {
                stable |= value.trim() == "stable";
            }
        }
    }
    stable
}

#[cfg(test)]
mod restoration_tests {
    use super::original_dye;
    #[test]
    fn stable_routing_does_not_mask_stale_dye() {
        assert!(original_dye("x-canary-active: stable\n"));
        assert!(!original_dye("x-canary-active: stable\nx-release-track: stable\n"));
        assert!(!original_dye("x-canary-active: canary\n"));
        assert!(!original_dye("Server: 0.0.0.0:30001\n"));
    }
}
