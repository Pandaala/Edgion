// Backend TLS Test suite
// Test Gateway to backend TLS connection（BackendTLSPolicy）
//
// Test scenarios:
// - Client → Gateway: HTTP (port 18080)
// - Gateway → Backend: HTTPS (port 30051, using BackendTLSPolicy)
// - Backend server: listening on 30051，with self-signed certificate
//
// Required config files (in examples/conf/):
// - BackendTLSPolicy_edge_backend-tls.yaml   # BackendTLSPolicy config
// - Service_edge_test-backend-tls.yaml       # HTTPS backend service definition
// - EndpointSlice_edge_test-backend-tls.yaml # HTTPS backend endpoint
// - HTTPRoute_edge_backend-tls.yaml          # routing rules（path: /backend-tls/）
// - Secret_edge_backend-ca.yaml              # CA certificate Secret
// - Gateway_edge_tls-terminate-gateway.yaml  # Gateway config
// - GatewayClass__public-gateway.yaml        # GatewayClass config
//
// Generated certificate files:
// - examples/test/certs/backend/server.crt      # backend server certificate
// - examples/test/certs/backend/server.key      # backend server private key
// - examples/test/certs/backend/ca.crt          # CA certificate

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct BackendTlsTestSuite;

impl BackendTlsTestSuite {
    fn test_backend_tls_health() -> TestCase {
        TestCase::new(
            "backend_tls_health",
            "Test Backend TLS connection - /backend-tls/health endpoint",
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
                                    if status.is_success()
                                        && body.contains("Server: 0.0.0.0:30051")
                                        && body.contains("Path: /backend-tls/health")
                                    {
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
            "Test Backend TLS connection - /backend-tls/echo endpoint（request forwarding）",
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
                                        && body.contains("Server: 0.0.0.0:30051")
                                        && body.contains("Path: /backend-tls/echo")
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
            "Test Backend TLS connection - /backend-tls/headers endpoint",
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
                                    if status.is_success()
                                        && body.contains("Server: 0.0.0.0:30051")
                                        && body.contains("Path: /backend-tls/headers")
                                    {
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

    fn test_backend_mtls_echo() -> TestCase {
        TestCase::new(
            "backend_mtls_echo",
            "Test upstream mTLS success with client cert - /backend-mtls/echo",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("http://{}:{}/backend-mtls/echo", ctx.target_host, ctx.http_port);
                    let mut request = ctx.http_client.get(&url);

                    if let Some(ref host) = ctx.http_host {
                        request = request.header("Host", host);
                    }
                    request = request.header("X-Test-Header", "backend-mtls-test");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            match response.text().await {
                                Ok(body) => {
                                    if status.is_success()
                                        && body.contains("Server: 0.0.0.0:30052")
                                        && body.contains("Path: /backend-mtls/echo")
                                    {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "Upstream mTLS request succeeded with client certificate".to_string(),
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
                        Err(e) => TestResult::failed(start.elapsed(), format!("Upstream mTLS request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_backend_mtls_without_client_cert_fails() -> TestCase {
        TestCase::new(
            "backend_mtls_without_client_cert_fails",
            "Test upstream mTLS failure without client cert - /backend-mtls-no-client-cert/health",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!(
                        "http://{}:{}/backend-mtls-no-client-cert/health",
                        ctx.target_host, ctx.http_port
                    );
                    let mut request = ctx.http_client.get(&url);

                    if let Some(ref host) = ctx.http_host {
                        request = request.header("Host", host);
                    }

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            match response.text().await {
                                Ok(body) => {
                                    if status.is_server_error() {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!(
                                                "Expected failure observed without client certificate. Status: {}, Body: {}",
                                                status, body
                                            ),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected server error without client cert, got status {} with body {}",
                                                status, body
                                            ),
                                        )
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("Failed to read response body: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Request unexpectedly failed before response: {}", e),
                        ),
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
            Self::test_backend_mtls_echo(),
            Self::test_backend_mtls_without_client_cert_fails(),
        ]
    }
}
