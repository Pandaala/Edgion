//! EdgionAIService provider credential rotation without process or config-server reloads.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::AiServiceSecretRotationInitialTestSuite)
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(suites::AiServiceSecretRotationUpdatedTestSuite)
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
];

async fn server_id(client: &reqwest::Client, base: &str, token: Option<&str>) -> Result<String> {
    let mut request = client.get(format!("{}/api/v1/server-info", base.trim_end_matches('/')));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.context("server-info request failed")?;
    let status = response.status();
    let body = response.text().await.context("server-info body read failed")?;
    anyhow::ensure!(status.is_success(), "server-info returned {status}: {body}");
    let value: Value = serde_json::from_str(&body).context("server-info returned invalid JSON")?;
    value
        .pointer("/data/server_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .context("server-info response has no non-empty data.server_id")
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let scenario = SCENARIO;
        let gateway_token = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty());
        let controller_before = server_id(
            &ctx.test_ctx.http_client,
            &ctx.controller_url,
            ctx.test_ctx.admin_token.as_deref(),
        )
        .await?;
        let gateway_before = server_id(&ctx.test_ctx.http_client, &ctx.gateway_url, gateway_token.as_deref()).await?;
        anyhow::ensure!(
            controller_before == gateway_before,
            "Controller and Gateway started from different config-server generations"
        );

        // Establish the phase explicitly because dynamic sweeps may run more
        // than once against the same Controller and Gateway processes.
        ctx.apply_dir("initial/00_Secret_ai-provider.yaml").await?;
        ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;

        let rotation_result = async {
            ctx.apply_dir("updates/00_Secret_ai-provider.yaml").await?;
            ctx.wait_phase(scenario, "updated", Duration::from_secs(15)).await
        }
        .await;

        // Restore the shared suite fixture even when the rotation assertion
        // fails, so later suites and a second --with-reload sweep start at A.
        let restore_result = async {
            ctx.apply_dir("initial/00_Secret_ai-provider.yaml").await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await
        }
        .await;
        restore_result?;
        rotation_result?;

        let controller_after = server_id(
            &ctx.test_ctx.http_client,
            &ctx.controller_url,
            ctx.test_ctx.admin_token.as_deref(),
        )
        .await?;
        let gateway_after = server_id(&ctx.test_ctx.http_client, &ctx.gateway_url, gateway_token.as_deref()).await?;
        anyhow::ensure!(
            controller_after == controller_before && gateway_after == gateway_before,
            "Secret rotation unexpectedly changed the config-server generation: controller {controller_before:?}->{controller_after:?}, gateway {gateway_before:?}->{gateway_after:?}"
        );
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "AiServiceSecretRotation",
    conf_path: "EdgionAIService/SingleProvider/DynamicTest",
    run,
    phases: PHASES,
};

#[cfg(test)]
mod tests {
    use serde_yaml::Value;

    const INITIAL_SECRET: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/integration/conf/EdgionAIService/SingleProvider/DynamicTest/initial/00_Secret_ai-provider.yaml"
    ));
    const UPDATED_SECRET: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/integration/conf/EdgionAIService/SingleProvider/DynamicTest/updates/00_Secret_ai-provider.yaml"
    ));

    fn api_key(manifest: &str) -> String {
        let value: Value = serde_yaml::from_str(manifest).unwrap();
        value["stringData"]["api-key"].as_str().unwrap().to_string()
    }

    #[test]
    fn tracked_rotation_fixtures_use_distinct_credentials() {
        assert_eq!(api_key(INITIAL_SECRET), "ai-provider-test-key");
        assert_eq!(api_key(UPDATED_SECRET), "ai-provider-rotated-test-key");
    }
}
