// Wasm Plugin Test Suite (proxy-wasm host)
//
// Exercises the reference guest artifact `edgion-tests/wasm/artifacts/header_demo.wasm`
// wired at both the request and upstream-response-filter stages via the
// `wasm-demo` EdgionPlugins resource (see
// `edgion-tests/integration/conf/EdgionPlugins/Wasm/`). The conf sets
// `failOpen: false`, so a guest trap fails closed with a 500.
//
// Guest contract:
// - Request phase: header `x-wasm-panic: 1` -> guest traps (host fail-close -> 500);
//   `x-wasm-deny: 1` -> local 403 with body `wasm denied`; otherwise adds the
//   request header `x-wasm-added: yes` (echoed back by the upstream /headers handler).
// - Response phase: adds the response header `x-wasm-resp: yes`.
//
// Test strategy:
// - request_header_added: happy path injects `x-wasm-added: yes` upstream.
// - local_response_deny: `x-wasm-deny: 1` short-circuits with 403 + `wasm denied`.
// - response_header: response carries `x-wasm-resp: yes`.
// - panic_recovery: a trapping request returns 500 (fail-close) AND the very next
//   normal request still succeeds (fresh instance; gateway survived the trap).
// - perf_smoke: 200 sequential happy-path GETs all return 200; report mean/max
//   latency and assert mean < 100ms as a generous regression tripwire.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde_json::Value;
use std::time::Instant;

const HOST: &str = "wasm.example.com";
/// Advanced-scenario route (callout / body observation / tick+shared-data),
/// backed by the `wasm-advanced` EdgionPlugins resource.
const ADV_HOST: &str = "wasm-adv.example.com";
/// The fake auth server the harness runs (start_all.sh --auth-port 30040);
/// `/verify` returns 200 for `Bearer valid-token`, 401 otherwise. The guest
/// reads this origin from the `x-wasm-callout-url` request header and dispatches
/// a callout to `<origin>/verify`. It is in the plugin's `calloutAllowlist`.
const CALLOUT_ORIGIN: &str = "http://127.0.0.1:30040";

fn callout_origin() -> String {
    std::env::var("EDGION_TEST_WASM_CALLOUT_ORIGIN").unwrap_or_else(|_| CALLOUT_ORIGIN.to_string())
}

fn tick_counter(body: &Value, counter_header: &str) -> Result<u64, String> {
    body["headers"][counter_header]
        .as_str()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| format!("Missing or invalid {counter_header} counter"))
}

fn tick_counter_delta(baseline: u64, current: u64) -> Result<u64, String> {
    current
        .checked_sub(baseline)
        .ok_or_else(|| "Wasm tick counter reset".to_string())
}

async fn probe_tick_counter(ctx: &TestContext, marker: &str, counter_header: &str) -> Result<u64, String> {
    let response = ctx
        .http_client
        .get(format!("{}/headers", ctx.http_url()))
        .header("host", ADV_HOST)
        .header(marker, "1")
        .send()
        .await
        .map_err(|_| "Wasm tick probe request failed")?;
    if response.status() != 200 {
        return Err(format!("Expected tick probe HTTP 200, got {}", response.status()));
    }
    let body = response.json::<Value>().await.map_err(|_| "Invalid tick probe JSON")?;
    tick_counter(&body, counter_header)
}

async fn wait_for_tick_progress(ctx: &TestContext, marker: &str, counter_header: &str) -> Result<u64, String> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let baseline = probe_tick_counter(ctx, marker, counter_header).await?;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let current = probe_tick_counter(ctx, marker, counter_header).await?;
            let delta = tick_counter_delta(baseline, current)?;
            if delta > 0 {
                return Ok(delta);
            }
        }
    })
    .await
    .map_err(|_| format!("No fresh {counter_header} progress within 10 seconds"))?
}

pub struct WasmPluginTestSuite;

