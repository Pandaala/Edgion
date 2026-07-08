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
/// `expected_port` is passed explicitly by each non-generic wrapper below rather than
/// captured by a closure, since `TestCase::new` requires a plain `fn` pointer (closures
/// can only coerce to `fn` if they capture nothing).
async fn check_routes_to(ctx: TestContext, expected_port: u16) -> TestResult {
    let start = Instant::now();
    let client = &ctx.http_client;
    // ctx.http_url() defaults to the test_server backend (30001) under dynamic-scenario
    // mode (ports.json has no "default" suite entry) — bypasses the gateway entirely.
    // Hardcode the edgion-plugins-gateway listener port, matching the static Canary
    // suite and other dynamic suites (e.g. basic_auth/dynamic.rs).
    let url = format!("http://{}:31180/canary/api", ctx.target_host);
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
        vec![Self::hotswap_initial_stable()]
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
}

impl TestSuite for CanaryHotSwapFinalTestSuite {
    fn name(&self) -> &str {
        "Canary HotSwap (final)"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::hotswap_final_canary()]
    }
}
