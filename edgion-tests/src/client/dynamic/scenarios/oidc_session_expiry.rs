//! Retained authenticated session deadlines survive OIDC configuration replacement.
//! Uses the shared introspection fixture, not an authorization-code/token endpoint.
//! The sealed cookie carries the access token, so every plugin generation serves it
//! statelessly, including when it arrives as Envoy-style chunk cookies.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HOST: &str = "oidc-session-expiry.example.com";
const SESSION_SECRET: &str = "oidc-expiry-fixture-secret-32-bytes-long";
const TOKEN: &str = "oidc-active-token";
/// Retained `(session cookie fragment, expected to authenticate)` pairs.
static RETAINED: Mutex<Option<Vec<(String, bool)>>> = Mutex::new(None);

struct RetainedGuard;
impl Drop for RetainedGuard {
    fn drop(&mut self) {
        *RETAINED.lock() = None;
    }
}

fn encode_cookie(payload: &Value) -> Result<String> {
    let cipher = Aes256Gcm::new_from_slice(&Sha256::digest(SESSION_SECRET.as_bytes()))
        .map_err(|_| anyhow::anyhow!("fixture cipher construction failed"))?;
    let random = uuid::Uuid::new_v4();
    let nonce = &random.as_bytes()[..12];
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(nonce), serde_json::to_vec(payload)?.as_slice())
        .map_err(|_| anyhow::anyhow!("fixture cookie encryption failed"))?;
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(nonce),
        URL_SAFE_NO_PAD.encode(ciphertext)
    ))
}

fn capture_cookies() -> Result<RetainedGuard> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let base = json!({
        "access_token": TOKEN,
        "created_at": now - 120,
        "session_expires_at": now + 600,
        "expires_at": now + 3600
    });
    let mut expired = base.clone();
    expired["session_expires_at"] = json!(now - 1);
    let mut future = base.clone();
    future["created_at"] = json!(now + 3600);
    future["session_expires_at"] = json!(now + 7200);
    let mut missing = base.clone();
    missing
        .as_object_mut()
        .context("fixture must be an object")?
        .remove("session_expires_at");
    let single = |value: String| format!("edgion_oidc_session={value}");
    let sealed = encode_cookie(&base)?;
    let (first, second) = sealed.split_at(sealed.len() / 2);
    let chunked =
        format!("edgion_oidc_session_chunks=2; edgion_oidc_session_0={first}; edgion_oidc_session_1={second}");
    // A count that promises a missing chunk, with no single fallback, fails closed.
    let malformed =
        format!("edgion_oidc_session_chunks=3; edgion_oidc_session_0={first}; edgion_oidc_session_1={second}");
    *RETAINED.lock() = Some(vec![
        (single(sealed.clone()), true),
        (single(encode_cookie(&expired)?), false),
        (single(encode_cookie(&future)?), false),
        (single(encode_cookie(&missing)?), false),
        (chunked, true),
        (malformed, false),
    ]);
    Ok(RetainedGuard)
}

