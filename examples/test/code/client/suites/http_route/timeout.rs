// Timeout 测试套件
//
// 依赖的配置文件（位于 examples/conf/）：
// - HTTPRoute_default_timeout-backend.yaml    # 后端超时测试路由
// - HTTPRoute_default_timeout-client.yaml     # 客户端超时测试路由
// - EdgionPlugins_default_timeout-debug.yaml  # Debug插件配置
// - EndpointSlice_edge_test-http.yaml         # HTTP 后端服务发现
// - Service_edge_test-http.yaml               # HTTP 服务定义
// - Gateway_edge_example-gateway.yaml         # Gateway 配置
// - EdgionGatewayConfig__example-gateway.yaml # GatewayConfig（client.readTimeout: 60s）
// - GatewayClass__public-gateway.yaml         # GatewayClass 配置

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct TimeoutTestSuite;

impl TimeoutTestSuite {
    fn test_normal_response() -> TestCase {
        TestCase::new(
            "normal_response",
            "测试正常响应（基准对照）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let mut request = ctx.http_client.get(format!("{}/delay/1", ctx.http_url()));
                    request = request.header("Host", "timeout-backend.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            let headers = response.headers().clone();

                            if !status.is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", status.as_u16()),
                                );
                            }

                            // 检查Debug header
                            if let Some(debug_header) = headers.get("X-Debug-Access-Log") {
                                if let Ok(debug_str) = debug_header.to_str() {
                                    if let Ok(debug_json) = serde_json::from_str::<serde_json::Value>(debug_str) {
                                        let internal_status =
                                            debug_json["request_info"]["status"].as_u64().unwrap_or(0);
                                        if internal_status == 200 {
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                format!(
                                                    "Normal response: HTTP {} / Internal {}",
                                                    status.as_u16(),
                                                    internal_status
                                                ),
                                            )
                                        } else {
                                            TestResult::failed(
                                                start.elapsed(),
                                                format!(
                                                    "Status mismatch: HTTP {} but Internal {}",
                                                    status.as_u16(),
                                                    internal_status
                                                ),
                                            )
                                        }
                                    } else {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("Response OK but failed to parse debug JSON: {}", debug_str),
                                        )
                                    }
                                } else {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Response OK but debug header not valid UTF-8".to_string(),
                                    )
                                }
                            } else {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Response OK but no debug header found".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_backend_request_timeout() -> TestCase {
        TestCase::new(
            "backend_request_timeout",
            "测试后端请求总超时（5秒延迟 vs 3秒超时）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let mut request = ctx.http_client.get(format!("{}/delay/5", ctx.http_url()));
                    request = request.header("Host", "timeout-backend.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            let headers = response.headers().clone();

                            // 期望504
                            if status.as_u16() != 504 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected HTTP 504, got {}", status.as_u16()),
                                );
                            }

                            // 检查Debug header中的内部状态码
                            if let Some(debug_header) = headers.get("X-Debug-Access-Log") {
                                if let Ok(debug_str) = debug_header.to_str() {
                                    if let Ok(debug_json) = serde_json::from_str::<serde_json::Value>(debug_str) {
                                        let internal_status =
                                            debug_json["request_info"]["status"].as_u64().unwrap_or(0);
                                        if internal_status == 504 {
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                "Backend request timeout: HTTP 504 / Internal 504 ✓".to_string(),
                                            )
                                        } else {
                                            TestResult::failed(
                                                start.elapsed(),
                                                format!("Status mismatch: HTTP 504 but Internal {}", internal_status),
                                            )
                                        }
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            "Got 504 but failed to parse debug JSON".to_string(),
                                        )
                                    }
                                } else {
                                    TestResult::failed(start.elapsed(), "Debug header not valid UTF-8".to_string())
                                }
                            } else {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Got HTTP 504 (debug header missing)".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_client_read_timeout_499() -> TestCase {
        TestCase::new(
            "client_read_timeout_499",
            "测试客户端主动断开连接返回499（reqwest超时3秒，服务端延迟10秒）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // 创建一个3秒超时的HTTP客户端，模拟客户端主动断开
                    let short_timeout_client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(3))
                        .build()
                        .expect("Failed to create short timeout client");

                    // 服务端延迟10秒（不会触发服务端超时，因为backend timeout是100s）
                    // 但客户端3秒后会主动断开
                    let mut request = short_timeout_client.get(format!("{}/delay/10", ctx.http_url()));
                    request = request.header("Host", "timeout-client.example.com");

                    match request.send().await {
                        Ok(response) => {
                            // 如果成功返回了响应，说明没有超时（不应该发生）
                            let status = response.status();
                            let elapsed = start.elapsed();
                            TestResult::failed(
                                elapsed,
                                format!(
                                    "Expected client to timeout but got HTTP {} after {}s",
                                    status.as_u16(),
                                    elapsed.as_secs()
                                ),
                            )
                        }
                        Err(e) => {
                            let elapsed = start.elapsed();
                            let error_msg = e.to_string();

                            // reqwest客户端超时断开连接，这是预期的
                            // Gateway应该检测到客户端断开并记录为499
                            // 检查是否包含timeout、timed out、operation timed out等关键词
                            let is_timeout = error_msg.to_lowercase().contains("timeout")
                                || error_msg.to_lowercase().contains("timed out")
                                || error_msg.to_lowercase().contains("time out")
                                || error_msg.contains("deadline");

                            if is_timeout || elapsed.as_secs() >= 2 && elapsed.as_secs() <= 5 {
                                // 客户端确实超时了（约3秒）
                                TestResult::passed_with_message(
                                elapsed,
                                format!("Client closed connection after ~{}s (reqwest timeout 3s) - Gateway should log this as 499. Error: {}", 
                                    elapsed.as_secs(), error_msg)
                            )
                            } else {
                                TestResult::failed(
                                    elapsed,
                                    format!(
                                        "Unexpected error or timing: {}s, error: {}",
                                        elapsed.as_secs(),
                                        error_msg
                                    ),
                                )
                            }
                        }
                    }
                })
            },
        )
    }

    fn test_client_read_timeout() -> TestCase {
        TestCase::new(
            "client_read_timeout",
            "测试客户端读超时（20秒延迟，框架30秒超时，观察gateway行为）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // 创建一个超时时间更长的HTTP客户端（25秒），模拟一个长时间等待但不会先超时的客户端
                    let long_timeout_client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(25))
                        .build()
                        .expect("Failed to create long timeout client");

                    let mut request = long_timeout_client.get(format!("{}/delay/20", ctx.http_url()));
                    request = request.header("Host", "timeout-client.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            let status_code = status.as_u16();
                            let headers = response.headers().clone();
                            let elapsed = start.elapsed();

                            // 正常情况下应该在20秒左右返回200
                            // 因为backend timeout是100s，client timeout是60s
                            if status_code == 200 {
                                TestResult::passed_with_message(
                                    elapsed,
                                    format!(
                                        "Got 200 OK after {}s - client/backend timeouts not triggered (both > 20s)",
                                        elapsed.as_secs()
                                    ),
                                )
                            } else {
                                // 检查Debug header获取内部状态码
                                if let Some(debug_header) = headers.get("X-Debug-Access-Log") {
                                    if let Ok(debug_str) = debug_header.to_str() {
                                        if let Ok(debug_json) = serde_json::from_str::<serde_json::Value>(debug_str) {
                                            let internal_status =
                                                debug_json["request_info"]["status"].as_u64().unwrap_or(0);
                                            TestResult::failed(
                                                elapsed,
                                                format!(
                                                    "Expected 200 but got HTTP {} / Internal {}",
                                                    status_code, internal_status
                                                ),
                                            )
                                        } else {
                                            TestResult::failed(
                                                elapsed,
                                                format!("Got HTTP {} (debug JSON parse error)", status_code),
                                            )
                                        }
                                    } else {
                                        TestResult::failed(elapsed, "Debug header not valid UTF-8".to_string())
                                    }
                                } else {
                                    TestResult::failed(elapsed, format!("Got HTTP {} (no debug header)", status_code))
                                }
                            }
                        }
                        Err(e) => {
                            let elapsed = start.elapsed();
                            TestResult::failed(
                                elapsed,
                                format!("Request failed: {} (should have returned 200 after 20s)", e),
                            )
                        }
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for TimeoutTestSuite {
    fn name(&self) -> &str {
        "Timeout"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_normal_response(),
            Self::test_backend_request_timeout(),
            Self::test_client_read_timeout(),
            Self::test_client_read_timeout_499(),
        ]
    }
}
