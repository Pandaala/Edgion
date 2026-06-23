// fallback_sni Test Suite
// Verifies EdgionGatewayConfig.spec.security_protect.fallback_sni behavior:
//   1. Client without SNI hits the listener → server falls back to
//      "default.example.com" → cert matches → handshake completes.
//   2. Client with an unknown SNI hits the listener → no Layer 1 / Layer 2
//      match → select_cb emits "cert not found" + closes the connection.
//
// Required config files (in edgion-tests/integration/conf/EdgionTls/fallback_sni/):
//   - Gateway.yaml          # Gateway with https-fallback listener (port 31198)
//   - EdgionTls.yaml        # EdgionTls hosting default.example.com
//   - HTTPRoute.yaml        # Route for default.example.com
//
// Also requires: edgion-tests/integration/conf/base/EdgionGatewayConfig.yaml has
//   securityProtect.fallbackSni = "default.example.com" (already configured).

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::process::Command;
use std::time::Instant;

pub struct FallbackSniTestSuite;

#[derive(Debug)]
struct ProbeResult {
    cipher: String,
    protocol: String,
    handshake_complete: bool,
}

/// Locate an `openssl s_client` binary that supports `-noservername`.
///
/// macOS ships LibreSSL which does NOT have `-noservername`. Try well-known
/// Homebrew paths first (arm64 then x86_64), then fall back to whatever
/// `openssl` resolves to in $PATH.
fn find_openssl_with_noservername() -> String {
    let candidates = [
        "/opt/homebrew/opt/openssl@3/bin/openssl",
        "/opt/homebrew/opt/openssl@3.6/bin/openssl",
        "/opt/homebrew/opt/openssl/bin/openssl",
        "/usr/local/opt/openssl@3/bin/openssl",
        "/usr/local/opt/openssl/bin/openssl",
        "openssl",
    ];
    for candidate in candidates {
        // Probe: run `openssl s_client --help` and look for "-noservername"
        if let Ok(out) = Command::new(candidate)
            .args(["s_client", "--help"])
            .stdin(std::process::Stdio::null())
            .output()
        {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            if combined.contains("noservername") {
                return candidate.to_string();
            }
        }
    }
    // Last resort: system openssl (may not support -noservername on macOS LibreSSL)
    "openssl".to_string()
}

/// Run `openssl s_client` and parse Protocol/Cipher. `extra_args` lets the
/// caller pass `-servername foo` or `-noservername` etc.
///
/// Handles both OpenSSL 1.x / LibreSSL output format:
///   `Protocol  : TLSv1.3`
///   `Cipher    : TLS_AES_256_GCM_SHA384`
/// and OpenSSL 3.x output format:
///   `Protocol: TLSv1.3`
///   `New, TLSv1.3, Cipher is TLS_AES_256_GCM_SHA384`
fn run_openssl_probe(host: &str, port: u16, extra_args: &[&str]) -> Result<ProbeResult, String> {
    let target = format!("{}:{}", host, port);
    let openssl_bin = find_openssl_with_noservername();
    let mut cmd = Command::new(&openssl_bin);
    cmd.args(["s_client", "-connect", &target]);
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.stdin(std::process::Stdio::null());

    let output = cmd
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
        // Also covers "Reused, TLSv1.3, Cipher is ..."
        if let Some(pos) = trimmed.find(", Cipher is ") {
            let after = &trimmed[pos + ", Cipher is ".len()..];
            let v = after.trim().to_string();
            if !v.is_empty() {
                cipher = v;
                // Extract protocol from the same line: "New, TLSv1.3, Cipher is ..."
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

    // Handshake completed if CONNECTED and a non-null cipher was negotiated
    let handshake_complete =
        combined.contains("CONNECTED") && !cipher.is_empty() && cipher != "(NONE)" && cipher != "0000";

    Ok(ProbeResult {
        cipher,
        protocol,
        handshake_complete,
    })
}

impl FallbackSniTestSuite {
    fn test_no_sni_resolves_to_fallback() -> TestCase {
        TestCase::new(
            "fallback_sni_no_servername_match",
            "no-SNI client → fallback to default.example.com → handshake succeeds",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let port = 31198;

                    match run_openssl_probe(&ctx.target_host, port, &["-noservername"]) {
                        Ok(info) => {
                            if info.handshake_complete {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "no-SNI handshake completed via fallback_sni: \
                                         Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "no-SNI handshake should succeed via fallback_sni but did not. \
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

    fn test_unknown_sni_rejected() -> TestCase {
        TestCase::new(
            "fallback_sni_unknown_sni_rejected",
            "client SNI unknown to any cert → handshake_failure + ssl.log 'cert not found'",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let port = 31198;

                    match run_openssl_probe(
                        &ctx.target_host,
                        port,
                        &["-servername", "definitely-not-registered.example.invalid"],
                    ) {
                        Ok(info) => {
                            if !info.handshake_complete {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "unknown SNI correctly rejected. \
                                         Protocol={}, Cipher={}",
                                        info.protocol, info.cipher
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "unknown SNI should be rejected but handshake completed. \
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
impl TestSuite for FallbackSniTestSuite {
    fn name(&self) -> &'static str {
        "fallback_sni Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_no_sni_resolves_to_fallback(),
            Self::test_unknown_sni_rejected(),
        ]
    }
}
