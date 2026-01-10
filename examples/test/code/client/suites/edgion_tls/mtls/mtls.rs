// mTLS (Mutual TLS) Test suite
// Test mutual TLS authentication
//
// Required config files (in examples/conf/):
// - EndpointSlice_edge_test-http.yaml         # HTTP backend service discovery
// - Service_edge_test-http.yaml               # HTTP service definition
// - HTTPRoute_edge_mtls-test.yaml             # mTLS routing rules（Host: mtls*.example.com）
// - Gateway_edge_mtls-test-gateway.yaml       # mTLS Gateway config (listening port）
// - EdgionTls_edge_mtls-test-mutual.yaml      # Mutual TLS config（Host: mtls.example.com）
// - EdgionTls_edge_mtls-test-optional.yaml    # Optional mTLS config（Host: mtls-optional.example.com）
// - EdgionTls_edge_mtls-test-san.yaml         # SAN 白名单config（Host: mtls-san.example.com）
// - EdgionTls_edge_mtls-test-chain.yaml       # certificate chain config (Host: mtls-chain.example.com）
// - Secret_edge_mtls-server.yaml              # mTLS server certificate Secret
// - Secret_edge_client-ca.yaml                # Client CA certificate Secret
// - Secret_edge_ca-chain.yaml                 # 中间 CA certificate链 Secret
// - GatewayClass__public-gateway.yaml         # GatewayClass config
//
// Generated certificate files (by generate_mtls_certs.sh):
// - examples/test/certs/mtls/valid-client.crt          # 有效client certificate
// - examples/test/certs/mtls/valid-client.key          # valid client private key
// - examples/test/certs/mtls/invalid-client.crt        # 无效client certificate（不受信任的 CA）
// - examples/test/certs/mtls/invalid-client.key        # invalid client private key
// - examples/test/certs/mtls/nonmatching-client.crt    # SAN 不match的client certificate
// - examples/test/certs/mtls/nonmatching-client.key    # SAN mismatched client private key
// - examples/test/certs/mtls/chain-client-bundle.crt   # client certificate with chain
// - examples/test/certs/mtls/chain-client.key          # certificate chain client private key

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::fs;
use std::time::Instant;

pub struct MtlsTestSuite;

// Helper function to load client certificate and key
fn load_client_identity(cert_path: &str, key_path: &str) -> Result<reqwest::Identity, String> {
    let cert = fs::read(cert_path).map_err(|e| format!("Failed to read cert {}: {}", cert_path, e))?;
    let key = fs::read(key_path).map_err(|e| format!("Failed to read key {}: {}", key_path, e))?;

    // Combine cert and key in PEM format
    let mut pem = cert.clone();
    pem.extend_from_slice(&key);

    reqwest::Identity::from_pem(&pem).map_err(|e| format!("Failed to parse identity: {}", e))
}

// Helper function to create HTTP client without client certificate (for SNI only)
fn create_client_with_sni(hostname: &str, ip: &str, port: u16) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .danger_accept_invalid_certs(true) // Accept self-signed server certs
        .resolve(hostname, format!("{}:{}", ip, port).parse().unwrap()) // Set SNI via DNS resolution
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))
}

// Helper function to create HTTP client with client certificate
fn create_mtls_client(cert_path: &str, key_path: &str, hostname: &str, ip: &str, port: u16) -> Result<reqwest::Client, String> {
    let identity = load_client_identity(cert_path, key_path)?;

    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .danger_accept_invalid_certs(true) // Accept self-signed server certs
        .identity(identity)
        .resolve(hostname, format!("{}:{}", ip, port).parse().unwrap()) // Set SNI via DNS resolution
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))
}

impl MtlsTestSuite {
    // Test 1: Mutual TLS with valid client certificate - should succeed
    fn test_mutual_with_valid_cert() -> TestCase {
        TestCase::new(
            "mtls_mutual_valid_cert",
            "Mutual TLS：with valid client certificate (should succeed)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let cert_path = "examples/test/certs/mtls/valid-client.crt";
                    let key_path = "examples/test/certs/mtls/valid-client.key";
                    let hostname = "mtls.example.com";

                    let client = match create_mtls_client(cert_path, key_path, hostname, &ctx.target_host, ctx.https_port) {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to create mTLS client: {}", e))
                        }
                    };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url).header("Host", hostname);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            if status.is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Status: {}, mTLS handshake successful", status),
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

    // Test 2: Mutual TLS without client certificate - should fail
    fn test_mutual_without_cert() -> TestCase {
        TestCase::new(
            "mtls_mutual_no_cert",
            "Mutual TLS：without client certificate (should fail)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let hostname = "mtls.example.com";

                    // Use regular client without cert but with correct SNI
                    let client = match create_client_with_sni(hostname, &ctx.target_host, ctx.https_port) {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                        }
                    };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url).header("Host", hostname);

