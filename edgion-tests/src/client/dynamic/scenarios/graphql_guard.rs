use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites::GraphqlGuardDynamicSuite;
use anyhow::{ensure, Result};
use std::{pin::Pin, time::Duration};
fn valid() -> Box<dyn TestSuite> {
    Box::new(GraphqlGuardDynamicSuite {
        invalid: false,
        strict: false,
        root_policy: false,
        calibration: false,
        measure: false,
    })
}
fn invalid() -> Box<dyn TestSuite> {
    Box::new(GraphqlGuardDynamicSuite {
        invalid: true,
        strict: false,
        root_policy: false,
        calibration: false,
        measure: false,
    })
}
fn strict() -> Box<dyn TestSuite> {
    Box::new(GraphqlGuardDynamicSuite {
        invalid: false,
        strict: true,
        root_policy: false,
        calibration: false,
        measure: false,
    })
}
fn root_policy() -> Box<dyn TestSuite> {
    Box::new(GraphqlGuardDynamicSuite {
        invalid: false,
        strict: false,
        root_policy: true,
        calibration: false,
        measure: false,
    })
}
fn measured() -> Box<dyn TestSuite> {
    Box::new(GraphqlGuardDynamicSuite {
        invalid: false,
        strict: false,
        root_policy: false,
        calibration: true,
        measure: true,
    })
}
fn enforced() -> Box<dyn TestSuite> {
    Box::new(GraphqlGuardDynamicSuite {
        invalid: false,
        strict: false,
        root_policy: false,
        calibration: true,
        measure: false,
    })
}
fn invalid_cost() -> Box<dyn TestSuite> {
    Box::new(GraphqlGuardDynamicSuite {
        invalid: true,
        strict: false,
        root_policy: false,
        calibration: true,
        measure: false,
    })
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: valid,
    },
    PhaseSuite {
        name: "invalid",
        factory: invalid,
    },
    PhaseSuite {
        name: "recovered",
        factory: valid,
    },
    PhaseSuite {
        name: "strict",
        factory: strict,
    },
    PhaseSuite {
        name: "root_policy",
        factory: root_policy,
    },
    PhaseSuite {
        name: "measure",
        factory: measured,
    },
    PhaseSuite {
        name: "invalid_mode_rejected",
        factory: measured,
    },
    PhaseSuite {
        name: "invalid_cost",
        factory: invalid_cost,
    },
    PhaseSuite {
        name: "measure_recovered",
        factory: measured,
    },
    PhaseSuite {
        name: "enforce",
        factory: enforced,
    },
    PhaseSuite {
        name: "metadata",
        factory: measured,
    },
    PhaseSuite {
        name: "invalid_metadata",
        factory: invalid_cost,
    },
    PhaseSuite {
        name: "metadata_recovered",
        factory: measured,
    },
];
fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        ctx.assert_phase(SCENARIO, "initial").await?;
        for (dir, phase) in [
            ("to_invalid", "invalid"),
            ("to_valid", "recovered"),
            ("to_strict", "strict"),
            ("to_root_policy", "root_policy"),
            ("to_measure", "measure"),
            ("to_invalid_cost", "invalid_cost"),
            ("to_measure", "measure_recovered"),
            ("to_enforce", "enforce"),
            ("to_metadata", "metadata"),
            ("to_invalid_metadata", "invalid_metadata"),
            ("to_metadata", "metadata_recovered"),
        ] {
            ctx.apply_dir(&format!("updates/{dir}")).await?;
            ctx.wait_phase(SCENARIO, phase, Duration::from_secs(15)).await?;
            if phase == "measure" {
                let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
                let source = ctx.resolve("updates/to_invalid_mode/EdgionPlugins_graphql-guard-dynamic.yaml");
                let content = std::fs::read_to_string(source)?;
                let response = client
                    .update(
                        "edgionplugins",
                        Some("edgion-default"),
                        "graphql-guard-dynamic",
                        content,
                    )
                    .await?;
                ensure!(
                    response.status().as_u16() == 400,
                    "unknown mode must reject the configuration write"
                );
                ctx.assert_phase(SCENARIO, "invalid_mode_rejected").await?;
            }
        }
        Ok(())
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "GraphqlGuardDynamic",
    conf_path: "EdgionPlugins/GraphqlGuard/DynamicTest",
    run,
    phases: PHASES,
};
