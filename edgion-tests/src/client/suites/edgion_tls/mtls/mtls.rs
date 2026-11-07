// mTLS (Mutual TLS) Test suite
// Test mutual TLS authentication
//
// Required config files (in edgion-tests/integration/conf/):
// - EndpointSlice_edge_test-http.yaml         # HTTP backend service discovery
// - Service_edge_test-http.yaml               # HTTP service definition
// - HTTPRoute_edge_mtls-test.yaml             # mTLS routing rules (Host: mtls*.example.com)
// - Gateway_edge_mtls-test-gateway.yaml       # mTLS Gateway config (listening port)
// - EdgionTls_edge_mtls-test-mutual.yaml      # Mutual TLS config（Host: mtls.example.com）
// - EdgionTls_edge_mtls-test-optional.yaml    # Optional mTLS config（Host: mtls-optional.example.com）
// - EdgionTls_edge_mtls-test-san.yaml         # SAN config（Host: mtls-san.example.com）
// - EdgionTls_edge_mtls-test-chain.yaml       # certificate chain config (Host: mtls-chain.example.com）
// - EdgionTls_edge_mtls-test-terminate.yaml   # Terminate mode（Host: mtls-terminate.example.com）
// - Secret_edge_mtls-server.yaml              # mTLS server certificate Secret
// - Secret_edge_client-ca.yaml                # Client CA certificate Secret
// - Secret_edge_ca-chain.yaml                 #  CA certificate Secret
// - GatewayClass__public-gateway.yaml         # GatewayClass config
//
// Generated certificate files (by generate_mtls_certs.sh):
// - edgion-tests/certs/mtls/valid-client.crt          # client certificate
// - edgion-tests/certs/mtls/valid-client.key          # valid client private key
// - edgion-tests/certs/mtls/invalid-client.crt        # invalid client certificate (untrusted CA)
// - edgion-tests/certs/mtls/invalid-client.key        # invalid client private key
// - edgion-tests/certs/mtls/nonmatching-client.crt    # SAN-mismatched client certificate
// - edgion-tests/certs/mtls/nonmatching-client.key    # SAN mismatched client private key
// - edgion-tests/certs/mtls/chain-client-bundle.crt   # client certificate with chain (leaf + intermediate)
// - edgion-tests/certs/mtls/chain-client.crt          # leaf-only client certificate (intermediate-signed)
// - edgion-tests/certs/mtls/chain-client.key          # certificate chain client private key

use super::super::mtls_failclose::{
    require_client_trust, require_dns_san, TlsProbe, INVALID_CLIENT, MTLS_SERVER, NONMATCHING_CLIENT, VALID_CLIENT,
};
use super::resumption::require_cross_sni_resumption_refused;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use rustls::AlertDescription;
use std::fs;
use std::net::{SocketAddr, ToSocketAddrs};
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
        .danger_accept_invalid_certs(true) // Accept self-signed server certs
        .no_proxy() // Disable proxy for local testing
        .resolve(hostname, target) // Set SNI via DNS resolution
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))
}

// Helper function to create HTTP client with client certificate
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
        .danger_accept_invalid_certs(true) // Accept self-signed server certs
        .no_proxy() // Disable proxy for local testing
        .identity(identity)
        .resolve(hostname, target) // Set SNI via DNS resolution
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))
}

// All preparation and control failures remain failed preconditions. A peer
// alert becomes policy evidence only after these independent controls succeed.
async fn require_mutual_rejection(
    ctx: &TestContext,
    hostname: &str,
    identity: Option<(&str, &str)>,
    expected: AlertDescription,
) -> Result<(), String> {
    let negative = TlsProbe::new(hostname, MTLS_SERVER, identity)?;
    require_client_trust(VALID_CLIENT.0, true)?;
    if let Some((cert, _)) = identity {
        require_client_trust(cert, expected != AlertDescription::UnknownCA)?;
    }
    if expected == AlertDescription::InternalError {
        require_dns_san(VALID_CLIENT.0, "client1.example.com", true)?;
        require_dns_san(NONMATCHING_CLIENT.0, "client1.example.com", false)?;
        TlsProbe::new("mtls.example.com", MTLS_SERVER, Some(NONMATCHING_CLIENT))?
            .run(&ctx.target_host, ctx.https_port)
            .await?
            .require_complete()?;
    }
    TlsProbe::new(hostname, MTLS_SERVER, Some(VALID_CLIENT))?
        .run(&ctx.target_host, ctx.https_port)
        .await?
        .require_complete()?;
    negative
        .run(&ctx.target_host, ctx.https_port)
        .await?
        .require_authenticated_alert(expected)
}

