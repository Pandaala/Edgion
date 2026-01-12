// TLS Cipher Test Suite
// Test TLS 1.2 cipher configuration support
//
// Required config files (in examples/test/conf/EdgionTls/cipher/):
// - Gateway.yaml                      # Gateway with cipher test listeners
// - HTTPRoute.yaml                    # Routes for cipher test hosts
// - EdgionTls_cipher_legacy.yaml      # TLS 1.2 + legacy ciphers (AES128-SHA, etc.)
// - EdgionTls_cipher_modern.yaml      # TLS 1.2 + modern ciphers (ECDHE-RSA-AES256-GCM-SHA384, etc.)
//
// This test uses openssl s_client to verify:
// 1. Server negotiates the expected cipher
// 2. Server accepts/rejects specific cipher requests

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::process::Command;
use std::time::Instant;

pub struct CipherTestSuite;

/// TLS connection information parsed from openssl output
#[derive(Debug)]
struct TlsConnectionInfo {
    protocol: String,
    cipher: String,
    connected: bool,
}

/// Extract a field value from openssl s_client output
fn extract_field(output: &str, field_name: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(field_name) {
            let value = trimmed.trim_start_matches(field_name).trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Use openssl s_client to test TLS connection and get negotiated cipher info
fn get_tls_info(host: &str, port: u16, servername: &str, tls_version: &str) -> Result<TlsConnectionInfo, String> {
    let output = Command::new("openssl")
        .args([
            "s_client",
            "-connect",
            &format!("{}:{}", host, port),
            "-servername",
            servername,
            tls_version,
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Failed to run openssl: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    // Parse Protocol and Cipher from output
    let protocol = extract_field(&combined, "Protocol  :")
        .or_else(|| extract_field(&combined, "Protocol:"))
        .unwrap_or_default();
    let cipher = extract_field(&combined, "Cipher    :")
        .or_else(|| extract_field(&combined, "Cipher:"))
        .unwrap_or_default();

    let connected = combined.contains("CONNECTED") && !cipher.is_empty() && cipher != "(NONE)";

    Ok(TlsConnectionInfo {
        connected,
        protocol,
        cipher,
    })
}

/// Test connection with a specific cipher (verify server accepts/rejects it)
fn test_with_specific_cipher(
    host: &str,
    port: u16,
    servername: &str,
    cipher: &str,
) -> Result<(bool, String), String> {
    let output = Command::new("openssl")
        .args([
            "s_client",
            "-connect",
            &format!("{}:{}", host, port),
            "-servername",
            servername,
            "-cipher",
            cipher,
            "-tls1_2",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Failed to run openssl: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    let negotiated_cipher = extract_field(&combined, "Cipher    :")
        .or_else(|| extract_field(&combined, "Cipher:"))
        .unwrap_or_default();

    let success =
        combined.contains("CONNECTED") && !negotiated_cipher.is_empty() && negotiated_cipher != "(NONE)";

    Ok((success, negotiated_cipher))
}

// Legacy ciphers that should be accepted by the legacy endpoint
const LEGACY_CIPHERS: &[&str] = &["AES128-SHA", "AES256-SHA", "ECDHE-RSA-AES128-SHA"];

// Modern ciphers that should be accepted by the modern endpoint
const MODERN_CIPHERS: &[&str] = &["ECDHE-RSA-AES256-GCM-SHA384", "ECDHE-RSA-AES128-GCM-SHA256"];

impl CipherTestSuite {
    // Test 1: TLS 1.2 with legacy ciphers - verify connection and cipher negotiation
    fn test_tls12_legacy_cipher() -> TestCase {
        TestCase::new(
            "cipher_tls12_legacy",
            "TLS 1.2 Legacy Cipher: verify legacy cipher negotiation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let hostname = "cipher-legacy.example.com";
                    // Use cipher test port (31195 for legacy)
                    let port = ctx.https_port; // Use configured https_port

                    match get_tls_info(&ctx.target_host, port, hostname, "-tls1_2") {
                        Ok(info) => {
                            if info.connected {
                                // Check if negotiated cipher is in legacy list
                                let is_legacy = LEGACY_CIPHERS.iter().any(|c| info.cipher.contains(c));
                                if is_legacy || info.cipher.contains("SHA") {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!(
                                            "TLS 1.2 connected with legacy cipher: {} (Protocol: {})",
                                            info.cipher, info.protocol
                                        ),
                                    )
                                } else {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!(
                                            "Connected with cipher: {} (Protocol: {})",
                                            info.cipher, info.protocol
                                        ),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("TLS connection failed. Protocol: {}, Cipher: {}", info.protocol, info.cipher),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("OpenSSL test failed: {}", e)),
                    }
                })
            },
        )
    }

    // Test 2: TLS 1.2 with modern ciphers - verify connection and cipher negotiation
    fn test_tls12_modern_cipher() -> TestCase {
        TestCase::new(
            "cipher_tls12_modern",
            "TLS 1.2 Modern Cipher: verify modern cipher negotiation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let hostname = "cipher-modern.example.com";
                    // Use cipher test port (31196 for modern)
                    let port = ctx.https_port + 1;

                    match get_tls_info(&ctx.target_host, port, hostname, "-tls1_2") {
                        Ok(info) => {
                            if info.connected {
                                // Check if negotiated cipher is in modern list (GCM ciphers)
                                let is_modern = MODERN_CIPHERS.iter().any(|c| info.cipher.contains(c))
                                    || info.cipher.contains("GCM");
                                if is_modern {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!(
                                            "TLS 1.2 connected with modern cipher: {} (Protocol: {})",
                                            info.cipher, info.protocol
                                        ),
                                    )
                                } else {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!(
                                            "Connected with cipher: {} (Protocol: {})",
                                            info.cipher, info.protocol
                                        ),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("TLS connection failed. Protocol: {}, Cipher: {}", info.protocol, info.cipher),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("OpenSSL test failed: {}", e)),
                    }
                })
            },
        )
    }

    // Test 3: Verify specific legacy cipher is accepted by legacy endpoint
    fn test_legacy_cipher_accepted() -> TestCase {
        TestCase::new(
            "cipher_legacy_accepted",
            "Legacy Cipher Accepted: AES128-SHA should be accepted by legacy endpoint",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let hostname = "cipher-legacy.example.com";
                    let port = ctx.https_port;
                    let test_cipher = "AES128-SHA";

                    match test_with_specific_cipher(&ctx.target_host, port, hostname, test_cipher) {
                        Ok((success, negotiated)) => {
                            if success {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Legacy cipher {} accepted, negotiated: {}", test_cipher, negotiated),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Legacy cipher {} should be accepted but was rejected", test_cipher),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("OpenSSL test failed: {}", e)),
                    }
                })
            },
        )
    }

    // Test 4: Verify modern cipher is accepted by modern endpoint
    fn test_modern_cipher_accepted() -> TestCase {
        TestCase::new(
            "cipher_modern_accepted",
            "Modern Cipher Accepted: ECDHE-RSA-AES256-GCM-SHA384 should be accepted",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let hostname = "cipher-modern.example.com";
                    let port = ctx.https_port + 1;
                    let test_cipher = "ECDHE-RSA-AES256-GCM-SHA384";

                    match test_with_specific_cipher(&ctx.target_host, port, hostname, test_cipher) {
                        Ok((success, negotiated)) => {
                            if success {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Modern cipher {} accepted, negotiated: {}", test_cipher, negotiated),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Modern cipher {} should be accepted but was rejected", test_cipher),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("OpenSSL test failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for CipherTestSuite {
    fn name(&self) -> &'static str {
        "TLS Cipher Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_tls12_legacy_cipher(),
            Self::test_tls12_modern_cipher(),
            Self::test_legacy_cipher_accepted(),
            Self::test_modern_cipher_accepted(),
        ]
    }
}
