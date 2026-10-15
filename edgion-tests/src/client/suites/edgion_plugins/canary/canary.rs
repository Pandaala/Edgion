// Canary Plugin Test Suite
//
// Covers:
// - Base-only (no override): full traffic goes to `activeProfile`'s backend (stable, port 30001)
// - dye: request X-Canary-Active carries the resolved profile name (asserted via echo body)
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
//   10_EdgionPlugins_default_rlr-dye-fail-close.yaml
//   11_HTTPRoute_default_rlr-dye-fail-close-test.yaml
//   12_EdgionPlugins_default_rlr-dye-fail-open.yaml
//   13_HTTPRoute_default_rlr-dye-fail-open-test.yaml
//   14_EdgionPlugins_default_canary-shared-dye-name.yaml
//   15_HTTPRoute_default_canary-shared-dye-name-test.yaml
//   16_EdgionPlugins_default_canary-dye-conditions.yaml
//   17_HTTPRoute_default_canary-dye-conditions-test.yaml
//   18_EdgionPlugins_default_canary-dye-template.yaml
//   19_HTTPRoute_default_canary-dye-template-test.yaml
//   20_EdgionPlugins_default_canary-kill-switch-strip.yaml
//   21_HTTPRoute_default_canary-kill-switch-strip-test.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct CanaryTestSuite;

const CANARY_HOST: &str = "canary-test.example.com";
const INVALID_REJECT_HOST: &str = "canary-invalid-reject.example.com";
const INVALID_FALLBACK_HOST: &str = "canary-invalid-fallback.example.com";
const DYE_LOCAL_REPLY_HOST: &str = "canary-dye-local-reply.example.com";
const RLR_DYE_FAIL_CLOSE_HOST: &str = "rlr-dye-fail-close.example.com";
const RLR_DYE_FAIL_OPEN_HOST: &str = "rlr-dye-fail-open.example.com";
const SHARED_DYE_NAME_HOST: &str = "canary-shared-dye.example.com";
const DYE_CONDITIONS_HOST: &str = "canary-dye-conditions.example.com";
const DYE_TEMPLATE_HOST: &str = "canary-dye-template.example.com";
const KILL_SWITCH_STRIP_HOST: &str = "canary-kill-switch-strip.example.com";

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

