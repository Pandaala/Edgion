// ACME Service Integration Test Suite
//
// Validates the full ACME certificate issuance flow against Pebble
// (Let's Encrypt's official ACME test server).
//
// Prerequisites:
//   cd examples/test/conf/Services/acme/pebble
//   docker compose up -d
//
// Run via test_client:
//   cargo run --example test_client -- -s Services/acme

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

use edgion::core::services::acme::ctrl::acme_client::AcmeClient;
use edgion::core::services::acme::ctrl::dns_provider::{create_dns_provider, DnsProvider};
use edgion::types::resources::edgion_acme::AcmeKeyType;

/// Pebble ACME directory URL
const PEBBLE_DIR: &str = "https://localhost:14000/dir";
/// challtestsrv management API
const CHALLTEST_API: &str = "http://localhost:8055";
/// Test email
const TEST_EMAIL: &str = "test@example.com";

pub struct AcmeTestSuite;

// ============================================================================
// TestSuite impl
// ============================================================================

impl TestSuite for AcmeTestSuite {
    fn name(&self) -> &str {
        "Services/acme"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_pebble_ready(),
            Self::test_account_create_and_restore(),
            Self::test_dns01_full_flow(),
            Self::test_dns01_multi_domain(),
        ]
    }
}

// ============================================================================
// Test cases
// ============================================================================

