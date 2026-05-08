// Scenario data structure and phase -> suite mapping.

use crate::dynamic::ctx::ScenarioCtx;
use crate::framework::TestSuite;
use anyhow::Result;
use std::future::Future;
use std::pin::Pin;

/// Return type of scenario.run. Each invocation yields an independent, Send-able Future.
pub type ScenarioRunFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

/// Function-pointer type for the scenario entry point.
pub type ScenarioRunFn = fn(ScenarioCtx) -> ScenarioRunFuture;

/// Phase name -> Suite factory. Every scenario declares its own supported phases.
pub struct PhaseSuite {
    pub name: &'static str,
    pub factory: fn() -> Box<dyn TestSuite>,
}

/// A dynamic scenario.
pub struct Scenario {
    /// Unique identifier (used by the CLI), e.g. "BasicAuthDynamic".
    pub id: &'static str,
    /// Scenario conf directory relative to `examples/test/conf/`, e.g. "EdgionPlugins/BasicAuth/DynamicTest".
    pub conf_path: &'static str,
    /// Scenario entry function (drives the entire N-phase state machine).
    pub run: ScenarioRunFn,
    /// Mapping from phase name to TestSuite.
    pub phases: &'static [PhaseSuite],
}

impl Scenario {
    /// Look up the suite factory for a given phase.
    pub fn phase_suite(&self, phase: &str) -> Option<&PhaseSuite> {
        self.phases.iter().find(|p| p.name == phase)
    }
}
