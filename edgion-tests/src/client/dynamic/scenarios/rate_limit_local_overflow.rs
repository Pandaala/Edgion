//! Local allowance, Redis overflow, and fail-closed dependency hot reload.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::time::{Duration, Instant};

const NS: &str = "edgion-default";
const NAME: &str = "rate-limit-overflow-dynamic";
const HOST: &str = "rate-limit-overflow-dynamic.example.com";
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Absent,
    Initial,
    Invalid,
    Repaired,
}

struct OverflowSuite(Phase);

impl TestSuite for OverflowSuite {
    fn name(&self) -> &str {
        "RateLimitLocalOverflowDynamic"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.0;
        vec![TestCase::new(
            "overflow_generation",
            "Check published generation and local/Redis admission",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check(&ctx, phase).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{phase:?}: {error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(10))]
    }
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(OverflowSuite(Phase::Absent))
}
fn initial() -> Box<dyn TestSuite> {
    Box::new(OverflowSuite(Phase::Initial))
}
fn invalid() -> Box<dyn TestSuite> {
    Box::new(OverflowSuite(Phase::Invalid))
}
fn repaired() -> Box<dyn TestSuite> {
    Box::new(OverflowSuite(Phase::Repaired))
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
        name: "invalid",
        factory: invalid,
    },
    PhaseSuite {
        name: "repaired",
        factory: repaired,
    },
];

fn authed(ctx: &TestContext, request: reqwest::RequestBuilder, gateway: bool) -> reqwest::RequestBuilder {
    let gateway_token = gateway
        .then(|| std::env::var("EDGION_GATEWAY_ADMIN_TOKEN").ok())
        .flatten();
    match gateway_token.as_deref().or(ctx.admin_token.as_deref()) {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

async fn published(ctx: &TestContext, kind: &str, gateway: bool) -> Result<Option<Value>> {
    let (base, prefix) = if gateway {
        (
            std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL")
                .ok()
                .filter(|url| !url.trim().is_empty())
                .unwrap_or_else(|| format!("http://{}:12001", ctx.target_host)),
            "configclient",
        )
    } else {
        (ctx.admin_api_url(), "configserver")
    };
    let response = authed(
        ctx,
        ctx.http_client
            .get(format!(
                "{}/{prefix}/{kind}?namespace={NS}&name={NAME}",
                base.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(3)),
        gateway,
    )
    .send()
    .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = response.status();
    let body: Value = response.json().await?;
    if gateway && status.is_success() && body["success"] == false && body["error"] == format!("{kind} not found") {
        return Ok(None);
    }
    ensure!(
        status.is_success() && body["success"] == true,
        "resource query failed: {status} {body}"
    );
    Ok(body.get("data").filter(|data| !data.is_null()).cloned())
}

async fn traffic(ctx: &TestContext, key: &str) -> Result<(u16, bool)> {
    let response = ctx
        .http_client
        .get(format!("{}/echo", ctx.http_url()))
        .header("host", HOST)
        .header("X-Overflow-Key", key)
        .timeout(Duration::from_secs(3))
        .send()
        .await?;
    let status = response.status().as_u16();
    let has_local_headers = response.headers().contains_key("x-ratelimit-limit");
    let _ = response.bytes().await?;
    Ok((status, has_local_headers))
}

async fn check(ctx: &TestContext, phase: Phase) -> Result<()> {
    if phase == Phase::Absent {
        for kind in ["HTTPRoute", "EdgionPlugins"] {
            for gateway in [false, true] {
                ensure!(
                    published(ctx, kind, gateway).await?.is_none(),
                    "{kind} remains published"
                );
            }
        }
        return Ok(());
    }
    let alias = if phase == Phase::Repaired {
        "local-repaired"
    } else {
        "local-initial"
    };
    let target = if phase == Phase::Invalid {
        "missing-local"
    } else {
        alias
    };
    for gateway in [false, true] {
        let resource = published(ctx, "EdgionPlugins", gateway)
            .await?
            .context("plugin absent")?;
        ensure!(
            resource["spec"]["requestPlugins"][0]["alias"] == alias,
            "stale producer alias"
        );
        ensure!(
            resource["spec"]["requestPlugins"][1]["conditions"]["run"]["allOf"][0]["pluginAlias"] == target,
            "stale consumer reference"
        );
        if !gateway {
            let conditions = resource["status"]["conditions"]
                .as_array()
                .context("missing status conditions")?;
            let accepted = conditions
                .iter()
                .find(|condition| condition["type"] == "Accepted")
                .context("missing Accepted")?;
            ensure!(
                accepted["status"] == if phase == Phase::Invalid { "False" } else { "True" },
                "wrong Accepted: {accepted}"
            );
            if let Some(generation) = resource["metadata"]["generation"].as_i64() {
                ensure!(
                    accepted["observedGeneration"] == generation,
                    "stale Accepted generation"
                );
            }
        }
    }
    // A fresh key per polling attempt keeps readiness probes independent. The
    // one-hour interval prevents rollover between the paired admission checks.
    let key = uuid::Uuid::new_v4().to_string();
    let (first, headers) = traffic(ctx, &key).await?;
    if phase == Phase::Invalid {
        ensure!(first == 500, "invalid dependency must seal the resource, got {first}");
    } else {
        ensure!(
            first == 200,
            "first local allowance must bypass unavailable Redis, got {first}"
        );
        ensure!(!headers, "local headers must be disabled in this recipe");
        let (second, _) = traffic(ctx, &key).await?;
        ensure!(second == 429, "local excess must execute Redis Deny, got {second}");
    }
    Ok(())
}

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    // Delete is idempotent so both a clean run and static-loader preloads work.
    let mut errors = Vec::new();
    for kind in ["HTTPRoute", "EdgionPlugins"] {
        let response = authed(
            &ctx.test_ctx,
            ctx.test_ctx
                .http_client
                .delete(format!(
                    "{}/api/v1/namespaced/{kind}/{NS}/{NAME}",
                    ctx.test_ctx.admin_api_url()
                ))
                .timeout(Duration::from_secs(3)),
            false,
        )
        .send()
        .await;
        match response {
            Ok(response) if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND => {}
            Ok(response) => errors.push(format!("delete {kind}: {}", response.status())),
            Err(error) => errors.push(format!("delete {kind}: {error}")),
        }
    }
    ensure!(errors.is_empty(), "cleanup failed: {errors:?}");
    ctx.wait_phase(SCENARIO, "absent", TIMEOUT).await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result: Result<()> = async {
            cleanup(&ctx).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", TIMEOUT).await?;
            ctx.apply_dir("invalid/").await?;
            ctx.wait_phase(SCENARIO, "invalid", TIMEOUT).await?;
            ctx.apply_dir("repaired/").await?;
            ctx.wait_phase(SCENARIO, "repaired", TIMEOUT).await
        }
        .await;
        let cleaned = cleanup(&ctx).await;
        match (result, cleaned) {
            (Err(error), Err(cleanup)) => Err(error.context(format!("cleanup also failed: {cleanup:#}"))),
            (Err(error), _) => Err(error),
            (Ok(()), result) => result,
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "RateLimitLocalOverflowDynamic",
    conf_path: "EdgionPlugins/RateLimitLocal/OverflowDynamic",
    run,
    phases: PHASES,
};
