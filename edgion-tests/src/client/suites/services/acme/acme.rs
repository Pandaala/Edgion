// ACME Service Integration Test Suite
//
// Validates ACME account creation and credential restoration against Pebble
// (Let's Encrypt's official ACME test server).
//
// Prerequisites:
//   cd edgion-tests/integration/conf/Services/acme/pebble
//   docker compose up -d
//
// Run via test_client:
//   cargo run -p edgion-tests --bin test_client -- -s Services/acme

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

use edgion_controller::services::acme::acme_client::AcmeClient;

/// Pebble ACME directory URL
const PEBBLE_DIR: &str = "https://localhost:14000/dir";
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
        vec![Self::test_pebble_ready(), Self::test_account_create_and_restore()]
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
                            "Pebble not reachable. Start with: cd edgion-tests/integration/conf/Services/acme/pebble && docker compose up -d".to_string(),
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
}

// ============================================================================
// Helpers
// ============================================================================

/// Check that Pebble is reachable.
///
/// Uses the same CA-pinning path as `AcmeClient::new_with_ca` instead of
/// `danger_accept_invalid_certs`. The project's reqwest is built with
/// `rustls-tls-webpki-roots` only — Pebble's minica self-signed cert is not in
/// the Mozilla root set, and `danger_accept_invalid_certs` over rustls + HTTP/2
/// fails fast (~10ms) before a real reachability probe runs. Loading Pebble's
/// minica CA bundle mirrors the path the issuance tests rely on.
async fn ensure_pebble_ready() -> bool {
    // Pin Pebble's minica CA so rustls-tls-webpki-roots accepts the self-signed
    // cert (matches the AcmeClient probe path). `danger_accept_invalid_certs`
    // over rustls + HTTP/2 fails fast (~10ms) and never reaches the server.
    let Ok(ca_path) = download_pebble_tls_ca().await else {
        return false;
    };
    let Ok(ca_pem) = std::fs::read(&ca_path) else {
        return false;
    };
    let Ok(cert) = reqwest::Certificate::from_pem(&ca_pem) else {
        return false;
    };
    let Ok(client) = reqwest::Client::builder().add_root_certificate(cert).build() else {
        return false;
    };

    // Reachability == server responded. Pebble's `/dir` rejects bare GET with
    // `400 Bad Request` (it expects an ACME-formed request); any HTTP response
    // means TLS handshake + connection both succeeded, which is what
    // "pebble_ready" actually wants to assert.
    client.get(PEBBLE_DIR).send().await.is_ok()
}

/// Download Pebble's minica TLS root CA certificate from the running container.
async fn download_pebble_tls_ca() -> anyhow::Result<std::path::PathBuf> {
    let path = crate::scratch::scratch_dir().await?.join("pebble-minica-ca.pem");

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