struct Phase(&'static str);
impl TestSuite for Phase {
    fn name(&self) -> &str {
        "OIDC session expiry hot reload"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.0;
        vec![TestCase::new(
            "retained_session_deadline",
            "Poll configured generation, retained session expiry and bearer independence",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result: Result<()> = async {
                        if phase == "deleted" {
                            let response = ctx
                                .http_client
                                .get(format!("{}/headers", ctx.http_url()))
                                .header("host", HOST)
                                .send()
                                .await?;
                            anyhow::ensure!(response.status() == 404, "owned route is not absent");
                            return Ok(());
                        }
                        let cookies = RETAINED.lock().clone().context("retained fixture cookies are absent")?;
                        for (index, (cookie, allowed)) in cookies.iter().enumerate() {
                            let response = ctx
                                .http_client
                                .get(format!("{}/headers", ctx.http_url()))
                                .header("host", HOST)
                                .header("cookie", format!("app=keep; {cookie}; edgion_oidc_state=state"))
                                .send()
                                .await?;
                            let expected = if *allowed { 200 } else { 401 };
                            anyhow::ensure!(
                                response.status().as_u16() == expected,
                                "session case {index} expected {expected}, got {}",
                                response.status()
                            );
                            if *allowed {
                                let body: Value = response.json().await?;
                                let marker = if phase == "initial" {
                                    "x-oidc-generation-one"
                                } else {
                                    "x-oidc-generation-two"
                                };
                                anyhow::ensure!(
                                    body["headers"][marker] == "oidc-user-123",
                                    "plugin generation is not current"
                                );
                                anyhow::ensure!(
                                    body["headers"]["cookie"] == "app=keep",
                                    "Gateway cookies reached upstream"
                                );
                            }
                        }
                        let response = ctx
                            .http_client
                            .get(format!("{}/headers", ctx.http_url()))
                            .header("host", HOST)
                            .header("authorization", format!("Bearer {TOKEN}"))
                            .header("cookie", cookies[1].0.clone())
                            .send()
                            .await?;
                        anyhow::ensure!(
                            response.status() == 200,
                            "expired cookie interfered with valid bearer authentication"
                        );
                        let body: Value = response.json().await?;
                        anyhow::ensure!(
                            body["headers"].get("cookie").is_none(),
                            "bearer path forwarded Gateway cookie"
                        );
                        Ok(())
                    }
                    .await;
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )]
    }
}

fn initial() -> Box<dyn TestSuite> {
    Box::new(Phase("initial"))
}
fn updated() -> Box<dyn TestSuite> {
    Box::new(Phase("updated"))
}
fn deleted() -> Box<dyn TestSuite> {
    Box::new(Phase("deleted"))
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
    },
    PhaseSuite {
        name: "deleted",
        factory: deleted,
    },
];

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let mut errors = Vec::new();
    for resource in ["route", "plugin", "secret"] {
        if let Err(error) = ctx.delete_resources(&format!("delete/{resource}.txt")).await {
            errors.push(format!("{resource}: {error:#}"));
        }
    }
    if let Err(error) = ctx.wait_phase(SCENARIO, "deleted", Duration::from_secs(15)).await {
        errors.push(format!("route absence: {error:#}"));
    }
    anyhow::ensure!(errors.is_empty(), "cleanup failed: {}", errors.join("; "));
    Ok(())
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            // Static fixture loading may have preloaded either generation.
            // Establish owned resources so deletion also works on a clean manual run.
            ctx.apply_dir("initial/").await?;
            cleanup(&ctx).await?;
            let _retained = capture_cookies()?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(15)).await?;
            ctx.apply_dir("updates/").await?;
            ctx.wait_phase(SCENARIO, "updated", Duration::from_secs(15)).await?;
            ctx.apply_dir("initial/01_EdgionPlugins_oidc-session-expiry.yaml")
                .await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(15)).await
        }
        .await;
        let cleanup = cleanup(&ctx).await;
        match (result, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!(
                "scenario failed: {error:#}; restoration failed: {cleanup:#}"
            )),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "OidcSessionExpiry",
    conf_path: "EdgionPlugins/OpenidConnect/SessionExpiry/DynamicTest",
    run,
    phases: PHASES,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_secret_fixture_matches_cookie_signer() {
        for yaml in [
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/integration/conf/EdgionPlugins/OpenidConnect/SessionExpiry/DynamicTest/initial/00_Secret_oidc-session-expiry.yaml")),
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/k8s/conf/EdgionPlugins/OpenidConnect/SessionExpiry/DynamicTest/initial/00_Secret_oidc-session-expiry.yaml")),
        ] {
            let secret: Value = yaml_serde::from_str(yaml).unwrap();
            assert_eq!(secret["stringData"]["sessionSecret"], SESSION_SECRET);
        }
    }
}
