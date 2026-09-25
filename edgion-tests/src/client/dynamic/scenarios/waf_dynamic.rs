//! Native WAF generation publication and last-known-good scenario.

use std::time::Duration;

use anyhow::{ensure, Context, Result};
use serde_json::Value;

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicInitialTestSuite)
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicUpdatedTestSuite)
}

fn recovered() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicRecoveredTestSuite)
}

fn deleted() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicDeletedTestSuite)
}

fn configured_prefix() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicConfiguredPrefixTestSuite)
}

fn detection() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicDetectionTestSuite)
}

fn injection() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicInjectionTestSuite)
}

fn control() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicControlTestSuite)
}

fn bundle_v1() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicBundleV1TestSuite)
}

fn bundle_v2() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicBundleV2TestSuite)
}

fn selector_high() -> Box<dyn TestSuite> {
    Box::new(suites::WafDynamicSelectorHighTestSuite)
}

async fn plugin_resource(ctx: &ScenarioCtx, gateway: bool) -> Result<Value> {
    let (base, prefix) = if gateway {
        (&ctx.gateway_url, "configclient")
    } else {
        (&ctx.controller_url, "configserver")
    };
    let url = format!(
        "{}/{prefix}/EdgionPlugins?namespace=edgion-default&name=waf-dynamic",
        base.trim_end_matches('/')
    );
    let mut request = ctx.test_ctx.http_client.get(url).timeout(Duration::from_secs(3));
    let token = if gateway {
        std::env::var("EDGION_GATEWAY_ADMIN_TOKEN").ok()
    } else {
        ctx.test_ctx.admin_token.clone()
    };
    if let Some(token) = token.filter(|token| !token.trim().is_empty()) {
        request = request.bearer_auth(token);
    }
    let response: Value = request.send().await?.error_for_status()?.json().await?;
    ensure!(response["success"] == true, "WAF resource lookup failed");
    response
        .get("data")
        .filter(|data| data.is_object())
        .cloned()
        .context("WAF resource missing")
}

async fn publication_generations(ctx: &ScenarioCtx) -> Result<Vec<String>> {
    let url = format!("{}/api/v1/debug/store-stats", ctx.gateway_url.trim_end_matches('/'));
    let mut request = ctx.test_ctx.http_client.get(url).timeout(Duration::from_secs(3));
    if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        if !token.trim().is_empty() {
            request = request.bearer_auth(token);
        }
    }
    let response: Value = request.send().await?.error_for_status()?.json().await?;
    ensure!(response["success"] == true, "WAF publication inventory query failed");
    let generations = response["data"]["plugin_publication_inventory"]["http_generations"]
        .as_array()
        .context("WAF publication generations are missing")?;
    Ok(generations
        .iter()
        .filter_map(Value::as_str)
        .filter(|value| value.starts_with("edgion-default/waf-dynamic@"))
        .map(str::to_string)
        .collect())
}