impl MtlsTestSuite {
    fn test_mutual_with_valid_cert() -> TestCase {
        TestCase::new(
            "mtls_mutual_valid_cert",
            "Mutual TLS：with valid client certificate (should succeed)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let cert_path = "edgion-tests/certs/mtls/valid-client.crt";
                    let key_path = "edgion-tests/certs/mtls/valid-client.key";
                    let hostname = "mtls.example.com";

                    let client =
                        match create_mtls_client(cert_path, key_path, hostname, &ctx.target_host, ctx.https_port) {
                            Ok(c) => c,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Failed to create mTLS client: {}", e),
                                )
                            }
                        };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url);

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

    fn test_mutual_without_cert() -> TestCase {
        TestCase::new(
            "mtls_mutual_no_cert",
            "Mutual TLS: without client certificate (should fail)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match require_mutual_rejection(&ctx, "mtls.example.com", None, AlertDescription::HandshakeFailure)
                        .await
                    {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Peer rejected the missing certificate after the valid-client control completed"
                                .to_string(),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    fn test_mutual_with_invalid_cert() -> TestCase {
        TestCase::new(
            "mtls_mutual_invalid_cert",
            "Mutual TLS: with valid untrusted certificate (should fail)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match require_mutual_rejection(&ctx, "mtls.example.com", Some(INVALID_CLIENT), AlertDescription::UnknownCA).await {
                    Ok(()) => TestResult::passed_with_message(start.elapsed(), "Peer returned UnknownCA for the prepared untrusted identity; valid-client control completed".to_string()),
                    Err(error) => TestResult::failed(start.elapsed(), error),
                }
                })
            },
        )
    }

    fn test_optional_with_cert() -> TestCase {
        TestCase::new(
            "mtls_optional_with_cert",
            "Optional Mutual TLS：with certificate (should succeed)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let cert_path = "edgion-tests/certs/mtls/valid-client.crt";
                    let key_path = "edgion-tests/certs/mtls/valid-client.key";
                    let hostname = "mtls-optional.example.com";

                    let client =
                        match create_mtls_client(cert_path, key_path, hostname, &ctx.target_host, ctx.https_port) {
                            Ok(c) => c,
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                            }
                        };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url);

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

    fn test_optional_without_cert() -> TestCase {
        TestCase::new(
            "mtls_optional_no_cert",
            "Optional Mutual TLS：with certificate (should succeed)",
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
                    let request = client.get(&url);

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

    fn test_san_whitelist_matching() -> TestCase {
        TestCase::new(
            "mtls_san_whitelist_match",
            "SAN ：match SAN（should succeed）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // valid-client has SAN=client1.example.com which matches whitelist
                    let cert_path = "edgion-tests/certs/mtls/valid-client.crt";
                    let key_path = "edgion-tests/certs/mtls/valid-client.key";
                    let hostname = "mtls-san.example.com";

                    let client =
                        match create_mtls_client(cert_path, key_path, hostname, &ctx.target_host, ctx.https_port) {
                            Ok(c) => c,
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                            }
                        };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url);

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

    fn test_san_whitelist_non_matching() -> TestCase {
        TestCase::new(
            "mtls_san_whitelist_no_match",
            "SAN whitelist: trusted client with nonmatching DNS SAN (should fail)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match require_mutual_rejection(
                        &ctx,
                        "mtls-san.example.com",
                        Some(NONMATCHING_CLIENT),
                        // BoringSSL changes a callback rejection with X509_V_OK
                        // to UNSPECIFIED, which yields InternalError. The trust,
                        // SAN and two-sided acceptance controls isolate this path.
                        AlertDescription::InternalError,
                    )
                    .await
                    {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Peer rejected the trusted SAN-mismatch identity after both isolation controls completed"
                                .to_string(),
                        ),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    // Terminate mode performs TLS termination only; it must not request a client cert.
    // A failure here indicates Terminate was accidentally wired into mTLS verification.
    fn test_terminate_without_cert() -> TestCase {
        TestCase::new(
            "mtls_terminate_no_cert",
            "Terminate mode：without client certificate (should succeed)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let hostname = "mtls-terminate.example.com";

                    let client = match create_client_with_sni(hostname, &ctx.target_host, ctx.https_port) {
                        Ok(c) => c,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                        }
                    };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            if status.is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Status: {}, Terminate mode (no client cert requested)", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failed (Terminate must not require client cert): {}", e),
                        ),
                    }
                })
            },
        )
    }

    // Even when the client offers a cert, Terminate mode must not validate it
    // (configure_mtls returns early without setting any verify mode).
    fn test_terminate_with_cert() -> TestCase {
        TestCase::new(
            "mtls_terminate_with_cert",
            "Terminate mode：with client certificate (should succeed, cert ignored)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let cert_path = "edgion-tests/certs/mtls/valid-client.crt";
                    let key_path = "edgion-tests/certs/mtls/valid-client.key";
                    let hostname = "mtls-terminate.example.com";

                    let client =
                        match create_mtls_client(cert_path, key_path, hostname, &ctx.target_host, ctx.https_port) {
                            Ok(c) => c,
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                            }
                        };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            if status.is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Status: {}, Terminate ignored client cert as expected", status),
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

    fn test_cert_chain_depth() -> TestCase {
        TestCase::new(
            "mtls_cert_chain_depth",
            "Certificate chain: verifyDepth=2（should succeed）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // chain-client-bundle contains client cert + intermediate CA
                    let cert_path = "edgion-tests/certs/mtls/chain-client-bundle.crt";
                    let key_path = "edgion-tests/certs/mtls/chain-client.key";
                    let hostname = "mtls-chain.example.com";

                    let client =
                        match create_mtls_client(cert_path, key_path, hostname, &ctx.target_host, ctx.https_port) {
                            Ok(c) => c,
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                            }
                        };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url);

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

    // Verifies that the gateway loads all PEM blocks from a multi-block CA bundle.
    // The intermediate CA lives only inside the `ca-chain` Secret (root + intermediate);
    // the client presents only the leaf cert. The gateway must load every PEM block
    // to build the chain to root.
    fn test_cert_chain_intermediate_in_ca_store() -> TestCase {
        TestCase::new(
            "mtls_cert_chain_intermediate_in_ca_store",
            "CA bundle multi-PEM: leaf-only client, intermediate loaded from ca.crt bundle（should succeed）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Leaf-only client cert (signed by the intermediate). The
                    // intermediate is intentionally NOT bundled with the client
                    // identity — it must come from the gateway's ca-chain Secret.
                    let cert_path = "edgion-tests/certs/mtls/chain-client.crt";
                    let key_path = "edgion-tests/certs/mtls/chain-client.key";
                    let hostname = "mtls-chain.example.com";

                    let client =
                        match create_mtls_client(cert_path, key_path, hostname, &ctx.target_host, ctx.https_port) {
                            Ok(c) => c,
                            Err(e) => {
                                return TestResult::failed(start.elapsed(), format!("Failed to create client: {}", e))
                            }
                        };

                    let url = format!("https://{}:{}/health", hostname, ctx.https_port);
                    let request = client.get(&url);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status();
                            if status.is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Status: {}, intermediate loaded from CA bundle", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Handshake failed — intermediate not loaded from the multi-PEM CA bundle? {}",
                                e
                            ),
                        ),
                    }
                })
            },
        )
    }

    fn test_cross_sni_resumption_refused() -> TestCase {
        TestCase::new(
            "mtls_cross_sni_resumption_refused",
            "Session from the terminate host must not resume under the Mutual host's SNI (TLS 1.2 and 1.3)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
                        if let Err(error) =
                            require_cross_sni_resumption_refused(&ctx.target_host, ctx.https_port, version).await
                        {
                            return TestResult::failed(start.elapsed(), format!("{:?}: {error}", version.version));
                        }
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Same-SNI control resumed; cross-SNI offer fell back to a full handshake and was rejected"
                            .to_string(),
                    )
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
            Self::test_cert_chain_intermediate_in_ca_store(),
            // Terminate mode (TLS termination only, no client cert verification)
            Self::test_terminate_without_cert(),
            Self::test_terminate_with_cert(),
            // SAN whitelist validation tests (TLS layer)
            Self::test_san_whitelist_matching(),
            Self::test_san_whitelist_non_matching(),
            // Session resumption must not cross client-auth policies (gw-infra-01)
            Self::test_cross_sni_resumption_refused(),
        ]
    }
}
