// last-known-good integration test suites.
//
// Verifies both halves of the feature across a hot-reload of an invalid (garbage) Secret update:
//   (a) GATEWAY data plane keeps serving the previously bound valid cert
//       (last-known-good): the served cert serial is unchanged after the bad update.
//   (b) CONTROLLER reports the failure on the EdgionTls `ResolvedRefs` status
//       condition as status=False, reason="InvalidCertificate".
// This is end-to-end availability/status evidence. The controller may reject an
// invalid Secret before publishing it; gateway-store unit tests cover its own
// last-known-good retention and rejection-log rearming independently.
//
// Phase wiring (see dynamic/scenarios/edgion_tls_last_known_good.rs):
//   - initial:  valid lkg-secret bound -> handshake OK, ResolvedRefs=True.
//               The static suite stashes the served serial. The dynamic runner
//               instead seeds the stash from its exact valid Secret fixture and
//               waits for that serial, avoiding stale-certificate convergence.
//   - fallback: garbage lkg-secret applied -> handshake STILL OK and serial
//               equals the stashed serial (old cert still serving), while
//               ResolvedRefs flips to False/InvalidCertificate.
//   - recovered: valid Secret restored -> ResolvedRefs=True and the original
//               serial is still served; this phase must not rewrite the stash.
// The dynamic scenario rotates to a fresh valid certificate, repeats fallback
// against that certificate, and restores the initial certificate on exit.
//
// Required fixtures (edgion-tests/integration/conf/EdgionTls/LastKnownGood/):
//   - initial/Gateway.yaml          # HTTPS listener https-lkg on 31201
//   - initial/EdgionTls.yaml        # lkg-tls, hosts [lkg.example.com], secretRef lkg-secret
//   - initial/HTTPRoute.yaml        # route for lkg.example.com
//   - initial/Secret_lkg-secret.yaml      # valid cert (lkg-valid.{crt,key})
//   - updates_garbage/Secret_lkg-secret.yaml  # garbage tls.crt/tls.key (the bad update)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::tls_child::{self, OPERATION_TIMEOUT};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::process::Command;
use tokio::time::Instant as TokioInstant;

const HOST: &str = "lkg.example.com";
const PORT: u16 = 31201;
const NS: &str = "edgion-test";
const TLS_NAME: &str = "lkg-tls";

/// Path used to stash the initial-phase served cert serial so the fallback phase
/// (a separate suite instance in the same scenario process) can compare against
/// it. Derived from the access-log directory, mirroring mtls_failclose's
/// ssl_log_path helper.
fn stash_path(ctx: &TestContext) -> PathBuf {
    ctx.access_log_path
        .parent()
        .map(|p| p.join("lkg_stashed_serial.txt"))
        .unwrap_or_else(|| PathBuf::from("logs/lkg_stashed_serial.txt"))
}

/// Drive `openssl s_client` against the HTTPS listener and return the served
/// leaf certificate's serial (via `openssl x509 -noout -serial`). An Err here
/// also means the handshake did not complete (no server certificate in output).
async fn get_served_serial(host: &str, port: u16, servername: &str) -> Result<String, String> {
    let deadline = TokioInstant::now() + OPERATION_TIMEOUT;
    let mut command = Command::new("openssl");
    command.args([
        "s_client",
        "-connect",
        &format!("{}:{}", host, port),
        "-servername",
        servername,
    ]);
    let s_out = tls_child::output(command, None, deadline)
        .await
        .map_err(|e| format!("failed to run openssl s_client: {e}"))?;

    let stdout = String::from_utf8_lossy(&s_out.stdout);
    let begin = "-----BEGIN CERTIFICATE-----";
    let end = "-----END CERTIFICATE-----";
    let start_idx = stdout
        .find(begin)
        .ok_or_else(|| "no server certificate in s_client output (handshake failed?)".to_string())?;
    let end_rel = stdout[start_idx..]
        .find(end)
        .ok_or_else(|| "malformed certificate PEM in s_client output".to_string())?;
    let end_idx = start_idx + end_rel + end.len();
    let pem = &stdout[start_idx..end_idx];

    get_pem_serial(pem.as_bytes(), deadline).await
}

async fn get_pem_serial(pem: &[u8], deadline: TokioInstant) -> Result<String, String> {
    let mut command = Command::new("openssl");
    command.args(["x509", "-noout", "-serial"]);
    let out = tls_child::output(command, Some(pem.to_vec()), deadline)
        .await
        .map_err(|e| format!("failed to run openssl x509: {e}"))?;

    let parsed = String::from_utf8_lossy(&out.stdout);
    let serial = parsed
        .trim()
        .strip_prefix("serial=")
        .ok_or_else(|| "unexpected openssl x509 serial output".to_string())?
        .trim()
        .to_string();
    if serial.is_empty() {
        return Err("openssl x509 returned an empty serial".to_string());
    }
    Ok(serial)
}

