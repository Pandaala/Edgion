// Gateway frontendValidation mTLS Test Suite
// Tests Gateway API standard frontendValidation field for mTLS at the Listener level.
//
// Required config files (in examples/test/conf/Gateway/FrontendValidation/):
// - Gateway_default_frontend-validation-gateway.yaml  # Gateway with frontendValidation
// - HTTPRoute_default_fv-route.yaml                   # Route for fv-mtls.example.com
//
// Reuses existing mTLS certificates from EdgionTls tests:
// - Secret_edgion-test_mtls-server (server cert, cross-namespace ref)
// - Secret_edgion-test_client-ca (client CA, cross-namespace ref)
// - examples/test/certs/mtls/valid-client.crt + .key
// - examples/test/certs/mtls/invalid-client.crt + .key

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::fs;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Instant;

const PORT: u16 = 31312;
const HOSTNAME: &str = "fv-mtls.example.com";

pub struct FrontendValidationTestSuite;

// Helper function to load client certificate and key
fn load_client_identity(cert_path: &str, key_path: &str) -> Result<reqwest::Identity, String> {
    let cert = fs::read(cert_path).map_err(|e| format!("Failed to read cert {}: {}", cert_path, e))?;
    let key = fs::read(key_path).map_err(|e| format!("Failed to read key {}: {}", key_path, e))?;

    let mut pem = cert.clone();
    pem.extend_from_slice(&key);

    reqwest::Identity::from_pem(&pem).map_err(|e| format!("Failed to parse identity: {}", e))
}

fn resolve_target(ip_or_host: &str, port: u16) -> Result<SocketAddr, String> {
    let mut addrs = format!("{}:{}", ip_or_host, port)
        .to_socket_addrs()
        .map_err(|e| format!("Failed to resolve target {}:{}: {}", ip_or_host, port, e))?;
    addrs
        .next()
        .ok_or_else(|| format!("No resolved address for {}:{}", ip_or_host, port))
}

fn create_client_with_sni(hostname: &str, ip: &str, port: u16) -> Result<reqwest::Client, String> {
    let target = resolve_target(ip, port)?;
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .resolve(hostname, target)
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))
}

fn create_mtls_client(
    cert_path: &str,
    key_path: &str,
    hostname: &str,
    ip: &str,
    port: u16,
) -> Result<reqwest::Client, String> {
    let identity = load_client_identity(cert_path, key_path)?;
    let target = resolve_target(ip, port)?;

    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .identity(identity)
        .resolve(hostname, target)
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))
}

impl FrontendValidationTestSuite {
    // Test 1: With valid client certificate - should succeed (200 OK)
    fn test_valid_cert() -> TestCase {
        TestCase::new(
            "fv_mtls_valid_cert",
            "frontendValidation mTLS: with valid client certificate (should succeed)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let cert_path = "examples/test/certs/mtls/valid-client.crt";
                    let key_path = "examples/test/certs/mtls/valid-client.key";

                    let client = match create_mtls_client(cert_path, key_path, HOSTNAME, &ctx.target_host, PORT) {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to create mTLS client: {}", e))
                        }
                    };

                    let url = format!("https://{}:{}/health", HOSTNAME, PORT);
                    let request = client.get(&url).header("Host", HOSTNAME);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            if status.is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Status: {}, frontendValidation mTLS handshake successful", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // Test 2: Without client certificate - should fail (TLS handshake failure)
    fn test_no_cert() -> TestCase {
        TestCase::new(
            "fv_mtls_no_cert",
            "frontendValidation mTLS: without client certificate (should fail)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = match create_client_with_sni(HOSTNAME, &ctx.target_host, PORT) {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                        }
                    };

                    let url = format!("https://{}:{}/health", HOSTNAME, PORT);
                    let request = client.get(&url).header("Host", HOSTNAME);

                    match request.send().await {
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected TLS handshake failure, but got status: {}", response.status()),
                        ),
                        Err(e) => {
                            let error_msg = e.to_string();
                            if error_msg.contains("tls")
                                || error_msg.contains("handshake")
                                || error_msg.contains("certificate")
                            {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "TLS handshake failed as expected (no client cert)".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Unexpected error: {}", error_msg))
                            }
                        }
                    }
                })
            },
        )
    }

    // Test 3: With invalid (untrusted) client certificate - should fail
    fn test_invalid_cert() -> TestCase {
        TestCase::new(
            "fv_mtls_invalid_cert",
            "frontendValidation mTLS: with invalid certificate (should fail)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let cert_path = "examples/test/certs/mtls/invalid-client.crt";
                    let key_path = "examples/test/certs/mtls/invalid-client.key";

                    let client = match create_mtls_client(cert_path, key_path, HOSTNAME, &ctx.target_host, PORT) {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Invalid cert rejected during client build: {}", e),
                            );
                        }
                    };

                    let url = format!("https://{}:{}/health", HOSTNAME, PORT);
                    let request = client.get(&url).header("Host", HOSTNAME);

                    match request.send().await {
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected certificate verification failure, but got status: {}",
                                response.status()
                            ),
                        ),
                        Err(e) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("Certificate verification failed as expected: {}", e),
                        ),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for FrontendValidationTestSuite {
    fn name(&self) -> &'static str {
        "Gateway/FrontendValidation"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_valid_cert(), Self::test_no_cert(), Self::test_invalid_cert()]
    }
}
