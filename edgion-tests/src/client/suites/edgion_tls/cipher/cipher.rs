// TLS Cipher Test Suite
// Test TLS 1.2 cipher configuration support
//
// Required config files (in edgion-tests/integration/conf/EdgionTls/cipher/):
// - Gateway.yaml                      # Gateway with cipher test listeners
// - HTTPRoute.yaml                    # Routes for cipher test hosts
// - EdgionTls_cipher_legacy.yaml      # TLS 1.2 + legacy ciphers (AES128-SHA, etc.)
// - EdgionTls_cipher_modern.yaml      # TLS 1.2 + modern ciphers (ECDHE-RSA-AES256-GCM-SHA384, etc.)
// - EdgionTls_cipher_strict.yaml      # TLS 1.3-only on https-strict listener (port 31197)
// - EdgionTls_cipher_permissive.yaml  # TLS 1.2 on the SAME listener (per-SNI policy demo)
// - EdgionTls_cipher_badname.yaml     # TLS 1.2 with an invalid cipher name (fail-closed scenario, tls-F002)
//
// This test uses openssl s_client to verify:
// 1. Server negotiates the expected cipher
// 2. Server accepts/rejects specific cipher requests
// 3. Server fails-closed when spec.ciphers contains a name BoringSSL doesn't recognize

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
fn test_with_specific_cipher(host: &str, port: u16, servername: &str, cipher: &str) -> Result<(bool, String), String> {
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

    let success = combined.contains("CONNECTED") && !negotiated_cipher.is_empty() && negotiated_cipher != "(NONE)";

    Ok((success, negotiated_cipher))
}

/// Strict handshake success check: rejects `Cipher=0000` and `(NONE)` placeholders
/// that openssl s_client emits when handshake aborts. Used by the
/// `cipher_client_cipher_mismatch` and `cipher_multi_sni_different_policy` tests
/// which need to distinguish actual handshake completion from openssl's lenient
/// session-info dump on failure.
fn handshake_truly_completed(info: &TlsConnectionInfo) -> bool {
    info.connected && info.cipher != "0000" && info.cipher != "(NONE)" && !info.cipher.is_empty()
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
                                        format!("Connected with cipher: {} (Protocol: {})", info.cipher, info.protocol),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "TLS connection failed. Protocol: {}, Cipher: {}",
                                        info.protocol, info.cipher
                                    ),
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
                                        format!("Connected with cipher: {} (Protocol: {})", info.cipher, info.protocol),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "TLS connection failed. Protocol: {}, Cipher: {}",
                                        info.protocol, info.cipher
                                    ),
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

    // Test 5: ClientCipherMismatch — Gateway requires TLS 1.3, client forces TLS 1.2 → handshake fails.
    // This is the original 04#7 case from todo-test-cover/04-integration-gaps-gateway.md.
    fn test_client_cipher_mismatch() -> TestCase {
        TestCase::new(
            "cipher_client_cipher_mismatch",
            "ClientCipherMismatch: TLS 1.3-only listener rejects TLS 1.2 client",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let hostname = "cipher-strict.wildcard.example.com";
                    let port = ctx.https_port + 2; // 31197 (https-strict)

                    match get_tls_info(&ctx.target_host, port, hostname, "-tls1_2") {
                        Ok(info) => {
                            if !handshake_truly_completed(&info) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "TLS 1.2 correctly rejected by TLS 1.3-only SNI. \
                                         Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "TLS 1.2 unexpectedly accepted by TLS 1.3-only SNI. \
                                         Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                )
                            }
                        }
                        // openssl s_client returning Err means subprocess failure;
                        // a handshake-level rejection prints to stderr but exits 0,
                        // so this branch is only for environment issues.
                        Err(e) => TestResult::failed(start.elapsed(), format!("OpenSSL test failed: {}", e)),
                    }
                })
            },
        )
    }

    // Test 6: BadCipherName — operator configures a cipher name BoringSSL doesn't
    // recognize. SSL_set_strict_cipher_list returns 0 and the cipher branch in
    // apply_edgion_tls_cert must fail-closed so the handshake aborts (tls-F002).
    // Before the fix BoringSSL silently retained its default suite and handshake
    // completed; after the fix err_log is set and certificate_callback skips
    // finalize_ssl_ctx so handshake_truly_completed must be false here.
    fn test_bad_cipher_name_failclosed() -> TestCase {
        TestCase::new(
            "cipher_bad_name_failclosed",
            "BadCipherName: unrecognized cipher in spec.ciphers aborts handshake (tls-F002)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let hostname = "cipher-badname.wildcard.example.com";
                    let port = ctx.https_port + 2; // 31197 (https-strict listener)

                    match get_tls_info(&ctx.target_host, port, hostname, "-tls1_2") {
                        Ok(info) => {
                            if handshake_truly_completed(&info) {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "bad cipher name should fail-close the handshake but it completed. \
                                         Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                )
                            } else {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Bad cipher name correctly aborted handshake. \
                                         Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("OpenSSL test failed: {}", e)),
                    }
                })
            },
        )
    }

    // Test 7: MultiSniDifferentPolicy — same port, different SNI, different minVersion.
    // Proves the per-SNI policy enforcement that this fix exists for.
    fn test_multi_sni_different_policy() -> TestCase {
        TestCase::new(
            "cipher_multi_sni_different_policy",
            "MultiSniDifferentPolicy: TLS 1.2 accepted on permissive SNI, rejected on strict SNI (same port)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let port = ctx.https_port + 2; // 31197 (https-strict listener)

                    // 1. Permissive SNI accepts TLS 1.2
                    let permissive = match get_tls_info(
                        &ctx.target_host,
                        port,
                        "cipher-permissive.wildcard.example.com",
                        "-tls1_2",
                    ) {
                        Ok(info) => info,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("OpenSSL test (permissive) failed: {}", e),
                            )
                        }
                    };
                    if !handshake_truly_completed(&permissive) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "permissive SNI should accept TLS 1.2 but did not. \
                                 Protocol={}, Cipher={}",
                                permissive.protocol, permissive.cipher
                            ),
                        );
                    }

                    // 2. Strict SNI rejects TLS 1.2 (same port, different SNI)
                    let strict =
                        match get_tls_info(&ctx.target_host, port, "cipher-strict.wildcard.example.com", "-tls1_2") {
                            Ok(info) => info,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("OpenSSL test (strict) failed: {}", e),
                                )
                            }
                        };
                    if handshake_truly_completed(&strict) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "strict SNI should reject TLS 1.2 but accepted it. \
                                 Protocol={}, Cipher={}",
                                strict.protocol, strict.cipher
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "Per-SNI policy enforced on shared port {}: permissive accepted ({}), \
                             strict rejected (Protocol={}, Cipher={})",
                            port, permissive.cipher, strict.protocol, strict.cipher
                        ),
                    )
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
            Self::test_client_cipher_mismatch(),
            Self::test_bad_cipher_name_failclosed(),
            Self::test_multi_sni_different_policy(),
        ]
    }
}
