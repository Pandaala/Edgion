// TCPRoute GlobalConnectionIpRestriction Enforcement Test Suite
//
// Tests that the GlobalConnectionIpRestriction stream plugin correctly allows/denies
// L4 TCP connections based on the active profile, which is hot-swapped by a Selector
// EdgionConfigData referenced via `activeProfileRef`.
//
// Model: the plugin config holds all profiles inline (git-owned). A Selector EdgionConfigData
// only names which inline profile is active. A Selector can switch among the inline profiles,
// but can never disable the plugin; on any selector error (unknown profile / missing
// EdgionConfigData) the plugin falls back to the inline `activeProfile` (fail-to-base).
//
// Required config files (in edgion-tests/integration/conf/TCPRoute/GlobalConnectionIpRestriction/):
// - 01_EdgionConfigData_gcir.yaml          # Selector (initial active=allow-local)
// - 02_EdgionStreamPlugins.yaml            # Stream plugin with inline allow-local + block-local profiles
// - 03_Gateway.yaml                        # Gateway with single TCP listener on 31296
// - 04_TCPRoute.yaml                       # Route proxying to test-tcp echo backend
//
// Inline profiles (base activeProfile = allow-local):
// - allow-local: defaultAction=deny, allow 127.0.0.0/8  → loopback ALLOWED
// - block-local: defaultAction=allow, deny  127.0.0.0/8  → loopback DENIED
//
// Port allocation (from ports.json "TCPRoute/GlobalConnectionIpRestriction"):
// - 31296 (tcp): single listener, behavior toggles via the Selector's active profile
//
// Test scenarios:
// 1. gcir_allow_localhost          — selector active=allow-local: loopback ALLOWED, echo succeeds
// 2. gcir_deny_after_switch        — selector active=block-local: loopback DENIED, connection closed
// 3. gcir_fallback_unknown_profile — selector active=<unknown>: fall back to base (allow-local) → ALLOWED
// 4. gcir_fallback_selector_missing — Selector deleted: fall back to base (allow-local) → ALLOWED

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct GcirEnforcementTestSuite;

/// Build a `Selector` EdgionConfigData YAML that hot-swaps the *active profile* of the
/// inline GlobalConnectionIpRestriction config in the `gcir-stream-plugins`
/// EdgionStreamPlugins (which owns both profiles). The selector is referenced via
/// `activeProfileRef: gcir-enforce-test`; re-applying it switches the active profile
/// among the inline profiles without touching the plugin config. An `active` that names
/// no inline profile makes the plugin fall back to its inline `activeProfile`.
fn selector_yaml_with(active_profile: &str) -> String {
    format!(
        r#"apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: gcir-enforce-test
  namespace: edgion-test
spec:
  data:
    type: Selector
    config:
      active: {active_profile}
"#,
        active_profile = active_profile,
    )
}

