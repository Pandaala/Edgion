// tls-06 last-known-good dynamic scenario.
//
// initial:  valid lkg-secret bound -> HTTPS handshake OK, ResolvedRefs=True;
//           the served cert serial is stashed.
// apply:    updates_garbage/ replaces lkg-secret with a key/cert-mismatch payload.
// fallback: handshake STILL OK and the served serial equals the stashed serial
//           (gateway keeps the last-known-good cert), while the controller flips
//           ResolvedRefs to False/InvalidCertificate.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::LastKnownGoodInitialTestSuite)
}
fn fallback() -> Box<dyn TestSuite> {
    Box::new(suites::LastKnownGoodFallbackTestSuite)
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
];

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let scenario = SCENARIO;
        ctx.assert_phase(scenario, "initial").await?;
        let update_path = if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
            "DynamicTest/updates/garbage"
        } else {
            "updates_garbage"
        };
        ctx.apply_dir(update_path).await?;
        ctx.wait_phase(scenario, "fallback", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "TlsLastKnownGood",
    conf_path: "EdgionTls/LastKnownGood",
    run,
    phases: PHASES,
};
