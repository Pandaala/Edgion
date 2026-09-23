//! CORS token validation, disabled entries, and recovery on the shared topology.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Context, Result};
use reqwest::Method;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const NAME: &str = "cors-validation-dynamic";
const NS: &str = "edgion-default";
const ORIGIN: &str = "https://app.example.com";
const METHODS: &str = "PROPFIND, custom-Method, GET";
const HEADERS: &str = "X-Custom_Header,\tX-Second";
const EXPOSE: &str = "X-Count, X-Count,";

#[derive(Clone, Copy)]
enum Phase {
    Absent,
    Valid,
    Invalid(&'static str),
    MergedRegex,
    Disabled,
}

fn plugin(phase: Phase) -> Value {
    let mut config = json!({
        "allowOrigins": ORIGIN,
        "allowCredentials": true,
        "allowMethods": METHODS,
        "allowHeaders": HEADERS,
        "exposeHeaders": EXPOSE
    });
    match phase {
        Phase::Invalid(field) => config[field] = json!("Bad Name"),
        Phase::MergedRegex => {
            config["allowCredentials"] = json!(false);
            config["allowOriginsByRegex"] = json!([r"(?P<sub>[a-z]+)\.example\.com", r"(?P<sub>[a-z]+)\.example\.org"]);
        }
        Phase::Disabled => config["allowMethods"] = json!("Bad Name"),
        _ => {}
    }
    json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"name": NAME, "namespace": NS},
        "spec": {"requestPlugins": [{
            "type": "Cors", "enable": !matches!(phase, Phase::Disabled), "config": config
        }]}
    })
}

