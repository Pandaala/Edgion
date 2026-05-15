// TCPRoute GlobalConnectionIpRestriction Enforcement Test Suite
//
// Tests that GlobalConnectionIpRestriction stream plugin correctly allows/denies
// L4 TCP connections based on the active profile in the referenced PluginMetaData.
//
// Required config files (in examples/test/conf/TCPRoute/GlobalConnectionIpRestriction/):
// - 01_PluginMetaData_gcir.yaml            # PM with allow-local + block-local profiles
// - 02_EdgionStreamPlugins.yaml            # Stream plugin referencing the PM
// - 03_Gateway.yaml                        # Gateway with single TCP listener on 31296
// - 04_TCPRoute.yaml                       # Route proxying to test-tcp echo backend
//
// Port allocation (from ports.json "TCPRoute/GlobalConnectionIpRestriction"):
// - 31296 (tcp): single listener, behavior toggles via PM activeProfile
//
// Test scenarios:
// 1. gcir_allow_localhost      — initial state (allow-local): loopback ALLOWED, echo succeeds
// 2. gcir_deny_after_switch    — switch to block-local: loopback DENIED, connection closed
// 3. gcir_fail_open_disabled   — disable plugin (enable=false): fail-open, echo succeeds again

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct GcirEnforcementTestSuite;

/// Build a PluginMetaData YAML with the given activeProfile and enable flag.
/// Used to re-apply the PM with a different state at runtime.
fn pm_yaml_with(active_profile: &str, enable: bool) -> String {
    format!(
        r#"apiVersion: edgion.io/v1
kind: PluginMetaData
metadata:
  name: gcir-enforce-test
  namespace: edgion-test
spec:
  metadata:
    type: GlobalConnectionIpRestriction
    config:
      enable: {enable}
      activeProfile: {active_profile}
      profiles:
        allow-local:
          defaultAction: deny
          allow:
            - name: loopback
              cidrs: ["127.0.0.0/8"]
        block-local:
          defaultAction: allow
          deny:
            - name: loopback-block
              cidrs: ["127.0.0.0/8"]
"#,
        enable = enable,
        active_profile = active_profile,
    )
}