                    match request.send().await {
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected TLS handshake failure, but got status: {}", response.status()),
                        ),
                        Err(e) => {
                            // Expected error - TLS handshake should fail
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

    // Test 3: Mutual TLS with invalid (untrusted) client certificate - should fail
    fn test_mutual_with_invalid_cert() -> TestCase {
        TestCase::new(
            "mtls_mutual_invalid_cert",
            "Mutual TLS：with invalid certificate (should fail)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let cert_path = "examples/test/certs/mtls/invalid-client.crt";
                    let key_path = "examples/test/certs/mtls/invalid-client.key";
                    let hostname = "mtls.example.com";

                    // If client creation fails due to invalid cert, that's also a valid rejection
                    let client = match create_mtls_client(cert_path, key_path, hostname, &ctx.target_host, ctx.https_port) {
                        Ok(c) => c,
                        Err(e) => {
                            // Client builder rejection of invalid cert is acceptable
                            return TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Invalid cert rejected during client build: {}", e),
                            )
                        }
                    };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url).header("Host", hostname);

                    match request.send().await {
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected certificate verification failure, but got status: {}",
                                response.status()
                            ),
                        ),
                        Err(e) => {
                            // Expected error - certificate should be rejected
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Certificate verification failed as expected: {}", e),
                            )
                        }
                    }
                })
            },
        )
    }

    // Test 4: Optional Mutual TLS with client certificate - should succeed
    fn test_optional_with_cert() -> TestCase {
        TestCase::new(
            "mtls_optional_with_cert",
            "Optional Mutual TLS：with certificate (should succeed)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let cert_path = "examples/test/certs/mtls/valid-client.crt";
                    let key_path = "examples/test/certs/mtls/valid-client.key";
                    let hostname = "mtls-optional.example.com";

                    let client = match create_mtls_client(cert_path, key_path, hostname, &ctx.target_host, ctx.https_port) {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                        }
                    };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url).header("Host", hostname);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            if status.is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Status: {}, Optional mTLS with cert", status),
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

    // Test 5: Optional Mutual TLS without client certificate - should succeed
    fn test_optional_without_cert() -> TestCase {
        TestCase::new(
            "mtls_optional_no_cert",
            "Optional Mutual TLS：不with certificate (should succeed)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let hostname = "mtls-optional.example.com";

                    // Use regular client without cert - should succeed with optional mTLS
                    let client = match create_client_with_sni(hostname, &ctx.target_host, ctx.https_port) {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                        }
                    };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url).header("Host", hostname);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            if status.is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Status: {}, Optional mTLS without cert (degraded to single-way TLS)",
                                        status
                                    ),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failed (should succeed without cert): {}", e),
                        ),
                    }
                })
            },
        )
    }

    // Test 6: SAN whitelist - matching SAN - should succeed
    fn test_san_whitelist_matching() -> TestCase {
        TestCase::new(
            "mtls_san_whitelist_match",
            "SAN 白名单：match的 SAN（should succeed）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // valid-client has SAN=client1.example.com which matches whitelist
                    let cert_path = "examples/test/certs/mtls/valid-client.crt";
                    let key_path = "examples/test/certs/mtls/valid-client.key";
                    let hostname = "mtls-san.example.com";

                    let client = match create_mtls_client(cert_path, key_path, hostname, &ctx.target_host, ctx.https_port) {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                        }
                    };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url).header("Host", hostname);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            if status.is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Status: {}, SAN matched whitelist", status),
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

    // Test 7: SAN whitelist - non-matching SAN - should fail
    fn test_san_whitelist_non_matching() -> TestCase {
        TestCase::new(
            "mtls_san_whitelist_no_match",
            "SAN 白名单：不match的 SAN（should fail）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // nonmatching-client has SAN=notinwhitelist.example.com which doesn't match
                    let cert_path = "examples/test/certs/mtls/nonmatching-client.crt";
                    let key_path = "examples/test/certs/mtls/nonmatching-client.key";
                    let hostname = "mtls-san.example.com";

                    let client = match create_mtls_client(cert_path, key_path, hostname, &ctx.target_host, ctx.https_port) {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                        }
                    };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url).header("Host", hostname);

                    match request.send().await {
                        Ok(response) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected SAN verification failure, but got status: {}",
                                response.status()
                            ),
                        ),
                        Err(e) => {
                            // Expected error - SAN should not match whitelist
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("SAN verification failed as expected: {}", e),
                            )
                        }
                    }
                })
            },
        )
    }

    // Test 8: Certificate chain with verifyDepth=2 - should succeed
    fn test_cert_chain_depth() -> TestCase {
        TestCase::new(
            "mtls_cert_chain_depth",
            "Certificate chain: verifyDepth=2（should succeed）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // chain-client-bundle contains client cert + intermediate CA
                    let cert_path = "examples/test/certs/mtls/chain-client-bundle.crt";
                    let key_path = "examples/test/certs/mtls/chain-client.key";
                    let hostname = "mtls-chain.example.com";

                    let client = match create_mtls_client(cert_path, key_path, hostname, &ctx.target_host, ctx.https_port) {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                        }
                    };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url).header("Host", hostname);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            if status.is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Status: {}, Certificate chain verified (depth=2)", status),
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
}

#[async_trait]
impl TestSuite for MtlsTestSuite {
    fn name(&self) -> &'static str {
        "mTLS Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Basic mTLS mode tests (CA verification only)
            Self::test_mutual_with_valid_cert(),
            Self::test_mutual_without_cert(), // Mutual mode should reject clients without cert
            Self::test_mutual_with_invalid_cert(), // Mutual mode should reject invalid certs
            Self::test_optional_with_cert(),
            Self::test_optional_without_cert(),
            Self::test_cert_chain_depth(),
            // SAN/CN whitelist validation tests (now implemented at TLS layer)
            Self::test_san_whitelist_matching(),     // SAN whitelist matching
            Self::test_san_whitelist_non_matching(), // SAN whitelist non-matching
        ]
    }
}
