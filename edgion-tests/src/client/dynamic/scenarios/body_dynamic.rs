use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial_suite() -> Box<dyn TestSuite> {
    Box::new(suites::BodyDynamicInitialTestSuite)
}

fn update_suite() -> Box<dyn TestSuite> {
    Box::new(suites::BodyDynamicUpdateTestSuite)
}

fn capped_suite() -> Box<dyn TestSuite> {
    Box::new(suites::BodyDynamicCappedTestSuite)
}

fn rejected_suite() -> Box<dyn TestSuite> {
    Box::new(suites::BodyDynamicRejectedTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial_suite,
    },
    PhaseSuite {
        name: "capped",
        factory: capped_suite,
    },
    PhaseSuite {
        name: "restored",
        factory: update_suite,
    },
    PhaseSuite {
        name: "rejected",
        factory: rejected_suite,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let result: Result<()> = async {
            let scenario = SCENARIO;
            ctx.apply_dir("updates/to_restored/EdgionGatewayConfig_example-gateway.yaml")
                .await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;
            ctx.apply_dir("updates/to_capped").await?;
            ctx.wait_phase(scenario, "capped", Duration::from_secs(15)).await?;
            ctx.apply_dir("updates/to_restored").await?;
            ctx.wait_phase(scenario, "restored", Duration::from_secs(15)).await?;
            ctx.apply_dir("updates/to_invalid").await?;
            ctx.wait_phase(scenario, "rejected", Duration::from_secs(15)).await?;
            Ok(())
        }
        .await;
        let mut errors = Vec::new();
        let policy = async {
            ctx.apply_dir("updates/to_restored/EdgionGatewayConfig_example-gateway.yaml")
                .await
        }
        .await;
        let policy_restored = policy.is_ok();
        if let Err(error) = policy {
            errors.push(format!("restore policy: {error:#}"));
        }
        let plugin = async { ctx.apply_dir(&initial_plugin(&ctx)?).await }.await;
        let plugin_restored = plugin.is_ok();
        if let Err(error) = plugin {
            errors.push(format!("restore plugin: {error:#}"));
        }
        if policy_restored && plugin_restored {
            if let Err(error) = ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(15)).await {
                errors.push(format!("verify initial: {error:#}"));
            }
        }
        let restored = if errors.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(errors.join("; ")))
        };
        match (result, restored) {
            (Ok(()), restored) => restored,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(restored)) => Err(anyhow::anyhow!("{error:#}; fixture restoration failed: {restored:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "BodyDynamic",
    conf_path: "EdgionPlugins/BodyDynamic/DynamicTest",
    run,
    phases: PHASES,
};

fn initial_fixture(ctx: &ScenarioCtx, local: &str, kubernetes: &str) -> Result<String> {
    let local = format!("initial/{local}");
    let kubernetes = format!("initial/{kubernetes}");
    match (ctx.resolve(&local).is_file(), ctx.resolve(&kubernetes).is_file()) {
        (true, false) => Ok(local),
        (false, true) => Ok(kubernetes),
        _ => anyhow::bail!("expected exactly one initial fixture: {local} or {kubernetes}"),
    }
}

fn initial_plugin(ctx: &ScenarioCtx) -> Result<String> {
    initial_fixture(
        ctx,
        "EdgionPlugins_body-dynamic.yaml",
        "01_EdgionPlugins_body-dynamic.yaml",
    )
}

#[cfg(test)]
mod fixture_path_tests {
    use super::*;
    use crate::dynamic::ctx::restoration_tests::{context, Recorder};

    #[test]
    fn initial_fixture_requires_exactly_one_topology_candidate() {
        let root = tempfile::tempdir().unwrap();
        let ctx = context(root.path(), SCENARIO, Recorder::default());
        let initial = ctx.resolve("initial");
        std::fs::create_dir_all(&initial).unwrap();
        assert!(initial_fixture(&ctx, "local.yaml", "k8s.yaml").is_err());
        std::fs::write(initial.join("local.yaml"), "").unwrap();
        assert_eq!(
            initial_fixture(&ctx, "local.yaml", "k8s.yaml").unwrap(),
            "initial/local.yaml"
        );
        std::fs::write(initial.join("k8s.yaml"), "").unwrap();
        assert!(initial_fixture(&ctx, "local.yaml", "k8s.yaml").is_err());
        std::fs::remove_file(initial.join("local.yaml")).unwrap();
        assert_eq!(
            initial_fixture(&ctx, "local.yaml", "k8s.yaml").unwrap(),
            "initial/k8s.yaml"
        );
    }
}