/// Fetch the EdgionTls `lkg-tls` per-parent status conditions via the Controller
/// Admin API. EdgionTls uses the Gateway-API per-parent status shape
/// (`status.parents[].conditions`), so we flatten the conditions across all
/// parents (there is a single parentRef in this scenario).
async fn fetch_tls_conditions(ctx: &TestContext) -> Result<Vec<serde_json::Value>, String> {
    let url = format!(
        "{}/configserver/EdgionTls?namespace={}&name={}",
        ctx.admin_api_url(),
        NS,
        TLS_NAME
    );

    let mut last_err = format!("EdgionTls '{}' not found", TLS_NAME);

    for _ in 0..20 {
        let mut req = ctx.http_client.get(&url);
        if let Some(ref token) = ctx.admin_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                    if let Some(parents) = body["data"]["status"]["parents"].as_array() {
                        let mut all: Vec<serde_json::Value> = Vec::new();
                        for parent in parents {
                            if let Some(conditions) = parent["conditions"].as_array() {
                                all.extend(conditions.iter().cloned());
                            }
                        }
                        if !all.is_empty() {
                            return Ok(all);
                        }
                        last_err = format!("EdgionTls '{}' found but status.parents has no conditions", TLS_NAME);
                    } else {
                        last_err = format!("EdgionTls '{}' found but status.parents not present", TLS_NAME);
                    }
                }
                Ok(body) => {
                    last_err = format!("API error: {}", body["error"].as_str().unwrap_or("unknown"));
                }
                Err(e) => {
                    last_err = format!("JSON parse error: {}", e);
                }
            },
            Ok(resp) => {
                last_err = format!("HTTP {}", resp.status());
            }
            Err(e) => {
                last_err = format!("Request error: {}", e);
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    Err(last_err)
}

fn find_condition<'a>(conditions: &'a [serde_json::Value], type_: &str) -> Option<&'a serde_json::Value> {
    conditions.iter().find(|c| c["type"].as_str() == Some(type_))
}

/// Determinism guard: reject any wall-clock-looking 4-digit year (1970..=2100)
/// in the status condition message. The controller's status message must be
/// byte-stable across reconciles, so it must not embed the current time.
fn contains_wall_clock_year(s: &str) -> bool {
    let bytes = s.as_bytes();
    for w in bytes.windows(4) {
        if w.iter().all(u8::is_ascii_digit) {
            // SAFETY: window is 4 ASCII digits.
            let year: i32 = std::str::from_utf8(w).unwrap().parse().unwrap();
            if (1970..=2100).contains(&year) {
                return true;
            }
        }
    }
    false
}

// ============================================================
// Initial phase: valid cert bound
// ============================================================

pub struct LastKnownGoodInitialTestSuite;

impl LastKnownGoodInitialTestSuite {
    /// Seed the dynamic baseline from the exact Secret being applied, so an old
    /// certificate plus an already-True status cannot satisfy its initial phase.
    pub(crate) async fn stash_fixture_serial(ctx: &TestContext, fixture: &Path) -> Result<String, String> {
        use base64::Engine;

        let content =
            std::fs::read_to_string(fixture).map_err(|_| "failed to read the valid TLS Secret fixture".to_string())?;
        let secret: yaml_serde::Value =
            yaml_serde::from_str(&content).map_err(|_| "failed to parse the valid TLS Secret fixture".to_string())?;
        let encoded = secret["data"]["tls.crt"]
            .as_str()
            .ok_or_else(|| "valid TLS Secret fixture has no data.tls.crt".to_string())?;
        let pem = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| "valid TLS Secret fixture has invalid certificate base64".to_string())?;
        let serial = get_pem_serial(&pem, TokioInstant::now() + OPERATION_TIMEOUT).await?;
        let path = stash_path(ctx);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create serial stash directory: {error}"))?;
        }
        std::fs::write(path, &serial)
            .map_err(|error| format!("failed to stash expected certificate serial: {error}"))?;
        Ok(serial)
    }

    pub(crate) fn recovery_test_cases() -> Vec<TestCase> {
        vec![
            LastKnownGoodFallbackTestSuite::serves_old_cert(),
            Self::resolved_refs_true(),
        ]
    }

    fn handshake_ok_and_stash_serial() -> TestCase {
        TestCase::new(
            "lkg_initial_handshake_and_stash_serial",
            "[INITIAL] HTTPS handshake succeeds; stash served cert serial",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match get_served_serial(&ctx.target_host, PORT, HOST).await {
                        Ok(serial) => {
                            let path = stash_path(&ctx);
                            if let Some(parent) = path.parent() {
                                if let Err(e) = std::fs::create_dir_all(parent) {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("failed to create stash directory {}: {}", parent.display(), e),
                                    );
                                }
                            }
                            if let Err(e) = std::fs::write(&path, &serial) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("failed to stash serial to {}: {}", path.display(), e),
                                );
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("handshake OK, stashed serial={} at {}", serial, path.display()),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("initial handshake failed: {}", e)),
                    }
                })
            },
        )
    }

    fn resolved_refs_true() -> TestCase {
        TestCase::new(
            "lkg_initial_resolved_refs_true",
            "[INITIAL] EdgionTls lkg-tls ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match fetch_tls_conditions(&ctx).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => match find_condition(&conditions, "ResolvedRefs") {
                            Some(c) if c["status"].as_str() == Some("True") => {
                                TestResult::passed_with_message(start.elapsed(), "ResolvedRefs=True".to_string())
                            }
                            Some(c) => TestResult::failed(start.elapsed(), format!("ResolvedRefs not True: {:?}", c)),
                            None => TestResult::failed(
                                start.elapsed(),
                                format!("ResolvedRefs condition not found: {:?}", conditions),
                            ),
                        },
                    }
                })
            },
        )
    }
}

