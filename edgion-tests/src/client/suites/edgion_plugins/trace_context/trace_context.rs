// TraceContext Plugin Test Suite
//
// Test strategy:
// - Verify the plugin passes through a valid inbound traceparent unchanged
// - Verify the plugin generates a new root-unsampled traceparent when none is present
// - Verify the plugin replaces a malformed traceparent with a well-formed one
// - Verify trustInbound:false ignores the inbound trace ID and generates a fresh one
// - Verify generateWhenMissing:false leaves no traceparent when none is present
// - Verify defaultSampled:true + echoToClient:true set the sampled flag and echo to the response

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde_json::Value;
use std::time::Instant;

const HOST_DEFAULT: &str = "trace-context.example.com";
const HOST_NOGEN: &str = "trace-context-nogen.example.com";
const HOST_UNTRUSTED: &str = "trace-context-untrusted.example.com";
const HOST_SAMPLED: &str = "trace-context-sampled.example.com";

const VALID_TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

pub struct TraceContextPluginTestSuite;

impl TraceContextPluginTestSuite {
    fn is_well_formed_traceparent(v: &str) -> bool {
        let p: Vec<&str> = v.split('-').collect();
        p.len() == 4
            && p[0] == "00"
            && p[1].len() == 32
            && p[2].len() == 16
            && p[3].len() == 2
            && p[1].bytes().all(|b| b.is_ascii_hexdigit())
    }

    // ==================== 1. Passthrough test ====================
    fn test_passthrough() -> TestCase {
        TestCase::new(
            "plugin_trace_context_passthrough",
            "TraceContext plugin: valid inbound traceparent is forwarded unchanged",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", HOST_DEFAULT)
                        .header("traceparent", VALID_TRACEPARENT)
                        .header("x-trace-id", "test-trace-context-passthrough");

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

                                    let got = match headers.get("traceparent").and_then(|v| v.as_str()) {
                                        Some(v) => v,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "traceparent header not found in upstream echo".to_string(),
                                            )
                                        }
                                    };

                                    if got != VALID_TRACEPARENT {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected traceparent='{}', got '{}'", VALID_TRACEPARENT, got),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("traceparent passed through unchanged: {}", got),
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

    // ==================== 2. Generate root unsampled ====================
    fn test_generate_root_unsampled() -> TestCase {
        TestCase::new(
            "plugin_trace_context_generate_root_unsampled",
            "TraceContext plugin: generates a well-formed unsampled traceparent when none present",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", HOST_DEFAULT)
                        .header("x-trace-id", "test-trace-context-generate");

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

                                    let got = match headers.get("traceparent").and_then(|v| v.as_str()) {
                                        Some(v) => v,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "traceparent header not generated by plugin".to_string(),
                                            )
                                        }
                                    };

                                    if !Self::is_well_formed_traceparent(got) {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Generated traceparent is malformed: '{}'", got),
                                        );
                                    }

                                    if !got.ends_with("-00") {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected unsampled traceparent (ends with -00), got '{}'", got),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("Generated unsampled traceparent: {}", got),
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

    // ==================== 3. Malformed input is regenerated ====================
    fn test_malformed_regenerates() -> TestCase {
        TestCase::new(
            "plugin_trace_context_malformed_regenerates",
            "TraceContext plugin: malformed inbound traceparent is replaced with a well-formed one",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", HOST_DEFAULT)
                        .header("traceparent", "garbage")
                        .header("x-trace-id", "test-trace-context-malformed");

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

                                    let got = match headers.get("traceparent").and_then(|v| v.as_str()) {
                                        Some(v) => v,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "traceparent header missing after malformed input".to_string(),
                                            )
                                        }
                                    };

                                    if got == "garbage" {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            "Plugin forwarded the malformed traceparent unchanged".to_string(),
                                        );
                                    }

