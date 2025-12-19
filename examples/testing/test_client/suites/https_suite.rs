// HTTPS 测试套件
// 只在 Gateway 模式下测试，使用 /secure/ 路径前缀区分

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct HttpsTestSuite;

impl HttpsTestSuite {
    fn test_https_secure_health() -> TestCase {
        TestCase::new(
            "https_secure_health",
            "测试 HTTPS /secure/health 端点",
            |ctx: TestContext| Box::pin(async move {
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
                                        format!("Status: {}, Body: {}", status, body)
                                    )
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        format!("Unexpected response. Status: {}, Body: {}", status, body)
                                    )
                                }
                            },
                            Err(e) => TestResult::failed(
                                start.elapsed(),
                                format!("Failed to read response body: {}", e)
                            ),
                        }
                    },
                    Err(e) => TestResult::failed(
                        start.elapsed(),
                        format!("HTTPS request failed: {}", e)
                    ),
                }
            })
        )
    }
    
    fn test_https_secure_echo() -> TestCase {
        TestCase::new(
            "https_secure_echo",
            "测试 HTTPS /secure/echo 端点",
            |ctx: TestContext| Box::pin(async move {
                let start = Instant::now();
                
                let url = format!("https://{}:{}/secure/echo", ctx.target_host, ctx.https_port);
                
                let mut request = ctx.http_client.post(&url)
                    .body("HTTPS Test Message");
                
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
                                        format!("Echo successful: {}", body)
                                    )
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        format!("Echo failed. Status: {}, Body: {}", status, body)
                                    )
                                }
                            },
                            Err(e) => TestResult::failed(
                                start.elapsed(),
                                format!("Failed to read response: {}", e)
                            ),
                        }
                    },
                    Err(e) => TestResult::failed(
                        start.elapsed(),
                        format!("HTTPS request failed: {}", e)
                    ),
                }
            })
        )
    }
    
    fn test_https_secure_status() -> TestCase {
        TestCase::new(
            "https_secure_status",
            "测试 HTTPS /secure/status/200 端点",
            |ctx: TestContext| Box::pin(async move {
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
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Status code: {}", status)
                            )
                        } else {
                            TestResult::failed(
                                start.elapsed(),
                                format!("Expected 200, got {}", status)
                            )
                        }
                    },
                    Err(e) => TestResult::failed(
                        start.elapsed(),
                        format!("HTTPS request failed: {}", e)
                    ),
                }
            })
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