async fn wait_optional_bundle_warning(ctx: &ScenarioCtx) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(resource) = plugin_resource(ctx, false).await {
            let conditions = crate::dynamic::status::conditions(&resource)?;
            if conditions.iter().any(|condition| {
                condition["type"] == "PartiallyInvalid"
                    && condition["status"] == "True"
                    && condition["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("optional bundleRefs"))
            }) {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    anyhow::bail!("optional WAF bundle did not report PartiallyInvalid=True")
}

async fn assert_tampered_bundle_processed(ctx: &ScenarioCtx, prior_generations: &[String]) -> Result<()> {
    for gateway in [false, true] {
        let resource = plugin_resource(ctx, gateway).await?;
        let errors = resource["spec"]["waf"]["resolutionErrors"]
            .as_array()
            .context("WAF bundle resolution errors are missing")?;
        ensure!(
            errors
                .iter()
                .any(|error| error["name"] == "waf-bundle-v1" && error["kind"] == "invalid"),
            "tampered bundle has not reached {}",
            if gateway { "Gateway" } else { "Controller" }
        );
        let accepted = crate::dynamic::status::conditions(&resource)?
            .iter()
            .find(|condition| condition["type"] == "Accepted")
            .context("WAF Accepted condition is missing")?;
        ensure!(accepted["status"] == "False", "tampered WAF candidate was accepted");
    }
    let current = publication_generations(ctx).await?;
    ensure!(
        current.iter().any(|generation| !prior_generations.contains(generation)),
        "tampered WAF candidate has not reached the Gateway publisher"
    );
    Ok(())
}

async fn wait_tampered_bundle_processed(ctx: &ScenarioCtx, prior_generations: &[String]) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut last = None;
    while tokio::time::Instant::now() < deadline {
        match assert_tampered_bundle_processed(ctx, prior_generations).await {
            Ok(()) => return Ok(()),
            Err(error) => last = Some(error),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(last
        .unwrap_or_else(|| anyhow::anyhow!("tampered WAF bundle was not observed"))
        .context("tampered WAF bundle did not reach Controller and Gateway"))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "configured-prefix",
        factory: configured_prefix,
    },
    PhaseSuite {
        name: "detection",
        factory: detection,
    },
    PhaseSuite {
        name: "injection",
        factory: injection,
    },
    PhaseSuite {
        name: "control",
        factory: control,
    },
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
    },
    PhaseSuite {
        name: "invalid-retained",
        factory: updated,
    },
    PhaseSuite {
        name: "recovered",
        factory: recovered,
    },
    PhaseSuite {
        name: "deleted",
        factory: deleted,
    },
    PhaseSuite {
        name: "bundle-v1",
        factory: bundle_v1,
    },
    PhaseSuite {
        name: "bundle-invalid-retained",
        factory: bundle_v1,
    },
    PhaseSuite {
        name: "bundle-v2",
        factory: bundle_v2,
    },
    PhaseSuite {
        name: "bundle-rollback",
        factory: bundle_v1,
    },
    PhaseSuite {
        name: "selector-low",
        factory: bundle_v1,
    },
    PhaseSuite {
        name: "selector-high",
        factory: selector_high,
    },
    PhaseSuite {
        name: "selector-unknown-default",
        factory: bundle_v1,
    },
];

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "initial").await?;
        ctx.apply_dir("updates/to_v2").await?;
        ctx.wait_phase(scenario, "updated", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_invalid").await?;
        ctx.wait_phase(scenario, "invalid-retained", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_prefix_1m").await?;
        ctx.wait_phase(scenario, "configured-prefix", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_detection").await?;
        ctx.wait_phase(scenario, "detection", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_injection").await?;
        ctx.wait_phase(scenario, "injection", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_control").await?;
        ctx.wait_phase(scenario, "control", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_recovered").await?;
        ctx.wait_phase(scenario, "recovered", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_bundle_v1").await?;
        ctx.wait_phase(scenario, "bundle-v1", Duration::from_secs(15)).await?;
        let prior_generations = publication_generations(&ctx).await?;
        ensure!(!prior_generations.is_empty(), "bundle V1 has no published generation");
        ctx.apply_dir("updates/to_bundle_tampered").await?;
        wait_tampered_bundle_processed(&ctx, &prior_generations).await?;
        ctx.wait_phase(scenario, "bundle-invalid-retained", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_bundle_v2").await?;
        ctx.wait_phase(scenario, "bundle-v2", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_bundle_rollback").await?;
        ctx.wait_phase(scenario, "bundle-rollback", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_selector_initial").await?;
        ctx.wait_phase(scenario, "selector-low", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_selector_high").await?;
        ctx.wait_phase(scenario, "selector-high", Duration::from_secs(15))
            .await?;
        wait_optional_bundle_warning(&ctx).await?;
        ctx.apply_dir("updates/to_selector_unknown").await?;
        ctx.wait_phase(scenario, "selector-unknown-default", Duration::from_secs(15))
            .await?;
        ctx.apply_dir("updates/to_selector_high").await?;
        ctx.wait_phase(scenario, "selector-high", Duration::from_secs(15))
            .await?;
        ctx.delete_resources("delete/resources.txt").await?;
        ctx.wait_phase(scenario, "deleted", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_recovered").await?;
        ctx.wait_phase(scenario, "recovered", Duration::from_secs(15)).await
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "WafDynamic",
    conf_path: "EdgionPlugins/Waf/DynamicTest",
    run,
    phases: PHASES,
};