                                    if !Self::is_well_formed_traceparent(got) {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Replaced traceparent is still malformed: '{}'", got),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("Malformed input replaced with valid traceparent: {}", got),
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

    // ==================== 4. trustInbound:false ignores inbound trace ID ====================
    fn test_untrusted_ignores_inbound() -> TestCase {
        TestCase::new(
            "plugin_trace_context_untrusted_ignores_inbound",
            "TraceContext plugin: trustInbound:false ignores the inbound trace-id and generates a fresh one",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", HOST_UNTRUSTED)
                        .header("traceparent", VALID_TRACEPARENT)
                        .header("x-trace-id", "test-trace-context-untrusted");

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

                                    let got = match headers.get("traceparent").and_then(|v| v.as_str()) {
                                        Some(v) => v,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "traceparent header not found in upstream echo".to_string(),
                                            )
                                        }
                                    };

                                    // The inbound trace-id must NOT be reused
                                    if got.contains("4bf92f3577b34da6a3ce929d0e0e4736") {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Plugin reused inbound trace-id despite trustInbound:false, got '{}'",
                                                got
                                            ),
                                        );
                                    }

                                    if !Self::is_well_formed_traceparent(got) {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Generated traceparent is malformed: '{}'", got),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("Inbound trace-id ignored; fresh traceparent generated: {}", got),
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

    // ==================== 5. generateWhenMissing:false leaves no traceparent ====================
    fn test_no_generate() -> TestCase {
        TestCase::new(
            "plugin_trace_context_no_generate",
            "TraceContext plugin: generateWhenMissing:false does not inject traceparent when none present",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", HOST_NOGEN)
                        .header("x-trace-id", "test-trace-context-nogen");

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

                                    if headers.contains_key("traceparent") {
                                        let got = headers["traceparent"].as_str().unwrap_or("<non-string>");
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected no traceparent (generateWhenMissing:false), but got '{}'",
                                                got
                                            ),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "No traceparent injected when generateWhenMissing:false".to_string(),
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

    // ==================== 6. defaultSampled:true + echoToClient:true ====================
    fn test_sampled_and_echo() -> TestCase {
        TestCase::new(
            "plugin_trace_context_sampled_and_echo",
            "TraceContext plugin: defaultSampled:true sets sampled flag; echoToClient:true echoes to response",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", HOST_SAMPLED)
                        .header("x-trace-id", "test-trace-context-sampled");

                    match request.send().await {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 OK, got {}", resp.status()),
                                );
                            }

                            // Read response-level traceparent BEFORE consuming the body
                            let resp_traceparent = resp
                                .headers()
                                .get("traceparent")
                                .and_then(|v| v.to_str().ok())
                                .map(|s| s.to_string());

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

                                    // Check upstream traceparent: must be well-formed and sampled
                                    let upstream_tp = match headers.get("traceparent").and_then(|v| v.as_str()) {
                                        Some(v) => v,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "traceparent header not found in upstream echo".to_string(),
                                            )
                                        }
                                    };

                                    if !Self::is_well_formed_traceparent(upstream_tp) {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Upstream traceparent is malformed: '{}'", upstream_tp),
                                        );
                                    }

                                    if !upstream_tp.ends_with("-01") {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected sampled traceparent (ends with -01), got '{}'",
                                                upstream_tp
                                            ),
                                        );
                                    }

                                    // Check response-level traceparent (echoToClient)
                                    let client_tp = match resp_traceparent {
                                        Some(ref v) => v.as_str(),
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "traceparent not echoed to client response headers".to_string(),
                                            )
                                        }
                                    };

                                    if !Self::is_well_formed_traceparent(client_tp) {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Client-echoed traceparent is malformed: '{}'", client_tp),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!(
                                            "Sampled traceparent forwarded upstream: {}; echoed to client: {}",
                                            upstream_tp, client_tp
                                        ),
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
}

impl TestSuite for TraceContextPluginTestSuite {
    fn name(&self) -> &str {
        "TraceContext Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_passthrough(),
            Self::test_generate_root_unsampled(),
            Self::test_malformed_regenerates(),
            Self::test_untrusted_ignores_inbound(),
            Self::test_no_generate(),
            Self::test_sampled_and_echo(),
        ]
    }
}
