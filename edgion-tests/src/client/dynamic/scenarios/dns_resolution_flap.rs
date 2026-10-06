// DnsResolutionFlap dynamic scenario.
//
// Proves an ExternalName Service's DNS resolver is hot-reloadable: flipping the
// `edgion.io/dns-resolver-ref` annotation at runtime takes effect on the gateway
// without a restart.
//
//   initial:  resolver-ref → dns-flap-dead (down httpdns, fallback: none) → 5xx
//   working:  resolver-ref → dns-flap-local (live httpdns → backend IP)     → 200

use crate::dynamic::scenario::{PhaseSuite, Scenario};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
use anyhow::Result;
use std::pin::Pin;
use std::time::Duration;

fn failing() -> Box<dyn TestSuite> {
    Box::new(suites::DnsFlapFailingTestSuite)
}

fn working() -> Box<dyn TestSuite> {
    Box::new(suites::DnsFlapWorkingTestSuite)
}

fn bounded() -> Box<dyn TestSuite> {
    Box::new(suites::DnsFlapBoundedTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "failing",
        factory: failing,
    },
    PhaseSuite {
        name: "working",
        factory: working,
    },
    PhaseSuite {
        name: "bounded",
        factory: bounded,
    },
];

fn run(ctx: ScenarioCtx) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let scenario = SCENARIO;
        // Initial state: dead resolver + fallback:none → resolution fails.
        ctx.wait_phase(scenario, "failing", Duration::from_secs(15)).await?;
        // Hot-reload the Service's resolver-ref annotation to a live httpdns.
        ctx.apply_dir("updates/to_working").await?;
        ctx.wait_phase(scenario, "working", Duration::from_secs(15)).await?;
        // A LinkSys update must also replace the endpoint policy generation.
        // Link-local remains denied even with an explicit loopback exception.
        ctx.apply_dir("updates/to_blocked").await?;
        ctx.wait_phase(scenario, "failing", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_restored").await?;
        ctx.wait_phase(scenario, "working", Duration::from_secs(15)).await?;
        // Tightening the body cap must reach the prepared transport generation.
        ctx.apply_dir("updates/to_small_response").await?;
        ctx.wait_phase(scenario, "bounded", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_restored").await?;
        ctx.wait_phase(scenario, "working", Duration::from_secs(15)).await?;
        // Advisory admission must not preserve an old usable client or silently
        // use System DNS when the latest connection cannot be prepared.
        ctx.apply_dir("updates/to_invalid_connection").await?;
        ctx.wait_phase(scenario, "failing", Duration::from_secs(15)).await?;
        ctx.apply_dir("updates/to_restored").await?;
        ctx.wait_phase(scenario, "working", Duration::from_secs(15)).await?;
        Ok(())
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "DnsResolutionFlap",
    conf_path: "HTTPRoute/Backend/DnsResolutionFlap/DynamicTest",
    run,
    phases: PHASES,
};
