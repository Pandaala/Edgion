// Wildcard Single-Label TLS cert selection test suite.
//
// Verifies RFC 6125 §6.4.3 single-label wildcard semantics in EdgionTls cert selection:
//   PASS: SNI `foo.example.com`       → single label left of the dot → cert matches → handshake completes.
//   FAIL: SNI `a.b.example.com`       → two labels left of the dot → no matching cert → handshake fails.
//   FAIL: SNI `example.com`           → bare apex, no wildcard label → no matching cert → handshake fails.
//
// Required config files (edgion-tests/conf/EdgionTls/wildcard_single_label/):
//   - Gateway.yaml       — HTTPS listener on port 31387
//   - EdgionTls.yaml     — EdgionTls with hosts: ["*.example.com"], secretRef: edge-tls
//   - HTTPRoute.yaml     — Route matching *.example.com → test-http:30001
//   - Service.yaml       — test-http Service (shared with https scenario)
//   - EndpointSlice.yaml — test-http EndpointSlice pointing to 127.0.0.1:30001
//
// The edge-tls Secret must include DNS:*.example.com in its SAN list.
// This is ensured by edgion-tests/scripts/gen_certs/generate_tls_certs.sh.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::process::Command;
use std::time::Instant;

pub struct WildcardSingleLabelTestSuite;

const LISTENER_PORT: u16 = 31387;

#[derive(Debug)]
struct ProbeResult {
    cipher: String,
    protocol: String,
    handshake_complete: bool,
}

/// Locate an `openssl s_client` binary.
///
/// macOS ships LibreSSL which may not support every flag; prefer Homebrew OpenSSL 3.x.
fn find_openssl() -> String {
    let candidates = [
        "/opt/homebrew/opt/openssl@3/bin/openssl",
        "/opt/homebrew/opt/openssl@3.6/bin/openssl",
        "/opt/homebrew/opt/openssl/bin/openssl",
        "/usr/local/opt/openssl@3/bin/openssl",
        "/usr/local/opt/openssl/bin/openssl",
        "openssl",
    ];
    for candidate in candidates {
        if std::path::Path::new(candidate).exists() || candidate == "openssl" {
            return candidate.to_string();
        }
    }
    "openssl".to_string()
}

/// Probe the gateway's TLS listener with the given SNI value.
///
/// Returns a `ProbeResult` describing whether the handshake completed and
/// which protocol/cipher were negotiated.
fn run_openssl_probe(host: &str, port: u16, servername: &str) -> Result<ProbeResult, String> {
    let target = format!("{}:{}", host, port);
    let openssl_bin = find_openssl();
    let output = Command::new(&openssl_bin)
        .args(["s_client", "-connect", &target, "-servername", servername])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Failed to run openssl ({}): {}", openssl_bin, e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    let mut protocol = String::new();
    let mut cipher = String::new();

    for line in combined.lines() {
        let trimmed = line.trim();

        // Protocol field: both "Protocol  : TLSv1.3" and "Protocol: TLSv1.3"
        if let Some(rest) = trimmed
            .strip_prefix("Protocol  :")
            .or_else(|| trimmed.strip_prefix("Protocol:"))
        {
            protocol = rest.trim().to_string();
        }

        // Cipher field (LibreSSL / OpenSSL 1.x): "Cipher    : TLS_AES_256_GCM_SHA384"
        if let Some(rest) = trimmed
            .strip_prefix("Cipher    :")
            .or_else(|| trimmed.strip_prefix("Cipher:"))
        {
            let v = rest.trim().to_string();
            if !v.is_empty() && v != "(NONE)" && v != "0000" {
                cipher = v;
            }
        }

        // Cipher field (OpenSSL 3.x): "New, TLSv1.3, Cipher is TLS_AES_256_GCM_SHA384"
        if let Some(pos) = trimmed.find(", Cipher is ") {
            let after = &trimmed[pos + ", Cipher is ".len()..];
            let v = after.trim().to_string();
            if !v.is_empty() {
                cipher = v;
                if protocol.is_empty() {
                    let before = &trimmed[..pos];
                    if let Some(proto) = before.split(", ").nth(1) {
                        if proto.starts_with("TLS") || proto.starts_with("SSL") {
                            protocol = proto.trim().to_string();
                        }
                    }
                }
            }
        }
    }

    let handshake_complete =
        combined.contains("CONNECTED") && !cipher.is_empty() && cipher != "(NONE)" && cipher != "0000";

    Ok(ProbeResult {
        cipher,
        protocol,
        handshake_complete,
    })
}

impl WildcardSingleLabelTestSuite {
    /// PASS case: SNI `foo.example.com` matches *.example.com (one label left).
    fn test_single_label_sni_matches() -> TestCase {
        TestCase::new(
            "wildcard_single_label_pass",
            "SNI foo.example.com matches *.example.com (single label) — handshake must succeed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match run_openssl_probe(&ctx.target_host, LISTENER_PORT, "foo.example.com") {
                        Ok(info) => {
                            if info.handshake_complete {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "single-label SNI accepted: Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "single-label SNI foo.example.com should succeed but handshake failed. \
                                         Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("OpenSSL probe failed: {}", e)),
                    }
                })
            },
        )
    }

    /// FAIL case: SNI `a.b.example.com` has two labels left of the dot.
    /// RFC 6125 §6.4.3 forbids matching this with *.example.com.
    fn test_multi_label_sni_rejected() -> TestCase {
        TestCase::new(
            "wildcard_single_label_fail_multi",
            "SNI a.b.example.com does NOT match *.example.com (two labels) — handshake must fail",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match run_openssl_probe(&ctx.target_host, LISTENER_PORT, "a.b.example.com") {
                        Ok(info) => {
                            if !info.handshake_complete {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "multi-label SNI correctly rejected. Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "multi-label SNI a.b.example.com should be rejected but handshake \
                                         completed. Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("OpenSSL probe failed: {}", e)),
                    }
                })
            },
        )
    }

    /// FAIL case: SNI `example.com` is the bare apex — the wildcard label `*`
    /// requires at least one label to its left, so the apex does not match.
    fn test_apex_sni_rejected() -> TestCase {
        TestCase::new(
            "wildcard_single_label_fail_apex",
            "SNI example.com (bare apex) does NOT match *.example.com — handshake must fail",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match run_openssl_probe(&ctx.target_host, LISTENER_PORT, "example.com") {
                        Ok(info) => {
                            if !info.handshake_complete {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "apex SNI correctly rejected. Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "apex SNI example.com should be rejected but handshake completed. \
                                         Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("OpenSSL probe failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for WildcardSingleLabelTestSuite {
    fn name(&self) -> &'static str {
        "WildcardSingleLabel TLS Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_single_label_sni_matches(),
            Self::test_multi_label_sni_rejected(),
            Self::test_apex_sni_rejected(),
        ]
    }
}
