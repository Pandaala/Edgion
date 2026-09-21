// GatewayDynamic scenario.
//
// Phase initial: verify the baseline Gateway configuration takes effect.
// Transition: apply updates/, delete resources_to_delete.txt, wait up to 15s for sync.
// Phase update: verify the new configuration takes effect after hot reload.

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::{Context, Result};
use std::pin::Pin;
use std::time::Duration;

fn initial_suite() -> Box<dyn TestSuite> {
    Box::new(suites::InitialPhaseTestSuite)
}

fn update_suite() -> Box<dyn TestSuite> {
    Box::new(suites::UpdatePhaseTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial_suite,
    },
    PhaseSuite {
        name: "update",
        factory: update_suite,
    },
];

/// Inspect the same processed route that the traffic assertions exercise, both
/// before and after incremental publication. Full status remains observable,
/// while the native current projection must identify the serving controller.
async fn assert_current_status(ctx: &ScenarioCtx) -> Result<()> {
    let url = format!("{}/configclient/HTTPRoute/list", ctx.gateway_url.trim_end_matches('/'));
    let mut request = ctx.test_ctx.http_client.get(url).timeout(Duration::from_secs(3));
    if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        if !token.trim().is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let body: serde_json::Value = request.send().await?.error_for_status()?.json().await?;
    anyhow::ensure!(body["success"] == true, "gateway resource list failed");
    let route = body["data"]
        .as_array()
        .context("gateway resource list has no data array")?
        .iter()
        .find(|item| item["metadata"]["namespace"] == "edgion-test" && item["metadata"]["name"] == "route-method-test")
        .context("dynamic route is missing from gateway cache")?;
    anyhow::ensure!(
        route["status"].is_object(),
        "full route status was lost during publication"
    );
    let owner = route["spec"]["resolvedStatusController"]
        .as_str()
        .context("processed route has no controller ownership stamp")?;
    let parents = route["spec"]["currentStatus"]["parents"]
        .as_array()
        .context("processed route has no native currentStatus parents")?;
    anyhow::ensure!(
        !parents.is_empty() && parents.iter().all(|parent| parent["controllerName"] == owner),
        "currentStatus contains no parent or a foreign controller parent"
    );
    anyhow::ensure!(
        parents.iter().any(|parent| {
            parent["parentRef"]["name"] == "dynamic-test-gateway"
                && parent["conditions"].as_array().is_some_and(|conditions| {
                    conditions
                        .iter()
                        .any(|condition| condition["type"] == "Accepted" && condition["status"] == "True")
                })
        }),
        "currentStatus does not authorize the dynamic route parent"
    );
    Ok(())
}

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "initial").await?;
        assert_current_status(&ctx).await?;
        ctx.apply_dir("updates/").await?;
        ctx.delete_resources("delete/resources_to_delete.txt").await?;
        ctx.wait_phase(scenario, "update", Duration::from_secs(15)).await?;
        assert_current_status(&ctx).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "GatewayDynamic",
    conf_path: "Gateway/DynamicTest",
    run,
    phases: PHASES,
};
