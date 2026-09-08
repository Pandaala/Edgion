// tls-06 last-known-good dynamic scenario.
//
// initial:  stash the valid fixture's expected serial, then wait until it is
//           served by the HTTPS listener and ResolvedRefs=True.
// apply:    updates_garbage/ replaces lkg-secret with a key/cert-mismatch payload.
// fallback: handshake STILL OK and the served serial equals the stashed serial
//           while the controller flips ResolvedRefs to False/InvalidCertificate.
// recovered: restore the valid Secret; verify ResolvedRefs=True and the original
//           serial without overwriting the initial stash, then repeat fallback.
// cleanup: restore the valid Secret on both success and failure.
// The controller may reject the bad Secret before gateway publication; gateway
// store unit tests separately verify its last-known-good and rejection-log state.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestSuite};
use crate::suites;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    // These assertions form a certificate-convergence barrier: an old served
    // certificate cannot pass merely because ResolvedRefs was already True.
    Box::new(ValidFixtureTestSuite {
        phase: "TLS Last-Known-Good - Initial Fixture Convergence",
    })
}
fn fallback() -> Box<dyn TestSuite> {
    Box::new(suites::LastKnownGoodFallbackTestSuite)
}

struct ValidFixtureTestSuite {
    phase: &'static str,
}

#[async_trait::async_trait]
impl TestSuite for ValidFixtureTestSuite {
    fn name(&self) -> &str {
        self.phase
    }

    fn test_cases(&self) -> Vec<TestCase> {
        suites::LastKnownGoodInitialTestSuite::recovery_test_cases()
    }
}

fn recovered() -> Box<dyn TestSuite> {
    Box::new(ValidFixtureTestSuite {
        phase: "TLS Last-Known-Good - Recovered Phase",
    })
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "fallback",
        factory: fallback,
    },
    PhaseSuite {
        name: "recovered",
        factory: recovered,
    },
];

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let scenario = SCENARIO;
        const VALID_SECRET: &str = "initial/Secret_lkg-secret.yaml";
        suites::LastKnownGoodInitialTestSuite::stash_fixture_serial(&ctx.test_ctx, &ctx.resolve(VALID_SECRET))
            .map_err(anyhow::Error::msg)?;
        let update_path = if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
            "DynamicTest/updates/garbage"
        } else {
            "updates_garbage"
        };
        let result = async {
            ctx.apply_dir(VALID_SECRET).await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;
            ctx.apply_dir(update_path).await?;
            ctx.wait_phase(scenario, "fallback", Duration::from_secs(15)).await?;
            ctx.apply_dir(VALID_SECRET).await?;
            ctx.wait_phase(scenario, "recovered", Duration::from_secs(15)).await?;
            ctx.apply_dir(update_path).await?;
            ctx.wait_phase(scenario, "fallback", Duration::from_secs(15)).await
        }
        .await;

        let restore_result = async {
            ctx.apply_dir(VALID_SECRET).await?;
            ctx.wait_phase(scenario, "recovered", Duration::from_secs(15)).await
        }
        .await;

        match (result, restore_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error.context("failed to restore the valid TLS Secret")),
            (Err(error), Err(restore_error)) => Err(error.context(format!(
                "also failed to restore the valid TLS Secret: {restore_error:#}"
            ))),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "TlsLastKnownGood",
    conf_path: "EdgionTls/LastKnownGood",
    run,
    phases: PHASES,
};
