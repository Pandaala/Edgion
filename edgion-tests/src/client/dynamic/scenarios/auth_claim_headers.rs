//! AUTH-01: configured claim destinations cannot retain untrusted request values.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};

const NAME: &str = "auth-claim-headers";
const HOST: &str = "auth-claim-headers.example.com";
const SECRET: &[u8] = b"my-jwt-secret-key-32-chars-long!!";

fn observed_headers(body: &str) -> Result<Vec<(&str, &str)>> {
    ensure!(
        body.lines().any(|line| line
            .strip_prefix("Server: ")
            .and_then(|server| server.rsplit(':').next())
            == Some("30001")),
        "unexpected echo backend"
    );
    for expected in ["Method: GET", "Path: /echo", "Headers:"] {
        ensure!(body.lines().any(|line| line == expected), "incomplete echo envelope");
    }
    let (_, section) = body
        .split_once("\nHeaders:\n")
        .ok_or_else(|| anyhow::anyhow!("missing echo headers"))?;
    let mut headers = Vec::new();
    for line in section.lines().take_while(|line| !line.trim().is_empty()) {
        let (name, value) = line
            .trim()
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("invalid echoed header"))?;
        ensure!(!name.is_empty(), "empty echoed header name");
        headers.push((name, value.trim()));
    }
    ensure!(
        headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("host") && *value == HOST),
        "wrong echo host"
    );
    Ok(headers)
}

struct ClaimSuite(Option<&'static str>);
impl TestSuite for ClaimSuite {
    fn name(&self) -> &str {
        "AuthClaimHeadersDynamic"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let destination = self.0;
        vec![TestCase::new(
            "claim_headers",
            "Observe authenticated claim sanitation at the upstream",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result: Result<()> = async {
                    for role in [None, Some("user")] {
                        let mut claims = serde_json::json!({"iss":"https://test.example", "key":"default-kid", "exp":4102444800u64});
                        if let Some(role) = role { claims["role"] = serde_json::json!(role); }
                        let token = jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
                            &claims, &jsonwebtoken::EncodingKey::from_secret(SECRET))?;
                        let response = ctx.http_client.get(format!("{}/echo", ctx.http_url()))
                            .header("host", HOST).header("authorization", format!("Bearer {token}"))
                            .header("x-role", "admin").header("X-Role", "forged")
                            .header("x-new-role", "admin").header("x-unrelated", "keep")
                            .send().await.map_err(|_| anyhow::anyhow!("echo request failed"))?;
                        if destination.is_none() {
                            ensure!(response.status().as_u16() == 404, "fixture route still present");
                            continue;
                        }
                        ensure!(response.status().as_u16() == 200, "unexpected authenticated response status");
                        let body = response.text().await.map_err(|_| anyhow::anyhow!("echo body read failed"))?;
                        let headers = observed_headers(&body)?;
                        let destination = destination.unwrap();
                        let values: Vec<_> = headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case(destination)).map(|(_, value)| *value).collect();
                        ensure!(values == role.into_iter().collect::<Vec<_>>(), "configured destination retained untrusted values");
                        ensure!(headers.iter().any(|(name, value)| *name == "x-unrelated" && *value == "keep"), "unrelated header changed");
                        let retired = if destination == "x-role" { "x-new-role" } else { "x-role" };
                        ensure!(headers.iter().any(|(name, value)| name.eq_ignore_ascii_case(retired) && value.contains("admin")), "unconfigured destination changed");
                    }
                    Ok(())
                }.await;
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                    }
                })
            },
        )]
    }
}
fn absent() -> Box<dyn TestSuite> {
    Box::new(ClaimSuite(None))
}
fn initial() -> Box<dyn TestSuite> {
    Box::new(ClaimSuite(Some("x-role")))
}
fn updated() -> Box<dyn TestSuite> {
    Box::new(ClaimSuite(Some("x-new-role")))
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
    let mut errors = Vec::new();
    for kind in ["HTTPRoute", "EdgionPlugins", "Secret"] {
        match client.delete(kind, Some("edgion-default"), NAME).await {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
            _ => errors.push(format!("{kind} cleanup failed")),
        }
    }
    if ctx
        .wait_phase(SCENARIO, "absent", Duration::from_secs(20))
        .await
        .is_err()
    {
        errors.push("route absence barrier failed".into());
    }
    ensure!(errors.is_empty(), "{}", errors.join("; "));
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
        let cleaned = cleanup(&ctx).await;
        match (result, cleaned) {
            (Ok(()), cleaned) => cleaned,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleaned)) => Err(anyhow::anyhow!("{error:#}; cleanup: {cleaned:#}")),
        }
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "AuthClaimHeadersDynamic",
    conf_path: "EdgionPlugins/JwtAuth/DynamicTest",
    run,
    phases: PHASES,
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absence_checks_require_a_complete_expected_echo() {
        let valid = format!(
            "Server: 0.0.0.0:30001\nMethod: GET\nPath: /echo\n\nHeaders:\n  host: {HOST}\n  x-unrelated: keep\n"
        );
        assert!(observed_headers(&valid).is_ok());
        for invalid in [
            String::new(),
            "unrelated response".into(),
            valid.replace("30001", "30002"),
            valid.replace("GET", "POST"),
            valid.replace("/echo", "/other"),
            valid.replace("Headers:", "Other:"),
            valid.replace(HOST, "other.example.com"),
            valid.replace("x-unrelated: keep", "invalid"),
        ] {
            assert!(observed_headers(&invalid).is_err());
        }
    }
    #[test]
    fn static_secret_fixture_is_present() {
        let fixture = include_str!(
            "../../../../integration/conf/EdgionPlugins/JwtAuth/DynamicTest/initial/01_Secret_auth-claim-headers.yaml"
        );
        assert!(fixture.contains("name: auth-claim-headers"));
        assert!(fixture.contains("key: ZGVmYXVsdC1raWQ="));
    }
}
