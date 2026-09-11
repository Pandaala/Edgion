// RealIp Plugin Test Suite
//
// Test strategy:
// - Verify the plugin extracts real IP from X-Forwarded-For
// - Verify trusted proxy list filtering logic
// - Verify recursive and non-recursive lookup modes
// - Verify direct connection scenario
//
// Config: trustedIps: [{ name: "private-ranges", cidrs: ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "127.0.0.1/32"] }]

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde_json::Value;
use std::net::IpAddr;
use std::time::Instant;

const HOST: &str = "real-ip-test.example.com";

pub struct RealIpPluginTestSuite;

impl RealIpPluginTestSuite {
    fn is_local_target(ctx: &TestContext) -> bool {
        ctx.target_host == "127.0.0.1" || ctx.target_host == "localhost"
    }

    fn is_valid_ip(value: &str) -> bool {
        value.parse::<IpAddr>().is_ok()
    }

    // ==================== 1. XFF extraction test (recursive mode) ====================
    fn test_xff_extraction_recursive() -> TestCase {
        TestCase::new(
            "plugin_real_ip_xff_recursive",
            "RealIp plugin: X-Forwarded-For recursive extraction",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    // Simulate proxy chain:
                    // Real client: 203.0.113.1 (NOT trusted)
                    // Proxy 1: 10.0.0.5 (in 10.0.0.0/8 - trusted)
                    // Proxy 2: 192.168.1.1 (in 192.168.0.0/16 - trusted)
                    let request = client
                        .get(&url)
                        .header("host", HOST)
                        .header("x-forwarded-for", "203.0.113.1, 10.0.0.5, 192.168.1.1")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker("test-real-ip-plugin-xff"),
                        );

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

                                    // Verify X-Real-IP is the first non-trusted IP (203.0.113.1)
                                    let real_ip = match headers.get("x-real-ip").and_then(|v| v.as_str()) {
                                        Some(ip) => ip,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "x-real-ip header not found in response".to_string(),
                                            )
                                        }
                                    };

                                    // Plugin re-extracts, expected result is 203.0.113.1
                                    if real_ip != "203.0.113.1" {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected X-Real-IP=203.0.113.1 (plugin extracted), got {}",
                                                real_ip
                                            ),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("Plugin extracted real IP: {}", real_ip),
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

    // ==================== 2. Direct connection test ====================
    fn test_direct_connection() -> TestCase {
        TestCase::new(
            "plugin_real_ip_direct_connection",
            "RealIp plugin: direct connection (no proxy)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client.get(&url).header("host", HOST).header(
                        "traceparent",
                        crate::access_log_client::traceparent_for_marker("test-real-ip-plugin-direct"),
                    );

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

                                    // Direct connection, X-Real-IP should carry the resolved RemoteIp.
                                    let real_ip = match headers.get("x-real-ip").and_then(|v| v.as_str()) {
                                        Some(ip) => ip,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "x-real-ip header not found in response".to_string(),
                                            )
                                        }
                                    };

                                    if !Self::is_valid_ip(real_ip) {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected X-Real-IP to be a valid IP, got {}", real_ip),
                                        );
                                    }

                                    if Self::is_local_target(&ctx) && real_ip != "127.0.0.1" && real_ip != "::1" {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected local loopback IP for direct local run, got '{}'",
                                                real_ip
                                            ),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("Direct connection IP: {}", real_ip),
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

    // ==================== 3. All trusted IPs test ====================
    fn test_all_trusted_ips() -> TestCase {
        TestCase::new(
            "plugin_real_ip_all_trusted",
            "RealIp plugin: all IPs are in the trusted list",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    // All IPs are in trustedIps, should return the leftmost IP
                    let request = client
                        .get(&url)
                        .header("host", HOST)
                        .header("x-forwarded-for", "192.168.1.1, 10.0.0.1, 172.16.0.1")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker("test-real-ip-plugin-all-trusted"),
                        );

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

                                    let real_ip = match headers.get("x-real-ip").and_then(|v| v.as_str()) {
                                        Some(ip) => ip,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "x-real-ip header not found in response".to_string(),
                                            )
                                        }
                                    };

                                    // All trusted, should use the leftmost IP
                                    if real_ip != "192.168.1.1" {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected X-Real-IP=192.168.1.1 (leftmost when all trusted), got {}",
                                                real_ip
                                            ),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("All trusted, using leftmost IP: {}", real_ip),
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

    fn test_stage_logs_contain_real_ip() -> TestCase {
        TestCase::new(
            "stage_logs_contain_real_ip",
            "Access log stage_logs contains RealIp plugin entry on normal request",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("test-real-ip-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/headers", ctx.http_url());

                    let _ = ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST)
                        .header("x-forwarded-for", "203.0.113.1, 10.0.0.5")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let entry = match ctx
                        .access_log_client()
                        .get_access_log_with_retry(&trace_id, 10, 200)
                        .await
                    {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Access log not found: {}", e)),
                    };

                    let found = entry.data["stage_logs"]
                        .as_array()
                        .map(|stages| {
                            stages.iter().any(|stage| {
                                stage["edgion_plugins"]
                                    .as_array()
                                    .map(|eps| {
                                        eps.iter().any(|ep| {
                                            ep["logs"]
                                                .as_array()
                                                .map(|logs| logs.iter().any(|l| l["name"].as_str() == Some("RealIp")))
                                                .unwrap_or(false)
                                        })
                                    })
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false);

                    let request_info = &entry.data["request_info"];
                    let direct_peer_ip = request_info["direct_peer_ip"].as_str();
                    let remote_ip = request_info["remote_ip"].as_str();
                    if direct_peer_ip.is_none() || remote_ip != Some("203.0.113.1") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "canonical request IP fields missing or incorrect: direct_peer_ip={direct_peer_ip:?}, remote_ip={remote_ip:?}"
                            ),
                        );
                    }
                    if direct_peer_ip == remote_ip {
                        return TestResult::failed(
                            start.elapsed(),
                            "direct_peer_ip must remain distinct from the XFF-resolved remote_ip in this test"
                                .to_string(),
                        );
                    }
                    if request_info.get("client-addr").is_some() || request_info.get("remote-addr").is_some() {
                        return TestResult::failed(
                            start.elapsed(),
                            "legacy client-addr/remote-addr fields must not be emitted".to_string(),
                        );
                    }

                    if found {
                        TestResult::passed_with_message(start.elapsed(), "RealIp found in stage_logs".to_string())
                    } else {
                        TestResult::failed(start.elapsed(), "RealIp not found in stage_logs".to_string())
                    }
                })
            },
        )
    }
}

impl TestSuite for RealIpPluginTestSuite {
    fn name(&self) -> &str {
        "RealIp Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_xff_extraction_recursive(),
            Self::test_direct_connection(),
            Self::test_all_trusted_ips(),
            // Access log: stage_logs includes RealIp entry
            Self::test_stage_logs_contain_real_ip(),
        ]
    }
}
