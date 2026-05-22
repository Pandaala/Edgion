// ScenarioCtx — context and combinators shared by all scenarios.

use crate::framework::TestContext;
use anyhow::Result;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::Instant;

/// Context provided to each scenario at execution time.
/// Holds:
/// - `test_ctx`: the existing suite framework's `TestContext` (used by `assert_phase` to invoke a suite)
/// - `conf_root`: absolute path of `examples/test/conf/`
/// - `conf_path`: the current scenario's relative conf directory (e.g. `Gateway/DynamicTest`)
/// - `controller_url`: Controller Admin API base URL (used for apply)
/// - `gateway_url`: Gateway observation endpoint base URL (used for resource_diff)
/// - `project_root`: repository root, used to invoke the resource_diff subprocess
pub struct ScenarioCtx {
    pub test_ctx: TestContext,
    pub conf_root: PathBuf,
    pub conf_path: String,
    pub controller_url: String,
    pub gateway_url: String,
    pub project_root: PathBuf,
}

impl ScenarioCtx {
    /// Returns the absolute path of the current scenario's full conf directory.
    pub fn scenario_dir(&self) -> PathBuf {
        self.conf_root.join(&self.conf_path)
    }

    /// Resolve a sub-path relative to the scenario root directory.
    pub fn resolve(&self, rel: &str) -> PathBuf {
        self.scenario_dir().join(rel)
    }

    /// Thin wrapper around tokio sleep.
    pub async fn sleep(&self, dur: Duration) {
        tokio::time::sleep(dur).await;
    }
}

// apply / delete / wait helpers are added progressively in follow-up tasks.

impl ScenarioCtx {
    /// Apply a sub-path (file or directory) relative to the scenario conf directory.
    pub async fn apply_dir(&self, rel_path: &str) -> Result<()> {
        let path = self.resolve(rel_path);
        if !path.exists() {
            anyhow::bail!("apply target does not exist: {}", path.display());
        }
        let client = crate::dynamic::apply::build_client(&self.controller_url)?;
        crate::dynamic::apply::apply_path(&client, &path).await
    }

    /// Delete resources according to the `delete/resources_to_delete.txt` list.
    pub async fn delete_resources(&self, list_rel_path: &str) -> Result<()> {
        let path = self.resolve(list_rel_path);
        if !path.exists() {
            // A missing list file is treated as "nothing to delete"; return Ok (matches the old bash behavior).
            return Ok(());
        }
        let client = crate::dynamic::apply::build_client(&self.controller_url)?;
        crate::dynamic::apply::delete_list(&client, &path).await
    }
}

impl ScenarioCtx {
    /// Dispatch to the corresponding TestSuite based on the current scenario + phase name and run it.
    /// All test cases must pass; otherwise returns Err.
    pub async fn assert_phase(&self, scenario: &crate::dynamic::scenario::Scenario, phase: &str) -> Result<()> {
        use crate::framework::TestRunner;

        let ps = scenario
            .phase_suite(phase)
            .ok_or_else(|| anyhow::anyhow!("Scenario {} has no phase '{}'", scenario.id, phase))?;
        let suite = (ps.factory)();

        let mut runner = TestRunner::new(self.test_ctx.clone());
        runner.add_suite(suite);
        let results = runner.run().await;
        if !results.has_failures() {
            Ok(())
        } else {
            let mut failed_names = Vec::new();
            for sr in &results.suite_results {
                for (name, tr) in &sr.test_results {
                    if !tr.passed {
                        failed_names.push(format!(
                            "{}::{} ({})",
                            sr.name,
                            name,
                            tr.error.clone().unwrap_or_else(|| "no error".into())
                        ));
                    }
                }
            }
            Err(anyhow::anyhow!(
                "Scenario {} phase '{}' had {}/{} failing test(s): {:?}",
                scenario.id,
                phase,
                results.total_failed(),
                results.total_tests(),
                failed_names
            ))
        }
    }
}

impl ScenarioCtx {
    /// Repeatedly execute the phase suite until all tests pass or the timeout is reached.
    ///
    /// The phase suite already encodes "the correct final state" — the framework only retries
    /// until the state matches. This is more accurate than "fixed sleep + single assertion" and
    /// more general than "scenario-defined predicate".
    ///
    /// - Recommended timeout: 15s (covers gRPC push + 100ms debounce + ArcSwap).
    /// - Internal poll interval is fixed at 200ms.
    /// - Errors from intermediate failing attempts are discarded (`TestRunner` itself is silent,
    ///   and `assert_phase` only constructs an error string when it returns Err), so report.log is not polluted.
    pub async fn wait_phase(
        &self,
        scenario: &crate::dynamic::scenario::Scenario,
        phase: &str,
        timeout: Duration,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let mut attempts: usize = 0;
        loop {
            attempts += 1;
            let err = match self.assert_phase(scenario, phase).await {
                Ok(()) => return Ok(()),
                Err(e) => e,
            };
            if Instant::now() >= deadline {
                return Err(anyhow::anyhow!(
                    "wait_phase('{}') for scenario '{}' timed out after {} attempt(s); last error: {:#}",
                    phase,
                    scenario.id,
                    attempts,
                    err
                ));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}
