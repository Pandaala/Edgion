// HTTPS 测试套件
// 只在 Gateway 模式下测试，使用 /secure/ 路径前缀区分
//
// 依赖的配置文件（位于 examples/conf/）：
// - EndpointSlice_edge_test-http.yaml         # HTTPS 后端服务（复用 HTTP 后端）
// - Service_edge_test-http.yaml               # HTTPS 服务定义
// - HTTPRoute_edge_test-http.yaml             # HTTPS 路由规则（Host: test.example.com, path: /secure/）
// - Gateway_edge_tls-terminate-gateway.yaml   # TLS 终止 Gateway 配置（监听 18443 端口）
// - EdgionTls_edge_edge-tls.yaml              # TLS 证书配置
// - Secret_edgion-test_edge-tls.yaml          # TLS 证书 Secret
// - GatewayClass__public-gateway.yaml         # GatewayClass 配置
//
// 生成的证书文件：
// - examples/testing/certs/server.crt         # 服务端证书（由 generate_certs.sh 生成）
// - examples/testing/certs/server.key         # 服务端私钥
// - examples/testing/certs/ca.pem             # CA 证书

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct HttpsTestSuite;

impl HttpsTestSuite {
    fn test_https_secure_health() -> TestCase {
        TestCase::new(
            "https_secure_health",
            "测试 HTTPS /secure/health 端点",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Build HTTPS URL - always use target_host (127.0.0.1)
                    let url = format!("https://{}:{}/secure/health", ctx.target_host, ctx.https_port);

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
                                    if status.is_success() && body.contains("healthy") {
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
                        Err(e) => TestResult::failed(start.elapsed(), format!("HTTPS request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_https_secure_echo() -> TestCase {
        TestCase::new(
            "https_secure_echo",
            "测试 HTTPS /secure/echo 端点",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("https://{}:{}/secure/echo", ctx.target_host, ctx.https_port);

                    let mut request = ctx.http_client.post(&url).body("HTTPS Test Message");

                    if let Some(ref host) = ctx.http_host {
                        request = request.header("Host", host);
                    }

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            match response.text().await {
                                Ok(body) => {
                                    if status.is_success() && body.contains("HTTPS Test Message") {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("Echo successful: {}", body),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Echo failed. Status: {}, Body: {}", status, body),
                                        )
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to read response: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("HTTPS request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_https_secure_status() -> TestCase {
        TestCase::new(
            "https_secure_status",
            "测试 HTTPS /secure/status/200 端点",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("https://{}:{}/secure/status/200", ctx.target_host, ctx.https_port);

                    let mut request = ctx.http_client.get(&url);

                    if let Some(ref host) = ctx.http_host {
                        request = request.header("Host", host);
                    }

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            if status.as_u16() == 200 {
                                TestResult::passed_with_message(start.elapsed(), format!("Status code: {}", status))
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("HTTPS request failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for HttpsTestSuite {
    fn name(&self) -> &str {
        "HTTPS"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_https_secure_health(),
            Self::test_https_secure_echo(),
            Self::test_https_secure_status(),
        ]
    }
}
