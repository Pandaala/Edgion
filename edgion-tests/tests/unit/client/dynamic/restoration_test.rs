use super::ScenarioCtx;
use crate::dynamic::scenario::Scenario;
use crate::dynamic::scenarios::{
    basic_auth_dynamic, body_dynamic, btlsp_ref_flap, canary_hot_swap, condition_refs_dynamic,
};
use crate::framework::TestContext;
use anyhow::Result;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Default)]
pub(crate) struct Recorder(Arc<Mutex<Script>>);
#[derive(Default)]
struct Script {
    calls: Vec<String>,
    failures: Vec<usize>,
}
impl Recorder {
    pub(crate) fn record(&self, operation: String) -> Result<()> {
        let mut script = self.0.lock();
        let index = script.calls.len();
        script.calls.push(operation);
        if script.failures.contains(&index) {
            anyhow::bail!("injected operation {index}");
        }
        Ok(())
    }
    fn calls(&self) -> Vec<String> {
        self.0.lock().calls.clone()
    }
}

fn scenarios() -> [&'static Scenario; 6] {
    [
        basic_auth_dynamic::SCENARIO,
        basic_auth_dynamic::GROUPED_SCENARIO,
        body_dynamic::SCENARIO,
        btlsp_ref_flap::SCENARIO,
        canary_hot_swap::SCENARIO,
        condition_refs_dynamic::SCENARIO,
    ]
}
pub(crate) fn context(root: &Path, scenario: &Scenario, recorder: Recorder) -> ScenarioCtx {
    ScenarioCtx {
        operations: Some(recorder),
        test_ctx: TestContext::new(
            "127.0.0.1".into(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            None,
            None,
            false,
            false,
            root.join("access.log"),
        ),
        conf_root: root.into(),
        conf_path: scenario.conf_path.into(),
        controller_url: "invalid-controller-url".into(),
        gateway_url: String::new(),
        project_root: root.into(),
    }
}
fn fixture_root(kubernetes: bool) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(if kubernetes { "k8s/conf" } else { "integration/conf" })
}
async fn execute(scenario: &'static Scenario, root: &Path, failures: &[usize]) -> (Result<()>, Vec<String>) {
    let recorder = Recorder::default();
    recorder.0.lock().failures = failures.to_vec();
    let result = (scenario.run)(context(root, scenario, recorder.clone())).await;
    (result, recorder.calls())
}
fn reset_count(scenario: &Scenario) -> usize {
    match scenario.id {
        "BodyDynamic" | "CanaryHotSwap" => 2,
        "ConditionRefsDynamic" => 5,
        _ => 1,
    }
}
fn cleanup_start(scenario: &Scenario, calls: &[String]) -> usize {
    calls.len() - reset_count(scenario) - if scenario.id == "ConditionRefsDynamic" { 4 } else { 1 }
}

#[tokio::test]
async fn real_scenario_futures_restore_after_every_setup_transition_and_barrier_error() {
    for root in [fixture_root(false), fixture_root(true)] {
        for scenario in scenarios() {
            let (result, baseline) = execute(scenario, &root, &[]).await;
            result.unwrap();
            let start = cleanup_start(scenario, &baseline);
            let resets = &baseline[start..start + reset_count(scenario)];
            assert!(resets.iter().all(|call| call.starts_with("apply:")));
            for failed in 0..start {
                let (result, calls) = execute(scenario, &root, &[failed]).await;
                assert!(format!("{:#}", result.unwrap_err()).contains(&format!("injected operation {failed}")));
                // A setup-directory failure may already have applied earlier documents.
                // Cleanup must not depend on whether the setup operation returned success.
                assert_eq!(
                    &calls[failed + 1..failed + 1 + resets.len()],
                    resets,
                    "{} at {failed}",
                    scenario.id
                );
                assert_eq!(&calls[..=failed], &baseline[..=failed]);
                assert_eq!(&calls[failed + 1..], &baseline[start..]);
            }
        }
    }
}

#[tokio::test]
async fn every_reset_is_attempted_and_recovery_errors_do_not_hide_other_groups() {
    for scenario in scenarios() {
        let root = fixture_root(false);
        let (_, baseline) = execute(scenario, &root, &[]).await;
        let start = cleanup_start(scenario, &baseline);
        let count = reset_count(scenario);
        for failed in start..baseline.len() {
            let (result, calls) = execute(scenario, &root, &[failed]).await;
            assert!(format!("{:#}", result.unwrap_err()).contains(&format!("injected operation {failed}")));
            assert_eq!(&calls[start..start + count], &baseline[start..start + count]);
            if failed >= start + count {
                assert_eq!(calls, baseline);
            } else if scenario.id == "ConditionRefsDynamic" {
                let failed_reset = failed - start;
                let expected: Vec<_> = [
                    (failed_reset != 4, "phase:initial"),
                    (failed_reset != 3, "phase:ip-initial"),
                    (failed_reset >= 3, "phase:realip-trusted"),
                    (failed_reset != 2, "phase:secondary-restored"),
                ]
                .into_iter()
                .filter_map(|(ready, phase)| ready.then_some(phase))
                .collect();
                assert_eq!(&calls[start + count..], expected.as_slice());
            } else {
                assert_eq!(calls.len(), start + count);
            }
        }
        let (result, calls) = execute(scenario, &root, &[0, 1]).await;
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("injected operation 0") && error.contains("injected operation 1"));
        assert!(error.contains("fixture restoration failed"));
        assert_eq!(&calls[1..1 + count], &baseline[start..start + count]);
    }
}

