//! Configured JWT identity outputs follow the current plugin generation.
use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Result};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::json;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct ClaimSuite(Option<&'static str>);
impl TestSuite for ClaimSuite {
    fn name(&self) -> &str {
        "JwtClaimHeadersDynamic"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let owned = self.0;
        vec![TestCase::new(
            "configured_identity",
            "Only verified claims may populate configured identity headers",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result: Result<()> = async {
                        for mode in ["missing", "present", "anonymous"] {
                            let mut request = ctx
                                .http_client
                                .get(format!("{}/echo", ctx.http_url()))
                                .header("host", "jwt-claim-headers.example.com")
                                .header("x-identity-a", "forged-a")
                                .header("x-identity-a", "duplicate-a")
                                .header("x-identity-b", "forged-b")
                                .header("x-unrelated", "keep");
                            if mode != "anonymous" {
                                let exp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + 3600;
                                let mut claims = json!({"iss":"https://test.example", "key":"default-kid", "exp":exp});
                                if mode == "present" {
                                    claims["sub"] = json!("trusted-user");
                                }
                                let token = encode(
                                    &Header::new(Algorithm::HS256),
                                    &claims,
                                    &EncodingKey::from_secret(b"my-jwt-secret-key-32-chars-long!!"),
                                )?;
                                request = request.bearer_auth(token);
                            }
                            let response = request.send().await?;
                            ensure!(
                                response.status().as_u16() == if owned.is_some() { 200 } else { 404 },
                                "unexpected status {} for {mode}",
                                response.status()
                            );
                            if let Some(owned) = owned {
                                let body = response.text().await?;
                                let values = |header: &str| -> Vec<String> {
                                    body.lines()
                                        .filter_map(|line| {
                                            let (name, value) = line.trim().split_once(':')?;
                                            name.eq_ignore_ascii_case(header).then(|| value.trim().to_string())
                                        })
                                        .collect()
                                };
                                let identity = values(owned);
                                if mode == "present" {
                                    ensure!(
                                        identity == ["trusted-user"],
                                        "verified identity was not the sole output"
                                    );
                                } else {
                                    ensure!(identity.is_empty(), "unverified configured identity reached upstream");
                                }
                                let other = if owned == "x-identity-a" {
                                    "x-identity-b"
                                } else {
                                    "x-identity-a"
                                };
                                ensure!(
                                    values(other).iter().any(|v| v.contains("forged-")),
                                    "unconfigured output was unexpectedly reserved"
                                );
                                ensure!(values("x-unrelated") == ["keep"], "unrelated header changed");
                                if mode == "anonymous" {
                                    ensure!(values("x-consumer-username") == ["guest"], "anonymous identity missing");
                                }
                            }
                        }
                        Ok(())
                    }
                    .await;
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(15))]
    }
}
fn absent() -> Box<dyn TestSuite> {
    Box::new(ClaimSuite(None))
}
fn initial() -> Box<dyn TestSuite> {
    Box::new(ClaimSuite(Some("x-identity-a")))
}
fn updated() -> Box<dyn TestSuite> {
    Box::new(ClaimSuite(Some("x-identity-b")))
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "absent",
        factory: absent,
    },
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
    },
];
async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let mut failures = Vec::new();
    for kind in ["HTTPRoute", "EdgionPlugins"] {
        match client.delete(kind, Some("edgion-default"), "jwt-claim-headers").await {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
            Ok(response) => failures.push(format!("{kind} cleanup status {}", response.status())),
            Err(error) => failures.push(format!("{kind} cleanup failed: {error}")),
        }
    }
    if let Err(error) = ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await {
        failures.push(error.to_string());
    }
    ensure!(failures.is_empty(), "{}", failures.join("; "));
    Ok(())
}
fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup(&ctx).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await?;
            ctx.apply_dir("updates/").await?;
            ctx.wait_phase(SCENARIO, "updated", Duration::from_secs(20)).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await
        }
        .await;
        let cleanup = cleanup(&ctx).await;
        match (result, cleanup) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "JwtClaimHeadersDynamic",
    conf_path: "EdgionPlugins/JwtAuth/DynamicTest",
    run,
    phases: PHASES,
};
