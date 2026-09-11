// tls-F011 mTLS fail-close integration tests
//
// Required fixtures (in edgion-tests/integration/conf/EdgionTls/mtls_failclose/):
// - Gateway.yaml                            # 31199 (EdgionTls SNI listener) + 31340 (frontendValidation listener)
// - HTTPRoute.yaml                          # routes for the 3 hostnames
// - EdgionTls_mtls-ca-missing.yaml          # caSecretRef -> nonexistent-ca
// - EdgionTls_mtls-ca-garbage.yaml          # caSecretRef -> garbage-ca
// - Secret_garbage-ca.yaml                  # ca.crt = `not a pem`
// - Secret_garbage-frontend-ca.yaml         # ca.crt = `not a pem`
//
// Required generated secrets (by generate_mtls_certs.sh, written to
// tmp/integration_testing/generated_secrets_manual/):
// - mtls-server (namespace edgion-test)     # reused server cert
// - fv-server-cert (namespace edgion-test)  # reused server cert for frontendValidation listener
//
// Helpers (extract_field, get_tls_info, handshake_truly_completed) are
// intentionally duplicated from `cipher/cipher.rs`: the three call sites do not
// justify a shared abstraction, and keeping the security-critical
// handshake-completion check inline makes the audit site self-contained.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct MtlsFailcloseTestSuite;

/// TLS connection information parsed from openssl s_client output.
#[derive(Debug)]
struct TlsConnectionInfo {
    protocol: String,
    cipher: String,
    connected: bool,
}

/// Extract a field value from openssl s_client output (one of "Protocol  :" / "Cipher    :" / ...).
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

/// Drive openssl s_client and return what was negotiated.
fn get_tls_info(host: &str, port: u16, servername: &str) -> Result<TlsConnectionInfo, String> {
    let mut child = Command::new("openssl")
        .args([
            "s_client",
            "-connect",
            &format!("{}:{}", host, port),
            "-servername",
            servername,
            "-tls1_2",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run openssl: {}", e))?;

    let start = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|e| format!("Failed to wait for openssl: {}", e))?
            .is_some()
        {
            break;
        }
        if start.elapsed() >= Duration::from_secs(10) {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to collect openssl output: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    let protocol = extract_field(&combined, "Protocol  :")
        .or_else(|| extract_field(&combined, "Protocol:"))
        .unwrap_or_default();
    let cipher = extract_field(&combined, "Cipher    :")
        .or_else(|| extract_field(&combined, "Cipher:"))
        .unwrap_or_default();

    let connected = combined.contains("CONNECTED");

    Ok(TlsConnectionInfo {
        connected,
        protocol,
        cipher,
    })
}

/// Strict handshake-completion check: rejects the "Cipher=0000" / "(NONE)"
/// placeholders openssl s_client emits when handshake aborts. Mirror of the
/// cipher/cipher.rs helper.
fn handshake_truly_completed(info: &TlsConnectionInfo) -> bool {
    info.connected && info.cipher != "0000" && info.cipher != "(NONE)" && !info.cipher.is_empty()
}

fn listener_has_condition(
    gateway: &Value,
    listener_name: &str,
    condition_type: &str,
    expected_status: &str,
    expected_reason: &str,
) -> bool {
    gateway["status"]["listeners"]
        .as_array()
        .and_then(|listeners| listeners.iter().find(|listener| listener["name"] == listener_name))
        .and_then(|listener| listener["conditions"].as_array())
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                condition["type"] == condition_type
                    && condition["status"] == expected_status
                    && condition["reason"] == expected_reason
            })
        })
}

