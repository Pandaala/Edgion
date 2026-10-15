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
use crate::tls_child::{self, ChildError, OPERATION_TIMEOUT};
use async_trait::async_trait;
use std::time::Instant;
use tokio::process::Command;
use tokio::time::Instant as TokioInstant;

pub struct SniCollisionTestSuite;

/// Locate a genuine OpenSSL binary. macOS ships LibreSSL whose `s_client`
/// mishandles a TLS 1.3 handshake against this BoringSSL listener, so prefer a
/// real OpenSSL build (CI already has it on PATH; local dev usually via brew).
/// Returns None when only LibreSSL is available so the caller can skip rather
/// than report a false failure.
async fn openssl_bin(deadline: TokioInstant) -> Result<Option<String>, ChildError> {
    openssl_from_candidates(
        &["openssl", "/opt/homebrew/bin/openssl", "/usr/local/bin/openssl"],
        deadline,
    )
    .await
}

async fn openssl_from_candidates(candidates: &[&str], deadline: TokioInstant) -> Result<Option<String>, ChildError> {
    for candidate in candidates {
        let mut command = Command::new(candidate);
        command.arg("version");
        match tls_child::output(command, None, deadline).await {
            Ok(out) if String::from_utf8_lossy(&out.stdout).starts_with("OpenSSL") => {
                return Ok(Some((*candidate).to_string()));
            }
            Ok(_) => {}
            Err(error) if error.is_not_found() => {}
            Err(error) => return Err(error),
        }
    }
    Ok(None)
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

                    let deadline = TokioInstant::now() + OPERATION_TIMEOUT;
                    let openssl = match openssl_bin(deadline).await {
                        Ok(openssl) => openssl,
                        Err(error) => {
                            return TestResult::failed(start.elapsed(), format!("OpenSSL discovery failed: {error}"))
                        }
                    };
                    let Some(openssl) = openssl else {
                        return TestResult::passed_with_message(
                            start.elapsed(),
                            "skipped: genuine OpenSSL not found (LibreSSL s_client unreliable against BoringSSL)"
                                .to_string(),
                        );
                    };

                    // Connect to the gateway by IP (target_host); the hostname is carried
                    // only in SNI (-servername). openssl s_client does real DNS for -connect
                    // (unlike the reqwest suites, which install a custom resolver), so a name
                    // here would resolve to the public example.com instead of the local gateway.
                    let mut command = Command::new(&openssl);
                    command.args([
                        "s_client",
                        "-connect",
                        &format!("{}:{}", ctx.target_host, ctx.https_port),
                        "-servername",
                        servername,
                        // ALPN mirrors the gateway_tls suite (BoringSSL HTTPS listener, HTTP/2 enabled).
                        "-alpn",
                        "h2,http/1.1",
                    ]);
                    let output = match tls_child::output(command, None, deadline).await {
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

#[cfg(test)]
#[path = "../../../../../tests/unit/client/suites/edgion_tls/sni_collision/sni_collision_test.rs"]
mod tests;
