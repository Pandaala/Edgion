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

/// The framework awaits cases and suites serially. Capture the admitted resource
/// before mutating it, including prepared Kubernetes peer annotations.
async fn capture_original(ctx: &TestContext, kind: &str, ns: &str, name: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/api/v1/namespaced/{kind}/{ns}/{name}", ctx.admin_api_url());
    let mut request = client.get(url);
    if let Some(token) = &ctx.admin_token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Cannot capture original {kind}/{ns}/{name}: {}",
            response.status()
        ));
    }
    let mut value: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    // A restoration is an unconditional publication of the original authored spec.
    value
        .as_object_mut()
        .ok_or("Original resource is not an object")?
        .remove("status");
    if let Some(metadata) = value["metadata"].as_object_mut() {
        for key in [
            "resourceVersion",
            "generation",
            "managedFields",
            "uid",
            "creationTimestamp",
            "deletionTimestamp",
            "deletionGracePeriodSeconds",
        ] {
            metadata.remove(key);
        }
    }
    Ok(value)
}

async fn apply_value(ctx: &TestContext, value: &serde_json::Value) -> Result<(), String> {
    ctx.apply_yaml(&serde_yaml::to_string(value).map_err(|e| e.to_string())?)
        .await
}

/// All restorations run even after an earlier failure. Four targets require a
/// 48s cleanup reserve. Four bounded captures take at most 20s;
/// 20s capture + 120s mutation/probes + 48s cleanup leaves 22s before 210s.
async fn restore_originals(ctx: &TestContext, originals: &[serde_json::Value]) -> Result<(), String> {
    let mut errors = Vec::new();
    for value in originals {
        match tokio::time::timeout(Duration::from_secs(12), apply_value(ctx, value)).await {
            Ok(Ok(())) => {}
            result => errors.push(format!(
                "{}/{}/{} restoration: {result:?}",
                value["kind"], value["metadata"]["namespace"], value["metadata"]["name"]
            )),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn list_with_cidr(original: &serde_json::Value, cidr: &str) -> serde_json::Value {
    let mut value = original.clone();
    value["spec"]["data"]["config"]["items"] = serde_json::json!([{"name":"live-peer", "cidrs":[cidr]}]);
    value
}

pub struct GcirEnforcementTestSuite;

enum TcpEchoProbe {
    Echoed(usize),
    Closed(String),
    Inconclusive(String),
}

async fn probe_tcp_echo(addr: &str, test_data: &[u8]) -> TcpEchoProbe {
    let mut stream = match tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => return TcpEchoProbe::Inconclusive(format!("connect failed: {e}")),
        Err(_) => return TcpEchoProbe::Inconclusive("connect timed out".to_string()),
    };

    if let Err(e) = stream.write_all(test_data).await {
        return TcpEchoProbe::Closed(format!("write failed: {e}"));
    }

    let mut buf = vec![0u8; 1024];
    match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => TcpEchoProbe::Echoed(n),
        Ok(Ok(_)) => TcpEchoProbe::Closed("connection closed with EOF".to_string()),
        Ok(Err(e)) => TcpEchoProbe::Closed(format!("read failed: {e}")),
        Err(_) => TcpEchoProbe::Inconclusive("read timed out".to_string()),
    }
}

async fn wait_for_tcp_echo(addr: &str, test_data: &[u8]) -> Result<usize, String> {
    let mut last_observation = "no probe completed".to_string();
    let wait = async {
        loop {
            match probe_tcp_echo(addr, test_data).await {
                TcpEchoProbe::Echoed(n) => return n,
                TcpEchoProbe::Closed(reason) | TcpEchoProbe::Inconclusive(reason) => {
                    last_observation = reason;
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };

    match tokio::time::timeout(Duration::from_secs(8), wait).await {
        Ok(n) => Ok(n),
        Err(_) => Err(format!(
            "TCP echo did not become available within 8s; last observation: {last_observation}"
        )),
    }
}

async fn wait_for_tcp_denial(addr: &str, test_data: &[u8]) -> Result<String, String> {
    let mut last_observation = "no probe completed".to_string();
    let wait = async {
        loop {
            match probe_tcp_echo(addr, test_data).await {
                TcpEchoProbe::Closed(reason) => return reason,
                TcpEchoProbe::Echoed(n) => {
                    last_observation = format!("received {n} echo bytes");
                }
                TcpEchoProbe::Inconclusive(reason) => {
                    last_observation = reason;
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };

    match tokio::time::timeout(Duration::from_secs(8), wait).await {
        Ok(reason) => Ok(reason),
        Err(_) => Err(format!(
            "TCP connection was not denied within 8s; last observation: {last_observation}"
        )),
    }
}

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
    fn selected_profile_live_lists_and_invalid_generation_recovery() -> TestCase {
        TestCase::new("selected_profile_live_lists_and_invalid_generation_recovery", "Selected GCIR profile reads live lists, preserves optional omissions, and rejects a published legacy generation", |ctx: TestContext| Box::pin(async move {
            let start = Instant::now();
            let consumer = match capture_original(&ctx, "EdgionStreamPlugins", "edgion-test", "gcir-stream-plugins").await {
                Ok(value) => value, Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            let selector = match capture_original(&ctx, "EdgionConfigData", "edgion-test", "gcir-enforce-test").await {
                Ok(value) => value, Err(error) => return TestResult::failed(start.elapsed(), error),
            };
            let config = &consumer["spec"]["plugins"][0]["config"];
            let allow_name = match config["profiles"]["allow-local"]["allowRefs"][0]["name"].as_str() { Some(name) => name.to_owned(), None => return TestResult::failed(start.elapsed(), "Missing allow profile reference".into()) };
            let deny_name = match config["profiles"]["block-local"]["denyRefs"][0]["name"].as_str() { Some(name) => name.to_owned(), None => return TestResult::failed(start.elapsed(), "Missing deny profile reference".into()) };
            let mut originals = Vec::new();
            for name in [&allow_name, &deny_name] {
                if originals.iter().any(|value: &serde_json::Value| value["metadata"]["name"].as_str() == Some(name)) { continue; }
                match capture_original(&ctx, "EdgionConfigData", "edgion-test", name).await {
                    Ok(value) => originals.push(value), Err(error) => return TestResult::failed(start.elapsed(), error),
                }
            }
            let allow = originals.iter().find(|value| value["metadata"]["name"].as_str() == Some(&allow_name)).unwrap().clone();
            let deny = originals.iter().find(|value| value["metadata"]["name"].as_str() == Some(&deny_name)).unwrap().clone();
            originals.push(consumer.clone()); originals.push(selector);
            let check = async {
                // Explicit list setup precedes the policy. Shared K8s peer identity
                // retains its exact annotations and is restored in this serial case.
                apply_value(&ctx, &allow).await?; apply_value(&ctx, &deny).await?;
                apply_value(&ctx, &consumer).await?;
                ctx.apply_yaml(&selector_yaml_with("allow-local")).await?;
                wait_for_tcp_echo(&ctx.tcp_addr(), b"acl-live-allow").await?;
                apply_value(&ctx, &list_with_cidr(&allow, "192.0.2.0/24")).await?;
                wait_for_tcp_denial(&ctx.tcp_addr(), b"acl-live-miss").await?;
                ctx.delete_resource("EdgionConfigData", "edgion-test", &allow_name).await?;
                wait_for_tcp_denial(&ctx.tcp_addr(), b"acl-live-missing").await?;
                apply_value(&ctx, &allow).await?;
                wait_for_tcp_echo(&ctx.tcp_addr(), b"acl-live-restored").await?;
                ctx.apply_yaml(&selector_yaml_with("block-local")).await?;
                wait_for_tcp_denial(&ctx.tcp_addr(), b"acl-live-deny").await?;
                ctx.delete_resource("EdgionConfigData", "edgion-test", &deny_name).await?;
                wait_for_tcp_echo(&ctx.tcp_addr(), b"acl-live-deny-omitted").await?;
                apply_value(&ctx, &deny).await?;
                wait_for_tcp_denial(&ctx.tcp_addr(), b"acl-live-deny-restored").await?;
                ctx.apply_yaml(&selector_yaml_with("allow-local")).await?;
                wait_for_tcp_echo(&ctx.tcp_addr(), b"acl-live-positive").await?;
                let mut invalid = consumer.clone();
                invalid["spec"]["plugins"][0]["config"]["profiles"]["allow-local"]["allow"] = serde_json::Value::Null;
                apply_value(&ctx, &invalid).await?;
                wait_for_tcp_denial(&ctx.tcp_addr(), b"acl-live-invalid").await?;
                apply_value(&ctx, &consumer).await?;
                wait_for_tcp_echo(&ctx.tcp_addr(), b"acl-live-clean").await.map(|_| ())
            };
            let result = tokio::time::timeout(Duration::from_secs(120), check).await.unwrap_or_else(|_| Err("GCIR live phases exceeded 120s".into()));
            let cleanup = restore_originals(&ctx, &originals).await;
            match (result, cleanup) {
                (Ok(()), Ok(())) => TestResult::passed(start.elapsed()),
                (result, cleanup) => TestResult::failed(start.elapsed(), format!("check={result:?}; cleanup={cleanup:?}")),
            }
        })).with_timeout(Duration::from_secs(210))
    }

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

                    let addr = ctx.tcp_addr();
                    let test_data = b"Hello GCIR allow-local";

                    match wait_for_tcp_echo(&addr, test_data).await {
                        Ok(n) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("TCP echo succeeded ({n} bytes) — 127.0.0.1 correctly allowed by allow-local"),
                        ),
                        Err(e) => {
                            TestResult::failed(start.elapsed(), format!("{e} — allow-local should permit 127.0.0.1"))
                        }
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

                    let addr = ctx.tcp_addr();
                    match wait_for_tcp_denial(&addr, b"Hello GCIR block-local").await {
                        Ok(reason) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("Connection rejected by GCIR block-local ({reason})"),
                        ),
                        Err(e) => {
                            TestResult::failed(start.elapsed(), format!("{e} — block-local should deny 127.0.0.1"))
                        }
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

                    let addr = ctx.tcp_addr();
                    let test_data = b"Hello GCIR unknown-profile";

                    let result = match wait_for_tcp_echo(&addr, test_data).await {
                        Ok(n) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "TCP echo succeeded ({n} bytes) — unknown selector profile fell back to base allow-local"
                            ),
                        ),
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("{e} — unknown profile should fall back to base allow-local"),
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
                    // Delete the Selector the GCIR plugin references.
                    if let Err(e) = ctx
                        .delete_resource("EdgionConfigData", "edgion-test", "gcir-enforce-test")
                        .await
                    {
                        return TestResult::failed(start.elapsed(), format!("Failed to delete Selector: {}", e));
                    }

                    let addr = ctx.tcp_addr();
                    let test_data = b"Hello GCIR no-selector";

                    let result = match wait_for_tcp_echo(&addr, test_data).await {
                        Ok(n) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("TCP echo succeeded ({n} bytes) — missing selector fell back to base allow-local"),
                        ),
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("{e} — missing selector should fall back to base allow-local"),
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
            Self::selected_profile_live_lists_and_invalid_generation_recovery(),
        ]
    }
}
