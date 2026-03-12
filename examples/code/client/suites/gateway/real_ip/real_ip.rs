// Real IP extraction test suite
//
// Required config files (in examples/conf/):
// - EndpointSlice_edge_test-http.yaml         # HTTP backend service discovery
// - Service_edge_test-http.yaml               # HTTP service definition
// - httproute_default_example-route.yaml      # HTTP routing rules（Host: test.example.com）
//   Note: route contains trustedIps config for real IP extraction
// - Gateway_edge_example-gateway.yaml         # Gateway config
// - GatewayClass__public-gateway.yaml         # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use reqwest::header::HeaderMap;
use serde_json::Value;
use std::net::IpAddr;
use std::time::Instant;

pub struct RealIpTestSuite;

impl RealIpTestSuite {
    fn is_local_target(ctx: &TestContext) -> bool {
        ctx.target_host == "127.0.0.1" || ctx.target_host == "localhost"
    }

    fn is_valid_ip(value: &str) -> bool {
        value.parse::<IpAddr>().is_ok()
    }

    fn test_xff_extraction() -> TestCase {
        TestCase::new(
            "xff_extraction_with_trusted_ips",
            "Test X-Forwarded-For extraction (with trusted IPs)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();
                    let url = format!("{}/headers", ctx.http_url());

                    // Simulate request through proxy chain:
                    // Real client: 203.0.113.1 (NOT in trustedIps)
                    // Proxy 1: 198.51.100.2 (in 198.51.100.0/24 - trusted)
                    // Proxy 2: 192.168.1.1 (in 192.168.0.0/16 - trusted)
                    let mut headers = HeaderMap::new();
                    headers.insert(
                        "x-forwarded-for",
                        "203.0.113.1, 198.51.100.2, 192.168.1.1".parse().unwrap(),
                    );
                    headers.insert("x-trace-id", "test-real-ip-xff".parse().unwrap());

                    if let Some(host) = &ctx.http_host {
                        headers.insert("host", host.parse().unwrap());
                    }

                    match client.get(&url).headers(headers).send().await {
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

                                    if real_ip != "203.0.113.1" {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected X-Real-IP=203.0.113.1, got {}", real_ip),
                                        );
                                    }

                                    // Verify X-Forwarded-For keeps original chain and appends current client address.
                                    let xff = match headers.get("x-forwarded-for").and_then(|v| v.as_str()) {
                                        Some(x) => x,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "x-forwarded-for header not found in response".to_string(),
                                            )
                                        }
                                    };

                                    let expected_prefix = "203.0.113.1, 198.51.100.2, 192.168.1.1";
                                    if xff == expected_prefix {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected appended client IP in XFF, got '{}'", xff),
                                        );
                                    }

                                    let appended =
                                        xff.strip_prefix(&(expected_prefix.to_string() + ", ")).unwrap_or("");
                                    if appended.is_empty() || !Self::is_valid_ip(appended) {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected appended valid IP in XFF, got '{}'", xff),
                                        );
                                    }

                                    if Self::is_local_target(&ctx) && appended != "127.0.0.1" && appended != "::1" {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected local loopback client IP in direct mode, got '{}'",
                                                appended
                                            ),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("✓ X-Real-IP: {}, X-Forwarded-For: {}", real_ip, xff),
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

    fn test_direct_connection() -> TestCase {
        TestCase::new(
            "direct_connection_without_xff",
            "Test direct connection (no X-Forwarded-For)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder().no_proxy().build().unwrap();
                    let url = format!("{}/headers", ctx.http_url());

                    let mut request = client.get(&url).header("x-trace-id", "test-real-ip-direct");

                    if let Some(host) = &ctx.http_host {
                        request = request.header("host", host);
                    }

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

                                    // Without XFF, X-Real-IP should be client_addr.
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

                                    // X-Forwarded-For should be created with just client_addr.
                                    let xff = match headers.get("x-forwarded-for").and_then(|v| v.as_str()) {
                                        Some(x) => x,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "x-forwarded-for header not found in response".to_string(),
                                            )
                                        }
                                    };

                                    if xff != real_ip {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected XFF to equal X-Real-IP ('{}'), got '{}'", real_ip, xff),
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
                                        format!("✓ X-Real-IP: {}, X-Forwarded-For: {}", real_ip, xff),
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

impl TestSuite for RealIpTestSuite {
    fn name(&self) -> &str {
        "Real IP Extraction Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_xff_extraction(), Self::test_direct_connection()]
    }
}
