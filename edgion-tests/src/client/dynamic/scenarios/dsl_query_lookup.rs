//! Query-ID examples use the existing shared Gateway/Controller/Redis topology.
use crate::dynamic::scenario::ScenarioRunFuture;
use crate::dynamic::{PhaseSuite, Scenario, ScenarioCtx};
use crate::framework::TestSuite;
use crate::suites::edgion_plugins::query_lookup::{self, QueryLookupPhase};
use anyhow::Result;
use std::time::Duration;

macro_rules! phase_factory {
    ($name:ident, $phase:literal) => {
        fn $name() -> Box<dyn TestSuite> {
            Box::new(QueryLookupPhase($phase))
        }
    };
}
phase_factory!(absent, "absent");
phase_factory!(published, "published");
phase_factory!(initial, "initial");
phase_factory!(rotated, "rotated");
phase_factory!(invalid_retained, "invalid-retained");
phase_factory!(unconfigured, "unconfigured");
phase_factory!(fresh_invalid, "fresh-invalid");
phase_factory!(recovered, "recovered");
phase_factory!(missing_provider, "missing-provider");
phase_factory!(corrupt, "corrupt");
phase_factory!(deleted, "deleted");
phase_factory!(expired, "expired");
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "absent",
        factory: absent,
    },
    PhaseSuite {
        name: "published",
        factory: published,
    },
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "rotated",
        factory: rotated,
    },
    PhaseSuite {
        name: "invalid-retained",
        factory: invalid_retained,
    },
    PhaseSuite {
        name: "unconfigured",
        factory: unconfigured,
    },
    PhaseSuite {
        name: "fresh-invalid",
        factory: fresh_invalid,
    },
    PhaseSuite {
        name: "recovered",
        factory: recovered,
    },
    PhaseSuite {
        name: "missing-provider",
        factory: missing_provider,
    },
    PhaseSuite {
        name: "corrupt",
        factory: corrupt,
    },
    PhaseSuite {
        name: "deleted",
        factory: deleted,
    },
    PhaseSuite {
        name: "expired",
        factory: expired,
    },
];
async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    // Retire ingress first; mappings are exact, finite and exclusively owned.
    for role in ["publisher", "executor"] {
        delete_owned(ctx, "HTTPRoute", &format!("dsl-query-{role}-dynamic")).await?;
    }
    query_lookup::clean_keys(
        &ctx.test_ctx,
        &[format!(
            "{}{}",
            query_lookup::DYNAMIC_NAMESPACE,
            query_lookup::dynamic_id()
        )],
    )
    .await?;
    let list = tokio::fs::read_to_string(ctx.resolve("delete/resources_to_delete.txt")).await?;
    for line in list.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let parts: Vec<_> = line.split('/').collect();
        anyhow::ensure!(
            parts.len() == 3
                && parts[1] == "edgion-default"
                && ["dsl-query-publisher-dynamic", "dsl-query-executor-dynamic"].contains(&parts[2]),
            "Unexpected owned cleanup resource"
        );
        if parts[0] != "HTTPRoute" {
            delete_owned(ctx, parts[0], parts[2]).await?;
        }
    }
    ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await
}
async fn delete_owned(ctx: &ScenarioCtx, kind: &str, name: &str) -> Result<()> {
    let request = ctx.test_ctx.http_client.delete(format!(
        "{}/api/v1/namespaced/{kind}/edgion-default/{name}",
        ctx.test_ctx.admin_api_url()
    ));
    let request = match &ctx.test_ctx.admin_token {
        Some(token) => request.bearer_auth(token),
        None => request,
    };
    let response = request.timeout(Duration::from_secs(3)).send().await?;
    anyhow::ensure!(
        response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND,
        "Owned resource deletion failed"
    );
    Ok(())
}
async fn wait_executor_absent(ctx: &ScenarioCtx) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let request = ctx.test_ctx.http_client.get(format!(
                "{}/api/v1/namespaced/EdgionPlugins/edgion-default/dsl-query-executor-dynamic",
                ctx.test_ctx.admin_api_url()
            ));
            let request = match &ctx.test_ctx.admin_token {
                Some(token) => request.bearer_auth(token),
                None => request,
            };
            if request.timeout(Duration::from_secs(3)).send().await?.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("Owned executor did not disappear before fresh invalid generation"))?
}
fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup(&ctx).await?;
            ctx.apply_dir("initial").await?;
            ctx.wait_phase(SCENARIO, "published", Duration::from_secs(20)).await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await?;
            ctx.apply_dir("updates/rotate").await?;
            ctx.wait_phase(SCENARIO, "rotated", Duration::from_secs(20)).await?;
            ctx.apply_dir("updates/invalid").await?;
            ctx.wait_phase(SCENARIO, "invalid-retained", Duration::from_secs(20))
                .await?;
            ctx.apply_dir("updates/recovered").await?;
            ctx.wait_phase(SCENARIO, "recovered", Duration::from_secs(20)).await?;
            ctx.test_ctx
                .delete_resource("EdgionPlugins", "edgion-default", "dsl-query-executor-dynamic")
                .await
                .map_err(anyhow::Error::msg)?;
            ctx.wait_phase(SCENARIO, "unconfigured", Duration::from_secs(20))
                .await?;
            wait_executor_absent(&ctx).await?;
            ctx.apply_dir("updates/invalid").await?;
            ctx.wait_phase(SCENARIO, "fresh-invalid", Duration::from_secs(20))
                .await?;
            ctx.apply_dir("updates/recovered").await?;
            ctx.wait_phase(SCENARIO, "recovered", Duration::from_secs(20)).await?;
            ctx.apply_dir("updates/missing-provider").await?;
            ctx.wait_phase(SCENARIO, "missing-provider", Duration::from_secs(20))
                .await?;
            ctx.apply_dir("updates/recovered").await?;
            ctx.wait_phase(SCENARIO, "recovered", Duration::from_secs(20)).await?;
            let key = format!("{}{}", query_lookup::DYNAMIC_NAMESPACE, query_lookup::dynamic_id());
            query_lookup::set(&ctx.test_ctx, &key, "{}", 60).await?;
            ctx.wait_phase(SCENARIO, "corrupt", Duration::from_secs(20)).await?;
            query_lookup::clean_keys(&ctx.test_ctx, std::slice::from_ref(&key)).await?;
            ctx.wait_phase(SCENARIO, "deleted", Duration::from_secs(20)).await?;
            query_lookup::publish_dynamic(&ctx.test_ctx).await?;
            ctx.wait_phase(SCENARIO, "recovered", Duration::from_secs(20)).await?;
            let record = serde_json::to_string(&serde_json::json!({"query":query_lookup::DYNAMIC_QUERY}))?;
            query_lookup::set(&ctx.test_ctx, &key, &record, 1).await?;
            ctx.wait_phase(SCENARIO, "expired", Duration::from_secs(20)).await?;
            query_lookup::publish_dynamic(&ctx.test_ctx).await?;
            ctx.wait_phase(SCENARIO, "recovered", Duration::from_secs(20)).await?;
            Ok(())
        }
        .await;
        let cleanup = cleanup(&ctx).await;
        match (result, cleanup) {
            (Ok(()), result) => result,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup: {cleanup:#}")),
        }
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "DslQueryLookupDynamic",
    conf_path: "EdgionPlugins/Dsl/QueryLookup/DynamicTest",
    run,
    phases: PHASES,
};