#[tokio::test]
async fn owned_reset_paths_exist_in_both_tracked_topologies_and_preserve_phase_order() {
    let phase_sequences = [
        vec!["initial", "update", "initial"],
        vec!["initial", "update", "initial"],
        vec!["initial", "capped", "restored", "rejected", "initial"],
        vec!["initial", "invalid", "valid", "initial"],
        vec!["initial", "final", "initial"],
        vec![
            "initial",
            "ip-initial",
            "ip-updated",
            "ip-deleted",
            "ip-initial",
            "realip-trusted",
            "realip-plugin-fallback",
            "realip-trusted",
            "realip-plugin-fallback",
            "realip-trusted",
            "realip-global-fallback",
            "realip-trusted",
            "realip-global-fallback",
            "realip-trusted",
            "realip-global-fallback",
            "realip-trusted",
            "updated",
            "deleted",
            "initial",
            "initial",
            "ip-initial",
            "realip-trusted",
            "secondary-restored",
        ],
    ];
    for root in [fixture_root(false), fixture_root(true)] {
        for (scenario, expected) in scenarios().into_iter().zip(&phase_sequences) {
            let (result, calls) = execute(scenario, &root, &[]).await;
            result.unwrap();
            let actual: Vec<_> = calls.iter().filter_map(|call| call.strip_prefix("phase:")).collect();
            assert_eq!(&actual, expected, "{}", scenario.id);
            for call in &calls {
                if let Some(path) = call.strip_prefix("apply:") {
                    assert!(root.join(scenario.conf_path).join(path).exists(), "{path}");
                }
            }
            let inventory = root.join(scenario.conf_path).join("endpoint-inventory.tsv");
            if inventory.exists() {
                let inventory = std::fs::read_to_string(inventory).unwrap();
                for phase in actual {
                    assert!(inventory.lines().any(|line| line.split('\t').next() == Some(phase)));
                }
            }
        }
    }
    let relative = "EdgionPlugins/ConditionRefsDynamic/DynamicTest/initial/00_EdgionConfigData_realip-secondary.yaml";
    let local: yaml_serde::Value =
        yaml_serde::from_str(&std::fs::read_to_string(fixture_root(false).join(relative)).unwrap()).unwrap();
    let k8s: yaml_serde::Value =
        yaml_serde::from_str(&std::fs::read_to_string(fixture_root(true).join(relative)).unwrap()).unwrap();
    assert_eq!(local, k8s);
    for fixture in [
        include_str!(
            "../../../../integration/conf/EdgionPlugins/BasicAuth/DynamicTest/initial_grouped/01_Secret_partner_a.yaml"
        ),
        include_str!(
            "../../../../integration/conf/EdgionPlugins/BasicAuth/DynamicTest/initial_grouped/02_Secret_partner_b.yaml"
        ),
    ] {
        let value: yaml_serde::Value = yaml_serde::from_str(fixture).unwrap();
        assert_eq!(value["kind"].as_str(), Some("Secret"));
        assert!(matches!(
            value["stringData"]["username"].as_str(),
            Some("alice" | "bob")
        ));
        assert!(!value["stringData"]["password"].as_str().unwrap().is_empty());
    }
}

#[tokio::test]
async fn dns_family_scenario_restores_all_owners_after_any_transition_or_reset_failure() {
    let scenario = crate::dynamic::scenarios::dns_resolution_flap::SCENARIO;
    let root = fixture_root(false);
    let (result, baseline) = execute(scenario, &root, &[]).await;
    result.unwrap();
    let start = baseline.len() - 5;
    let resets = &baseline[start..start + 4];
    for failure in 0..baseline.len() {
        let (result, calls) = execute(scenario, &root, &[failure]).await;
        assert!(result.is_err());
        let cleanup = if failure < start { failure + 1 } else { start };
        assert_eq!(&calls[cleanup..cleanup + 4], resets, "failure at {failure}");
    }
    let (result, calls) = execute(scenario, &root, &[0, 1]).await;
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("injected operation 0") && error.contains("injected operation 1"));
    assert_eq!(&calls[1..5], resets);
}
