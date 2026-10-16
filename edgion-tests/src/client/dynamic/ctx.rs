// ScenarioCtx — context and combinators shared by all scenarios.

use crate::framework::TestContext;
use anyhow::{Context, Result};
use std::error::Error;
use std::fmt;
use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use tokio::time::Instant;

const DEFAULT_K8S_PHASE_ACK_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug)]
struct K8sPhaseBarrierError(String);

impl fmt::Display for K8sPhaseBarrierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for K8sPhaseBarrierError {}

/// Context provided to each scenario at execution time.
/// Holds:
/// - `test_ctx`: the existing suite framework's `TestContext` (used by `assert_phase` to invoke a suite)
/// - `conf_root`: absolute path of `edgion-tests/integration/conf/`
/// - `conf_path`: the current scenario's relative conf directory (e.g. `Gateway/DynamicTest`)
/// - `controller_url`: Controller Admin API base URL (used for apply)
/// - `gateway_url`: Gateway observation endpoint base URL (used for resource_diff)
/// - `project_root`: repository root, used to invoke the resource_diff subprocess
pub struct ScenarioCtx {
    #[cfg(test)]
    pub(crate) operations: Option<restoration_tests::Recorder>,
    pub test_ctx: TestContext,
    pub conf_root: PathBuf,
    pub conf_path: String,
    pub controller_url: String,
    pub gateway_url: String,
    pub project_root: PathBuf,
}

impl ScenarioCtx {
    async fn acknowledge_k8s_phase(&self, scenario: &crate::dynamic::scenario::Scenario, phase: &str) -> Result<()> {
        if std::env::var("EDGION_TEST_K8S_PHASE_BARRIER").as_deref() != Ok("true") {
            return Ok(());
        }

        println!("@@DYNAMIC_PHASE {} {}", scenario.id, phase);
        std::io::stdout()
            .flush()
            .map_err(|error| K8sPhaseBarrierError(format!("K8s phase barrier output failed: {error}")))?;

        let expected = format!("@@DYNAMIC_PHASE_ACK {} {} OK", scenario.id, phase);
        let acknowledgement_timeout = std::env::var("EDGION_TEST_K8S_PHASE_ACK_TIMEOUT_SECONDS").map_or(
            Ok(DEFAULT_K8S_PHASE_ACK_TIMEOUT),
            |value| {
                value
                    .parse::<u64>()
                    .ok()
                    .filter(|seconds| *seconds > 0)
                    .map(Duration::from_secs)
                    .ok_or_else(|| K8sPhaseBarrierError(format!("invalid K8s phase acknowledgement timeout: {value}")))
            },
        )?;
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = std::io::stdin().lock().read_line(&mut line).map(|_| line);
            let _ = sender.send(result);
        });
        let acknowledgement = tokio::task::spawn_blocking(move || receiver.recv_timeout(acknowledgement_timeout))
            .await
            .map_err(|error| K8sPhaseBarrierError(format!("K8s phase barrier input task failed: {error}")))?
            .map_err(|error| {
                K8sPhaseBarrierError(format!(
                    "K8s phase barrier acknowledgement timed out after {}s: {error}",
                    acknowledgement_timeout.as_secs()
                ))
            })?
            .map_err(|error| K8sPhaseBarrierError(format!("K8s phase barrier input failed: {error}")))?;
        if acknowledgement.trim_end() != expected {
            return Err(K8sPhaseBarrierError(format!(
                "K8s phase barrier for scenario {} phase '{}' failed: {}",
                scenario.id,
                phase,
                acknowledgement.trim_end()
            ))
            .into());
        }
        Ok(())
    }

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

    /// Resolve the configured Gateway target for clients that override a logical host name.
    pub async fn target_socket_addr(&self, port: u16) -> Result<SocketAddr> {
        match self.test_ctx.target_host.parse() {
            Ok(ip) => Ok(SocketAddr::new(ip, port)),
            Err(_) => tokio::net::lookup_host((self.test_ctx.target_host.as_str(), port))
                .await
                .with_context(|| format!("failed to resolve test target {}", self.test_ctx.target_host))?
                .next()
                .with_context(|| format!("test target {} resolved to no addresses", self.test_ctx.target_host)),
        }
    }
}

impl ScenarioCtx {
    /// Apply a sub-path (file or directory) relative to the scenario conf directory.
    pub async fn apply_dir(&self, rel_path: &str) -> Result<()> {
        #[cfg(test)]
        if let Some(recorder) = &self.operations {
            return recorder.record(format!("apply:{rel_path}"));
        }
        let path = self.resolve(rel_path);
        if !path.exists() {
            anyhow::bail!("apply target does not exist: {}", path.display());
        }
        let client = crate::dynamic::apply::build_client(&self.controller_url)?;
        crate::dynamic::apply::apply_path(&client, &path).await
    }

    /// Delete resources according to the `delete/resources_to_delete.txt` list.
    pub async fn delete_resources(&self, list_rel_path: &str) -> Result<()> {
        #[cfg(test)]
        if let Some(recorder) = &self.operations {
            return recorder.record(format!("delete:{list_rel_path}"));
        }
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
        #[cfg(test)]
        if let Some(recorder) = &self.operations {
            return recorder.record(format!("phase:{phase}"));
        }
        use crate::framework::TestRunner;

        let ps = scenario
            .phase_suite(phase)
            .ok_or_else(|| anyhow::anyhow!("Scenario {} has no phase '{}'", scenario.id, phase))?;
        let suite = (ps.factory)();

        let mut runner = TestRunner::new(self.test_ctx.clone());
        if self.test_ctx.gateway {
            let port_config = crate::port_config::PortConfig::load().map_err(anyhow::Error::msg)?;
            runner.set_port_resolver(Box::new(move |key, kind| {
                port_config.require_port(key, kind)?;
                port_config.get_ports(key).cloned()
            }));
        }
        runner.add_suite(suite);
        let results = runner.run().await;
        if !results.has_failures() {
            self.acknowledge_k8s_phase(scenario, phase).await
        } else {
            let mut failure_details = results.run_errors.clone();
            for sr in &results.suite_results {
                for error in &sr.lifecycle_errors {
                    failure_details.push(format!("{}::{} ({})", sr.name, error.phase.as_str(), error.message));
                }
                for (name, tr) in &sr.test_results {
                    if !tr.passed {
                        failure_details.push(format!(
                            "{}::{} ({})",
                            sr.name,
                            name,
                            tr.error.clone().unwrap_or_else(|| "no error".into())
                        ));
                    }
                }
            }
            Err(anyhow::anyhow!(
                "Scenario {} phase '{}' failed ({}/{} test cases failed): {:?}",
                scenario.id,
                phase,
                results.total_failed(),
                results.total_tests(),
                failure_details
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
        #[cfg(test)]
        if let Some(recorder) = &self.operations {
            return recorder.record(format!("phase:{phase}"));
        }
        let deadline = Instant::now() + timeout;
        let mut attempts: usize = 0;
        loop {
            attempts += 1;
            let err = match self.assert_phase(scenario, phase).await {
                Ok(()) => return Ok(()),
                Err(e) => e,
            };
            if err.downcast_ref::<K8sPhaseBarrierError>().is_some() {
                return Err(err);
            }
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

#[cfg(test)]
#[path = "../../../tests/unit/client/dynamic/restoration_test.rs"]
pub(crate) mod restoration_tests;
