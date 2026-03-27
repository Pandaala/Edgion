use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct BackendTlsSanTestSuite;

impl BackendTlsSanTestSuite {
    fn test_san_hostname_match() -> TestCase {
        TestCase::new(
            "san_hostname_match",
            "Backend SAN validation passes when cert DNS SAN matches policy Hostname",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:{}/backend-san-match/health", ctx.target_host, ctx.http_port);
                    let mut request = ctx.http_client.get(&url);
                    if let Some(ref host) = ctx.http_host {
                        request = request.header("Host", host);
                    }
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            let body = response.text().await.unwrap_or_default();
                            if status.is_success() && body.contains("Server: 0.0.0.0:30051") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("SAN match succeeded. Status: {}", status),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected success, got status: {}, body: {}", status, body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_san_hostname_mismatch() -> TestCase {
        TestCase::new(
            "san_hostname_mismatch",
            "Backend SAN validation rejects when cert DNS SAN does not match policy Hostname",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!(
                        "http://{}:{}/backend-san-mismatch/health",
                        ctx.target_host, ctx.http_port
                    );
                    let mut request = ctx.http_client.get(&url);
                    if let Some(ref host) = ctx.http_host {
                        request = request.header("Host", host);
                    }
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            let body = response.text().await.unwrap_or_default();
                            if status.as_u16() == 502 || status.is_server_error() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("SAN mismatch correctly rejected. Status: {}", status),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 502/5xx for SAN mismatch, got: {}, body: {}", status, body),
                                )
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
impl TestSuite for BackendTlsSanTestSuite {
    fn name(&self) -> &str {
        "backend_tls_san"
    }

    fn port_key(&self) -> Option<&str> {
        Some("Gateway/TLS/BackendTLSSAN")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_san_hostname_match(), Self::test_san_hostname_mismatch()]
    }
}