impl GcirEnforcementTestSuite {
    /// Test 1: initial state (activeProfile=allow-local)
    ///
    /// The PM starts with activeProfile=allow-local:
    ///   defaultAction: deny, allow: [127.0.0.0/8]
    /// Connection from 127.0.0.1 should be ALLOWED → echo data round-trip succeeds.
    fn gcir_allow_localhost() -> TestCase {
        TestCase::new(
            "gcir_allow_localhost",
            "GCIR allow-local profile: connection from 127.0.0.1 should be allowed and echo data",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Ensure PM is in allow-local state
                    if let Err(e) = ctx.apply_yaml(&pm_yaml_with("allow-local", true)).await {
                        return TestResult::failed(start.elapsed(), format!("Failed to apply PM (allow-local): {}", e));
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

    /// Test 2: switch activeProfile to block-local
    ///
    /// Re-apply the PM with activeProfile=block-local:
    ///   defaultAction: allow, deny: [127.0.0.0/8]
    /// Connection from 127.0.0.1 should be DENIED → EOF or connection closed.
    fn gcir_deny_after_switch() -> TestCase {
        TestCase::new(
            "gcir_deny_after_switch",
            "GCIR block-local profile: connection from 127.0.0.1 should be denied after profile switch",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Switch PM to block-local
                    if let Err(e) = ctx.apply_yaml(&pm_yaml_with("block-local", true)).await {
                        return TestResult::failed(start.elapsed(), format!("Failed to apply PM (block-local): {}", e));
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

    /// Test 3: disable the plugin (enable=false) — fail-open
    ///
    /// Re-apply the PM with enable=false.
    /// The plugin should fail-open: all connections pass through → echo succeeds.
    /// Also resets the PM back to a clean state for subsequent test runs.
    fn gcir_fail_open_disabled() -> TestCase {
        TestCase::new(
            "gcir_fail_open_disabled",
            "GCIR with enable=false should fail-open: connection from 127.0.0.1 allowed regardless of profile",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Disable the plugin — any activeProfile while disabled should allow all
                    if let Err(e) = ctx.apply_yaml(&pm_yaml_with("block-local", false)).await {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to apply PM (enable=false): {}", e),
                        );
                    }

                    // Wait for config propagation
                    tokio::time::sleep(Duration::from_secs(1)).await;

                    let addr = ctx.tcp_addr();
                    let test_data = b"Hello GCIR disabled";

                    match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr)).await {
                        Ok(Ok(mut stream)) => {
                            if let Err(e) = stream.write_all(test_data).await {
                                return TestResult::failed(start.elapsed(), format!("Write failed: {}", e));
                            }

                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf)).await {
                                Ok(Ok(n)) if n > 0 => TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("TCP echo succeeded ({} bytes) — disabled GCIR correctly fails open", n),
                                ),
                                Ok(Ok(_)) => TestResult::failed(
                                    start.elapsed(),
                                    "Connection closed immediately (0 bytes) — disabled GCIR should fail open"
                                        .to_string(),
                                ),
                                Ok(Err(e)) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Read failed: {} — disabled GCIR should fail open", e),
                                ),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Read timed out — disabled GCIR should fail open".to_string(),
                                ),
                            }
                        }
                        Ok(Err(e)) => TestResult::failed(
                            start.elapsed(),
                            format!("Connection refused (disabled GCIR should fail open): {}", e),
                        ),
                        Err(_) => TestResult::failed(
                            start.elapsed(),
                            "Connection timed out (disabled GCIR should fail open)".to_string(),
                        ),
                    }
                })
            },
        )
    }
    /// Test 4: PM missing → fail-closed
    ///
    /// Delete the PluginMetaData that the GCIR stream plugin references.
    /// `on_connection` cannot resolve the PM → records "fail_closed_pm_missing"
    /// and returns `Deny("ip:deny:pm-missing")`. The connection must be closed.
    fn gcir_fail_closed_pm_missing() -> TestCase {
        TestCase::new(
            "gcir_fail_closed_pm_missing",
            "GCIR with missing PM: connection must be denied (fail-closed), not passed through",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // First ensure the PM exists so delete has a known baseline.
                    if let Err(e) = ctx.apply_yaml(&pm_yaml_with("allow-local", true)).await {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Failed to apply PM before delete: {}", e),
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    // Delete the PM the GCIR plugin references.
                    if let Err(e) = ctx.delete_resource("PluginMetaData", "edgion-test", "gcir-enforce-test").await {
                        return TestResult::failed(start.elapsed(), format!("Failed to delete PM: {}", e));
                    }

                    // Wait for deletion to propagate to the Gateway.
                    tokio::time::sleep(Duration::from_millis(1500)).await;

                    let addr = ctx.tcp_addr();

                    let result =
                        match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
                            Ok(Ok(mut stream)) => {
                                let _ = stream.write_all(b"Hello GCIR no-PM").await;
                                let mut buf = vec![0u8; 1024];
                                match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await {
                                    Ok(Ok(0)) => TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Connection closed by GCIR (PM missing → fail-closed, EOF)".to_string(),
                                    ),
                                    Ok(Ok(n)) => TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "Expected denial (PM missing) but received {} bytes — fail-open leak",
                                            n
                                        ),
                                    ),
                                    Ok(Err(_)) => TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Connection rejected by GCIR (PM missing → fail-closed, read error)"
                                            .to_string(),
                                    ),
                                    Err(_) => TestResult::failed(
                                        start.elapsed(),
                                        "Read timed out — GCIR PM-missing did not deny the connection".to_string(),
                                    ),
                                }
                            }
                            Ok(Err(_)) => TestResult::passed_with_message(
                                start.elapsed(),
                                "Connection refused by GCIR (PM missing → fail-closed)".to_string(),
                            ),
                            Err(_) => TestResult::passed_with_message(
                                start.elapsed(),
                                "Connection dropped by GCIR (PM missing → fail-closed, timeout)".to_string(),
                            ),
                        };

                    // Restore PM so subsequent test runs start in a known-good state.
                    let _ = ctx.apply_yaml(&pm_yaml_with("allow-local", true)).await;

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
            Self::gcir_fail_open_disabled(),
            Self::gcir_fail_closed_pm_missing(),
        ]
    }
}