fn is_k8s_mode() -> bool {
    std::env::var("EDGION_TEST_K8S_MODE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

/// Derive the ssl.log path from TestContext.access_log_path (same directory).
fn ssl_log_path(ctx: &TestContext) -> PathBuf {
    ctx.access_log_path
        .parent()
        .map(|p| p.join("ssl.log"))
        .unwrap_or_else(|| PathBuf::from("logs/ssl.log"))
}

/// Scan ssl.log for an entry matching the given SNI whose err_log starts with
/// `expected_prefix` and which carries no handshake_complete_time field.
/// Returns Ok on first match. Returns Err with all entries for the SNI if none
/// matched (caller emits the diagnostic via TestResult::failed).
fn assert_ssl_log_err_log(ctx: &TestContext, servername: &str, expected_prefix: &str) -> Result<String, String> {
    let path = ssl_log_path(ctx);
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read ssl.log at {}: {}", path.display(), e))?;

    let mut matching_entries: Vec<String> = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Cheap structural match: the entry is single-line JSON. The sni field
        // is what we filter on; a substring check is enough (the value is
        // wrapped in quotes and the hostname strings used by these tests do
        // not collide as substrings of each other).
        let sni_needle = format!("\"sni\":\"{}\"", servername);
        if !line.contains(&sni_needle) {
            continue;
        }
        matching_entries.push(line.to_string());

        let err_needle = format!("\"err_log\":\"{}", expected_prefix);
        let no_handshake_field = !line.contains("\"handshake_complete_time\"");
        if line.contains(&err_needle) && no_handshake_field {
            return Ok(line.to_string());
        }
    }

    if matching_entries.is_empty() {
        Err(format!(
            "ssl.log at {} has no entry for sni={}",
            path.display(),
            servername
        ))
    } else {
        Err(format!(
            "ssl.log at {} had {} entry/entries for sni={}, none matching err_log prefix '{}' \
             with no handshake_complete_time. Entries: {}",
            path.display(),
            matching_entries.len(),
            servername,
            expected_prefix,
            matching_entries.join(" | ")
        ))
    }
}

async fn wait_for_ssl_log_err_log(
    ctx: &TestContext,
    servername: &str,
    expected_prefixes: &[&str],
) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let mut last_error = String::new();
        for expected_prefix in expected_prefixes {
            match assert_ssl_log_err_log(ctx, servername, expected_prefix) {
                Ok(entry) => return Ok(entry),
                Err(error) => last_error = error,
            }
        }
        if Instant::now() >= deadline {
            return Err(last_error);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

impl MtlsFailcloseTestSuite {
    // Case 1: EdgionTls caSecretRef points to a Secret that doesn't exist.
    // ca_cert_pem() returns "CA secret not loaded by controller", configure_mtls
    // returns Err, apply_edgion_tls_cert stamps err_log = "mtls config: ..." and
    // returns BEFORE cert/key bind, so BoringSSL aborts the handshake.
    fn test_mtls_ca_missing_failclosed() -> TestCase {
        TestCase::new(
            "mtls_ca_missing_failclosed",
            "EdgionTls caSecretRef -> nonexistent Secret aborts handshake (tls-F011)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let hostname = "mtls-ca-missing.example.com";
                    let port: u16 = 31199;

                    match get_tls_info(&ctx.target_host, port, hostname) {
                        Ok(info) => {
                            if handshake_truly_completed(&info) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Mutual mTLS with missing CA Secret should abort the handshake, \
                                         but it completed. Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                );
                            }
                            if is_k8s_mode() {
                                return TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Handshake aborted in K8s mode (Protocol={}, Cipher={}); gateway ssl.log is not mounted in the test-client pod",
                                        info.protocol, info.cipher
                                    ),
                                );
                            }
                            match wait_for_ssl_log_err_log(&ctx, hostname, &["mtls config:"]).await {
                                Ok(_) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Handshake aborted (Protocol={}, Cipher={}) and ssl.log recorded \
                                         err_log starting with 'mtls config:'",
                                        info.protocol, info.cipher
                                    ),
                                ),
                                Err(e) => TestResult::failed(start.elapsed(), e),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("OpenSSL test failed: {}", e)),
                    }
                })
            },
        )
    }

    // Case 2: EdgionTls caSecretRef resolves to a Secret whose ca.crt is garbage.
    // add_ca_bundle_to_store returns `Invalid CA certificate PEM` Err; same
    // err_log prefix `mtls config:` as case 1 (caller wraps both Err sources).
    fn test_mtls_ca_garbage_pem_failclosed() -> TestCase {
        TestCase::new(
            "mtls_ca_garbage_pem_failclosed",
            "EdgionTls caSecretRef -> garbage ca.crt aborts handshake (tls-F011)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let hostname = "mtls-ca-garbage.example.com";
                    let port: u16 = 31199;

                    match get_tls_info(&ctx.target_host, port, hostname) {
                        Ok(info) => {
                            if handshake_truly_completed(&info) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Mutual mTLS with garbage CA PEM should abort the handshake, \
                                         but it completed. Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                );
                            }
                            if is_k8s_mode() {
                                return TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Handshake aborted in K8s mode (Protocol={}, Cipher={}); gateway ssl.log is not mounted in the test-client pod",
                                        info.protocol, info.cipher
                                    ),
                                );
                            }
                            match wait_for_ssl_log_err_log(&ctx, hostname, &["mtls config:"]).await {
                                Ok(_) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Handshake aborted (Protocol={}, Cipher={}) and ssl.log recorded \
                                         err_log starting with 'mtls config:'",
                                        info.protocol, info.cipher
                                    ),
                                ),
                                Err(e) => TestResult::failed(start.elapsed(), e),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("OpenSSL test failed: {}", e)),
                    }
                })
            },
        )
    }

    // Case 3: Gateway listener frontendValidation references a Secret whose
    // ca.crt is garbage. The Controller marks that listener unprogrammed, and
    // the listener connection gate rejects it before TLS callbacks run.
    fn test_frontend_mtls_garbage_ca_pem_failclosed() -> TestCase {
        TestCase::new(
            "frontend_mtls_garbage_ca_pem_failclosed",
            "Gateway frontendValidation -> garbage ca.crt aborts handshake (tls-F011)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let hostname = "frontend-mtls-garbage.example.com";
                    let port: u16 = 31340;

                    match get_tls_info(&ctx.target_host, port, hostname) {
                        Ok(info) => {
                            if handshake_truly_completed(&info) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "frontendValidation with garbage CA PEM should abort the handshake, \
                                         but it completed. Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                );
                            }
                            if !info.connected {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "frontendValidation test did not reach the Gateway listener".to_string(),
                                );
                            }

                            match helpers::wait_resource_status(
                                &ctx,
                                "Gateway",
                                "edgion-test",
                                "mtls-failclose-gateway",
                                Duration::from_secs(5),
                                |gateway| {
                                    listener_has_condition(
                                        gateway,
                                        "https-frontend-mtls-failclose",
                                        "ResolvedRefs",
                                        "False",
                                        "InvalidCACertificateRef",
                                    ) && listener_has_condition(
                                        gateway,
                                        "https-frontend-mtls-failclose",
                                        "Programmed",
                                        "False",
                                        "Invalid",
                                    )
                                },
                            )
                            .await
                            {
                                Ok(_) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Handshake aborted (Protocol={}, Cipher={}) and listener status recorded \
                                         the invalid frontend CA rejection",
                                        info.protocol, info.cipher
                                    ),
                                ),
                                Err(e) => TestResult::failed(start.elapsed(), e),
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
impl TestSuite for MtlsFailcloseTestSuite {
    fn name(&self) -> &'static str {
        "TLS mTLS Fail-Close Tests (tls-F011)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_mtls_ca_missing_failclosed(),
            Self::test_mtls_ca_garbage_pem_failclosed(),
            Self::test_frontend_mtls_garbage_ca_pem_failclosed(),
        ]
    }
}
