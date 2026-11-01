use std::time::Duration;

use anyhow::{anyhow, ensure, Context, Result};
use serde_json::{json, Value};
use tokio::time::{sleep, timeout};

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::{apply, status, ScenarioCtx};
use crate::framework::TestSuite;
use crate::suites;
use crate::suites::status::helpers;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::CredentialInjectorDynamicInitialTestSuite)
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(suites::CredentialInjectorDynamicUpdatedTestSuite)
}

fn deleted() -> Box<dyn TestSuite> {
    Box::new(suites::CredentialInjectorDynamicDeletedTestSuite)
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

const PLUGIN_NAME: &str = "credential-injector-dynamic";
const PLUGIN_NAMESPACE: &str = "edgion-default";
const PLUGIN_FIXTURE: &str = "initial/01_EdgionPlugins_credential-injector-dynamic.yaml";
const PHASE_HEADER: &str = "x-edgion-credential-ref-phase";
const PHASE_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

async fn controller_plugin(ctx: &ScenarioCtx) -> Result<Value> {
    timeout(
        REQUEST_TIMEOUT,
        helpers::fetch_resource_status(&ctx.test_ctx, "EdgionPlugins", PLUGIN_NAMESPACE, PLUGIN_NAME),
    )
    .await
    .map_err(|_| anyhow!("Controller plugin read timed out"))?
    .map_err(|_| anyhow!("Controller plugin read failed"))
}

fn matches_phase(resource: &Value, authored: &Value) -> bool {
    let entries = &resource["spec"]["requestPlugins"];
    let expected = &authored["spec"]["requestPlugins"];
    resource["metadata"]["name"] == PLUGIN_NAME
        && resource["metadata"]["namespace"] == PLUGIN_NAMESPACE
        && entries.as_array().map(Vec::len) == expected.as_array().map(Vec::len)
        && entries[0]["type"] == "CredentialInjector"
        && entries[0]["config"]["credentialRef"] == expected[0]["config"]["credentialRef"]
        && entries[1]["type"] == expected[1]["type"]
        && entries[1]["config"]["set"] == expected[1]["config"]["set"]
}

async fn gateway_plugin(ctx: &ScenarioCtx) -> Result<Value> {
    let mut request = ctx
        .test_ctx
        .http_client
        .get(format!(
            "{}/configclient/EdgionPlugins",
            ctx.gateway_url.trim_end_matches('/')
        ))
        .query(&[("namespace", PLUGIN_NAMESPACE), ("name", PLUGIN_NAME)])
        .timeout(REQUEST_TIMEOUT);
    if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|_| anyhow!("Gateway plugin read failed"))?;
    ensure!(
        response.status().is_success(),
        "Gateway plugin read returned an error status"
    );
    let body: Value = response
        .json()
        .await
        .map_err(|_| anyhow!("Gateway plugin response was not JSON"))?;
    ensure!(body["success"] == true, "Gateway plugin is unavailable");
    Ok(body["data"].clone())
}

async fn phase_request(ctx: &ScenarioCtx, marker: Option<&str>) -> Result<()> {
    let response = ctx
        .test_ctx
        .http_client
        .get(format!("{}/headers", ctx.test_ctx.http_url()))
        .header("host", "credential-injector-dynamic.example.com")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|_| anyhow!("Credential phase request failed"))?;
    ensure!(
        response.status().as_u16() == 200,
        "Credential phase request was not successful"
    );
    let body: Value = response
        .json()
        .await
        .map_err(|_| anyhow!("Credential phase echo was not JSON"))?;
    ensure!(
        body["headers"]["authorization"].as_str() == Some("Bearer dynamic-token-initial"),
        "Credential phase did not inject the initial generation"
    );
    ensure!(
        body["headers"][PHASE_HEADER].as_str() == marker,
        "Credential phase execution marker did not match"
    );
    Ok(())
}

