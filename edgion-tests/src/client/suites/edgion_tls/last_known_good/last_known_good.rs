// tls-06 last-known-good integration test suites.
//
// Proves the two halves of the feature TOGETHER, across a hot-reload of an
// invalid (garbage) Secret update:
//   (a) GATEWAY data plane keeps serving the previously bound valid cert
//       (last-known-good): the served cert serial is unchanged after the bad
//       update.
//   (b) CONTROLLER reports the failure on the EdgionTls `ResolvedRefs` status
//       condition as status=False, reason="InvalidCertificate".
//
// Phase wiring (see dynamic/scenarios/edgion_tls_last_known_good.rs):
//   - initial:  valid lkg-secret bound -> handshake OK, ResolvedRefs=True.
//               The served cert serial is stashed for the next phase.
//   - fallback: garbage lkg-secret applied -> handshake STILL OK and serial
//               equals the stashed serial (old cert still serving), while
//               ResolvedRefs flips to False/InvalidCertificate.
//
// Required fixtures (edgion-tests/integration/conf/EdgionTls/LastKnownGood/):
//   - initial/Gateway.yaml          # HTTPS listener https-lkg on 31201
//   - initial/EdgionTls.yaml        # lkg-tls, hosts [lkg.example.com], secretRef lkg-secret
//   - initial/HTTPRoute.yaml        # route for lkg.example.com
//   - initial/Secret_lkg-secret.yaml      # valid cert (lkg-valid.{crt,key})
//   - updates_garbage/Secret_lkg-secret.yaml  # garbage tls.crt/tls.key (the bad update)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

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
fn get_served_serial(host: &str, port: u16, servername: &str) -> Result<String, String> {
    let s_out = Command::new("openssl")
        .args([
            "s_client",
            "-connect",
            &format!("{}:{}", host, port),
            "-servername",
            servername,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("failed to run openssl s_client: {}", e))?;

    let stdout = String::from_utf8_lossy(&s_out.stdout);
    let begin = "-----BEGIN CERTIFICATE-----";
    let end = "-----END CERTIFICATE-----";
    let start_idx = stdout.find(begin).ok_or_else(|| {
        format!(
            "no server certificate in s_client output (handshake failed?); stderr: {}",
            String::from_utf8_lossy(&s_out.stderr)
        )
    })?;
    let end_rel = stdout[start_idx..]
        .find(end)
        .ok_or_else(|| "malformed certificate PEM in s_client output".to_string())?;
    let end_idx = start_idx + end_rel + end.len();
    let pem = &stdout[start_idx..end_idx];

    let mut child = Command::new("openssl")
        .args(["x509", "-noout", "-serial"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn openssl x509: {}", e))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| "openssl x509 stdin unavailable".to_string())?
        .write_all(pem.as_bytes())
        .map_err(|e| format!("failed to write PEM to openssl x509: {}", e))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("failed to read openssl x509 output: {}", e))?;

    let parsed = String::from_utf8_lossy(&out.stdout);
    let serial = parsed
        .trim()
        .strip_prefix("serial=")
        .ok_or_else(|| format!("unexpected openssl x509 serial output: '{}'", parsed.trim()))?
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
    fn handshake_ok_and_stash_serial() -> TestCase {
        TestCase::new(
            "lkg_initial_handshake_and_stash_serial",
            "[INITIAL] HTTPS handshake succeeds; stash served cert serial",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match get_served_serial(&ctx.target_host, PORT, HOST) {
                        Ok(serial) => {
                            let path = stash_path(&ctx);
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

                    match get_served_serial(&ctx.target_host, PORT, HOST) {
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