impl AcmeTestSuite {
    /// Pre-flight: verify Pebble environment is running
    fn test_pebble_ready() -> TestCase {
        TestCase::new(
            "pebble_ready",
            "Pebble ACME test server is reachable",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    match ensure_pebble_ready().await {
                        true => TestResult::passed_with_message(
                            start.elapsed(),
                            "Pebble ACME directory reachable".to_string(),
                        ),
                        false => TestResult::failed(
                            start.elapsed(),
                            "Pebble not reachable. Start with: cd examples/test/conf/Services/acme/pebble && docker compose up -d".to_string(),
                        ),
                    }
                })
            },
        )
    }

    /// Test: ACME account creation and credential serialization/restore
    fn test_account_create_and_restore() -> TestCase {
        TestCase::new(
            "account_create_and_restore",
            "Create ACME account, serialize credentials, and restore",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Create account
                    let ca_path = match download_pebble_tls_ca().await {
                        Ok(p) => p,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("TLS CA download: {}", e)),
                    };

                    let (client, creds) = match AcmeClient::new_with_ca(PEBBLE_DIR, TEST_EMAIL, &ca_path).await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Account create: {}", e)),
                    };

                    let account_id = client.account_id().to_string();

                    // Serialize + deserialize credentials
                    let creds_json = match serde_json::to_string(&creds) {
                        Ok(j) => j,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Serialize: {}", e)),
                    };
                    let restored_creds: instant_acme::AccountCredentials = match serde_json::from_str(&creds_json) {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Deserialize: {}", e)),
                    };

                    // Restore account
                    let restored = match AcmeClient::from_credentials_with_ca(restored_creds, &ca_path).await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Restore: {}", e)),
                    };

                    if restored.account_id() != account_id {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("ID mismatch: {} != {}", restored.account_id(), account_id),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("Account {} created and restored", account_id),
                    )
                })
            },
        )
    }

    /// Test: DNS-01 full certificate issuance flow
    fn test_dns01_full_flow() -> TestCase {
        TestCase::new(
            "dns01_full_flow",
            "DNS-01 challenge: single domain certificate issuance via Pebble",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = match create_pebble_client().await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Client: {}", e)),
                    };
                    let dns_provider = create_pebble_dns_provider();
                    let domains = vec!["dns01-test.example.com".to_string()];

                    // Phase 1: Prepare order
                    let (pending, mut order_ctx) = match client.prepare_dns01_order(&domains).await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Prepare: {}", e)),
                    };

                    if pending.is_empty() {
                        return TestResult::failed(start.elapsed(), "No pending challenges".to_string());
                    }

                    // Phase 2: Create DNS TXT records
                    for ch in &pending {
                        if let Err(e) = dns_provider.create_txt_record(&ch.domain, &ch.digest).await {
                            return TestResult::failed(start.elapsed(), format!("TXT create: {}", e));
                        }
                    }

                    // Phase 3: Wait for propagation
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                    // Phase 4: Activate challenges
                    if let Err(e) = client.activate_dns01_challenges(&mut order_ctx).await {
                        return TestResult::failed(start.elapsed(), format!("Activate: {}", e));
                    }

                    // Phase 5: Complete order
                    let cert_result = match client
                        .complete_dns01_order(order_ctx, &domains, &AcmeKeyType::EcdsaP256)
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Complete: {}", e)),
                    };

                    // Validate
                    if !cert_result.certificate_pem.contains("BEGIN CERTIFICATE") {
                        return TestResult::failed(start.elapsed(), "Missing PEM certificate".to_string());
                    }
                    if !cert_result.private_key_pem.contains("BEGIN") {
                        return TestResult::failed(start.elapsed(), "Missing PEM private key".to_string());
                    }

                    // Parse cert info
                    let msg = match x509_parser::pem::parse_x509_pem(cert_result.certificate_pem.as_bytes()) {
                        Ok((_, pem)) => match pem.parse_x509() {
                            Ok(cert) => format!(
                                "Certificate issued, serial={}, not_after={:?}",
                                cert.serial.to_str_radix(16),
                                cert.validity().not_after,
                            ),
                            Err(_) => "Certificate issued (parse failed)".to_string(),
                        },
                        Err(_) => "Certificate issued (PEM parse failed)".to_string(),
                    };

                    // Cleanup
                    for ch in &pending {
                        let _ = dns_provider.remove_txt_record(&ch.domain, &ch.digest).await;
                    }

                    TestResult::passed_with_message(start.elapsed(), msg)
                })
            },
        )
    }

    /// Test: DNS-01 multi-domain (SAN) certificate
    fn test_dns01_multi_domain() -> TestCase {
        TestCase::new(
            "dns01_multi_domain",
            "DNS-01 challenge: multi-domain SAN certificate (3 domains)",
            |_ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = match create_pebble_client().await {
                        Ok(c) => c,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Client: {}", e)),
                    };
                    let dns_provider = create_pebble_dns_provider();
                    let domains = vec![
                        "multi1.example.com".to_string(),
                        "multi2.example.com".to_string(),
                        "multi3.example.com".to_string(),
                    ];

                    // Prepare
                    let (pending, mut order_ctx) = match client.prepare_dns01_order(&domains).await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Prepare: {}", e)),
                    };

                    if pending.len() != 3 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 3 challenges, got {}", pending.len()),
                        );
                    }

                    // Create DNS records
                    for ch in &pending {
                        if let Err(e) = dns_provider.create_txt_record(&ch.domain, &ch.digest).await {
                            return TestResult::failed(start.elapsed(), format!("TXT create: {}", e));
                        }
                    }

                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                    // Activate & complete
                    if let Err(e) = client.activate_dns01_challenges(&mut order_ctx).await {
                        return TestResult::failed(start.elapsed(), format!("Activate: {}", e));
                    }

                    let cert_result = match client
                        .complete_dns01_order(order_ctx, &domains, &AcmeKeyType::EcdsaP256)
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Complete: {}", e)),
                    };

                    if !cert_result.certificate_pem.contains("BEGIN CERTIFICATE") {
                        return TestResult::failed(start.elapsed(), "Missing PEM certificate".to_string());
                    }

                    // Cleanup
                    for ch in &pending {
                        let _ = dns_provider.remove_txt_record(&ch.domain, &ch.digest).await;
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "3-domain SAN certificate issued".to_string(),
                    )
                })
            },
        )
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Check that Pebble is reachable.
async fn ensure_pebble_ready() -> bool {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();

    match client.get(PEBBLE_DIR).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Download Pebble's minica TLS root CA certificate from the running container.
async fn download_pebble_tls_ca() -> anyhow::Result<std::path::PathBuf> {
    let path = std::env::temp_dir().join("pebble-minica-ca.pem");

    if path.exists() {
        return Ok(path);
    }

    let output = tokio::process::Command::new("docker")
        .args([
            "cp",
            "pebble-pebble-1:/test/certs/pebble.minica.pem",
            path.to_str().unwrap(),
        ])
        .output()
        .await?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to extract Pebble TLS CA: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(path)
}

/// Create an AcmeClient connected to Pebble.
async fn create_pebble_client() -> anyhow::Result<AcmeClient> {
    let ca_path = download_pebble_tls_ca().await?;
    let (client, _creds) = AcmeClient::new_with_ca(PEBBLE_DIR, TEST_EMAIL, &ca_path).await?;
    Ok(client)
}

/// Create the Pebble DNS provider (calls challtestsrv REST API).
fn create_pebble_dns_provider() -> Box<dyn DnsProvider> {
    let mut creds = std::collections::HashMap::new();
    creds.insert("api-url".to_string(), CHALLTEST_API.to_string());
    create_dns_provider("pebble", &creds).expect("Failed to create pebble DNS provider")
}
