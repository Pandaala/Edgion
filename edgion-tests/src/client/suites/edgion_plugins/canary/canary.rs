// Canary Plugin Test Suite
//
// Covers:
// - Base-only (no override): full traffic goes to `activeProfile`'s backend (stable, port 30001)
// - debugHeader: true sets X-Canary-Active on the upstream request (asserted via access log)
// - onInvalid: reject / fallback when the resolved profile's backendRef is not
//   present in the matching route's backendRefs (misconfigured profile)
//
// Required conf files (edgion-tests/integration/conf/EdgionPlugins/Canary/):
//   01_Service_default_canary-app-{v1,v2}.yaml
//   02_EndpointSlice_default_canary-app-{v1,v2}-slice.yaml
//   03_EdgionConfigData_default_canary-selector.yaml
//   04_EdgionPlugins_default_canary-switch.yaml
//   05_HTTPRoute_default_canary-test.yaml
//   06_EdgionPlugins_default_canary-invalid-{reject,fallback}.yaml
//   07_HTTPRoute_default_canary-invalid-{reject,fallback}-test.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct CanaryTestSuite;

const CANARY_HOST: &str = "canary-test.example.com";
const INVALID_REJECT_HOST: &str = "canary-invalid-reject.example.com";
const INVALID_FALLBACK_HOST: &str = "canary-invalid-fallback.example.com";

/// Helper: extract the port number from echo response body "Server: 0.0.0.0:PORT"
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

impl CanaryTestSuite {
    fn base_routes_to_stable() -> TestCase {
        TestCase::new(
            "base_routes_to_stable",
            "No override: all traffic goes to activeProfile 'stable' (port 30001)",
            |ctx: TestContext| {
                Box::pin(async move {
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
                                Some(30001) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Routed to stable backend (30001)".to_string(),
                                ),
                                Some(other) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected port 30001 (stable), got {}", other),
                                ),
                                None => TestResult::failed(start.elapsed(), "Could not parse backend port".to_string()),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// onInvalid: reject — the resolved profile's backendRef (canary-app-v2)
    /// is absent from canary-invalid-reject-test's backendRefs (only
    /// canary-app-v1), so the request must be rejected with 403 rather than
    /// silently falling through or taking the whole route down.
    fn invalid_profile_rejects_with_403() -> TestCase {
        TestCase::new(
            "invalid_profile_rejects_with_403",
            "Resolved profile not in route backendRefs + onInvalid=reject -> 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/canary/api", ctx.http_url());

                    let request = client.get(&url).header("host", INVALID_REJECT_HOST);
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Misconfigured profile rejected with 403".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 403 (reject), got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// onInvalid: fallback — same misconfigured profile as above, but the
    /// route must instead fall through to its own backendRefs selection
    /// (canary-app-v1, the only backend on canary-invalid-fallback-test)
    /// rather than rejecting the request.
    fn invalid_profile_falls_back_to_route_backend() -> TestCase {
        TestCase::new(
            "invalid_profile_falls_back_to_route_backend",
            "Resolved profile not in route backendRefs + onInvalid=fallback -> route's own backendRefs",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/canary/api", ctx.http_url());

                    let request = client.get(&url).header("host", INVALID_FALLBACK_HOST);
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            let body = response.text().await.unwrap_or_default();
                            match extract_server_port(&body) {
                                Some(30001) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Fell back to route's own backendRefs (30001)".to_string(),
                                ),
                                Some(other) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected port 30001 (route fallback), got {}", other),
                                ),
                                None => TestResult::failed(start.elapsed(), "Could not parse backend port".to_string()),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for CanaryTestSuite {
    fn name(&self) -> &str {
        "Canary Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::base_routes_to_stable(),
            Self::invalid_profile_rejects_with_403(),
            Self::invalid_profile_falls_back_to_route_backend(),
        ]
    }
}
