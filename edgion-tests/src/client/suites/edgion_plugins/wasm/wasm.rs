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
                                    let headers = match body["headers"].as_object() {
                                        Some(h) => h,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "Response body missing 'headers' object".to_string(),
                                            )
                                        }
                                    };

                                    let got = match headers.get("x-wasm-added").and_then(|v| v.as_str()) {
                                        Some(v) => v,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "x-wasm-added header not found in upstream echo".to_string(),
                                            )
                                        }
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
                                    let headers = match body["headers"].as_object() {
                                        Some(h) => h,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "Recovery response body missing 'headers' object".to_string(),
                                            )
                                        }
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
        ]
    }
}
