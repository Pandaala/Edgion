use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct BackendTlsGatewayCertTestSuite;

impl BackendTlsGatewayCertTestSuite {
    fn test_gateway_cert_to_mtls_backend() -> TestCase {
        TestCase::new(
            "gateway_cert_to_mtls_backend",
            "Gateway-level clientCertificateRef is used for mTLS to backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:{}/backend-gw-cert/health", ctx.target_host, ctx.http_port);
                    let mut request = ctx.http_client.get(&url);
                    if let Some(ref host) = ctx.http_host {
                        request = request.header("Host", host);
                    }
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            let body = response.text().await.unwrap_or_default();
                            if status.is_success() && body.contains("Server: 0.0.0.0:30052") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Gateway client cert mTLS succeeded. Status: {}", status),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected 200 with Server: 0.0.0.0:30052, got status: {}, body: {}",
                                        status, body
                                    ),
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
impl TestSuite for BackendTlsGatewayCertTestSuite {
    fn name(&self) -> &str {
        "backend_tls_gateway_cert"
    }

    fn port_key(&self) -> Option<&str> {
        Some("Gateway/TLS/BackendTLSGatewayCert")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_gateway_cert_to_mtls_backend()]
    }
}