async fn apply_phase(
    ctx: &ScenarioCtx,
    authored: &Value,
    previous: &Value,
    generation_floor: &mut i64,
    marker: Option<&str>,
    kubernetes: bool,
) -> Result<Value> {
    let mut next = authored.clone();
    let expected_generation = if kubernetes {
        None
    } else {
        *generation_floor = (*generation_floor)
            .max(previous["metadata"]["generation"].as_i64().unwrap_or(0))
            .checked_add(1)
            .context("Credential phase generation overflow")?;
        next["metadata"]["generation"] = json!(*generation_floor);
        Some(*generation_floor)
    };
    let client =
        apply::build_client(&ctx.controller_url).map_err(|_| anyhow!("Credential phase client setup failed"))?;
    let yaml = yaml_serde::to_string(&next).map_err(|_| anyhow!("Credential phase serialization failed"))?;
    timeout(
        PHASE_TIMEOUT,
        apply::apply_yaml_documents(&client, &ctx.resolve(PLUGIN_FIXTURE), &yaml),
    )
    .await
    .map_err(|_| anyhow!("Credential phase apply timed out"))?
    .map_err(|_| anyhow!("Credential phase apply failed"))?;

    let observed = timeout(PHASE_TIMEOUT, async {
        loop {
            let attempt = async {
                let current = controller_plugin(ctx).await?;
                ensure!(
                    matches_phase(&current, authored),
                    "Controller phase fields have not converged"
                );
                let generation = current["metadata"]["generation"]
                    .as_i64()
                    .context("Missing phase generation")?;
                let old_uid = previous["metadata"].get("uid");
                ensure!(
                    old_uid.is_none() || current["metadata"].get("uid") == old_uid,
                    "Plugin owner changed"
                );
                if let Some(expected) = expected_generation {
                    ensure!(generation == expected, "Authored phase generation has not converged");
                } else {
                    let before = previous["metadata"]["generation"]
                        .as_i64()
                        .context("Missing previous generation")?;
                    let unchanged_restore = marker.is_none() && matches_phase(previous, authored);
                    ensure!(
                        generation > before || (unchanged_restore && generation == before),
                        "Assigned phase generation has not converged"
                    );
                    let version = current["metadata"]["resourceVersion"]
                        .as_str()
                        .filter(|v| !v.is_empty());
                    ensure!(
                        version.is_some()
                            && (unchanged_restore
                                || current["metadata"]["resourceVersion"] != previous["metadata"]["resourceVersion"]),
                        "Assigned phase version has not converged"
                    );
                }
                let conditions = status::conditions(&current).map_err(|_| anyhow!("Missing controller conditions"))?;
                for kind in ["Accepted", "ResolvedRefs"] {
                    ensure!(
                        conditions.iter().any(|c| c["type"] == kind
                            && c["status"] == "True"
                            && c["observedGeneration"].as_i64() == Some(generation)),
                        "Current phase conditions have not converged"
                    );
                }
                let received = gateway_plugin(ctx).await?;
                ensure!(
                    matches_phase(&received, authored)
                        && received["metadata"]["generation"].as_i64() == Some(generation)
                        && received["metadata"].get("uid") == current["metadata"].get("uid"),
                    "Gateway phase receipt has not converged"
                );
                phase_request(ctx, marker).await?;
                Ok::<_, anyhow::Error>(current)
            }
            .await;
            if let Ok(current) = attempt {
                break current;
            }
            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("Credential phase convergence timed out"))?;
    // The harness ACK has its own bounded timeout and must not become a retryable probe.
    ctx.wait_phase(SCENARIO, "initial", PHASE_TIMEOUT).await?;
    Ok(observed)
}

async fn check_secret_groups(ctx: &ScenarioCtx) -> Result<()> {
    let bytes = tokio::fs::read_to_string(ctx.resolve(PLUGIN_FIXTURE))
        .await
        .map_err(|_| anyhow!("Credential plugin fixture read failed"))?;
    let original: Value =
        yaml_serde::from_str(&bytes).map_err(|_| anyhow!("Credential plugin fixture parse failed"))?;
    ensure!(
        original["metadata"]["name"] == PLUGIN_NAME
            && original["metadata"]["namespace"] == PLUGIN_NAMESPACE
            && original["spec"]["requestPlugins"].as_array().map(Vec::len) == Some(1)
            && original["spec"]["requestPlugins"][0]["type"] == "CredentialInjector"
            && original["spec"]["requestPlugins"][0]["config"]["credentialRef"]
                .get("group")
                .is_none(),
        "Unexpected original CredentialInjector fixture"
    );
    let kubernetes = std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true");
    let mut previous = controller_plugin(ctx).await?;
    if kubernetes {
        ensure!(
            previous["metadata"]["generation"].as_i64().is_some()
                && previous["metadata"]["uid"].as_str().is_some_and(|v| !v.is_empty())
                && previous["metadata"]["resourceVersion"]
                    .as_str()
                    .is_some_and(|v| !v.is_empty()),
            "Missing Kubernetes starting identity"
        );
    }
    let mut generation_floor = previous["metadata"]["generation"].as_i64().unwrap_or(0).max(0);
    let nonce = uuid::Uuid::new_v4();
    let primary = async {
        for (ordinal, group) in [None, Some(""), Some("core"), None].into_iter().enumerate() {
            let marker = format!("{nonce}-{ordinal}");
            let mut authored = original.clone();
            if let Some(group) = group {
                authored["spec"]["requestPlugins"][0]["config"]["credentialRef"]["group"] = json!(group);
            }
            authored["spec"]["requestPlugins"]
                .as_array_mut()
                .context("Missing plugin entries")?
                .push(json!({
                    "type": "RequestHeaderModifier", "config": {"set": [{"name": PHASE_HEADER, "value": marker}]}
                }));
            previous = apply_phase(
                ctx,
                &authored,
                &previous,
                &mut generation_floor,
                Some(&marker),
                kubernetes,
            )
            .await
            .with_context(|| format!("Credential group phase {ordinal} failed"))?;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;

    // A failed preliminary read must not prevent a possibly committed mutation from being restored.
    let before_restore = controller_plugin(ctx).await;
    let cleanup = apply_phase(
        ctx,
        &original,
        before_restore.as_ref().unwrap_or(&previous),
        &mut generation_floor,
        None,
        kubernetes,
    )
    .await
    .and_then(|_| before_restore.map(|_| ()));
    match (primary, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(()), Err(cleanup)) => Err(cleanup.context("Credential plugin restoration failed")),
        (Err(primary), Err(cleanup)) => Err(anyhow!(
            "{primary:#}; credential plugin restoration also failed: {cleanup:#}"
        )),
    }
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.apply_dir("initial/").await?;
        ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;

        check_secret_groups(&ctx).await?;

        let update_result = async {
            ctx.apply_dir("updates/00_Secret_credential-injector-dynamic.yaml")
                .await?;
            ctx.wait_phase(scenario, "updated", Duration::from_secs(15)).await
        }
        .await;

        let delete_result = async {
            ctx.delete_resources("delete/secret.txt").await?;
            ctx.wait_phase(scenario, "deleted", Duration::from_secs(15)).await
        }
        .await;

        let restore_result = async {
            ctx.apply_dir("initial/00_Secret_credential-injector-dynamic.yaml")
                .await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await
        }
        .await;

        restore_result?;
        update_result?;
        delete_result
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "CredentialInjectorDynamic",
    conf_path: "EdgionPlugins/CredentialInjector/DynamicTest",
    run,
    phases: PHASES,
};