impl GcirEnforcementTestSuite {
    /// Test 1: selector active=allow-local
    ///
    /// The selector selects the inline allow-local profile:
    ///   defaultAction: deny, allow: [127.0.0.0/8]
    /// Connection from 127.0.0.1 should be ALLOWED → echo data round-trip succeeds.
    fn gcir_allow_localhost() -> TestCase {
        TestCase::new(
            "gcir_allow_localhost",
            "GCIR selector active=allow-local: connection from 127.0.0.1 should be allowed and echo data",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Ensure the selector points at the allow-local profile.
                    if let Err(e) = ctx.apply_yaml(&selector_yaml_with("allow-local")).await {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to apply Selector (allow-local): {}", e),
                        );
                    }

                    // Brief pause for the Gateway to pick up the updated configuration
                    tokio::time::sleep(Duration::from_millis(1500)).await;

                    let addr = ctx.tcp_addr();
                    let test_data = b"Hello GCIR allow-local";

                    match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                            }

                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf)).await {
                                Ok(Ok(n)) if n > 0 => TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "TCP echo succeeded ({} bytes) — 127.0.0.1 correctly allowed by allow-local",
                                        n
                                    ),
                                ),
                                Ok(Ok(_)) => TestResult::failed(
                                    start.elapsed(),
                                    "Connection closed immediately (0 bytes) — allow-local should permit 127.0.0.1"
                                        .to_string(),
                                ),
                                Ok(Err(e)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Read failed: {} — allow-local should permit 127.0.0.1", e),
                                ),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Read timed out — allow-local should permit 127.0.0.1".to_string(),
                                ),
                            }
                        }
                        Ok(Err(e)) => TestResult::failed(
                            start.elapsed(),
                            format!("Connection refused (allow-local should permit 127.0.0.1): {}", e),
                        ),
                        Err(_) => TestResult::failed(
                            start.elapsed(),
                            "Connection timed out (allow-local should permit 127.0.0.1)".to_string(),
                        ),
                    }
                })
            },
        )
    }

    /// Test 2: switch the selector to the block-local profile
    ///
    /// Re-apply the selector with active=block-local:
    ///   defaultAction: allow, deny: [127.0.0.0/8]
    /// Connection from 127.0.0.1 should be DENIED → EOF or connection closed.
    fn gcir_deny_after_switch() -> TestCase {
        TestCase::new(
            "gcir_deny_after_switch",
            "GCIR selector active=block-local: connection from 127.0.0.1 should be denied after profile switch",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Switch the selector to block-local
                    if let Err(e) = ctx.apply_yaml(&selector_yaml_with("block-local")).await {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to apply Selector (block-local): {}", e),
                        );
                    }

                    // Wait for the config sync to propagate to the Gateway
                    tokio::time::sleep(Duration::from_secs(1)).await;

                    let addr = ctx.tcp_addr();

                    // Connect — TCP handshake may succeed (plugin runs after accept),
                    // but the connection should be closed immediately without proxying.
                    match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            let test_data = b"Hello GCIR block-local";
                            let _ = stream.write_all(test_data).await;

                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await {
                                Ok(Ok(0)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Connection closed by GCIR block-local (EOF) — deny working".to_string(),
                                ),
                                Ok(Ok(n)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected denial but received {} bytes — block-local not enforced", n),
                                ),
                                Ok(Err(_)) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Connection rejected by GCIR block-local (read error)".to_string(),
                                ),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Read timed out — GCIR block-local did not deny the connection".to_string(),
                                ),
                            }
                        }
                        Ok(Err(_)) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Connection refused by GCIR block-local".to_string(),
                        ),
                        Err(_) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Connection dropped by GCIR block-local (timeout)".to_string(),
                        ),
                    }
                })
            },
        )
    }

    /// Test 3: selector names an unknown profile → fall back to base
    ///
    /// Re-apply the selector with active=<unknown profile>. The name is not one of the
    /// inline profiles, so the plugin falls back to its inline `activeProfile`
    /// (allow-local) → connection from 127.0.0.1 ALLOWED, echo succeeds.
    fn gcir_fallback_unknown_profile() -> TestCase {
        TestCase::new(
            "gcir_fallback_unknown_profile",
            "GCIR selector with unknown profile name: falls back to inline activeProfile (allow-local) → allowed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Selector names a profile that does not exist inline → fail-to-base.
                    if let Err(e) = ctx.apply_yaml(&selector_yaml_with("no-such-profile")).await {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to apply Selector (unknown profile): {}", e),
                        );
                    }

                    // Wait for config propagation
                    tokio::time::sleep(Duration::from_secs(1)).await;

                    let addr = ctx.tcp_addr();
                    let test_data = b"Hello GCIR unknown-profile";

                    let result = match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                            }

                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf)).await {
                                Ok(Ok(n)) if n > 0 => TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "TCP echo succeeded ({} bytes) — unknown selector profile fell back to base allow-local",
                                        n
                                    ),
                                ),
                                Ok(Ok(_)) => TestResult::failed(
                                    start.elapsed(),
                                    "Connection closed immediately (0 bytes) — unknown profile should fall back to base allow-local"
                                        .to_string(),
                                ),
                                Ok(Err(e)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Read failed: {} — unknown profile should fall back to base allow-local", e),
                                ),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Read timed out — unknown profile should fall back to base allow-local".to_string(),
                                ),
                            }
                        }
                        Ok(Err(e)) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Connection refused (unknown profile should fall back to base allow-local): {}",
                                e
                            ),
                        ),
                        Err(_) => TestResult::failed(
                            start.elapsed(),
                            "Connection timed out (unknown profile should fall back to base allow-local)".to_string(),
                        ),
                    };

                    // Restore the selector to allow-local for a known-good baseline.
                    let _ = ctx.apply_yaml(&selector_yaml_with("allow-local")).await;

                    result
                })
            },
        )
    }

    /// Test 4: Selector EdgionConfigData missing → fall back to base
    ///
    /// Delete the Selector EdgionConfigData that the GCIR plugin references via
    /// `activeProfileRef`. With no selector, the plugin uses its inline `activeProfile`
    /// (allow-local) → connection from 127.0.0.1 ALLOWED. The inline config is git-owned,
    /// so a missing selector is fail-to-base, NOT fail-closed.
    fn gcir_fallback_selector_missing() -> TestCase {
        TestCase::new(
            "gcir_fallback_selector_missing",
            "GCIR with missing Selector EdgionConfigData: falls back to inline activeProfile (allow-local) → allowed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // First ensure the selector exists so delete has a known baseline.
                    if let Err(e) = ctx.apply_yaml(&selector_yaml_with("allow-local")).await {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to apply Selector before delete: {}", e),
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    // Delete the Selector the GCIR plugin references.
                    if let Err(e) = ctx
                        .delete_resource("EdgionConfigData", "edgion-test", "gcir-enforce-test")
                        .await
                    {
                        return TestResult::failed(start.elapsed(), format!("Failed to delete Selector: {}", e));
                    }

                    // Wait for deletion to propagate to the Gateway.
                    tokio::time::sleep(Duration::from_millis(1500)).await;

                    let addr = ctx.tcp_addr();
                    let test_data = b"Hello GCIR no-selector";

                    let result = match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                            }

                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf)).await {
                                Ok(Ok(n)) if n > 0 => TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "TCP echo succeeded ({} bytes) — missing selector fell back to base allow-local",
                                        n
                                    ),
                                ),
                                Ok(Ok(_)) => TestResult::failed(
                                    start.elapsed(),
                                    "Connection closed immediately (0 bytes) — missing selector should fall back to base allow-local"
                                        .to_string(),
                                ),
                                Ok(Err(e)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Read failed: {} — missing selector should fall back to base allow-local", e),
                                ),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Read timed out — missing selector should fall back to base allow-local".to_string(),
                                ),
                            }
                        }
                        Ok(Err(e)) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Connection refused (missing selector should fall back to base allow-local): {}",
                                e
                            ),
                        ),
                        Err(_) => TestResult::failed(
                            start.elapsed(),
                            "Connection timed out (missing selector should fall back to base allow-local)".to_string(),
                        ),
                    };

                    // Restore the selector so subsequent test runs start in a known-good state.
                    let _ = ctx.apply_yaml(&selector_yaml_with("allow-local")).await;

                    result
                })
            },
        )
    }
}

impl TestSuite for GcirEnforcementTestSuite {
    fn name(&self) -> &str {
        "TCPRoute GlobalConnectionIpRestriction"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::gcir_allow_localhost(),
            Self::gcir_deny_after_switch(),
            Self::gcir_fallback_unknown_profile(),
            Self::gcir_fallback_selector_missing(),
        ]
    }
}