impl WasmPluginTestSuite {
    // ==================== 1. Request header injected upstream ====================
    fn test_request_header_added() -> TestCase {
        TestCase::new(
            "plugin_wasm_request_header_added",
            "Wasm plugin: request phase adds 'x-wasm-added: yes' visible to the upstream",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client.get(&url).header("host", HOST);

                    match request.send().await {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 OK, got {}", resp.status()),
                                );
                            }

                            match resp.json::<Value>().await {
                                Ok(body) => {
                                    let Some(headers) = body["headers"].as_object() else {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            "Response body missing 'headers' object".to_string(),
                                        );
                                    };

                                    let Some(got) = headers.get("x-wasm-added").and_then(|v| v.as_str()) else {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            "x-wasm-added header not found in upstream echo".to_string(),
                                        );
                                    };

                                    if got != "yes" {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected x-wasm-added='yes', got '{}'", got),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Guest injected request header x-wasm-added=yes".to_string(),
                                    )
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Failed to parse JSON: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 2. Local response (deny) ====================
    fn test_local_response_deny() -> TestCase {
        TestCase::new(
            "plugin_wasm_local_response_deny",
            "Wasm plugin: x-wasm-deny:1 short-circuits with a 403 local response",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client.get(&url).header("host", HOST).header("x-wasm-deny", "1");

                    match request.send().await {
                        Ok(resp) => {
                            let status = resp.status();
                            if status != 403 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 403 Forbidden, got {}", status),
                                );
                            }

                            match resp.text().await {
                                Ok(body) => {
                                    if !body.contains("wasm denied") {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected body to contain 'wasm denied', got '{}'", body),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Guest returned 403 local response with body 'wasm denied'".to_string(),
                                    )
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Failed to read body: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 3. Response header injected ====================
    fn test_response_header() -> TestCase {
        TestCase::new(
            "plugin_wasm_response_header",
            "Wasm plugin: response phase adds 'x-wasm-resp: yes' to the client response",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client.get(&url).header("host", HOST);

                    match request.send().await {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 OK, got {}", resp.status()),
                                );
                            }

                            let got = resp
                                .headers()
                                .get("x-wasm-resp")
                                .and_then(|v| v.to_str().ok())
                                .map(|s| s.to_string());

                            match got {
                                Some(v) if v == "yes" => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Guest injected response header x-wasm-resp=yes".to_string(),
                                ),
                                Some(v) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected x-wasm-resp='yes', got '{}'", v),
                                ),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    "x-wasm-resp header not found in client response".to_string(),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 4. Panic (trap) fails closed, gateway recovers ====================
    fn test_panic_recovery() -> TestCase {
        TestCase::new(
            "plugin_wasm_panic_recovery",
            "Wasm plugin: a trapping request returns 500 (fail-close) and the next request still succeeds",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    // Step 1: trapping request must fail closed with 500.
                    let panic_req = client.get(&url).header("host", HOST).header("x-wasm-panic", "1");
                    match panic_req.send().await {
                        Ok(resp) => {
                            let status = resp.status();
                            if status != 500 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 500 on guest trap (failOpen:false), got {}", status),
                                );
                            }
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Panic request failed: {}", e));
                        }
                    }

                    // Step 2: an immediate normal request must still succeed with a
                    // fresh instance (gateway process survived the trap).
                    let ok_req = client.get(&url).header("host", HOST);
                    match ok_req.send().await {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 on recovery request, got {}", resp.status()),
                                );
                            }

                            match resp.json::<Value>().await {
                                Ok(body) => {
                                    let Some(headers) = body["headers"].as_object() else {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            "Recovery response body missing 'headers' object".to_string(),
                                        );
                                    };

                                    match headers.get("x-wasm-added").and_then(|v| v.as_str()) {
                                        Some("yes") => TestResult::passed_with_message(
                                            start.elapsed(),
                                            "Guest trap returned 500 (fail-close); gateway recovered, next request 200 with x-wasm-added=yes"
                                                .to_string(),
                                        ),
                                        Some(v) => TestResult::failed(
                                            start.elapsed(),
                                            format!("Recovery request: expected x-wasm-added='yes', got '{}'", v),
                                        ),
                                        None => TestResult::failed(
                                            start.elapsed(),
                                            "Recovery request: x-wasm-added header missing (fresh instance not applying plugin)"
                                                .to_string(),
                                        ),
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Failed to parse JSON: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Recovery request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 5. Performance smoke ====================
    fn test_perf_smoke() -> TestCase {
        TestCase::new(
            "plugin_wasm_perf_smoke",
            "Wasm plugin: 200 sequential requests all succeed with acceptable mean latency",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    const ITERATIONS: usize = 200;
                    let mut total_ms: f64 = 0.0;
                    let mut max_ms: f64 = 0.0;

                    for i in 0..ITERATIONS {
                        let req_start = Instant::now();
                        let request = client.get(&url).header("host", HOST);
                        match request.send().await {
                            Ok(resp) => {
                                let status = resp.status();
                                // Drain the body so the connection can be reused.
                                let _ = resp.bytes().await;
                                if status != 200 {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Request {} of {}: expected 200, got {}", i + 1, ITERATIONS, status),
                                    );
                                }
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Request {} of {} failed: {}", i + 1, ITERATIONS, e),
                                );
                            }
                        }
                        let elapsed_ms = req_start.elapsed().as_secs_f64() * 1000.0;
                        total_ms += elapsed_ms;
                        if elapsed_ms > max_ms {
                            max_ms = elapsed_ms;
                        }
                    }

                    let mean_ms = total_ms / ITERATIONS as f64;
                    let summary = format!("{} requests OK; mean {:.2}ms, max {:.2}ms", ITERATIONS, mean_ms, max_ms);

                    if mean_ms >= 100.0 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Mean latency regression: {} (expected mean < 100ms)", summary),
                        );
                    }

                    TestResult::passed_with_message(start.elapsed(), summary)
                })
            },
        )
    }

    // ==================== 6. Callout: authorized token resumes ====================
    fn test_callout_authorized() -> TestCase {
        TestCase::new(
            "plugin_wasm_callout_authorized",
            "Wasm plugin: a callout to the auth server returns 200 for a valid token, guest resumes",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let resp = ctx
                        .http_client
                        .get(&url)
                        .header("host", ADV_HOST)
                        .header("x-wasm-callout-url", callout_origin())
                        .header("authorization", "Bearer valid-token")
                        .send()
                        .await;
                    match resp {
                        Ok(r) if r.status() == 200 => {
                            // The resumed request carries x-wasm-added (set by the callback).
                            match r.json::<Value>().await {
                                Ok(body) if body["headers"]["x-wasm-added"].as_str() == Some("yes") => {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Authorized callout (200) resumed the request".to_string(),
                                    )
                                }
                                Ok(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Resumed request missing x-wasm-added header".to_string(),
                                ),
                                Err(e) => TestResult::failed(start.elapsed(), format!("JSON parse: {}", e)),
                            }
                        }
                        Ok(r) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200 after authorized callout, got {}", r.status()),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 7. Callout: bad token denied ====================
    fn test_callout_denied() -> TestCase {
        TestCase::new(
            "plugin_wasm_callout_denied",
            "Wasm plugin: a callout returning 401 for a bad token makes the guest deny with 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());
                    let resp = ctx
                        .http_client
                        .get(&url)
                        .header("host", ADV_HOST)
                        .header("x-wasm-callout-url", callout_origin())
                        .header("authorization", "Bearer wrong-token")
                        .send()
                        .await;
                    match resp {
                        Ok(r) if r.status() == 403 => {
                            let body = r.text().await.unwrap_or_default();
                            if body.contains("callout denied") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Unauthorized callout (401) denied with 403 'callout denied'".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected body 'callout denied', got '{}'", body),
                                )
                            }
                        }
                        Ok(r) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected 403 after unauthorized callout, got {}", r.status()),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 8. Tick + shared data ====================
    fn test_tick_shared_data() -> TestCase {
        TestCase::new(
            "plugin_wasm_tick_shared_data",
            "Wasm plugin: on_tick increments a shared counter readable by request hooks",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match wait_for_tick_progress(&ctx, "x-wasm-read-ticks", "x-wasm-ticks").await {
                        Ok(delta) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("Fresh x-wasm-ticks progress: {delta}"),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    // ==================== 9. Tick-phase callout ====================
    fn test_tick_callout() -> TestCase {
        TestCase::new(
            "plugin_wasm_tick_callout",
            "Wasm plugin: on_tick dispatches a callout to the auth server; the callback publishes via shared data",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match wait_for_tick_progress(&ctx, "x-wasm-read-callout", "x-wasm-calloutticks").await {
                        Ok(delta) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("Fresh x-wasm-calloutticks progress: {delta}"),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }
}

impl TestSuite for WasmPluginTestSuite {
    fn name(&self) -> &str {
        "Wasm Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_request_header_added(),
            Self::test_local_response_deny(),
            Self::test_response_header(),
            Self::test_panic_recovery(),
            Self::test_perf_smoke(),
            Self::test_callout_authorized(),
            Self::test_callout_denied(),
            Self::test_tick_shared_data(),
            Self::test_tick_callout(),
        ]
    }
}

#[cfg(test)]
mod tick_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tick_observations_require_numeric_headers() {
        for header in ["x-wasm-ticks", "x-wasm-calloutticks"] {
            assert_eq!(tick_counter(&json!({"headers": {header: "41"}}), header), Ok(41));
            assert!(tick_counter(&json!({}), header).is_err());
            for value in [
                Value::Null,
                json!(41),
                json!("bad"),
                json!("-1"),
                json!("18446744073709551616"),
            ] {
                assert!(tick_counter(&json!({"headers": {header: value}}), header).is_err());
            }
        }
    }

    #[test]
    fn tick_progress_requires_growth_from_the_observed_baseline() {
        assert_eq!(tick_counter_delta(41, 41), Ok(0));
        assert_eq!(tick_counter_delta(41, 42), Ok(1));
        assert_eq!(tick_counter_delta(0, 1), Ok(1));
        assert!(tick_counter_delta(41, 0).is_err());
        assert!(tick_counter_delta(u64::MAX, 0).is_err());
        assert_eq!(tick_counter_delta(u64::MAX - 1, u64::MAX), Ok(1));
    }
}