async fn check(ctx: &TestContext, phase: Phase) -> Result<()> {
    if !matches!(phase, Phase::Absent) {
        let mut request = ctx
            .http_client
            .get(format!(
                "{}/configserver/EdgionPlugins?namespace={NS}&name={NAME}",
                ctx.admin_api_url()
            ))
            .timeout(Duration::from_secs(3));
        if let Some(token) = &ctx.admin_token {
            request = request.bearer_auth(token);
        }
        let body: Value = request.send().await?.error_for_status()?.json().await?;
        ensure!(body["success"] == true, "Controller resource lookup failed");
        let resource = &body["data"];
        let expected = plugin(phase);
        let entry = &resource["spec"]["requestPlugins"][0];
        let expected_entry = &expected["spec"]["requestPlugins"][0];
        ensure!(
            entry["enable"].as_bool().unwrap_or(true) == expected_entry["enable"].as_bool().unwrap_or(true),
            "Controller has a different enable state"
        );
        for field in ["allowMethods", "allowHeaders", "exposeHeaders", "allowOriginsByRegex"] {
            ensure!(
                entry["config"][field] == expected_entry["config"][field],
                "Controller has a different {field}"
            );
        }
        let accepted = resource["status"]["conditions"]
            .as_array()
            .and_then(|conditions| conditions.iter().find(|c| c["type"] == "Accepted"))
            .context("missing Accepted condition")?;
        let invalid = matches!(phase, Phase::Invalid(_) | Phase::MergedRegex);
        if let Some(generation) = resource["metadata"]["generation"].as_i64() {
            ensure!(accepted["observedGeneration"] == generation, "stale Accepted condition");
        }
        ensure!(
            accepted["status"] == if invalid { "False" } else { "True" },
            "unexpected Accepted condition: {accepted}"
        );
        if let Phase::Invalid(field) = phase {
            ensure!(accepted["reason"] == "Invalid", "unexpected invalid reason");
            ensure!(
                accepted["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(field)),
                "missing field diagnostic"
            );
        } else if matches!(phase, Phase::MergedRegex) {
            ensure!(accepted["reason"] == "Invalid", "unexpected invalid reason");
            ensure!(
                accepted["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("allow_origins_by_regex item 2")),
                "missing merged-regex diagnostic"
            );
        }
    }

    for method in [Method::GET, Method::OPTIONS] {
        if !matches!(phase, Phase::Valid | Phase::Invalid(_) | Phase::MergedRegex) && method == Method::OPTIONS {
            continue;
        }
        let preflight = method == Method::OPTIONS;
        let mut request = ctx
            .http_client
            .request(method, format!("{}/echo", ctx.http_url()))
            .header("host", "cors-validation-dynamic.example.com")
            .header("origin", ORIGIN)
            .timeout(Duration::from_secs(3));
        if preflight {
            request = request.header("access-control-request-method", "custom-Method");
        }
        let response = request.send().await?;
        let expected = match phase {
            Phase::Absent => 404,
            Phase::Invalid(_) | Phase::MergedRegex => 500,
            Phase::Valid if preflight => 204,
            _ => 200,
        };
        ensure!(
            response.status().as_u16() == expected,
            "expected {expected}, got {}",
            response.status()
        );
        let headers = response.headers();
        if matches!(phase, Phase::Valid) {
            for (name, expected) in [
                ("access-control-allow-origin", ORIGIN),
                ("access-control-allow-credentials", "true"),
                ("access-control-expose-headers", EXPOSE),
            ] {
                ensure!(
                    headers.get(name).and_then(|v| v.to_str().ok()) == Some(expected),
                    "unexpected {name}"
                );
            }
            for (name, value) in [
                ("access-control-allow-methods", METHODS),
                ("access-control-allow-headers", HEADERS),
            ] {
                let expected = preflight.then_some(value);
                ensure!(
                    headers.get(name).and_then(|v| v.to_str().ok()) == expected,
                    "unexpected {name}"
                );
            }
        } else {
            ensure!(
                !headers.contains_key("access-control-allow-origin"),
                "inactive/invalid CORS emitted an allow header"
            );
        }
    }
    Ok(())
}

struct Suite(Phase);
impl TestSuite for Suite {
    fn name(&self) -> &str {
        "CorsValidation"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.0;
        vec![TestCase::new(
            "cors_validation_generation",
            "Check status, wire values and fail-closed behavior",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check(&ctx, phase).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )]
    }
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "absent",
        factory: || Box::new(Suite(Phase::Absent)),
    },
    PhaseSuite {
        name: "valid",
        factory: || Box::new(Suite(Phase::Valid)),
    },
    PhaseSuite {
        name: "methods",
        factory: || Box::new(Suite(Phase::Invalid("allowMethods"))),
    },
    PhaseSuite {
        name: "headers",
        factory: || Box::new(Suite(Phase::Invalid("allowHeaders"))),
    },
    PhaseSuite {
        name: "expose",
        factory: || Box::new(Suite(Phase::Invalid("exposeHeaders"))),
    },
    PhaseSuite {
        name: "merged_regex",
        factory: || Box::new(Suite(Phase::MergedRegex)),
    },
    PhaseSuite {
        name: "disabled",
        factory: || Box::new(Suite(Phase::Disabled)),
    },
];

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let mut errors = Vec::new();
    for kind in ["HTTPRoute", "EdgionPlugins"] {
        match client.delete(kind, Some(NS), NAME).await {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
            Ok(response) => errors.push(format!("{kind}: {}", response.status())),
            Err(error) => errors.push(format!("{kind}: {error}")),
        }
    }
    ensure!(errors.is_empty(), "cleanup failed: {}", errors.join("; "));
    ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup(&ctx).await?;
            ctx.apply_dir("initial").await?;
            let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
            for (name, phase) in [
                ("valid", Phase::Valid),
                ("methods", Phase::Invalid("allowMethods")),
                ("headers", Phase::Invalid("allowHeaders")),
                ("expose", Phase::Invalid("exposeHeaders")),
                ("merged_regex", Phase::MergedRegex),
                ("disabled", Phase::Disabled),
                ("valid", Phase::Valid),
            ] {
                crate::dynamic::apply::apply_yaml_documents(
                    &client,
                    &ctx.resolve("phase.yaml"),
                    &serde_yaml::to_string(&plugin(phase))?,
                )
                .await?;
                ctx.wait_phase(SCENARIO, name, Duration::from_secs(20)).await?;
            }
            Ok(())
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
    id: "CorsValidation",
    conf_path: "EdgionPlugins/Cors/DynamicTest",
    run,
    phases: PHASES,
};
