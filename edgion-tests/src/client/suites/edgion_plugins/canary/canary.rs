// Canary Plugin Test Suite
//
// Covers:
// - Base-only (no override): full traffic goes to `activeProfile`'s backend (stable, port 30001)
// - dyeHeaders: request X-Canary-Active carries the resolved profile name (asserted via echo body)
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
//   08_EdgionPlugins_default_canary-dye-local-reply.yaml
//   09_HTTPRoute_default_canary-dye-local-reply-test.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct CanaryTestSuite;

const CANARY_HOST: &str = "canary-test.example.com";
const INVALID_REJECT_HOST: &str = "canary-invalid-reject.example.com";
const INVALID_FALLBACK_HOST: &str = "canary-invalid-fallback.example.com";
const DYE_LOCAL_REPLY_HOST: &str = "canary-dye-local-reply.example.com";

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

    /// dyeHeaders: the resolved profile name reaches the upstream as
    /// X-Canary-Active (value omitted in config => resolved selection value).
    ///
    /// Uses /echo rather than /canary/api: the test-server route for
    /// /canary/api has no dedicated handler and falls through to
    /// catch_all_handler, which title-cases header names for display
    /// (e.g. "X-Canary-Active") instead of the lowercase form echo_handler
    /// renders headers in. See 05_HTTPRoute_default_canary-test.yaml — the
    /// route has no explicit path match (defaults to a "/" prefix), so
    /// /echo reaches the same backend/plugin chain as /canary/api.
    fn dye_header_carries_active_profile() -> TestCase {
        TestCase::new(
            "dye_header_carries_active_profile",
            "dyeHeaders request entry X-Canary-Active carries resolved profile 'stable'",
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
                            if body.contains("x-canary-active: stable") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("x-canary-active: stable not found in echo body: {}", body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Anti-spoofing: on a fallback (non-dyed) path, a client-supplied header
    /// with the configured dye name must be stripped, not forwarded.
    ///
    /// Uses /echo for the same reason as dye_header_carries_active_profile
    /// above (catch_all_handler title-cases header names for /canary/api,
    /// which would defeat a lowercase substring check either way).
    fn spoofed_dye_header_stripped_on_fallback() -> TestCase {
        TestCase::new(
            "spoofed_dye_header_stripped_on_fallback",
            "Client-sent X-Canary-Active is stripped on the onInvalid=fallback path",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", INVALID_FALLBACK_HOST)
                        .header("X-Canary-Active", "spoofed")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            let body = response.text().await.unwrap_or_default();
                            if body.contains("x-canary-active") {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Spoofed x-canary-active leaked to upstream: {}", body),
                                )
                            } else {
                                TestResult::passed(start.elapsed())
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Pending-op semantic pin: a response dye queued by a successful Canary
    /// selection must survive onto the local 403 reply produced by a LATER
    /// request-stage plugin (FaultInjection abort), since that reply is
    /// produced via the standard ErrResponse termination path which drains
    /// the pending response-header-op queue.
    fn response_dye_survives_local_error_reply() -> TestCase {
        TestCase::new(
            "response_dye_survives_local_error_reply",
            "Queued response dye X-Served-By appears on a later plugin's local 403 reply (FaultInjection abort)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/canary/api", ctx.http_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", DYE_LOCAL_REPLY_HOST)
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 403 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 403 (FaultInjection abort), got {}", status),
                                );
                            }
                            match response.headers().get("X-Served-By").and_then(|v| v.to_str().ok()) {
                                Some("edge-1") => TestResult::passed(start.elapsed()),
                                other => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected X-Served-By: edge-1 on the 403 local reply, got {:?}", other),
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

impl TestSuite for CanaryTestSuite {
    fn name(&self) -> &str {
        "Canary Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::base_routes_to_stable(),
            Self::invalid_profile_rejects_with_403(),
            Self::invalid_profile_falls_back_to_route_backend(),
            Self::dye_header_carries_active_profile(),
            Self::spoofed_dye_header_stripped_on_fallback(),
            Self::response_dye_survives_local_error_reply(),
        ]
    }
}