#[async_trait::async_trait]
impl TestSuite for LastKnownGoodInitialTestSuite {
    fn name(&self) -> &str {
        "TLS Last-Known-Good - Initial Phase (tls-06)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::handshake_ok_and_stash_serial(), Self::resolved_refs_true()]
    }
}

// ============================================================
// Fallback phase: garbage cert applied, old cert still served
// ============================================================

pub struct LastKnownGoodFallbackTestSuite;

impl LastKnownGoodFallbackTestSuite {
    fn serves_old_cert() -> TestCase {
        TestCase::new(
            "lkg_fallback_serves_old_cert",
            "[FALLBACK] handshake still OK and served serial equals stashed (last-known-good)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let path = stash_path(&ctx);
                    let stashed = match std::fs::read_to_string(&path) {
                        Ok(s) => s.trim().to_string(),
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("failed to read stashed serial from {}: {}", path.display(), e),
                            );
                        }
                    };
                    if stashed.is_empty() {
                        return TestResult::failed(start.elapsed(), "stashed serial is empty".to_string());
                    }

                    match get_served_serial(&ctx.target_host, PORT, HOST).await {
                        Ok(current) if current == stashed => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("old cert still served (serial={})", current),
                        ),
                        Ok(current) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "served serial changed after garbage update: stashed={}, current={} \
                                 (last-known-good NOT preserved)",
                                stashed, current
                            ),
                        ),
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "handshake failed after garbage update (cert dropped, not preserved): {}",
                                e
                            ),
                        ),
                    }
                })
            },
        )
    }

    fn resolved_refs_false_invalid_certificate() -> TestCase {
        TestCase::new(
            "lkg_fallback_resolved_refs_false_invalid_certificate",
            "[FALLBACK] EdgionTls lkg-tls ResolvedRefs=False/InvalidCertificate (deterministic message)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match fetch_tls_conditions(&ctx).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => match find_condition(&conditions, "ResolvedRefs") {
                            Some(c) => {
                                if c["status"].as_str() != Some("False") {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("ResolvedRefs should be False: {:?}", c),
                                    );
                                }
                                if c["reason"].as_str() != Some("InvalidCertificate") {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("ResolvedRefs reason should be InvalidCertificate: {:?}", c),
                                    );
                                }
                                let msg = c["message"].as_str().unwrap_or("");
                                if contains_wall_clock_year(msg) {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("ResolvedRefs message must not contain a wall-clock year: '{}'", msg),
                                    );
                                }
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False/InvalidCertificate (no wall-clock year)".to_string(),
                                )
                            }
                            None => TestResult::failed(
                                start.elapsed(),
                                format!("ResolvedRefs condition not found: {:?}", conditions),
                            ),
                        },
                    }
                })
            },
        )
    }
}

#[async_trait::async_trait]
impl TestSuite for LastKnownGoodFallbackTestSuite {
    fn name(&self) -> &str {
        "TLS Last-Known-Good - Fallback Phase (tls-06)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::serves_old_cert(), Self::resolved_refs_false_invalid_certificate()]
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/client/suites/edgion_tls/last_known_good/last_known_good_test.rs"]
mod tests;
