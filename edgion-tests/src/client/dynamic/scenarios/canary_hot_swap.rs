// CanaryHotSwap dynamic scenario.
//
// Initial: Canary plugin activeProfile=stable, Selector.active=stable → routes to port 30001.
// Update:  Selector.active flipped to "canary" (only the EdgionConfigData changes, no
//          EdgionPlugins/HTTPRoute edit) → routes to port 30002.
// This is the core "dynamic switch" claim of the Canary plugin design.

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::CanaryHotSwapInitialTestSuite)
}

fn final_phase() -> Box<dyn TestSuite> {
    Box::new(suites::CanaryHotSwapFinalTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "final",
        factory: final_phase,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let result: Result<()> = async {
            let scenario = SCENARIO;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;
            ctx.apply_dir("updates/flip_to_v2").await?;
            ctx.wait_phase(scenario, "final", Duration::from_secs(15)).await?;
            Ok(())
        }
        .await;
        let mut errors = Vec::new();
        let selector = async {
            ctx.apply_dir(&initial_fixture(
                &ctx,
                "EdgionConfigData_default_canary-hotswap-selector.yaml",
                "03_EdgionConfigData_canary-hotswap-selector.yaml",
            )?)
            .await
        }
        .await;
        let selector_restored = selector.is_ok();
        if let Err(error) = selector {
            errors.push(format!("restore selector: {error:#}"));
        }
        let plugin = async {
            ctx.apply_dir(&initial_fixture(
                &ctx,
                "EdgionPlugins_default_canary-hotswap-switch.yaml",
                "04_EdgionPlugins_canary-hotswap-switch.yaml",
            )?)
            .await
        }
        .await;
        let plugin_restored = plugin.is_ok();
        if let Err(error) = plugin {
            errors.push(format!("restore plugin: {error:#}"));
        }
        if selector_restored && plugin_restored {
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
    id: "CanaryHotSwap",
    conf_path: "EdgionPlugins/Canary/HotSwap/DynamicTest",
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
