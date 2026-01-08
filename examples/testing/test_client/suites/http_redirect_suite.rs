// HTTP to HTTPS Redirect 测试套件
//
// 测试 Gateway annotation `edgion.io/http-to-https-redirect: "true"` 功能
//
// 依赖的配置文件（位于 examples/conf/）：
// - Gateway_edge_redirect-gateway.yaml   # 启用重定向的 Gateway (端口 10081)
//
// 测试场景：
// 1. 简单路径重定向
// 2. 带查询参数的重定向
// 3. 验证 301 状态码
// 4. 验证 Location 头格式

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use reqwest::redirect::Policy;
use std::time::Instant;

/// HTTP redirect 测试使用的端口
const REDIRECT_PORT: u16 = 10081;

pub struct HttpRedirectTestSuite;

impl HttpRedirectTestSuite {
    /// 测试简单路径重定向
    fn test_simple_redirect() -> TestCase {
        TestCase::new(
            "http_redirect_simple",
            "测试简单路径 HTTP->HTTPS 重定向",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // 创建不自动跟随重定向的客户端
                    let client = reqwest::Client::builder()
                        .redirect(Policy::none())
                        .danger_accept_invalid_certs(true)
                        .build()
                        .unwrap();

                    let url = format!("http://{}:{}/health", ctx.target_host, REDIRECT_PORT);

                    match client.get(&url).header("Host", "test.example.com").send().await {
                        Ok(response) => {
                            let status = response.status();

                            // 验证是 301 重定向
                            if status.as_u16() != 301 {
                                return TestResult::failed(start.elapsed(), format!("Expected 301, got {}", status));
                            }

                            // 验证 Location 头
                            match response.headers().get("location") {
                                Some(location) => {
                                    let location_str = location.to_str().unwrap_or("");
                                    let expected = "https://test.example.com:10443/health";

                                    if location_str == expected {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("301 Redirect to: {}", location_str),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Location mismatch. Expected: {}, Got: {}", expected, location_str),
                                        )
                                    }
                                }
                                None => TestResult::failed(start.elapsed(), "Missing Location header".to_string()),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// 测试带查询参数的重定向
    fn test_redirect_with_query() -> TestCase {
        TestCase::new(
            "http_redirect_with_query",
            "测试带查询参数的重定向",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .redirect(Policy::none())
                        .danger_accept_invalid_certs(true)
                        .build()
                        .unwrap();

                    let url = format!("http://{}:{}/api/users?page=1&limit=10", ctx.target_host, REDIRECT_PORT);

                    match client.get(&url).header("Host", "api.example.com").send().await {
                        Ok(response) => {
                            let status = response.status();

                            if status.as_u16() != 301 {
                                return TestResult::failed(start.elapsed(), format!("Expected 301, got {}", status));
                            }

                            match response.headers().get("location") {
                                Some(location) => {
                                    let location_str = location.to_str().unwrap_or("");

                                    // 验证查询参数被保留
                                    if location_str.contains("page=1") && location_str.contains("limit=10") {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("Query params preserved: {}", location_str),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Query params not preserved in: {}", location_str),
                                        )
                                    }
                                }
                                None => TestResult::failed(start.elapsed(), "Missing Location header".to_string()),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// 测试重定向使用正确的 HTTPS scheme
    fn test_redirect_https_scheme() -> TestCase {
        TestCase::new(
            "http_redirect_https_scheme",
            "验证重定向 URL 使用 HTTPS scheme",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .redirect(Policy::none())
                        .danger_accept_invalid_certs(true)
                        .build()
                        .unwrap();

                    let url = format!("http://{}:{}/test", ctx.target_host, REDIRECT_PORT);

                    match client.get(&url).header("Host", "secure.example.com").send().await {
                        Ok(response) => {
                            if response.status().as_u16() != 301 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 301, got {}", response.status()),
                                );
                            }

                            match response.headers().get("location") {
                                Some(location) => {
                                    let location_str = location.to_str().unwrap_or("");

                                    if location_str.starts_with("https://") {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("HTTPS scheme verified: {}", location_str),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected https:// scheme, got: {}", location_str),
                                        )
                                    }
                                }
                                None => TestResult::failed(start.elapsed(), "Missing Location header".to_string()),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// 测试 POST 请求也会被重定向
    fn test_redirect_post_request() -> TestCase {
        TestCase::new(
            "http_redirect_post",
            "测试 POST 请求也返回重定向",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .redirect(Policy::none())
                        .danger_accept_invalid_certs(true)
                        .build()
                        .unwrap();

                    let url = format!("http://{}:{}/api/create", ctx.target_host, REDIRECT_PORT);

                    match client
                        .post(&url)
                        .header("Host", "api.example.com")
                        .body("test data")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            let status = response.status();

                            if status.as_u16() == 301 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("POST request redirected with {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 301 for POST, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for HttpRedirectTestSuite {
    fn name(&self) -> &str {
        "HTTP Redirect"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_simple_redirect(),
            Self::test_redirect_with_query(),
            Self::test_redirect_https_scheme(),
            Self::test_redirect_post_request(),
        ]
    }
}
