// RealIp Plugin Test Suite
//
// 测试策略：
// - 验证插件能够从 X-Forwarded-For 提取真实 IP
// - 验证可信代理列表的过滤逻辑
// - 验证递归查找和非递归模式
// - 验证直接连接场景
//
// 配置：trustedIps: ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "127.0.0.1/32"]

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use reqwest::header::HeaderMap;
use serde_json::Value;
use std::time::Instant;

pub struct RealIpPluginTestSuite;

impl RealIpPluginTestSuite {
    // ==================== 1. XFF 提取测试（递归模式）====================
    fn test_xff_extraction_recursive() -> TestCase {
        TestCase::new(
            "plugin_real_ip_xff_recursive",
            "RealIp 插件: X-Forwarded-For 递归提取",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    // 模拟代理链:
                    // 真实客户端: 203.0.113.1 (NOT trusted)
                    // 代理 1: 198.51.100.2 (在 198.51.100.0/24 - 不在 trustedIps 中，应该被识别为真实 IP)
                    // 代理 2: 192.168.1.1 (在 192.168.0.0/16 - trusted)
                    let mut request = client.get(&url).header(
                        "x-forwarded-for",
                        "203.0.113.1, 10.0.0.5, 192.168.1.1",
                    ).header("x-trace-id", "test-real-ip-plugin-xff");

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

                                    // 验证 X-Real-IP 是第一个非可信 IP (203.0.113.1)
                                    let real_ip = match headers.get("x-real-ip").and_then(|v| v.as_str()) {
                                        Some(ip) => ip,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "x-real-ip header not found in response".to_string(),
                                            )
                                        }
                                    };

                                    // 由于插件会重新提取，期望是 203.0.113.1
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
                                        format!("✓ Plugin extracted real IP: {}", real_ip),
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

    // ==================== 2. 直接连接测试 ====================
    fn test_direct_connection() -> TestCase {
        TestCase::new(
            "plugin_real_ip_direct_connection",
            "RealIp 插件: 直接连接（无代理）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    if let Some(host) = &ctx.http_host {
                        request = request.header("host", host);
                    }
                    request = request.header("x-trace-id", "test-real-ip-plugin-direct");

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

                                    // 直接连接，X-Real-IP 应该是 client_addr (127.0.0.1)
                                    let real_ip = match headers.get("x-real-ip").and_then(|v| v.as_str()) {
                                        Some(ip) => ip,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "x-real-ip header not found in response".to_string(),
                                            )
                                        }
                                    };

                                    if real_ip != "127.0.0.1" {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected X-Real-IP=127.0.0.1 (direct connection), got {}",
                                                real_ip
                                            ),
                                        );
                                    }

                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("✓ Direct connection IP: {}", real_ip),
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

    // ==================== 3. 全部可信 IP 测试 ====================
    fn test_all_trusted_ips() -> TestCase {
        TestCase::new(
            "plugin_real_ip_all_trusted",
            "RealIp 插件: 全部 IP 都在可信列表中",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    // 所有 IP 都在 trustedIps 中，应该返回最左边的 IP
                    let mut request = client.get(&url).header(
                        "x-forwarded-for",
                        "192.168.1.1, 10.0.0.1, 172.16.0.1",
                    ).header("x-trace-id", "test-real-ip-plugin-all-trusted");

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

                                    let real_ip = match headers.get("x-real-ip").and_then(|v| v.as_str()) {
                                        Some(ip) => ip,
                                        None => {
                                            return TestResult::failed(
                                                start.elapsed(),
                                                "x-real-ip header not found in response".to_string(),
                                            )
                                        }
                                    };

                                    // 全部可信，应该使用最左边的 IP
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
                                        format!("✓ All trusted, using leftmost IP: {}", real_ip),
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

impl TestSuite for RealIpPluginTestSuite {
    fn name(&self) -> &str {
        "RealIp Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_xff_extraction_recursive(),
            Self::test_direct_connection(),
            Self::test_all_trusted_ips(),
        ]
    }
}
