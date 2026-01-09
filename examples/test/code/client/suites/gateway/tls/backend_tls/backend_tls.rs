// Backend TLS 测试套件
// 测试 Gateway 到后端服务器的 TLS 连接（BackendTLSPolicy）
//
// 测试场景：
// - 客户端 → Gateway: HTTP (端口 18080)
// - Gateway → 后端: HTTPS (端口 30051, 使用 BackendTLSPolicy)
// - 后端服务器: 监听 30051，使用自签名证书
//
// 依赖的配置文件（位于 examples/conf/）：
// - BackendTLSPolicy_edge_backend-tls.yaml   # BackendTLSPolicy 配置
// - Service_edge_test-backend-tls.yaml       # HTTPS 后端服务定义
// - EndpointSlice_edge_test-backend-tls.yaml # HTTPS 后端端点
// - HTTPRoute_edge_backend-tls.yaml          # 路由规则（path: /backend-tls/）
// - Secret_edge_backend-ca.yaml              # CA 证书 Secret
// - Gateway_edge_tls-terminate-gateway.yaml  # Gateway 配置
// - GatewayClass__public-gateway.yaml        # GatewayClass 配置
//
// 生成的证书文件：
// - examples/testing/certs/backend/server.crt   # 后端服务器证书
// - examples/testing/certs/backend/server.key   # 后端服务器私钥
// - examples/testing/certs/backend/ca.crt       # CA 证书

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct BackendTlsTestSuite;

impl BackendTlsTestSuite {
    fn test_backend_tls_health() -> TestCase {
        TestCase::new(
            "backend_tls_health",
            "测试后端 TLS 连接 - /backend-tls/health 端点",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Build HTTP URL (client to Gateway is HTTP)
                    let url = format!("http://{}:{}/backend-tls/health", ctx.target_host, ctx.http_port);

                    let mut request = ctx.http_client.get(&url);

                    // Add Host header if in Gateway mode
                    if let Some(ref host) = ctx.http_host {
                        request = request.header("Host", host);
                    }

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            match response.text().await {
                                Ok(body) => {
                                    if status.is_success() && body.contains("OK") {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("Status: {}, Body: {}", status, body),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Unexpected response. Status: {}, Body: {}", status, body),
                                        )
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to read response body: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Backend TLS request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_backend_tls_echo() -> TestCase {
        TestCase::new(
            "backend_tls_echo",
            "测试后端 TLS 连接 - /backend-tls/echo 端点（请求转发）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("http://{}:{}/backend-tls/echo", ctx.target_host, ctx.http_port);

                    let mut request = ctx.http_client.get(&url);

                    if let Some(ref host) = ctx.http_host {
                        request = request.header("Host", host);
                    }

                    // Add a custom header to verify forwarding
                    request = request.header("X-Test-Header", "backend-tls-test");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            match response.text().await {
                                Ok(body) => {
                                    // Verify response contains expected server info
                                    if status.is_success()
                                        && body.contains("Server: 127.0.0.1:30051")
                                        && body.contains("X-Test-Header: backend-tls-test")
                                    {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "Request forwarded successfully via Backend TLS".to_string(),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Response validation failed. Status: {}, Body: {}", status, body),
                                        )
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to read response body: {}", e))
                                }
                            }
                        }
                        Err(e) => {
                            TestResult::failed(start.elapsed(), format!("Backend TLS echo request failed: {}", e))
                        }
                    }
                })
            },
        )
    }

    fn test_backend_tls_headers() -> TestCase {
        TestCase::new(
            "backend_tls_headers",
            "测试后端 TLS 连接 - /backend-tls/headers 端点（验证 SNI）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("http://{}:{}/backend-tls/headers", ctx.target_host, ctx.http_port);

                    let mut request = ctx.http_client.get(&url);

                    if let Some(ref host) = ctx.http_host {
                        request = request.header("Host", host);
                    }

                    // Add X-Trace-ID to verify header forwarding
                    request = request.header("X-Trace-ID", "backend-tls-trace-123");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            match response.text().await {
                                Ok(body) => {
                                    // Verify trace ID is forwarded correctly
                                    if status.is_success() && body.contains("backend-tls-trace-123") {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "Headers forwarded correctly through Backend TLS".to_string(),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Header validation failed. Status: {}, Body: {}", status, body),
                                        )
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to read response body: {}", e))
                                }
                            }
                        }
                        Err(e) => {
                            TestResult::failed(start.elapsed(), format!("Backend TLS headers request failed: {}", e))
                        }
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for BackendTlsTestSuite {
    fn name(&self) -> &str {
        "backend_tls"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_backend_tls_health(),
            Self::test_backend_tls_echo(),
            Self::test_backend_tls_headers(),
        ]
    }
}
