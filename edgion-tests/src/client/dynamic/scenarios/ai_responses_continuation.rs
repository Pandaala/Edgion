//! Hot-update a raw credential declaration without switching a continuation principal.
use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use std::time::Duration;
fn allowed() -> Box<dyn TestSuite> {
    Box::new(suites::ResponsesContinuationAllowedSuite)
}
fn rejected() -> Box<dyn TestSuite> {
    Box::new(suites::ResponsesContinuationRejectedSuite)
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "allowed",
        factory: allowed,
    },
    PhaseSuite {
        name: "rejected",
        factory: rejected,
    },
];
fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let source = ctx
            .conf_root
            .join("EdgionBackend/Responses/03_EdgionBackend_responses.yaml");
        let original = tokio::fs::read_to_string(&source).await?;
        let mut changed: serde_yaml::Value = serde_yaml::from_str(&original)?;
        changed["spec"]["ai"]["credentialPool"]["credentials"]
            .as_sequence_mut()
            .ok_or_else(|| anyhow::anyhow!("missing credentials"))?
            .push(serde_yaml::from_str(
                "name: unresolved\nsecretRef:\n  name: responses-missing-credential\n",
            )?);
        let changed = serde_yaml::to_string(&changed)?;
        let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
        crate::dynamic::apply::apply_yaml_documents(&client, &source, &original).await?;
        ctx.wait_phase(SCENARIO, "allowed", Duration::from_secs(15)).await?;
        let update = async {
            crate::dynamic::apply::apply_yaml_documents(&client, &source, &changed).await?;
            ctx.wait_phase(SCENARIO, "rejected", Duration::from_secs(15)).await
        }
        .await;
        let restore = async {
            crate::dynamic::apply::apply_yaml_documents(&client, &source, &original).await?;
            ctx.wait_phase(SCENARIO, "allowed", Duration::from_secs(15)).await
        }
        .await;
        restore?;
        update?;
        Ok(())
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "AiResponsesContinuation",
    conf_path: "EdgionBackend/Responses/DynamicTest",
    run,
    phases: PHASES,
};