async fn observe_spoofed_dye_response(response: reqwest::Response) -> Result<(), &'static str> {
    if response.status().as_u16() != 200 {
        return Err("Expected HTTP 200 for the Canary echo observation");
    }
    let body = response
        .text()
        .await
        .map_err(|_| "Canary echo response body read failed")?;
    if extract_server_port(&body) != Some(30001)
        || !["Method: GET", "Path: /echo", "Headers:"]
            .iter()
            .all(|expected| body.lines().any(|line| line == *expected))
    {
        return Err("Canary response did not identify the expected backend echo");
    }
    if body.contains("x-canary-active") {
        return Err("Spoofed x-canary-active leaked to upstream");
    }
    Ok(())
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

    /// dye: the resolved profile name reaches the upstream as
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
            "dye request entry X-Canary-Active carries resolved profile 'stable'",
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
                        Ok(response) => match observe_spoofed_dye_response(response).await {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(message) => TestResult::failed(start.elapsed(), message.to_string()),
                        },
                        Err(_) => TestResult::failed(start.elapsed(), "Canary echo request failed".to_string()),
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

    /// Self-reject dye (ErrTerminateRequest path): RateLimitRedis, unreachable
    /// Redis, onRedisFailure: Deny. The plugin self-writes its 429 reject
    /// reply via `write_response_header` + `write_response_body`, and the
    /// entry-level dye response rule (recorded BEFORE that self-write) must
    /// survive the drain -- pins Task 10's `write_response_header` drain
    /// amendment.
    fn rate_limit_redis_self_reject_dye_fail_close() -> TestCase {
        TestCase::new(
            "rate_limit_redis_self_reject_dye_fail_close",
            "RateLimitRedis fail-close self-reject reply carries the fail-close outcome header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", RLR_DYE_FAIL_CLOSE_HOST)
                        .header("X-Rate-Key", "dye-fail-close-test-key")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 429 {
                                return TestResult::failed(start.elapsed(), format!("Expected 429, got {}", status));
                            }
                            match response.headers().get("X-Edgion-Outcome").and_then(|v| v.to_str().ok()) {
                                Some("rate_limit_redis:fail-close:redis-unreachable") => TestResult::passed(start.elapsed()),
                                other => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected X-Edgion-Outcome: rate_limit_redis:fail-close:redis-unreachable, got {:?}",
                                        other
                                    ),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Fail-open dye: RateLimitRedis, unreachable Redis, onRedisFailure: Allow.
    /// The request proceeds to a normal 200 upstream response, which must
    /// carry the appended fail-open outcome header.
    fn rate_limit_redis_fail_open_dye_header() -> TestCase {
        TestCase::new(
            "rate_limit_redis_fail_open_dye_header",
            "RateLimitRedis fail-open lets the request through with the fail-open outcome header appended",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", RLR_DYE_FAIL_OPEN_HOST)
                        .header("X-Rate-Key", "dye-fail-open-test-key")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            match response.headers().get("X-Edgion-Outcome").and_then(|v| v.to_str().ok()) {
                                Some("rate_limit_redis:fail-open:redis-unreachable") => TestResult::passed(start.elapsed()),
                                other => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected X-Edgion-Outcome: rate_limit_redis:fail-open:redis-unreachable, got {:?}",
                                        other
                                    ),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Shared name (append order): Canary (fallback branch, misconfigured
    /// profile) and RateLimitRedis (fail-open, unreachable Redis) both
    /// record to X-Edgion-Outcome. Both must appear, in the entries' list
    /// (execution) order.
    fn shared_dye_name_appends_in_execution_order() -> TestCase {
        TestCase::new(
            "shared_dye_name_appends_in_execution_order",
            "Two plugin entries recording to the same header name append two values in execution order",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", SHARED_DYE_NAME_HOST)
                        .header("X-Rate-Key", "shared-dye-test-key")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            let values: Vec<String> = response
                                .headers()
                                .get_all("X-Edgion-Outcome")
                                .iter()
                                .filter_map(|v| v.to_str().ok().map(|s| s.to_string()))
                                .collect();
                            let expected = vec![
                                "canary:fallback:on-invalid".to_string(),
                                "rate_limit_redis:fail-open:redis-unreachable".to_string(),
                            ];
                            if values == expected {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected X-Edgion-Outcome values {:?} in order, got {:?}",
                                        expected, values
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

    /// Conditions-gated dye: the response rule only renders when the run
    /// condition (keyExist header X-Debug-Token) is satisfied.
    fn conditions_gated_dye_present_with_token() -> TestCase {
        TestCase::new(
            "conditions_gated_dye_present_with_token",
            "Debug outcome header renders when X-Debug-Token is present",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/canary/api", ctx.http_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", DYE_CONDITIONS_HOST)
                        .header("X-Debug-Token", "1")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            match response.headers().get("X-Debug-Outcome").and_then(|v| v.to_str().ok()) {
                                Some("stable") => TestResult::passed(start.elapsed()),
                                other => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected X-Debug-Outcome: stable, got {:?}", other),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Conditions-gated dye, negative case: without X-Debug-Token, the
    /// condition-skipped rule never renders (conditions gate rendering, not
    /// the (response-direction, so N/A here) strip).
    fn conditions_gated_dye_absent_without_token() -> TestCase {
        TestCase::new(
            "conditions_gated_dye_absent_without_token",
            "Debug outcome header is absent when X-Debug-Token is missing",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/canary/api", ctx.http_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", DYE_CONDITIONS_HOST)
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            match response.headers().get("X-Debug-Outcome") {
                                None => TestResult::passed(start.elapsed()),
                                Some(v) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected X-Debug-Outcome absent, got {:?}", v),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Templated value: the response rule's `value` is an operator template
    /// (`${header:X-Account-Id}`), rendered on the `fallback` outcome from
    /// the live request's own header.
    fn templated_dye_value_renders_live_header() -> TestCase {
        TestCase::new(
            "templated_dye_value_renders_live_header",
            "Templated dye value renders the live X-Account-Id request header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/canary/api", ctx.http_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", DYE_TEMPLATE_HOST)
                        .header("X-Account-Id", "acct-42")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            match response
                                .headers()
                                .get("X-Fallback-Reason")
                                .and_then(|v| v.to_str().ok())
                            {
                                Some("fallback for acct-42") => TestResult::passed(start.elapsed()),
                                other => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected X-Fallback-Reason: 'fallback for acct-42', got {:?}", other),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Strip hardening: Canary's own kill switch (config.enable: false)
    /// never reaches the dye-recording code paths, but the runtime-owned
    /// anti-spoof strip runs before that kill switch is even checked, so a
    /// client-supplied X-Canary-Active must still be removed.
    fn spoofed_dye_header_stripped_when_kill_switched() -> TestCase {
        TestCase::new(
            "spoofed_dye_header_stripped_when_kill_switched",
            "Client-sent X-Canary-Active is stripped even when the declaring plugin is kill-switched",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/echo", ctx.http_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", KILL_SWITCH_STRIP_HOST)
                        .header("X-Canary-Active", "spoofed")
                        .send()
                        .await
                    {
                        Ok(response) => match observe_spoofed_dye_response(response).await {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(message) => TestResult::failed(start.elapsed(), message.to_string()),
                        },
                        Err(_) => TestResult::failed(start.elapsed(), "Canary echo request failed".to_string()),
                    }
                })
            },
        )
    }

    /// Makes the Canary onInvalid=fallback branch directly observable: the
    /// response rule added to 06_EdgionPlugins_default_canary-invalid-fallback.yaml
    /// renders `canary:fallback:on-invalid` (spec's motivating example for
    /// the failure-dye vocabulary).
    fn fallback_dye_outcome_observable_on_invalid_fallback() -> TestCase {
        TestCase::new(
            "fallback_dye_outcome_observable_on_invalid_fallback",
            "onInvalid=fallback branch renders X-Canary-Outcome: canary:fallback:on-invalid",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/canary/api", ctx.http_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", INVALID_FALLBACK_HOST)
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            match response.headers().get("X-Canary-Outcome").and_then(|v| v.to_str().ok()) {
                                Some("canary:fallback:on-invalid") => TestResult::passed(start.elapsed()),
                                other => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected X-Canary-Outcome: canary:fallback:on-invalid, got {:?}", other),
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
            Self::rate_limit_redis_self_reject_dye_fail_close(),
            Self::rate_limit_redis_fail_open_dye_header(),
            Self::shared_dye_name_appends_in_execution_order(),
            Self::conditions_gated_dye_present_with_token(),
            Self::conditions_gated_dye_absent_without_token(),
            Self::templated_dye_value_renders_live_header(),
            Self::spoofed_dye_header_stripped_when_kill_switched(),
            Self::fallback_dye_outcome_observable_on_invalid_fallback(),
        ]
    }
}

#[cfg(test)]
mod observation_tests {
    use super::observe_spoofed_dye_response;

    use bytes::Bytes;
    use futures::stream;
    use http::Response as HttpResponse;
    use reqwest::{Body, Response};
    use std::io;

    fn response(status: u16, body: &str) -> Response {
        HttpResponse::builder()
            .status(status)
            .body(body.to_string())
            .unwrap()
            .into()
    }

    fn interrupted_response(status: u16, prefix: &str) -> Response {
        let chunks = [
            Ok(Bytes::copy_from_slice(prefix.as_bytes())),
            Err(io::Error::other("sensitive-observation-sentinel")),
        ];
        HttpResponse::builder()
            .status(status)
            .body(Body::wrap_stream(stream::iter(chunks)))
            .unwrap()
            .into()
    }

    const ECHO: &str = "Server: 0.0.0.0:30001\nClient: 127.0.0.1:40000\nMethod: GET\nPath: /echo\n\nHeaders:\n  host: canary-test.example.com\n";

    #[tokio::test]
    async fn canary_requires_the_completed_expected_echo() {
        assert_eq!(observe_spoofed_dye_response(response(200, ECHO)).await, Ok(()));
        for body in [
            String::new(),
            "unrelated success".to_string(),
            ECHO.replace("30001", "30002"),
            ECHO.replace("Method: GET", "Method: POST"),
            ECHO.replace("Path: /echo", "Path: /other"),
            ECHO.replace("Headers:", "Other:"),
        ] {
            assert!(observe_spoofed_dye_response(response(200, &body)).await.is_err());
        }
        assert!(observe_spoofed_dye_response(response(500, ECHO)).await.is_err());
    }

    #[tokio::test]
    async fn canary_rejects_a_completed_spoofed_header_without_disclosing_the_body() {
        let leaked = format!("{ECHO}  x-canary-active: sensitive-observation-sentinel\n");
        assert_eq!(
            observe_spoofed_dye_response(response(200, &leaked)).await,
            Err("Spoofed x-canary-active leaked to upstream"),
        );
    }

    #[tokio::test]
    async fn canary_read_errors_fail_even_after_valid_echo_bytes() {
        for prefix in ["", ECHO] {
            assert_eq!(
                observe_spoofed_dye_response(interrupted_response(200, prefix)).await,
                Err("Canary echo response body read failed"),
            );
        }
    }
}
