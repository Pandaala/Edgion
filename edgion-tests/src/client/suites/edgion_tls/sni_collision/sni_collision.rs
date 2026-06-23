// EdgionTls SNI collision test suite.
//
// Scenario: one HTTPS listener with TWO EdgionTls resources colliding on the
// same hostname (gateway-tls.test.com — the only SAN both reused certs carry,
// so neither is dropped by SAN validation before the collision):
//   - older (creationTimestamp 2020-01-01) -> RSA edge-tls, CN=test.example.com
//   - newer (creationTimestamp 2024-01-01) -> ECDSA edge-tls-ecdsa, CN=ecdsa-dualcert.example.com
//
// Collision rule: the resource with the OLDER creationTimestamp wins. This verifies
// that creationTimestamp survives the config-sync pipeline and that the
// edgion_tls_precedence sort selects the older cert: SNI gateway-tls.test.com
// must be served the RSA cert.
//
// Required config files (in edgion-tests/integration/conf/EdgionTls/sni_collision/):
//   - Gateway.yaml            # HTTPS listener "https" (port 31390)
//   - EdgionTls_older.yaml     # older EdgionTls -> edge-tls (RSA)
//   - EdgionTls_newer.yaml     # newer EdgionTls -> edge-tls-ecdsa (ECDSA)
//   - HTTPRoute.yaml           # route for gateway-tls.test.com
//   - Service.yaml / EndpointSlice.yaml  # shared HTTP backend (test-http:30001)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::process::Command;
use std::time::Instant;

pub struct SniCollisionTestSuite;

/// Locate a genuine OpenSSL binary. macOS ships LibreSSL whose `s_client`
/// mishandles a TLS 1.3 handshake against this BoringSSL listener, so prefer a
/// real OpenSSL build (CI already has it on PATH; local dev usually via brew).
/// Returns None when only LibreSSL is available so the caller can skip rather
/// than report a false failure.
fn openssl_bin() -> Option<String> {
    for cand in ["openssl", "/opt/homebrew/bin/openssl", "/usr/local/bin/openssl"] {
        if let Ok(out) = Command::new(cand).arg("version").output() {
            if String::from_utf8_lossy(&out.stdout).starts_with("OpenSSL") {
                return Some(cand.to_string());
            }
        }
    }
    None
}

impl SniCollisionTestSuite {
    fn test_oldest_wins() -> TestCase {
        TestCase::new(
            "edgion_tls_sni_collision_oldest_wins",
            "Test EdgionTls SNI collision - older creationTimestamp cert is served",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let servername = "gateway-tls.test.com";

                    let openssl =
                        match openssl_bin() {
                            Some(bin) => bin,
                            None => return TestResult::passed_with_message(
                                start.elapsed(),
                                "skipped: genuine OpenSSL not found (LibreSSL s_client unreliable against BoringSSL)"
                                    .to_string(),
                            ),
                        };

                    // Connect to the gateway by IP (target_host); the hostname is carried
                    // only in SNI (-servername). openssl s_client does real DNS for -connect
                    // (unlike the reqwest suites, which install a custom resolver), so a name
                    // here would resolve to the public example.com instead of the local gateway.
                    let output = match Command::new(&openssl)
                        .args([
                            "s_client",
                            "-connect",
                            &format!("{}:{}", ctx.target_host, ctx.https_port),
                            "-servername",
                            servername,
                            // ALPN mirrors the gateway_tls suite (BoringSSL HTTPS listener, HTTP/2 enabled).
                            "-alpn",
                            "h2,http/1.1",
                        ])
                        .stdin(std::process::Stdio::null())
                        .output()
                    {
                        Ok(o) => o,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Failed to run openssl: {}", e)),
                    };

                    let combined = format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    );

                    // A completed handshake prints the served certificate subject;
                    // an aborted one prints a TLS alert / handshake failure instead.
                    let handshake_ok = combined.contains("subject=") && !combined.contains("handshake failure");
                    if !handshake_ok {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "TLS handshake to SNI {} did not complete (no subject= / handshake failure)",
                                servername
                            ),
                        );
                    }

                    // The OLDER EdgionTls references the RSA edge-tls cert
                    // (CN=test.example.com); the NEWER one references the ECDSA
                    // edge-tls-ecdsa cert (CN=ecdsa-dualcert.example.com).
                    if combined.contains("ecdsa-dualcert.example.com") {
                        return TestResult::failed(
                            start.elapsed(),
                            "newer EdgionTls cert served; creationTimestamp precedence not honored".to_string(),
                        );
                    }
                    if !combined.contains("test.example.com") {
                        return TestResult::failed(
                            start.elapsed(),
                            "older EdgionTls cert (CN=test.example.com) was not served on the wire".to_string(),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "older EdgionTls (creationTimestamp 2020) cert CN=test.example.com served; precedence honored"
                            .to_string(),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for SniCollisionTestSuite {
    fn name(&self) -> &str {
        "EdgionTlsSniCollision"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_oldest_wins()]
    }
}
