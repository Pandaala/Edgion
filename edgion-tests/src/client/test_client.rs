// Edgion Unified Test Client
// Supports all protocol tests: HTTP/HTTPS, gRPC, WebSocket, TCP, UDP

#![allow(clippy::module_inception)]
#![allow(dead_code)]
#![allow(unused_imports)]

// Shared client framework lives in the `edgion_tests` library crate.
use edgion_tests::framework::{self, TestContext, TestRunner};
use edgion_tests::reporter::{ConsoleReporter, JsonReporter};
use edgion_tests::{dynamic, port_config, suites};

use anyhow::Result;
use clap::Parser;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Once, OnceLock};
use std::time::Instant;

static INIT: Once = Once::new();
static CAPABILITY_REGISTRY: OnceLock<String> = OnceLock::new();
const KNOWN_GATEWAY_CAPABILITIES: &[&str] = &["body-buffering"];
const EXPECTED_DYNAMIC_SCENARIO_IDS: &[&str] = &[
    "RequestMirrorReferenceGrant",
    "ServiceDegradeThreshold",
    "DirectEndpointPortResolution",
    "AllEndpointStatusTls",
    "AiBackendLifecycle",
    "AiBackendPolicyIntegration",
    "AiBackendSecretRotation",
    "AiBackendWeighted",
    "GatewayDynamic",
    "DslDynamic",
    "BodyDynamic",
    "ConditionRefsDynamic",
    "CredentialInjectorDynamic",
    "CsrfSecretRotation",
    "JsonSchemaValidationDynamic",
    "FormJsonTransformDynamic",
    "BasicAuthDynamic",
    "BasicAuthDynamicGrouped",
    "RateLimitDynamic",
    "RequestAccessPolicyHmacDynamic",
    "RouteDelegationDynamic",
    "GrpcRouteDelegationDynamic",
    "ObservedGenHighFreq",
    "BtlspRefFlap",
    "PmdRefFlap",
    "RegionRouteValidation",
    "SpBackendChange",
    "SpWeighted",
    "CanaryHotSwap",
    "DnsResolutionFlap",
    "TcpEspReload",
    "GcParamsFlap",
    "TlsLastKnownGood",
    "WafDynamic",
];

fn capability_entry_applies(kind: &str, entry_id: &str, id: &str) -> bool {
    entry_id == id || (kind == "suite" && id.strip_prefix(entry_id).is_some_and(|suffix| suffix.starts_with('/')))
}

fn validate_capability_registry(contents: &str) -> Result<()> {
    let mut entries = HashSet::new();
    let conf_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("integration/conf");
    // Runtime images intentionally contain the test binary, not the local
    // source tree used to compile it. Keep the directory cross-check when the
    // source tree is present (developer and CI runs); the registry syntax,
    // capability names, duplicates, and dynamic ids remain validated in every
    // environment.
    let validate_suite_directories = conf_root.is_dir();
    for (index, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 3 {
            anyhow::bail!(
                "capability registry line {} must contain exactly three tab-separated fields",
                index + 1
            );
        }
        let (kind, id, capability) = (fields[0], fields[1], fields[2]);
        if !matches!(kind, "suite" | "dynamic") {
            anyhow::bail!("capability registry line {} has unknown kind `{kind}`", index + 1);
        }
        if id.is_empty() || capability.is_empty() {
            anyhow::bail!("capability registry line {} contains an empty field", index + 1);
        }
        if !KNOWN_GATEWAY_CAPABILITIES.contains(&capability) {
            anyhow::bail!(
                "capability registry line {} has unknown capability `{capability}`",
                index + 1
            );
        }
        if !entries.insert((kind, id)) {
            anyhow::bail!("capability registry contains duplicate {kind} id `{id}`");
        }
        match kind {
            "suite" if validate_suite_directories && !conf_root.join(id).is_dir() => {
                anyhow::bail!(
                    "capability registry suite `{id}` does not exist under {}",
                    conf_root.display()
                );
            }
            "dynamic" if !dynamic::all().iter().any(|scenario| scenario.id == id) => {
                anyhow::bail!("capability registry dynamic scenario `{id}` is not registered");
            }
            _ => {}
        }
    }
    Ok(())
}

fn capability_registry() -> Result<&'static str> {
    if CAPABILITY_REGISTRY.get().is_none() {
        let contents = match std::env::var_os("EDGION_CAPABILITY_REGISTRY") {
            Some(path) => {
                let path = PathBuf::from(path);
                std::fs::read_to_string(&path).map_err(|err| anyhow::anyhow!("read {}: {err}", path.display()))?
            }
            None => include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/integration/capabilities.tsv")).to_string(),
        };
        validate_capability_registry(&contents)?;
        let _ = CAPABILITY_REGISTRY.set(contents);
    }
    Ok(CAPABILITY_REGISTRY.get().expect("capability registry initialized"))
}

fn validate_dynamic_scenario_ids(ids: &[&str]) -> Result<()> {
    if ids.is_empty() {
        anyhow::bail!("dynamic scenario registry is empty");
    }
    if ids.iter().any(|id| id.trim().is_empty()) {
        anyhow::bail!("dynamic scenario registry contains an empty id");
    }
    let actual: HashSet<&str> = ids.iter().copied().collect();
    if actual.len() != ids.len() {
        anyhow::bail!("dynamic scenario registry contains duplicate ids");
    }
    let expected: HashSet<&str> = EXPECTED_DYNAMIC_SCENARIO_IDS.iter().copied().collect();
    if actual != expected {
        let mut missing: Vec<_> = expected.difference(&actual).copied().collect();
        let mut unexpected: Vec<_> = actual.difference(&expected).copied().collect();
        missing.sort_unstable();
        unexpected.sort_unstable();
        anyhow::bail!(
            "dynamic scenario registry differs from the expected manifest: missing={missing:?}, unexpected={unexpected:?}"
        );
    }
    Ok(())
}

fn validated_dynamic_scenarios() -> Result<Vec<&'static dynamic::Scenario>> {
    capability_registry()?;
    let scenarios = dynamic::all();
    let ids: Vec<_> = scenarios.iter().map(|scenario| scenario.id).collect();
    validate_dynamic_scenario_ids(&ids)?;
    for scenario in &scenarios {
        dynamic_port_contract(scenario.id)?;
    }
    Ok(scenarios)
}

fn required_capability(kind: &str, id: &str) -> Result<Option<&'static str>> {
    Ok(capability_registry()?.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let mut fields = line.split('\t');
        match (fields.next(), fields.next(), fields.next(), fields.next()) {
            (Some(entry_kind), Some(entry_id), Some(capability), None)
                if entry_kind == kind && capability_entry_applies(kind, entry_id, id) =>
            {
                Some(capability)
            }
            _ => None,
        }
    }))
}

fn gateway_has_capability(capability: &str) -> bool {
    std::env::var("EDGION_TEST_GATEWAY_CAPABILITIES")
        .ok()
        .is_some_and(|value| value.lines().any(|line| line == format!("{capability}=enabled")))
}

fn missing_capability(kind: &str, id: &str) -> Result<Option<&'static str>> {
    Ok(required_capability(kind, id)?.filter(|capability| !gateway_has_capability(capability)))
}

#[cfg(test)]
mod capability_tests {
    use super::{
        capability_entry_applies, dynamic_port_contract, required_port_kind, resolve_suite, suite_to_port_key,
        validate_capability_registry, validate_dynamic_scenario_ids, EXPECTED_DYNAMIC_SCENARIO_IDS,
    };
    use edgion_tests::{dynamic, port_config::PortConfig};
    use std::collections::BTreeSet;

    fn runner_suite_sweeps(runner: &str) -> Result<(&str, &str), &'static str> {
        let branches = runner
            .split_once("run_all_tests() {")
            .ok_or("runner must define run_all_tests")?
            .1
            .split_once(r#"    if [ -n "$G_RESOURCE" ]; then"#)
            .ok_or("run_all_tests must contain the resource selection branch")?
            .1;
        let (targeted, default_and_tail) = branches
            .split_once("\n    else\n")
            .ok_or("resource selection branch must contain a default sweep")?;
        let default = default_and_tail
            .split_once("\n    fi\n")
            .ok_or("default sweep must end with the resource selection branch")?
            .0;
        Ok((targeted, default))
    }

    fn validate_single_runner_registration(runner: &str, suite: &str) -> Result<(), String> {
        let (targeted, default) = runner_suite_sweeps(runner).map_err(str::to_string)?;
        let needle = format!(r#"run_test "{suite}""#);
        let targeted_count = targeted.matches(&needle).count();
        let default_count = default.matches(&needle).count();
        if (targeted_count, default_count) == (1, 1) {
            Ok(())
        } else {
            Err(format!(
                "{suite} must be registered exactly once in both runner sweeps; found targeted={targeted_count}, default={default_count}"
            ))
        }
    }

    fn runner_fixture(targeted: &[&str], default: &[&str]) -> String {
        let registrations = |suites: &[&str]| {
            suites
                .iter()
                .map(|suite| format!(r#"        run_test "{suite}" command"#))
                .collect::<Vec<_>>()
                .join("\n")
        };
        format!(
            "run_all_tests() {{\n    if [ -n \"$G_RESOURCE\" ]; then\n{}\n    else\n{}\n    fi\n}}",
            registrations(targeted),
            registrations(default)
        )
    }

    #[test]
    fn empty_registry_is_valid() {
        validate_capability_registry("# kind\tid\trequired-capability\n").unwrap();
    }

    #[test]
    fn dynamic_registry_manifest_is_complete_unique_and_nonempty() {
        let actual: Vec<_> = dynamic::all().iter().map(|scenario| scenario.id).collect();
        validate_dynamic_scenario_ids(&actual).unwrap();
        assert_eq!(actual.len(), EXPECTED_DYNAMIC_SCENARIO_IDS.len());

        assert!(validate_dynamic_scenario_ids(&[]).is_err());
        assert!(validate_dynamic_scenario_ids(&["AiBackendLifecycle", "AiBackendLifecycle"]).is_err());
        assert!(validate_dynamic_scenario_ids(&[""]).is_err());
    }

    #[test]
    fn shell_dynamic_enumeration_failure_is_never_a_successful_noop() {
        let runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_integration.sh"
        ));
        let function = runner
            .split_once("run_dynamic_scenarios() {")
            .and_then(|(_, rest)| rest.split_once("\n}\n"))
            .map(|(body, _)| body)
            .expect("runner must define run_dynamic_scenarios");

        assert!(function.contains(r#"if ! ids=$("$test_client" --list-dynamic-scenarios); then"#));
        assert!(
            !function.contains("2>/dev/null"),
            "enumeration stderr must remain visible"
        );
        assert!(
            !function.contains("|| true"),
            "enumeration failure must not be suppressed"
        );
        assert!(function.contains("Dynamic scenario enumeration returned an empty registry"));
        assert_eq!(
            runner.matches("run_dynamic_scenarios || test_failed=true").count(),
            2,
            "both dynamic-enumeration call sites must fail the run"
        );
    }

    #[test]
    fn integration_readmes_publish_only_supported_runner_commands_and_artifacts() {
        let scripts_readme = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/integration/scripts/README.md"));
        let gateway_readme = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/conf/Gateway/README.md"
        ));
        let dynamic_readme = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/conf/Gateway/DynamicTest/README.md"
        ));
        let dynamic_summary = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/conf/Gateway/DynamicTest/TEST_SUMMARY.md"
        ));
        let prepare = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/utils/prepare.sh"
        ));

        for document in [scripts_readme, gateway_readme, dynamic_readme, dynamic_summary] {
            for unsupported in ["--test ", "--skip ", "--dynamic-test"] {
                assert!(
                    !document.contains(unsupported),
                    "unsupported runner option {unsupported}"
                );
            }
        }
        for supported in ["-r HTTPRoute", "-i Match", "--no-prepare", "--full-test", "--suites"] {
            assert!(scripts_readme.contains(supported), "README must document {supported}");
        }
        let expected_artifacts: BTreeSet<_> = [
            "edgion-controller",
            "edgion-gateway",
            "edgion-cli",
            "test_server",
            "test_client",
            "test_client_direct",
            "resource_diff",
            "config_load_validator",
            "cache_residue",
            "credential_reservation_redis",
            "token_bucket_partial_refill_redis",
            "oidc_egress_resolver",
        ]
        .into_iter()
        .collect();
        let artifact_section = scripts_readme
            .split_once("Built artifacts under `target/debug/`:")
            .and_then(|(_, rest)| rest.split_once("\n\nOutput location:"))
            .map(|(section, _)| section)
            .expect("README must contain the prepared artifact section");
        let documented_artifacts: BTreeSet<_> = artifact_section
            .lines()
            .filter_map(|line| line.trim().strip_prefix("- `"))
            .filter_map(|line| line.split_once('`').map(|(artifact, _)| artifact))
            .collect();
        assert_eq!(documented_artifacts, expected_artifacts);

        let mut prepared_artifacts: BTreeSet<_> = prepare
            .lines()
            .filter_map(|line| {
                line.split_once("--bin ")
                    .and_then(|(_, name)| name.split_whitespace().next())
                    .map(|name| name.trim_end_matches(';'))
            })
            .collect();
        if let Some((_, rest)) = prepare.split_once("for ignored_test in ") {
            let names = rest.split_once(';').map(|(names, _)| names).unwrap_or_default();
            prepared_artifacts.extend(names.split_whitespace());
        }
        assert!(
            prepare.contains("test_oidc_egress.sh") && prepare.contains("--build-resolver-only"),
            "prepare must build the OIDC egress resolver before runtime"
        );
        prepared_artifacts.insert("oidc_egress_resolver");
        assert_eq!(documented_artifacts, prepared_artifacts);
    }

    #[test]
    fn rejects_malformed_unknown_and_duplicate_entries() {
        assert!(validate_capability_registry("suite base body-buffering\n").is_err());
        assert!(validate_capability_registry("suite\tbase\tunknown\n").is_err());
        assert!(validate_capability_registry("other\tbase\tbody-buffering\n").is_err());
        assert!(validate_capability_registry("suite\tbase\tbody-buffering\nsuite\tbase\tbody-buffering\n").is_err());
    }

    #[test]
    fn suite_capability_applies_to_nested_config_only() {
        assert!(capability_entry_applies(
            "suite",
            "EdgionPlugins/Body",
            "EdgionPlugins/Body"
        ));
        assert!(capability_entry_applies(
            "suite",
            "EdgionPlugins/Body",
            "EdgionPlugins/Body/initial"
        ));
        assert!(!capability_entry_applies(
            "suite",
            "EdgionPlugins/Body",
            "EdgionPlugins/BodyOther"
        ));
        assert!(!capability_entry_applies(
            "dynamic",
            "body-reload",
            "body-reload/initial"
        ));
    }

    #[test]
    fn every_dynamic_scenario_has_a_valid_listener_contract() {
        let config: PortConfig = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/conf/ports.json"
        )))
        .expect("ports.json must parse");

        for scenario in dynamic::all() {
            if let Some((key, kind)) = dynamic_port_contract(scenario.id).expect("scenario contract must be declared") {
                config
                    .require_port(key, kind)
                    .unwrap_or_else(|error| panic!("scenario `{}`: {error}", scenario.id));
            }
        }
    }

    #[test]
    fn every_registered_data_plane_suite_has_a_primary_port() {
        let config: PortConfig = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/conf/ports.json"
        )))
        .expect("ports.json must parse");
        let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/client/test_client.rs"));
        let arms = source
            .split_once("\nfn add_suites_for_suite")
            .expect("suite registration function must exist")
            .1;
        let arms = arms
            .split_once("\n}\n")
            .expect("suite registration function must end")
            .0;
        let suites: BTreeSet<_> = arms
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                line.strip_prefix('"')
                    .and_then(|rest| rest.find('"').map(|end| &rest[..end]))
                    .filter(|suite| suite.contains('/'))
            })
            .collect();
        assert!(
            suites.len() > 100,
            "suite registration parser found only {} entries",
            suites.len()
        );

        let failures: Vec<_> = suites
            .into_iter()
            .filter(|suite| *suite != "Services/acme")
            .filter_map(|suite| {
                let key = suite_to_port_key(suite);
                required_port_kind(key).and_then(|kind| {
                    config
                        .require_port(key, kind)
                        .err()
                        .map(|error| format!("{suite} -> {key}: {error}"))
                })
            })
            .collect();
        assert!(
            failures.is_empty(),
            "invalid suite listener mappings:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn k8s_mounts_the_canonical_port_registry() {
        let deployment = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/k8s/kubernetes/test/test-client/deployment.yaml"
        ));
        let deploy_script = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/k8s/scripts/common/deploy_integration.sh"
        ));

        assert!(!deployment.contains("ports.json: |"));
        assert!(deployment.contains("name: test-ports"));
        assert!(deployment.contains("mountPath: /usr/local/edgion/conf"));
        assert!(!deployment.contains("subPath: ports.json"));
        assert!(deploy_script.contains("edgion-tests/integration/conf/ports.json"));
        assert!(deploy_script.contains("create configmap edgion-test-ports"));
        assert!(deploy_script.contains("rollout restart deployment/edgion-test-client"));
    }

    #[test]
    fn body_suites_are_registered_in_resource_and_default_sweeps() {
        let runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_integration.sh"
        ));
        assert!(
            runner.contains(r#"log_warn "${name} test skipped""#),
            "capability skips must use the runner's defined warning helper"
        );
        let starter = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/utils/start_all_with_conf.sh"
        ));
        assert!(
            starter.contains("body-buffering=enabled"),
            "the shared starter must verify the Gateway capability snapshot"
        );
        for suite in ["DslBody", "BodyPlugins", "JsonSchemaValidation"] {
            let registration = format!("EdgionPlugins_{suite}");
            validate_single_runner_registration(runner, &registration).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(
                runner.matches(&format!("EdgionPlugins/{suite}")).count(),
                1,
                "{suite} config must be included in the EdgionPlugins resource-wide load list"
            );
        }
    }

    #[test]
    fn runner_registration_accepts_one_entry_in_each_sweep() {
        let runner = runner_fixture(&["Suite"], &["Suite"]);
        assert!(validate_single_runner_registration(&runner, "Suite").is_ok());
    }

    #[test]
    fn runner_registration_rejects_duplicate_targeted_entries() {
        let runner = runner_fixture(&["Suite", "Suite"], &["Suite"]);
        assert!(validate_single_runner_registration(&runner, "Suite").is_err());
    }

    #[test]
    fn runner_registration_rejects_missing_default_entry() {
        let runner = runner_fixture(&["Suite"], &[]);
        assert!(validate_single_runner_registration(&runner, "Suite").is_err());
    }

    #[test]
    fn runner_registration_rejects_missing_targeted_entry_even_with_duplicate_defaults() {
        let runner = runner_fixture(&[], &["Suite", "Suite"]);
        assert!(validate_single_runner_registration(&runner, "Suite").is_err());
    }

    #[test]
    fn integration_report_covers_isolated_and_final_gates_before_finalization() {
        let runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_integration.sh"
        ));
        assert!(runner.contains(r#"report_test "Isolated_${name}" "PASS""#));
        assert!(runner.contains(r#"report_test "Isolated_${name}" "FAIL""#));
        assert!(runner.contains("Status: ${status}"));
        for phase in [
            r#"REPORT_PHASE="main tests""#,
            r#"REPORT_PHASE="Controller and Gateway reload""#,
            r#"REPORT_PHASE="final metrics validation""#,
            r#"REPORT_PHASE="final unknown-field validation""#,
            r#"REPORT_PHASE="final requeue-loop validation""#,
            r#"REPORT_PHASE="final cache-residue validation""#,
        ] {
            assert!(runner.contains(phase), "runner must track phase {phase}");
        }

        let final_gate = runner
            .rfind("Final Step: Cache Residue Detection")
            .expect("runner must contain the final cache-residue gate");
        let final_pass = runner
            .rfind(r#"finalize_report "PASS""#)
            .expect("runner must finalize a passing report");
        let final_fail = runner
            .rfind(r#"finalize_report "FAIL""#)
            .expect("runner must finalize a failing report");
        let success_log = runner
            .rfind(r#"log_success "All tests passed!""#)
            .expect("runner must emit its final success message");
        assert!(final_pass > final_gate);
        assert!(final_fail > final_gate);
        assert!(final_pass > success_log);

        let finalize_body = runner
            .split_once("finalize_report() {")
            .and_then(|(_, rest)| rest.split_once("\n}"))
            .map(|(body, _)| body)
            .expect("runner must define finalize_report");
        let final_console_output = finalize_body
            .rfind(r#"echo "Report saved to: ${REPORT_FILE}""#)
            .expect("finalize_report must print the report path");
        let finalized_flag = finalize_body
            .rfind("REPORT_FINALIZED=true")
            .expect("finalize_report must set its terminal flag");
        assert!(finalized_flag > final_console_output);
    }

    #[test]
    fn mandatory_final_validation_prerequisites_fail_closed() {
        let runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_integration.sh"
        ));

        assert!(runner.contains("/metrics could not be read"));
        assert!(runner.contains("Unknown-config-fields check failed -- controller admin token unavailable"));
        assert!(runner.contains("controller.log is missing or unreadable"));
        assert!(runner.contains(r#"report_test "Final_RequeueDeadLoop" "FAIL""#));
        assert!(runner.contains(r#"report_test "Final_CacheResidue" "FAIL""#));
        assert!(runner.contains("invalid RESIDUE_MODE=${residue_mode}; expected fail or warn"));
        assert!(!runner.contains(r#"report_test "Final_RequeueDeadLoop" "SKIP""#));
        assert!(runner.contains(r#"report_test "Final_CacheResidue" "SKIP" "0" "explicitly disabled""#));
    }

    #[test]
    fn shell_orchestrators_apply_owned_process_group_deadlines() {
        let main_runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_integration.sh"
        ));
        let aggregate_runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_all_integration.sh"
        ));
        let deadline_runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/utils/run_with_deadline.py"
        ));

        assert!(main_runner.contains("ISOLATED_SCENARIO_TIMEOUT_SECONDS"));
        assert!(main_runner.contains("ISOLATED_PHASE_TIMEOUT_SECONDS"));
        assert!(main_runner.contains(r#"report_test "Isolated_${name}" "TIMEOUT""#));
        assert!(main_runner.contains("Timed out: ${TIMED_OUT_TESTS}"));
        assert!(aggregate_runner.contains("JOB_TIMEOUT_SECONDS"));
        assert!(aggregate_runner.contains("RUN_TIMEOUT_SECONDS"));
        assert!(aggregate_runner.contains(r#"RESULT_STATUS+=("TIMEOUT")"#));
        assert!(aggregate_runner.contains("trap 'forward_termination TERM' TERM"));
        assert!(aggregate_runner.contains("CURRENT_WATCHDOG_PID=$!"));
        assert!(aggregate_runner.contains("BUILD_BLOCKED_STATUS"));
        assert!(deadline_runner.contains("start_new_session=True"));
        assert!(deadline_runner.contains("os.killpg"));
        assert!(deadline_runner.contains("signal.SIGTERM"));
        assert!(deadline_runner.contains("signal.SIGKILL"));
    }

    #[test]
    fn execution_phase_uses_only_precompiled_test_artifacts() {
        let main_runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_integration.sh"
        ));
        let aggregate_runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_all_integration.sh"
        ));
        let acme_runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_acme_test.sh"
        ));
        let prepare = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/utils/prepare.sh"
        ));

        assert!(!main_runner.contains("cargo build"));
        assert!(!main_runner.contains("cargo test"));
        assert!(main_runner.contains("EDGION_CACHE_RESIDUE_BIN"));
        assert!(main_runner.contains("EDGION_CREDENTIAL_RESERVATION_TEST_BIN"));
        assert!(main_runner.contains("EDGION_TOKEN_BUCKET_PARTIAL_REFILL_TEST_BIN"));
        assert!(main_runner.contains("cache residue validator missing"));
        assert!(main_runner.contains("credential reservation test missing"));
        assert!(main_runner.contains("token bucket partial refill test missing"));
        assert!(main_runner.contains(r#"run_test "CredentialReservationRedis" "${test_bin} --ignored --nocapture""#));
        assert!(main_runner.contains(r#"run_test "TokenBucketPartialRefillRedis" "${test_bin} --ignored --nocapture""#));
        assert!(main_runner.contains("run_token_bucket_partial_refill_redis_test || test_failed=true"));
        assert!(!main_runner.contains(r#"\"${test_bin}\" --ignored --nocapture"#));
        let build_phase = main_runner
            .find(r#"REPORT_PHASE="build""#)
            .expect("main runner must have a build phase");
        let external_start = main_runner
            .find(r#"REPORT_PHASE="external dependency startup""#)
            .expect("main runner must have an external dependency phase");
        assert!(build_phase < external_start);
        assert!(aggregate_runner.contains(r#"run_acme_test.sh --no-build"#));
        assert!(aggregate_runner.contains(r#""$PREPARE_SCRIPT""#));
        assert!(!aggregate_runner.contains("exec cargo build"));

        let runtime = acme_runner
            .split_once("# Start Pebble")
            .map(|(_, runtime)| runtime)
            .expect("ACME runner must have an explicit runtime boundary");
        assert!(!runtime.contains("cargo build"));
        assert!(!runtime.contains("cargo run"));
        assert!(runtime.contains(r#""$TEST_CLIENT_BIN" -r Services -i acme"#));

        assert!(prepare.contains("--bin cache_residue"));
        assert!(prepare.contains(r#"--test "$ignored_test""#));
        assert!(prepare.contains("--no-run"));
        assert!(prepare.contains("stable_ignored_test_bin"));
        assert!(prepare.contains(r#"export CARGO_TARGET_DIR="${PROJECT_ROOT}/target""#));
    }

    #[test]
    fn every_edgion_plugins_sweep_item_with_own_fixtures_is_preloaded() {
        let runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_integration.sh"
        ));
        let dispatch_start = runner
            .find(r#"case "$G_RESOURCE" in"#)
            .expect("run_integration.sh must contain the resource dispatch block");
        let dispatch = &runner[dispatch_start..];
        let plugins_start = dispatch
            .find("\n            EdgionPlugins)")
            .expect("resource dispatch must contain EdgionPlugins");
        let plugins = &dispatch[plugins_start..];
        let plugins_end = plugins
            .find("\n            EdgionTls)")
            .expect("EdgionPlugins dispatch must be followed by EdgionTls");
        let plugins = &plugins[..plugins_end];

        let load_start = runner
            .find("# Auto-infer suites from resource/item")
            .expect("runner must contain automatic suite selection");
        let load = &runner[load_start..];
        let load_start = load
            .find(r#"if [ "$G_RESOURCE" = "EdgionPlugins" ]; then"#)
            .expect("automatic suite selection must contain EdgionPlugins");
        let load = &load[load_start..];
        let load_start = load
            .find(r#"if [ -z "$G_ITEM" ]; then"#)
            .expect("EdgionPlugins selection must contain a resource-wide branch");
        let load = &load[load_start..];
        let load_end = load
            .find("\n            else")
            .expect("resource-wide EdgionPlugins selection must have an item-specific branch");
        let load = &load[..load_end];

        let fixture_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("integration/conf/EdgionPlugins");
        let mut checked = 0usize;
        let mut missing = Vec::new();
        for line in plugins
            .lines()
            .filter(|line| line.contains("run_test \"EdgionPlugins_") && line.contains("-r EdgionPlugins -i "))
        {
            let item = line
                .split("-r EdgionPlugins -i ")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .expect("static EdgionPlugins invocation must name an item")
                .trim_end_matches('"');
            if fixture_root.join(item).is_dir() {
                checked += 1;
                let suite = format!("EdgionPlugins/{item}");
                if !load.contains(&suite) {
                    missing.push(suite);
                }
            }
        }
        missing.sort();
        missing.dedup();

        assert!(checked > 30, "expected to check the full EdgionPlugins sweep");
        assert!(
            missing.is_empty(),
            "EdgionPlugins sweep item(s) have fixture directories that the resource-wide run does \
             not preload:\n  {missing:?}\nFix: add each directory to the full EdgionPlugins \
             `suites` list in run_integration.sh."
        );
    }

    /// Every registered suite is actually executed by some runner path.
    ///
    /// This is the outer guard. `every_targeted_sweep_suite_also_runs_in_the_default_sweep`
    /// below only compares the runner's two suite lists against each other, so a
    /// suite missing from **both** falls in its blind spot — which is exactly what
    /// happened: 25 registered suites (including `EdgionPlugins/Wasm` and
    /// `EdgionPlugins/TraceContext`, whole plugins with real conf fixtures) were
    /// never executed by any runner, in either sweep, for an unknown length of time.
    ///
    /// Coverage is proven three ways, in order of preference:
    ///   1. the runner invokes it — resolved through the real `resolve_suite`, so
    ///      alias forms (`-g http-security`, `-g h2-mtls-sp`, ...) count;
    ///   2. a dynamic scenario drives it — matched on the suite id's last segment
    ///      against `dynamic::all()` ids, since `run_dynamic_scenarios` enumerates
    ///      those from the registry rather than from a hardcoded list;
    ///   3. an explicit `KNOWN_EXEMPT` entry with a stated reason.
    #[test]
    fn every_registered_suite_is_executed_by_some_runner_path() {
        // Suites that no runner invokes, by decision rather than oversight.
        // Adding an entry requires a reason; an empty list is the goal state.
        const KNOWN_EXEMPT: &[(&str, &str)] = &[
            ("all", "not a suite — the CLI keyword that expands to every suite"),
            (
                "tcp",
                "protocol shorthand handled by the direct-mode block, not a suite id",
            ),
            (
                "udp",
                "protocol shorthand handled by the direct-mode block, not a suite id",
            ),
            (
                "Services/acme",
                "ACME has its own runner (test_acme.sh); it boots a Pebble CA and is not part of \
                 the shared-Gateway sweep",
            ),
            (
                "HTTPRoute/Protocol",
                "parent grouping id; the concrete Protocol/* children are each invoked",
            ),
            (
                "Status/ObservedGeneration",
                "parent grouping id; the HighFrequency child runs as a dynamic scenario",
            ),
            (
                "HTTPRoute/Protocol/HTTPS",
                "alternate id for HttpsTestSuite, which the runner executes as EdgionTls/https",
            ),
            (
                "GRPCRoute/TLS",
                "alternate id for GrpcTlsTestSuite, which the runner executes as EdgionTls/grpctls",
            ),
            (
                "EdgionBackendTrafficPolicy/upstream_authority",
                "snake_case alias of EdgionBackendTrafficPolicy/UpstreamAuthority, which is invoked",
            ),
            ("EdgionTls", "resource-level id; every EdgionTls/* child is invoked"),
            ("Gateway", "resource-level id; every Gateway/* child is invoked"),
            ("GRPCRoute", "resource-level id; every GRPCRoute/* child is invoked"),
            ("TLSRoute", "resource-level id; every TLSRoute/* child is invoked"),
        ];

        let own_src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/client/test_client.rs"));
        let runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_integration.sh"
        ));

        // Registered suite ids = the string patterns of `add_suites_for_suite`'s match arms.
        // Match the definition at column 0, not the first textual occurrence — this
        // test module sits above the function and mentions its name in a literal.
        let arms_start = own_src
            .find("\nfn add_suites_for_suite")
            .expect("add_suites_for_suite must be defined at column 0");
        let arms = &own_src[arms_start..];
        let arms_end = arms.find("\n}\n").expect("add_suites_for_suite must be terminated");
        let registered: BTreeSet<String> = arms[..arms_end]
            .lines()
            .filter_map(|l| {
                let t = l.trim();
                t.strip_prefix('"')
                    .and_then(|r| r.find('"').map(|e| r[..e].to_string()))
                    .filter(|s| !s.is_empty() && s.contains(|c: char| c.is_ascii_uppercase() || c == '/'))
            })
            .collect();
        assert!(
            registered.len() > 100,
            "expected >100 registered suites, parsed {} — the arm-extraction heuristic is stale",
            registered.len()
        );

        // What the runner actually invokes, canonicalized through resolve_suite.
        let mut invoked: BTreeSet<String> = BTreeSet::new();
        for (i, _) in runner.match_indices("${EDGION_TEST_CLIENT_BIN}") {
            let rest = &runner[i + "${EDGION_TEST_CLIENT_BIN}".len()..];
            let Some(end) = rest.find('"') else { continue };
            let args: Vec<&str> = rest[..end].split_whitespace().collect();
            if args.iter().any(|a| a.contains('$')) && !args.iter().any(|a| *a == "-r") {
                continue; // dynamic `${item_safe}` invocation, nothing static to resolve
            }
            let val = |flag: &str| -> Option<String> {
                args.iter()
                    .position(|a| *a == flag)
                    .and_then(|p| args.get(p + 1))
                    .filter(|v| !v.starts_with('-') && !v.contains('$'))
                    .map(|v| v.to_string())
            };
            let (r, it) = (val("-r"), val("-i"));
            // `resolve_suite` short-circuits on `legacy`, so it may only be read
            // when there is no `-r` — otherwise the first bare token is `-r`'s own
            // value and every invocation resolves to the resource name.
            let legacy = if r.is_some() {
                None
            } else {
                args.iter()
                    .find(|a| !a.starts_with('-') && !a.contains('$'))
                    .map(|s| s.to_string())
            };
            if r.is_none() && legacy.is_none() {
                continue;
            }
            invoked.insert(resolve_suite(r.as_deref(), it.as_deref(), legacy.as_deref()));
        }

        // A suite counts as dynamic-covered when a registered scenario drives it.
        // Matched exactly, two ways, rather than by prefix — a prefix rule would
        // let a parent id inherit a child scenario's coverage:
        //   `conf_path == "<suite>/DynamicTest"`  (the usual layout), or
        //   `conf_path == "<suite>Test"`          (Gateway/Dynamic -> Gateway/DynamicTest).
        // The scenario id itself is also accepted when it equals the suite's last
        // segment (EdgionPlugins/BasicAuthDynamic -> "BasicAuthDynamic").
        let scenario_ids: BTreeSet<&str> = dynamic::all().iter().map(|s| s.id).collect();
        let conf_paths: BTreeSet<&str> = dynamic::all().iter().map(|s| s.conf_path).collect();
        let dynamic_covers = |suite: &str| -> bool {
            scenario_ids.contains(suite.rsplit('/').next().unwrap_or(suite))
                || conf_paths.contains(format!("{suite}/DynamicTest").as_str())
                || conf_paths.contains(format!("{suite}Test").as_str())
        };

        let mut missing: Vec<String> = registered
            .iter()
            .filter(|s| !invoked.contains(*s))
            .filter(|s| !dynamic_covers(s))
            .filter(|s| !KNOWN_EXEMPT.iter().any(|(k, _)| k == s))
            .cloned()
            .collect();
        missing.sort();

        assert!(
            missing.is_empty(),
            "registered test suite(s) that NO runner path executes — they compile, they have conf \
             fixtures, and they never run:\n  {missing:?}\n\
             Fix: add a `run_test` line to run_integration.sh (both the `-r <Resource>` dispatch \
             and the default sweep), or add a KNOWN_EXEMPT entry stating why it cannot run."
        );
    }

    /// Every suite reachable via `-r <Resource>` must also run in the default sweep.
    ///
    /// `run_integration.sh` carries the suite list twice: once inside the
    /// `case "$G_RESOURCE" in` block that serves `-r <Resource>`, and once in the
    /// default full run. `cicd/coverage.sh` invokes the script with no arguments,
    /// i.e. the default list — so a suite present only in the targeted block is
    /// **never executed by CI**, and the failure is silent because the report
    /// only counts tests that ran.
    ///
    /// This has already happened twice. Commit `82fcfda3` (2026-06-24) created
    /// the duplication and added suites to the targeted block only; six suites
    /// went unexecuted for up to five weeks (`EdgionPlugins_Cors`,
    /// `HTTPRoute_Protocol_{H2C,GrpcWeb,H2TrailerMalformed}`,
    /// `GRPCRoute_Delegation`, `TCPRoute_GlobalConnectionIpRestriction`). The
    /// hardcoded three-suite check above was the first patch for one instance of
    /// this; this test generalizes it to the whole list.
    ///
    /// A targeted entry may be satisfied by finer-grained default entries — the
    /// targeted block runs bundles like `HTTPRoute_Backend` where the default
    /// sweep runs `HTTPRoute_Backend_WeightedBackend` and friends — so an
    /// `EXACT` or `EXACT_` prefix match both count as covered.
    #[test]
    fn every_targeted_sweep_suite_also_runs_in_the_default_sweep() {
        let runner = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/scripts/integration/run_integration.sh"
        ));

        let (targeted, default_sweep) =
            runner_suite_sweeps(runner).expect("run_integration.sh must define both suite sweeps");

        // Collect `run_test "<name>"` names from a region.
        let names = |region: &str| -> Vec<String> {
            region
                .match_indices("run_test \"")
                .filter_map(|(i, _)| {
                    let after = &region[i + "run_test \"".len()..];
                    after.find('"').map(|end| after[..end].to_string())
                })
                .collect()
        };

        // Suites deliberately reachable only via `-r <Resource>`. Each entry is a
        // KNOWN COVERAGE GAP, recorded rather than silently tolerated; adding the
        // suite to the default sweep and deleting the entry is always the fix.
        // Empty is the goal state — keep it that way.
        const KNOWN_TARGETED_ONLY: &[(&str, &str)] = &[];

        let targeted_names = names(targeted);
        assert!(
            targeted_names.len() > 50,
            "expected the targeted block to hold the bulk of the suite list, found {} — \
             the region markers in this test are probably out of date",
            targeted_names.len()
        );

        let mut missing: Vec<String> = targeted_names
            .iter()
            // `run_test "Foo_${item_safe}"` is the dynamic `-i <Item>` path, not a
            // static suite name; there is nothing for the default sweep to match.
            .filter(|n| !n.contains('$'))
            .filter(|n| !KNOWN_TARGETED_ONLY.iter().any(|(known, _)| *known == n.as_str()))
            .filter(|n| {
                !default_sweep.contains(&format!("run_test \"{n}\""))
                    && !default_sweep.contains(&format!("run_test \"{n}_"))
            })
            .cloned()
            .collect();
        missing.sort();
        missing.dedup();

        assert!(
            missing.is_empty(),
            "suite(s) runnable via `-r <Resource>` but absent from the default sweep, so \
             `cicd/coverage.sh` never executes them:\n  {missing:?}\n\
             Fix: add a matching `run_test` line to the default full-run block of \
             run_integration.sh (the section after the `case \"$G_RESOURCE\"` dispatch)."
        );
    }
}

#[derive(Parser, Debug)]
#[command(name = "test-client")]
#[command(about = "Edgion ")]
struct Cli {
    /// Resource type (HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLS, Security, Plugins)
    #[arg(short = 'r', long = "resource")]
    resource: Option<String>,

    /// sub-item (Match, Backend, Filters, Protocol )
    #[arg(short = 'i', long = "item")]
    item: Option<String>,

    /// Enable gateway mode (tests target the running Edgion gateway).
    #[arg(short = 'g', long = "gateway")]
    gateway: bool,

    #[arg(long, default_value = "127.0.0.1")]
    target_host: String,

    #[arg(long, default_value = "30001")]
    http_port: u16,

    #[arg(long, default_value = "30021")]
    grpc_port: u16,

    #[arg(long, default_value = "30005")]
    websocket_port: u16,

    #[arg(long, default_value = "30010")]
    tcp_port: u16,

    #[arg(long, default_value = "30011")]
    udp_port: u16,

    #[arg(long, default_value = "10443")]
    https_port: u16,

    #[arg(long, default_value = "18443")]
    grpc_https_port: u16,

    #[arg(long, default_value = "12101")]
    admin_port: u16,

    #[arg(long)]
    json: bool,

    #[arg(long, default_value = "test_report.json")]
    json_output: String,

    #[arg(short, long)]
    verbose: bool,

    /// Include slow, wall-clock integration cases in suites that define them.
    #[arg(long)]
    full_test: bool,

    /// Test phase for dynamic tests (initial or update)
    #[arg(long)]
    phase: Option<String>,

    /// List all registered dynamic scenario ids (one per line) and exit.
    #[arg(long = "list-dynamic-scenarios")]
    list_dynamic_scenarios: bool,

    /// Run a single dynamic scenario by id.
    #[arg(long = "dynamic-scenario")]
    dynamic_scenario: Option<String>,

    /// Legacy positional command (use -r/-i instead).
    #[arg(value_name = "COMMAND")]
    legacy_command: Option<String>,
}

/// Resolve the test suite name from resource/item flags or a legacy command.
fn resolve_suite(resource: Option<&str>, item: Option<&str>, legacy: Option<&str>) -> String {
    if let Some(cmd) = legacy {
        return match cmd.to_lowercase().as_str() {
            "http" => "HTTPRoute/Basic".to_string(),
            "http-match" | "httpmatch" => "HTTPRoute/Match".to_string(),
            "http-redirect" | "httpredirect" => "HTTPRoute/Filters/Redirect".to_string(),
            "http-security" | "httpsecurity" => "HTTPRoute/Filters/Security".to_string(),
            "https" => "EdgionTls/https".to_string(),
            "websocket" => "HTTPRoute/Protocol/WebSocket".to_string(),
            "h2c" => "HTTPRoute/Protocol/H2C".to_string(),
            "h2-trailer-malformed" | "h2trailermalformed" | "h2tm" => {
                "HTTPRoute/Protocol/H2TrailerMalformed".to_string()
            }
            "transport" | "http-transport" => "HTTPRoute/Transport".to_string(),
            "lb-rr" | "lbrr" | "lb-roundrobin" => "HTTPRoute/Backend/LBRoundRobin".to_string(),
            "lb-ch" | "lbch" | "lb-consistenthash" => "HTTPRoute/Backend/LBConsistentHash".to_string(),
            "lb-lc" | "lblc" | "lb-leastconn" => "HTTPRoute/Backend/LBLeastConn".to_string(),
            "lb-lc-rs" | "lblcrs" | "lb-leastconn-retry-on-status" => {
                "HTTPRoute/Backend/LBLeastConnRetryOnStatus".to_string()
            }
            "lb-ewma" | "lbewma" | "lb-ewma-test" => "HTTPRoute/Backend/LBEwma".to_string(),
            "lb-ewma-rs" | "lbewmars" | "lb-ewma-retry-on-status" => {
                "HTTPRoute/Backend/LBEwmaRetryOnStatus".to_string()
            }
            "weighted-backend" | "weightedbackend" => "HTTPRoute/Backend/WeightedBackend".to_string(),
            "timeout" => "HTTPRoute/Backend/Timeout".to_string(),
            "retry-on-status" | "retryonstatus" => "HTTPRoute/Backend/RetryOnStatus".to_string(),
            "per-try-timeout" | "pertrytimeout" => "HTTPRoute/Backend/PerTryTimeout".to_string(),
            "circuit-breaker" | "circuitbreaker" => "HTTPRoute/Backend/CircuitBreaker".to_string(),
            "health-check" | "healthcheck" => "HTTPRoute/Backend/HealthCheck".to_string(),
            "grpc-health-check" | "grpchealthcheck" => "HTTPRoute/Backend/GrpcHealthCheck".to_string(),
            "health-check-transition" | "healthcheck-transition" => {
                "HTTPRoute/Backend/HealthCheckTransition".to_string()
            }
            "session-persistence" | "sp" => "HTTPRoute/Backend/SessionPersistence".to_string(),
            "h2-mtls-sp" | "h2_mtls_sp" | "h2mtlssp" => "HTTPRoute/Backend/H2BackendMtlsPersistence".to_string(),
            "backend-tls-sni-host-override" | "backend-tls-sni" | "btls-sni" => {
                "HTTPRoute/Backend/BackendTLSSNIHostOverride".to_string()
            }
            "grpc" => "GRPCRoute/Basic".to_string(),
            "grpc-match" | "grpcmatch" => "GRPCRoute/Match".to_string(),
            "grpc-rate-limit" | "grpcratelimit" => "GRPCRoute/RateLimit".to_string(),
            "grpc-sp" | "grpcsp" => "GRPCRoute/Backend/SessionPersistence".to_string(),
            "grpc-tls" | "grpctls" => "EdgionTls/grpctls".to_string(),
            "tcp" => "TCPRoute/Basic".to_string(),
            "tcp-keepalive-smoke" | "tcpkeepalivesmoke" => "TCPRoute/KeepaliveSmoke".to_string(),
            "tcp-denied-backend-ref" | "tcpdeniedbackendref" => "TCPRoute/DeniedBackendRef".to_string(),
            "udp" => "UDPRoute/Basic".to_string(),
            "udp-denied-backend-ref" | "udpdeniedbackendref" => "UDPRoute/DeniedBackendRef".to_string(),
            "mtls" => "EdgionTls/mTLS".to_string(),
            "security" => "Gateway/Security".to_string(),
            "stream-plugins" | "streamplugins" | "connection-filter" => "Gateway/StreamPlugins".to_string(),
            "tcp-stream-plugins" | "tcpstreamplugins" => "TCPRoute/StreamPlugins".to_string(),
            "udp-stream-plugins" | "udpstreamplugins" => "UDPRoute/StreamPlugins".to_string(),
            "tcp-gcir" | "tcpgcir" | "tcp-global-connection-ip-restriction" => {
                "TCPRoute/GlobalConnectionIpRestriction".to_string()
            }
            "tls" | "tls-route" | "tlsroute" => "TLSRoute/Basic".to_string(),
            "tls-pp2" | "tlspp2" | "tls-proxy-protocol" => "TLSRoute/ProxyProtocol".to_string(),
            "tls-stream-plugins" | "tlsstreamplugins" => "TLSRoute/StreamPlugins".to_string(),
            "tls-multi-sni" | "tlsmultisni" | "multi-sni" => "TLSRoute/MultiSNI".to_string(),
            "tls-passthrough" | "tlspassthrough" => "TLSRoute/Passthrough".to_string(),
            "tls-unknown-kind-backend" | "tlsunknownkindbackend" => "TLSRoute/UnknownKindBackend".to_string(),
            "tls-denied-backend-ref" | "tlsdeniedbackendref" => "TLSRoute/DeniedBackendRef".to_string(),
            "real-ip" | "realip" => "Gateway/RealIP".to_string(),
            "backend-tls" | "backendtls" => "Gateway/TLS/BackendTLS".to_string(),
            "backend-tls-san" | "backendtlssan" => "Gateway/TLS/BackendTLSSAN".to_string(),
            "backend-tls-gateway-cert" | "backendtlsgatewaycert" => "Gateway/TLS/BackendTLSGatewayCert".to_string(),
            "misdirected-request" | "misdirectedrequest" | "421" => "Gateway/TLS/MisdirectedRequest".to_string(),
            "frontend-validation" | "frontendvalidation" | "fv-mtls" => "Gateway/FrontendValidation".to_string(),
            "csrf" => "EdgionPlugins/Csrf".to_string(),
            "credential-injector" | "credentialinjector" => "EdgionPlugins/CredentialInjector".to_string(),
            "plugin-logs" | "pluginlogs" => "EdgionPlugins/DebugAccessLog".to_string(),
            "plugin-condition" | "plugincondition" => "EdgionPlugins/PluginCondition".to_string(),
            "all-conditions" | "allconditions" => "EdgionPlugins/PluginCondition/AllConditions".to_string(),
            "jwt-auth" | "jwtauth" => "EdgionPlugins/JwtAuth".to_string(),
            "jwe-decrypt" | "jwedecrypt" => "EdgionPlugins/JweDecrypt".to_string(),
            "key-auth" | "keyauth" => "EdgionPlugins/KeyAuth".to_string(),
            "hmac-auth" | "hmacauth" => "EdgionPlugins/HmacAuth".to_string(),
            "header-cert-auth" | "headercertauth" => "EdgionPlugins/HeaderCertAuth".to_string(),
            "ldap-auth" | "ldapauth" => "EdgionPlugins/LdapAuth".to_string(),
            "ldap-auth-tls" | "ldapauthtls" => "EdgionPlugins/LdapAuthTls".to_string(),
            "forward-auth" | "forwardauth" => "EdgionPlugins/ForwardAuth".to_string(),
            "ext-proc" | "extproc" => "EdgionPlugins/ExtProc".to_string(),
            "ai-guard" | "aiguard" => "EdgionPlugins/AiGuard".to_string(),
            "guardrail" => "EdgionPlugins/Guardrail".to_string(),
            "openid-connect" | "openidconnect" | "oidc" => "EdgionPlugins/OpenidConnect".to_string(),
            "webhook-keyget" | "webhookkeyget" => "EdgionPlugins/WebhookKeyGet".to_string(),
            "rate-limit-redis" | "ratelimitredis" | "rlr" => "EdgionPlugins/RateLimitRedis".to_string(),
            "dsl" => "EdgionPlugins/Dsl".to_string(),
            "dsl-body" | "dslbody" => "EdgionPlugins/DslBody".to_string(),
            "body-plugins" | "bodyplugins" => "EdgionPlugins/BodyPlugins".to_string(),
            "json-schema-validation" | "jsonschemavalidation" => "EdgionPlugins/JsonSchemaValidation".to_string(),
            "EdgionConfigDataManaged" | "edgionconfigdatamanaged" | "data-managed" => {
                "EdgionPlugins/EdgionConfigDataManaged".to_string()
            }
            "global-access-control" | "globalaccesscontrol" | "gac" => "EdgionPlugins/RequestAccessPolicy".to_string(),
            "global-connection-ip-restriction" | "globalconnectioniprestriction" | "gcir" => {
                "EdgionPlugins/GlobalConnectionIpRestriction".to_string()
            }
            "global-plugins-snapshot" | "globalpluginssnapshot" | "gp-snapshot" => {
                "EdgionPlugins/GlobalPluginsSnapshot".to_string()
            }
            "resolved-refs" | "resolvedrefs" | "ep-resolved-refs" => "EdgionPlugins/ResolvedRefs".to_string(),
            "dangling-extension-ref" | "danglingextensionref" => "HTTPRoute/DanglingExtensionRef".to_string(),
            "linksys-status" | "linksysstatus" | "ls-status" => "LinkSys/Status".to_string(),
            "ref-grant-status" => "Status/RefGrant".to_string(),
            "hostname-intersection-status" => "Status/HostnameIntersection".to_string(),
            "backend-tls-status" | "btls-status" => "Status/BackendTLSPolicy".to_string(),
            "gateway-frontend-validation-status" | "gwfv-status" => "Status/GatewayFrontendValidation".to_string(),
            "grpc-route-status" | "grpcroutestatus" => "Status/GRPCRoute".to_string(),
            "tls-route-status" | "tlsroutestatus" => "Status/TLSRoute".to_string(),
            "observed-generation-status" | "observedgeneration" => "Status/ObservedGeneration".to_string(),
            _ => cmd.to_string(),
        };
    }

    //  -r/-i
    match (resource, item) {
        (Some(r), Some(i)) => format!("{}/{}", r, i),
        (Some(r), None) => r.to_string(),
        (None, Some(i)) => format!("HTTPRoute/{}", i), //  HTTPRoute
        (None, None) => "all".to_string(),
    }
}

/// Return the port-config key for the given suite name.
fn suite_to_port_key(suite: &str) -> &str {
    match suite {
        "all" => "HTTPRoute/Basic",
        "EdgionBackend/AnthropicProvider" => "EdgionBackend/AnthropicProvider",
        "EdgionBackend/CredentialExhaustion" => "EdgionBackend/CredentialExhaustion",
        "EdgionBackend/Resilience" => "EdgionBackend/Resilience",
        "EdgionBackend/SingleProvider" => "EdgionBackend/SingleProvider",
        // Control-plane suite: no data-plane listener of its own; borrow a valid
        // HTTP port key so the per-suite context still resolves a port.
        "EdgionBackendTrafficPolicy" => "HTTPRoute/Backend/CircuitBreaker",
        // Data-plane suite: dynamic upstream authority (Host/SNI dispatch). Has
        // its own Gateway/HTTPRoute fixture and its own ports.json entry, so
        // (unlike the control-plane suite above) it does not borrow another
        // suite's port. Accept both the brief's literal `-i upstream_authority`
        // and the repo's usual CamelCase item spelling.
        "EdgionBackendTrafficPolicy/upstream_authority" | "EdgionBackendTrafficPolicy/UpstreamAuthority" => {
            "EdgionBackendTrafficPolicy/UpstreamAuthority"
        }
        // Data-plane suite: per-backend retry budget (retryConstraint). Own
        // Gateway/HTTPRoute/EBTP fixture and ports.json entry.
        "EdgionBackendTrafficPolicy/RetryBudget" => "EdgionBackendTrafficPolicy/RetryBudget",
        // Data-plane suite: per-backend connect-timeout override. Own
        // Gateway/HTTPRoute/EBTP fixture and ports.json entry.
        "EdgionBackendTrafficPolicy/ConnectTimeout" => "EdgionBackendTrafficPolicy/ConnectTimeout",
        // HTTPRoute
        "HTTPRoute/Basic" | "HTTPRoute" => "HTTPRoute/Basic",
        "HTTPRoute/SseStreaming" => "HTTPRoute/SseStreaming",
        "HTTPRoute/Delegation" => "HTTPRoute/Delegation",
        "HTTPRoute/Match" => "HTTPRoute/Match",
        "HTTPRoute/Backend" | "HTTPRoute/Backend/LBRoundRobin" => "HTTPRoute/Backend/LBRoundRobin",
        "HTTPRoute/Backend/LBConsistentHash" => "HTTPRoute/Backend/LBConsistentHash",
        "HTTPRoute/Backend/LBLeastConn" => "HTTPRoute/Backend/LBLeastConn",
        "HTTPRoute/Backend/LBLeastConnRetryOnStatus" => "HTTPRoute/Backend/LBLeastConnRetryOnStatus",
        "HTTPRoute/Backend/LBEwma" => "HTTPRoute/Backend/LBEwma",
        "HTTPRoute/Backend/LBEwmaRetryOnStatus" => "HTTPRoute/Backend/LBEwmaRetryOnStatus",
        "HTTPRoute/Backend/WeightedBackend" => "HTTPRoute/Backend/WeightedBackend",
        "HTTPRoute/Backend/Timeout" => "HTTPRoute/Backend/Timeout",
        "HTTPRoute/Backend/RetryOnStatus" => "HTTPRoute/Backend/RetryOnStatus",
        "HTTPRoute/Backend/PerTryTimeout" => "HTTPRoute/Backend/PerTryTimeout",
        "HTTPRoute/Backend/CircuitBreaker" => "HTTPRoute/Backend/CircuitBreaker",
        "HTTPRoute/Backend/HealthCheck" => "HTTPRoute/Backend/HealthCheck",
        "HTTPRoute/Backend/GrpcHealthCheck" => "HTTPRoute/Backend/GrpcHealthCheck",
        "HTTPRoute/Backend/HealthCheckTransition" => "HTTPRoute/Backend/HealthCheckTransition",
        "HTTPRoute/Backend/DnsResolutionFailure" => "HTTPRoute/Backend/DnsResolutionFailure",
        "HTTPRoute/Backend/DnsResolution" => "HTTPRoute/Backend/DnsResolution",
        "HTTPRoute/Backend/PartiallyInvalid" => "HTTPRoute/Backend/PartiallyInvalid",
        "HTTPRoute/Backend/SessionPersistence" => "HTTPRoute/Backend/SessionPersistence",
        "HTTPRoute/Backend/SessionPersistenceBackendChange" => "HTTPRoute/Backend/SessionPersistenceBackendChange",
        "HTTPRoute/Backend/SessionPersistenceWeighted" => "HTTPRoute/Backend/SessionPersistenceWeighted",
        "HTTPRoute/Backend/H2BackendMtlsPersistence" => "HTTPRoute/Backend/H2BackendMtlsPersistence",
        "HTTPRoute/Filters/FilterConflict" => "HTTPRoute/Filters/FilterConflict",
        "HTTPRoute/Filters/PercentMirror" => "HTTPRoute/Filters/PercentMirror",
        "HTTPRoute/Filters" | "HTTPRoute/Filters/Redirect" => "HTTPRoute/Filters/Redirect",
        "HTTPRoute/Filters/Security" => "HTTPRoute/Filters/Security",
        "HTTPRoute/Protocol" => "HTTPRoute/Protocol/WebSocket",
        "HTTPRoute/Protocol/WebSocket" => "HTTPRoute/Protocol/WebSocket",
        "HTTPRoute/Protocol/H2C" => "HTTPRoute/Protocol/H2C",
        "HTTPRoute/Protocol/GrpcWeb" => "HTTPRoute/Protocol/GrpcWeb",
        "HTTPRoute/Protocol/H2TrailerMalformed" => "HTTPRoute/Protocol/H2TrailerMalformed",
        "HTTPRoute/Transport" => "HTTPRoute/Transport",
        "HTTPRoute/MultiPort" => "HTTPRoute/MultiPort",
        // DanglingExtensionRef is parented to edgion-plugins-gateway (port 31180),
        // shared with EdgionPlugins ExtensionRef-based scenarios.
        "HTTPRoute/DanglingExtensionRef" => "EdgionPlugins",
        // GRPCRoute
        "GRPCRoute/Basic" | "GRPCRoute" => "GRPCRoute/Basic",
        "GRPCRoute/Delegation" => "GRPCRoute/Delegation",
        "GRPCRoute/Match" => "GRPCRoute/Match",
        "GRPCRoute/UnmatchedRoute" => "GRPCRoute/UnmatchedRoute",
        "GRPCRoute/RateLimit" => "GRPCRoute/RateLimit",
        "GRPCRoute/TLS" => "EdgionTls/grpctls",
        "GRPCRoute/Backend/SessionPersistence" => "GRPCRoute/Backend/SessionPersistence",
        "GRPCRoute/Backend/SessionPersistenceWeighted" => "GRPCRoute/Backend/SessionPersistenceWeighted",
        // TCPRoute
        "TCPRoute/Basic" | "TCPRoute" => "TCPRoute/Basic",
        "TCPRoute/IdleTimeout" => "TCPRoute/Basic",
        "TCPRoute/DuplexBackpressure" => "TCPRoute/DuplexBackpressure",
        "TCPRoute/KeepaliveSmoke" => "TCPRoute/Basic",
        "TCPRoute/StreamPlugins" => "TCPRoute/StreamPlugins",
        "TCPRoute/GlobalConnectionIpRestriction" => "TCPRoute/GlobalConnectionIpRestriction",
        "TCPRoute/DeniedBackendRef" => "TCPRoute/DeniedBackendRef",
        // TLSRoute
        "TLSRoute/Basic" | "TLSRoute" => "TLSRoute/Basic",
        "TLSRoute/ProxyProtocol" => "TLSRoute/ProxyProtocol",
        "TLSRoute/StreamPlugins" => "TLSRoute/StreamPlugins",
        "TLSRoute/MultiSNI" => "TLSRoute/MultiSNI",
        "TLSRoute/BothAbsentParentRef" => "TLSRoute/BothAbsentParentRef",
        "TLSRoute/Passthrough" => "TLSRoute/Passthrough",
        "TLSRoute/UnknownKindBackend" => "TLSRoute/UnknownKindBackend",
        "TLSRoute/DeniedBackendRef" => "TLSRoute/DeniedBackendRef",
        // UDPRoute
        "UDPRoute/Basic" | "UDPRoute" => "UDPRoute/Basic",
        "UDPRoute/StreamPlugins" => "UDPRoute/StreamPlugins",
        "UDPRoute/DeniedBackendRef" => "UDPRoute/DeniedBackendRef",
        // Gateway
        "Gateway/Security" | "Gateway" => "Gateway/Security",
        "Gateway/RealIP" => "Gateway/RealIP",
        "Gateway/TLS/BackendTLS" => "Gateway/TLS/BackendTLS",
        "Gateway/TLS/BackendTLSSAN" => "Gateway/TLS/BackendTLSSAN",
        "Gateway/TLS/BackendTLSGatewayCert" => "Gateway/TLS/BackendTLSGatewayCert",
        "Gateway/TLS/GatewayTLS" => "Gateway/TLS/GatewayTLS",
        "Gateway/TLS/MisdirectedRequest" => "Gateway/TLS/MisdirectedRequest",
        "Gateway/TLS/NoHostnameListener" => "Gateway/TLS/NoHostnameListener",
        "Gateway/FrontendValidation" => "Gateway/FrontendValidation",
        "EdgionPlugins/DebugAccessLog" => "EdgionPlugins",
        "EdgionPlugins/PluginCondition" => "EdgionPlugins",
        "EdgionPlugins/PluginCondition/AllConditions" => "EdgionPlugins",
        "EdgionPlugins/Cors" => "EdgionPlugins",
        "EdgionPlugins/Csrf" => "EdgionPlugins",
        "EdgionPlugins/CredentialInjector" => "EdgionPlugins",
        "EdgionPlugins/CtxSet" => "EdgionPlugins",
        "EdgionPlugins/BasicAuth" => "EdgionPlugins",
        "EdgionPlugins/BasicAuthDynamic" => "EdgionPlugins",
        "EdgionPlugins/BasicAuthDynamicGrouped" => "EdgionPlugins",
        "EdgionPlugins/JwtAuth" => "EdgionPlugins",
        "EdgionPlugins/JweDecrypt" => "EdgionPlugins",
        "EdgionPlugins/KeyAuth" => "EdgionPlugins",
        "EdgionPlugins/HmacAuth" => "EdgionPlugins",
        "EdgionPlugins/HeaderCertAuth" => "EdgionPlugins",
        "EdgionPlugins/LdapAuth" => "EdgionPlugins",
        "EdgionPlugins/LdapAuthTls" => "EdgionPlugins",
        "EdgionPlugins/ProxyRewrite" => "EdgionPlugins",
        "EdgionPlugins/RateLimit" => "EdgionPlugins",
        "EdgionPlugins/RateLimitDynamic" => "EdgionPlugins",
        "EdgionPlugins/RateLimitRedis" => "EdgionPlugins",
        "EdgionPlugins/BandwidthLimit" => "EdgionPlugins",
        "EdgionPlugins/RealIp" => "EdgionPlugins",
        "EdgionPlugins/TraceContext" => "EdgionPlugins",
        "EdgionPlugins/Wasm" => "EdgionPlugins",
        "EdgionPlugins/RequestMirror" => "EdgionPlugins",
        "EdgionPlugins/RequestRestriction" => "EdgionPlugins",
        "EdgionPlugins/IpRestriction" => "EdgionPlugins",
        "EdgionPlugins/IpRestrictionRefs" => "EdgionPlugins",
        "EdgionPlugins/IpRestrictionPartialGood" => "EdgionPlugins",
        "EdgionPlugins/ResponseRewrite" => "EdgionPlugins",
        "EdgionPlugins/FaultInjection" => "EdgionPlugins",
        "EdgionPlugins/ForwardAuth" => "EdgionPlugins",
        "EdgionPlugins/ExtProc" => "EdgionPlugins",
        "EdgionPlugins/AiGuard" => "EdgionBackend/SingleProvider",
        "EdgionPlugins/Guardrail" => "EdgionBackend/SingleProvider",
        "EdgionPlugins/DirectEndpoint" => "EdgionPlugins",
        "EdgionPlugins/DynamicInternalUpstream" => "EdgionPlugins",
        "EdgionPlugins/DynamicExternalUpstream" => "EdgionPlugins",
        "EdgionPlugins/RegionRoute" => "EdgionPlugins",
        "EdgionPlugins/EdgionConfigDataManaged" => "EdgionPlugins",
        "EdgionPlugins/RequestAccessPolicy" => "EdgionPlugins",
        "EdgionPlugins/Canary" => "EdgionPlugins",
        "EdgionPlugins/GlobalConnectionIpRestriction" => "EdgionPlugins",
        "EdgionPlugins/GlobalPluginsSnapshot" => "EdgionPlugins",
        "EdgionPlugins/EdgionConfigDataStatus" => "EdgionPlugins",
        "EdgionPlugins/EdgionConfigDataStatus/RefFlap" => "EdgionPlugins",
        "EdgionPlugins/PluginValidationStatus" => "EdgionPlugins",
        "EdgionPlugins/ResolvedRefs" => "EdgionPlugins",
        "EdgionPlugins/AllEndpointStatus" => "EdgionPlugins",
        "LinkSys/Status" => "EdgionPlugins",
        "EdgionPlugins/OpenidConnect" => "EdgionPlugins",
        "EdgionPlugins/PathNormalization" => "EdgionPlugins",
        "EdgionPlugins/PathNormalization/PreserveParam" => "EdgionPlugins",
        "EdgionPlugins/PathNormalization/ForwardView" => "EdgionPlugins",
        "EdgionPlugins/MatchedRouteMeta" => "EdgionPlugins",
        "EdgionPlugins/AccessLogExtern" => "EdgionPlugins/AccessLogExtern",
        "EdgionPlugins/WebhookKeyGet" => "EdgionPlugins",
        "EdgionPlugins/Dsl" => "EdgionPlugins",
        "EdgionPlugins/DslBody" => "EdgionPlugins",
        "EdgionPlugins/BodyPlugins" => "EdgionPlugins",
        "EdgionPlugins/JsonSchemaValidation" => "EdgionPlugins",
        "EdgionPlugins/BodyDynamic" => "EdgionPlugins",
        "EdgionPlugins/DslDynamic" => "EdgionPlugins",
        "EdgionPlugins/Waf" => "EdgionPlugins",
        "EdgionPlugins/DslGrpcError" => "GRPCRoute/Basic",
        "Gateway/AllowedRoutes/Selector" => "Gateway/AllowedRoutes/Selector",
        "HTTPRoute/Protocol/HTTPS" => "EdgionTls/https",
        "Status/RefGrant/TCPRouteCrossNsSvc" => "Status/RefGrant/TCPRouteCrossNsSvc",
        "Status/RefGrant/TLSRouteCrossNsSvc" => "Status/RefGrant/TLSRouteCrossNsSvc",
        "Status/RefGrant/EdgionConfigData" => "Status/RefGrant/EdgionConfigData",
        "Status/ObservedGeneration/HighFrequency" => "Status/ObservedGeneration/HighFrequency",
        "Status/BackendTLSPolicy/RefFlap" => "Status/BackendTLSPolicy/RefFlap",
        "Gateway/StreamPlugins" => "Gateway/StreamPlugins",
        "Gateway/PortConflict" => "Gateway/PortConflict",
        // EdgionTls
        "EdgionTls" | "EdgionTls/https" => "EdgionTls/https",
        "EdgionTls/grpctls" => "EdgionTls/grpctls",
        "EdgionTls/mTLS" => "EdgionTls/mTLS",
        "EdgionTls/cipher" => "EdgionTls/cipher",
        "EdgionTls/mtls_failclose" => "EdgionTls/mtls_failclose",
        "EdgionTls/fallback_sni" => "EdgionTls/fallback_sni",
        "EdgionTls/port_only" => "EdgionTls/port_only",
        "EdgionTls/BothAbsentParentRef" => "EdgionTls/BothAbsentParentRef",
        "EdgionTls/wildcard_single_label" => "EdgionTls/wildcard_single_label",
        _ => suite,
    }
}

fn required_port_kind(port_key: &str) -> Option<port_config::PortKind> {
    use port_config::PortKind;

    if port_key.starts_with("Status/") || port_key == "Gateway/PortConflict" {
        None
    } else if port_key.starts_with("GRPCRoute/") {
        Some(PortKind::Grpc)
    } else if port_key.starts_with("TCPRoute/") {
        Some(PortKind::Tcp)
    } else if port_key.starts_with("TLSRoute/") {
        Some(PortKind::Tls)
    } else if port_key.starts_with("UDPRoute/") {
        Some(PortKind::Udp)
    } else if port_key == "EdgionTls/grpctls" {
        Some(PortKind::GrpcTls)
    } else if port_key.starts_with("EdgionTls/")
        || matches!(
            port_key,
            "Gateway/TLS/GatewayTLS"
                | "Gateway/TLS/MisdirectedRequest"
                | "Gateway/TLS/NoHostnameListener"
                | "Gateway/FrontendValidation"
        )
    {
        Some(PortKind::Https)
    } else {
        Some(PortKind::Http)
    }
}

fn dynamic_port_contract(id: &str) -> Result<Option<(&'static str, port_config::PortKind)>> {
    use port_config::PortKind;

    let contract = match id {
        "AiBackendLifecycle" | "AiBackendPolicyIntegration" | "AiBackendSecretRotation" | "AiBackendWeighted" => {
            Some(("EdgionBackend/SingleProvider", PortKind::Http))
        }
        "AllEndpointStatusTls"
        | "RequestMirrorReferenceGrant"
        | "DirectEndpointPortResolution"
        | "BasicAuthDynamic"
        | "BasicAuthDynamicGrouped"
        | "BodyDynamic"
        | "CanaryHotSwap"
        | "ConditionRefsDynamic"
        | "CredentialInjectorDynamic"
        | "CsrfSecretRotation"
        | "DslDynamic"
        | "FormJsonTransformDynamic"
        | "JsonSchemaValidationDynamic"
        | "PmdRefFlap"
        | "RegionRouteValidation"
        | "RateLimitDynamic"
        | "RequestAccessPolicyHmacDynamic" => Some(("EdgionPlugins", PortKind::Http)),
        "WafDynamic" => Some(("EdgionPlugins", PortKind::Http)),
        "ServiceDegradeThreshold" => Some(("HTTPRoute/Backend/DegradeThreshold", PortKind::Http)),
        "GatewayDynamic" => Some(("Gateway/Dynamic", PortKind::Http)),
        "RouteDelegationDynamic" => Some(("HTTPRoute/Delegation/DynamicTest", PortKind::Http)),
        "GrpcRouteDelegationDynamic" => Some(("GRPCRoute/Delegation/DynamicTest", PortKind::Grpc)),
        "ObservedGenHighFreq" => Some(("Status/ObservedGeneration/HighFrequency", PortKind::Http)),
        "BtlspRefFlap" => Some(("Status/BackendTLSPolicy/RefFlap", PortKind::Http)),
        "SpBackendChange" => Some(("HTTPRoute/Backend/SessionPersistenceBackendChange", PortKind::Http)),
        "SpWeighted" => Some(("HTTPRoute/Backend/SessionPersistenceWeighted", PortKind::Http)),
        "DnsResolutionFlap" => Some(("HTTPRoute/Backend/DnsResolutionFlap", PortKind::Http)),
        "TcpEspReload" => Some(("TCPRoute/StreamPlugins/Dynamic", PortKind::Tcp)),
        "TlsLastKnownGood" => Some(("EdgionTls/LastKnownGood", PortKind::Https)),
        // This scenario only inspects control-plane status and intentionally has
        // no data-plane listener.
        "GcParamsFlap" => None,
        _ => anyhow::bail!("dynamic scenario `{id}` has no listener contract"),
    };
    Ok(contract)
}

/// Register test suites matching the given suite name into the runner.
fn add_suites_for_suite(runner: &mut TestRunner, suite: &str, gateway: bool, phase: Option<&str>, full_test: bool) {
    match suite {
        "EdgionBackend/AnthropicProvider" => {
            if !gateway {
                eprintln!("Error: EdgionBackend/AnthropicProvider tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AnthropicProviderTestSuite));
        }
        "EdgionBackend/SingleProvider" => {
            if !gateway {
                eprintln!("Error: EdgionBackend/SingleProvider tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::SingleProviderTestSuite));
        }
        "EdgionBackend/Resilience" => {
            if !gateway {
                eprintln!("Error: EdgionBackend/Resilience tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ResilienceTestSuite));
        }
        "EdgionBackend/CredentialExhaustion" => {
            if !gateway {
                eprintln!("Error: EdgionBackend/CredentialExhaustion tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CredentialExhaustionTestSuite));
        }
        // EdgionBackendTrafficPolicy (control-plane: policy sync, LB store
        // mapping, oldest-wins conflict, hot reload, resync survival).
        "EdgionBackendTrafficPolicy" => {
            runner.add_suite(Box::new(suites::EdgionBackendTrafficPolicyTestSuite));
        }
        // Dynamic upstream authority (Task 8 of the dynamic-upstream-authority
        // feature): HTTPS/cleartext per-tenant dispatch, fail-closed, injection,
        // outbound-path degradation, pinning, and health-probe authority.
        "EdgionBackendTrafficPolicy/upstream_authority" | "EdgionBackendTrafficPolicy/UpstreamAuthority" => {
            runner.add_suite(Box::new(suites::UpstreamAuthorityTestSuite));
        }
        // Per-backend retry budget (Task 8 of the retry-budget / retryConstraint
        // feature): floor-permitted trickle at low volume, percentage cap
        // refusing the majority of retries under load with a deterministic 503,
        // and the denial counter advancing.
        "EdgionBackendTrafficPolicy/RetryBudget" => {
            runner.add_suite(Box::new(suites::RetryBudgetTestSuite));
        }
        // Per-backend connect-timeout override: a blackholed backend with the
        // BTP `connection.connectTimeout` override fails fast, while a sibling
        // backend with no override keeps the global default and still works.
        "EdgionBackendTrafficPolicy/ConnectTimeout" => {
            runner.add_suite(Box::new(suites::ConnectTimeoutTestSuite));
        }
        // HTTPRoute
        "HTTPRoute/Basic" | "HTTPRoute" => {
            runner.add_suite(Box::new(suites::HttpTestSuite));
        }
        "HTTPRoute/SseStreaming" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/SseStreaming tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::SseStreamingTestSuite));
        }
        "HTTPRoute/Delegation" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Delegation tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DelegationTestSuite));
        }
        "HTTPRoute/Match" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Match tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpMatchTestSuite));
        }
        "HTTPRoute/Backend" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Backend tests require --gateway flag");
                std::process::exit(1);
            }
            use port_config::PortKind::Http;
            runner.add_suite(framework::with_listener(
                Box::new(suites::LBRoundRobinTestSuite),
                "HTTPRoute/Backend/LBRoundRobin",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::LBConsistentHashTestSuite),
                "HTTPRoute/Backend/LBConsistentHash",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::LBLeastConnTestSuite),
                "HTTPRoute/Backend/LBLeastConn",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::LBLeastConnRetryOnStatusTestSuite),
                "HTTPRoute/Backend/LBLeastConnRetryOnStatus",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::LBEwmaTestSuite),
                "HTTPRoute/Backend/LBEwma",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::LBEwmaRetryOnStatusTestSuite),
                "HTTPRoute/Backend/LBEwmaRetryOnStatus",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::WeightedBackendTestSuite),
                "HTTPRoute/Backend/WeightedBackend",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::TimeoutTestSuite),
                "HTTPRoute/Backend/Timeout",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::HealthCheckTestSuite),
                "HTTPRoute/Backend/HealthCheck",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::GrpcHealthCheckTestSuite),
                "HTTPRoute/Backend/GrpcHealthCheck",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::RetryOnStatusTestSuite),
                "HTTPRoute/Backend/RetryOnStatus",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::PerTryTimeoutTestSuite),
                "HTTPRoute/Backend/PerTryTimeout",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::CircuitBreakerTestSuite),
                "HTTPRoute/Backend/CircuitBreaker",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::DegradeThresholdTestSuite),
                "HTTPRoute/Backend/DegradeThreshold",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::SessionPersistenceTestSuite),
                "HTTPRoute/Backend/SessionPersistence",
                Http,
            ));
        }
        "HTTPRoute/Backend/LBRoundRobin" => {
            if !gateway {
                eprintln!("Error: LB RoundRobin tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBRoundRobinTestSuite));
        }
        "HTTPRoute/Backend/LBConsistentHash" => {
            if !gateway {
                eprintln!("Error: LB ConsistentHash tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBConsistentHashTestSuite));
        }
        "HTTPRoute/Backend/LBLeastConn" => {
            if !gateway {
                eprintln!("Error: LB LeastConn tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBLeastConnTestSuite));
        }
        "HTTPRoute/Backend/LBLeastConnRetryOnStatus" => {
            if !gateway {
                eprintln!("Error: LB LeastConn RetryOnStatus tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBLeastConnRetryOnStatusTestSuite));
        }
        "HTTPRoute/Backend/LBEwma" => {
            if !gateway {
                eprintln!("Error: LB EWMA tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBEwmaTestSuite));
        }
        "HTTPRoute/Backend/LBEwmaRetryOnStatus" => {
            if !gateway {
                eprintln!("Error: LB EWMA RetryOnStatus tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBEwmaRetryOnStatusTestSuite));
        }
        "HTTPRoute/Backend/WeightedBackend" => {
            if !gateway {
                eprintln!("Error: Weighted backend tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::WeightedBackendTestSuite));
        }
        "HTTPRoute/Backend/Timeout" => {
            if !gateway {
                eprintln!("Error: Timeout tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TimeoutTestSuite));
        }
        "HTTPRoute/Backend/RetryOnStatus" => {
            if !gateway {
                eprintln!("Error: RetryOnStatus tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RetryOnStatusTestSuite));
        }
        "HTTPRoute/Backend/PerTryTimeout" => {
            if !gateway {
                eprintln!("Error: PerTryTimeout tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PerTryTimeoutTestSuite));
        }
        "HTTPRoute/Backend/CircuitBreaker" => {
            if !gateway {
                eprintln!("Error: CircuitBreaker tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CircuitBreakerTestSuite));
        }
        "HTTPRoute/Backend/HealthCheck" => {
            if !gateway {
                eprintln!("Error: HealthCheck tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HealthCheckTestSuite));
        }
        "HTTPRoute/Backend/DegradeThreshold" => {
            if !gateway {
                eprintln!("Error: DegradeThreshold tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DegradeThresholdTestSuite));
        }
        "HTTPRoute/Backend/GrpcHealthCheck" => {
            if !gateway {
                eprintln!("Error: GrpcHealthCheck tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcHealthCheckTestSuite));
        }
        "HTTPRoute/Backend/HealthCheckTransition" => {
            if !gateway {
                eprintln!("Error: HealthCheckTransition tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HealthCheckTransitionTestSuite));
        }
        "HTTPRoute/Backend/DnsResolutionFailure" => {
            if !gateway {
                eprintln!("Error: DnsResolutionFailure tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DnsResolutionFailureTestSuite));
        }
        "HTTPRoute/Backend/DnsResolution" => {
            if !gateway {
                eprintln!("Error: DnsResolution tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DnsResolutionTestSuite));
        }
        "HTTPRoute/Backend/PartiallyInvalid" => {
            if !gateway {
                eprintln!("Error: PartiallyInvalid tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PartiallyInvalidTestSuite));
        }
        "HTTPRoute/Backend/SessionPersistence" => {
            if !gateway {
                eprintln!("Error: Session Persistence tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::SessionPersistenceTestSuite));
        }
        "HTTPRoute/Backend/SessionPersistenceBackendChange" => {
            if !gateway {
                eprintln!("Error: SessionPersistenceBackendChange tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::SpChangeInitialTestSuite));
                }
                Some("final") => {
                    runner.add_suite(Box::new(suites::SpChangeFinalTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::SpChangeInitialTestSuite));
                    runner.add_suite(Box::new(suites::SpChangeFinalTestSuite));
                }
            }
        }
        "HTTPRoute/Backend/SessionPersistenceWeighted" => {
            if !gateway {
                eprintln!("Error: SessionPersistenceWeighted tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::SpWeightedInitialTestSuite));
                }
                Some("zero_weight") => {
                    runner.add_suite(Box::new(suites::SpWeightedZeroWeightTestSuite));
                }
                Some("final") => {
                    runner.add_suite(Box::new(suites::SpWeightedFinalTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::SpWeightedInitialTestSuite));
                    runner.add_suite(Box::new(suites::SpWeightedZeroWeightTestSuite));
                    runner.add_suite(Box::new(suites::SpWeightedFinalTestSuite));
                }
            }
        }
        "HTTPRoute/Backend/H2BackendMtlsPersistence" => {
            if !gateway {
                eprintln!("Error: H2BackendMtlsPersistence tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::H2MtlsPersistenceTestSuite));
        }
        "HTTPRoute/Backend/BackendTLSSNIHostOverride" => {
            if !gateway {
                eprintln!("Error: BackendTLSSNIHostOverride tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BackendTlsSniHostOverrideTestSuite));
        }
        "HTTPRoute/Filters" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Filters tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpRedirectTestSuite));
            runner.add_suite(Box::new(suites::HttpSecurityTestSuite));
            runner.add_suite(Box::new(suites::HeaderModifierTestSuite));
            runner.add_suite(Box::new(suites::FilterConflictTestSuite));
            runner.add_suite(Box::new(suites::PercentMirrorTestSuite));
        }
        "HTTPRoute/Filters/PercentMirror" => {
            if !gateway {
                eprintln!("Error: PercentMirror tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PercentMirrorTestSuite));
        }
        "HTTPRoute/Filters/FilterConflict" => {
            if !gateway {
                eprintln!("Error: FilterConflict tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::FilterConflictTestSuite));
        }
        "HTTPRoute/Filters/Redirect" => {
            if !gateway {
                eprintln!("Error: HTTP Redirect tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpRedirectTestSuite));
        }
        "HTTPRoute/Filters/Security" => {
            if !gateway {
                eprintln!("Error: HTTP Security tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpSecurityTestSuite));
        }
        "HTTPRoute/Filters/HeaderModifier" => {
            if !gateway {
                eprintln!("Error: HTTP Header Modifier tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HeaderModifierTestSuite));
        }
        "HTTPRoute/DanglingExtensionRef" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/DanglingExtensionRef tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DanglingExtensionRefTestSuite));
        }
        "HTTPRoute/Protocol" => {
            runner.add_suite(framework::with_listener(
                Box::new(suites::WebSocketTestSuite::new(gateway)),
                "HTTPRoute/Protocol/WebSocket",
                port_config::PortKind::Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::H2cTestSuite),
                "HTTPRoute/Protocol/H2C",
                port_config::PortKind::Http,
            ));
            if gateway {
                runner.add_suite(framework::with_listener(
                    Box::new(suites::HttpsTestSuite),
                    "EdgionTls/https",
                    port_config::PortKind::Https,
                ));
            }
        }
        "HTTPRoute/Protocol/WebSocket" => {
            runner.add_suite(Box::new(suites::WebSocketTestSuite::new(gateway)));
        }
        "HTTPRoute/Protocol/H2C" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Protocol/H2C tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::H2cTestSuite));
        }
        "HTTPRoute/Protocol/GrpcWeb" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Protocol/GrpcWeb tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcWebTestSuite::default()));
            runner.add_suite(Box::new(suites::GrpcWebTestSuite::http1_only()));
        }
        "HTTPRoute/Protocol/H2TrailerMalformed" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Protocol/H2TrailerMalformed tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::H2TrailerMalformedTestSuite));
        }
        "HTTPRoute/Transport" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Transport tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TransportTestSuite));
        }
        "HTTPRoute/Protocol/HTTPS" => {
            if !gateway {
                eprintln!("Error: HTTPS tests only support Gateway mode. Use -g flag.");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpsTestSuite));
        }
        "HTTPRoute/MultiPort" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/MultiPort tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::MultiPortTestSuite));
        }
        // GRPCRoute
        "GRPCRoute" => {
            runner.add_suite(framework::with_listener(
                Box::new(suites::GrpcTestSuite),
                "GRPCRoute/Basic",
                port_config::PortKind::Grpc,
            ));
            if gateway {
                runner.add_suite(framework::with_listener(
                    Box::new(suites::GrpcMatchTestSuite),
                    "GRPCRoute/Match",
                    port_config::PortKind::Grpc,
                ));
            }
        }
        "GRPCRoute/Basic" => {
            runner.add_suite(Box::new(suites::GrpcTestSuite));
        }
        "GRPCRoute/Delegation" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/Delegation tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcDelegationTestSuite));
        }
        "GRPCRoute/Match" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/Match tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcMatchTestSuite));
        }
        "GRPCRoute/UnmatchedRoute" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/UnmatchedRoute tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcUnmatchedRouteTestSuite));
        }
        "GRPCRoute/RateLimit" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/RateLimit tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcRateLimitTestSuite));
        }
        "GRPCRoute/Backend/SessionPersistence" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/Backend/SessionPersistence tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcSessionPersistenceTestSuite));
        }
        "GRPCRoute/Backend/SessionPersistenceWeighted" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/Backend/SessionPersistenceWeighted tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcSpWeightedTestSuite));
        }
        "GRPCRoute/TLS" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/TLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcTlsTestSuite));
        }
        // TCP/UDP
        "tcp" | "TCPRoute" | "TCPRoute/Basic" => {
            runner.add_suite(Box::new(suites::TcpTestSuite));
        }
        "TCPRoute/StreamPlugins" => {
            if !gateway {
                eprintln!("Error: TCPRoute/StreamPlugins tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TcpStreamPluginsTestSuite));
        }
        "TCPRoute/GlobalConnectionIpRestriction" => {
            if !gateway {
                eprintln!("Error: TCPRoute/GlobalConnectionIpRestriction tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GcirEnforcementTestSuite));
        }
        "TCPRoute/IdleTimeout" => {
            if !gateway {
                eprintln!("Error: TCPRoute/IdleTimeout tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TcpIdleTimeoutTestSuite));
        }
        "TCPRoute/DuplexBackpressure" => {
            if !gateway {
                eprintln!("Error: TCPRoute/DuplexBackpressure tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::L4DuplexTestSuite));
        }
        "TCPRoute/GeoRestriction" => {
            if !gateway {
                eprintln!("Error: TCPRoute/GeoRestriction tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TcpGeoRestrictionTestSuite));
        }
        "TCPRoute/DeniedBackendRef" => {
            if !gateway {
                eprintln!("Error: TCPRoute/DeniedBackendRef tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TcpDeniedBackendRefTestSuite));
        }
        "TCPRoute/KeepaliveSmoke" => {
            if !gateway {
                eprintln!("Error: TCPRoute/KeepaliveSmoke tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TcpKeepaliveSmokeTestSuite));
        }
        // TLSRoute
        "TLSRoute" | "TLSRoute/Basic" => {
            if !gateway {
                eprintln!("Error: TLSRoute tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsRouteTestSuite));
        }
        "TLSRoute/ProxyProtocol" => {
            if !gateway {
                eprintln!("Error: TLSRoute/ProxyProtocol tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsProxyProtocolTestSuite));
        }
        "TLSRoute/StreamPlugins" => {
            if !gateway {
                eprintln!("Error: TLSRoute/StreamPlugins tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsStreamPluginsTestSuite));
        }
        "TLSRoute/MultiSNI" => {
            if !gateway {
                eprintln!("Error: TLSRoute/MultiSNI tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsMultiSniTestSuite));
        }
        "TLSRoute/GeoRestriction" => {
            if !gateway {
                eprintln!("Error: TLSRoute/GeoRestriction tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsGeoRestrictionTestSuite));
        }
        "TLSRoute/BothAbsentParentRef" => {
            if !gateway {
                eprintln!("Error: TLSRoute/BothAbsentParentRef tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsBothAbsentParentRefTestSuite));
        }
        "TLSRoute/Passthrough" => {
            if !gateway {
                eprintln!("Error: TLSRoute/Passthrough tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsPassthroughTestSuite));
        }
        "TLSRoute/UnknownKindBackend" => {
            if !gateway {
                eprintln!("Error: TLSRoute/UnknownKindBackend tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsUnknownKindBackendTestSuite));
        }
        "TLSRoute/DeniedBackendRef" => {
            if !gateway {
                eprintln!("Error: TLSRoute/DeniedBackendRef tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsDeniedBackendRefTestSuite));
        }
        "udp" | "UDPRoute" | "UDPRoute/Basic" => {
            runner.add_suite(Box::new(suites::UdpTestSuite));
        }
        "UDPRoute/StreamPlugins" => {
            if !gateway {
                eprintln!("Error: UDPRoute/StreamPlugins tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::UdpStreamPluginsTestSuite));
        }
        "UDPRoute/DeniedBackendRef" => {
            if !gateway {
                eprintln!("Error: UDPRoute/DeniedBackendRef tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::UdpDeniedBackendRefTestSuite));
        }
        // Gateway
        "Gateway" => {
            if !gateway {
                eprintln!("Error: Gateway tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(framework::with_listener(
                Box::new(suites::SecurityTestSuite),
                "Gateway/Security",
                port_config::PortKind::Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::RealIpTestSuite),
                "Gateway/RealIP",
                port_config::PortKind::Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::PluginLogsTestSuite),
                "EdgionPlugins",
                port_config::PortKind::Http,
            ));
        }
        "Gateway/Security" => {
            if !gateway {
                eprintln!("Error: Gateway/Security tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::SecurityTestSuite));
        }
        "Gateway/RealIP" => {
            if !gateway {
                eprintln!("Error: Gateway/RealIP tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RealIpTestSuite));
        }
        "Gateway/TLS/BackendTLS" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/BackendTLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BackendTlsTestSuite));
        }
        "Gateway/TLS/BackendTLSSAN" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/BackendTLSSAN tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BackendTlsSanTestSuite));
        }
        "Gateway/TLS/BackendTLSGatewayCert" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/BackendTLSGatewayCert tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BackendTlsGatewayCertTestSuite));
        }
        "Gateway/TLS/GatewayTLS" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/GatewayTLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GatewayTlsTestSuite));
        }
        "Gateway/TLS/NoHostnameListener" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/NoHostnameListener tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GatewayTlsNoHostnameListenerTestSuite));
        }
        "Gateway/TLS/MisdirectedRequest" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/MisdirectedRequest tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::MisdirectedRequestTestSuite));
        }
        "Gateway/FrontendValidation" => {
            if !gateway {
                eprintln!("Error: Gateway/FrontendValidation tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::FrontendValidationTestSuite));
        }
        "EdgionPlugins/DebugAccessLog" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DebugAccessLog tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PluginLogsTestSuite));
        }
        "EdgionTls/port_only" => {
            if !gateway {
                eprintln!("Error: EdgionTls/port_only tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PortOnlyEdgionTlsTestSuite));
        }
        "EdgionTls/BothAbsentParentRef" => {
            if !gateway {
                eprintln!("Error: EdgionTls/BothAbsentParentRef tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::EdgionTlsBothAbsentParentRefTestSuite));
        }
        "EdgionPlugins/PluginCondition" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PluginCondition tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PluginConditionTestSuite));
            runner.add_suite(Box::new(suites::AllConditionsTestSuite));
        }
        "EdgionPlugins/PluginCondition/AllConditions" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PluginCondition/AllConditions tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllConditionsTestSuite));
        }
        "EdgionPlugins/JwtAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/JwtAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::JwtAuthTestSuite));
        }
        "EdgionPlugins/BasicAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/BasicAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BasicAuthTestSuite));
        }
        "EdgionPlugins/BasicAuthDynamic" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/BasicAuthDynamic tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::BasicAuthDynamicInitialTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::BasicAuthDynamicUpdateTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::BasicAuthDynamicInitialTestSuite));
                    runner.add_suite(Box::new(suites::BasicAuthDynamicUpdateTestSuite));
                }
            }
        }
        "EdgionPlugins/BasicAuthDynamicGrouped" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/BasicAuthDynamicGrouped tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::BasicAuthDynamicGroupedInitialTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::BasicAuthDynamicGroupedUpdateTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::BasicAuthDynamicGroupedInitialTestSuite));
                    runner.add_suite(Box::new(suites::BasicAuthDynamicGroupedUpdateTestSuite));
                }
            }
        }
        "EdgionPlugins/JweDecrypt" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/JweDecrypt tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::JweDecryptTestSuite));
        }
        "EdgionPlugins/KeyAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/KeyAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::KeyAuthTestSuite));
        }
        "EdgionPlugins/CredentialInjector" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/CredentialInjector tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CredentialInjectorTestSuite));
        }
        "EdgionPlugins/HmacAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/HmacAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HmacAuthTestSuite));
        }
        "EdgionPlugins/HeaderCertAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/HeaderCertAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HeaderCertAuthTestSuite));
        }
        "EdgionPlugins/LdapAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/LdapAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LdapAuthTestSuite));
        }
        "EdgionPlugins/LdapAuthTls" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/LdapAuthTls tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LdapAuthTlsTestSuite));
        }
        "EdgionPlugins/ProxyRewrite" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/ProxyRewrite tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ProxyRewriteTestSuite));
        }
        "EdgionPlugins/ResponseRewrite" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/ResponseRewrite tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ResponseRewriteTestSuite));
        }
        "EdgionPlugins/RateLimit" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RateLimit tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RateLimitTestSuite));
        }
        "EdgionPlugins/RateLimitDynamic" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RateLimitDynamic tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::RateLimitDynamicInitialTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::RateLimitDynamicUpdateTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::RateLimitDynamicInitialTestSuite));
                    runner.add_suite(Box::new(suites::RateLimitDynamicUpdateTestSuite));
                }
            }
        }
        "EdgionPlugins/RateLimitRedis" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RateLimitRedis tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RateLimitRedisTestSuite));
        }
        "EdgionPlugins/RealIp" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RealIp tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RealIpPluginTestSuite));
        }
        "EdgionPlugins/TraceContext" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/TraceContext tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TraceContextPluginTestSuite));
        }
        "EdgionPlugins/Wasm" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/Wasm tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::WasmPluginTestSuite));
        }
        "EdgionPlugins/RequestMirror" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RequestMirror tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RequestMirrorTestSuite));
        }
        "EdgionPlugins/RequestRestriction" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RequestRestriction tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RequestRestrictionTestSuite));
        }
        "EdgionPlugins/RequestAccessPolicy" => {
            if !gateway {
                eprintln!("Error: RequestAccessPolicy tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RequestAccessPolicyTestSuite));
        }
        "EdgionPlugins/Canary" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/Canary tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CanaryTestSuite));
        }
        "EdgionPlugins/IpRestriction" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/IpRestriction tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::IpRestrictionPluginTestSuite));
        }
        "EdgionPlugins/IpRestrictionRefs" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/IpRestrictionRefs tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::IpRestrictionRefsPluginTestSuite));
        }
        "EdgionPlugins/IpRestrictionPartialGood" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/IpRestrictionPartialGood tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::IpRestrictionPartialGoodPluginTestSuite));
        }
        "EdgionPlugins/FaultInjection" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/FaultInjection tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::FaultInjectionTestSuite));
        }
        "EdgionPlugins/ForwardAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/ForwardAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ForwardAuthTestSuite));
        }
        "EdgionPlugins/ExtProc" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/ExtProc tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ExtProcTestSuite::new(full_test)));
        }
        "EdgionPlugins/AiGuard" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/AiGuard tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AiGuardTestSuite));
        }
        "EdgionPlugins/Guardrail" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/Guardrail tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GuardrailTestSuite));
        }
        "EdgionPlugins/DirectEndpoint" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DirectEndpoint tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DirectEndpointTestSuite));
        }
        "EdgionPlugins/DynamicInternalUpstream" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DynamicInternalUpstream tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DynamicInternalUpstreamTestSuite));
            // The extra suite applies local loopback fixtures. K8s shares this
            // dispatch but must keep using its existing namespace/Pod fixtures.
            if std::env::var("EDGION_TEST_K8S_MODE").as_deref() != Ok("true") {
                runner.add_suite(Box::new(suites::DeniedNamedJumpTestSuite));
            }
        }
        "EdgionPlugins/DynamicExternalUpstream" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DynamicExternalUpstream tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DynamicExternalUpstreamTestSuite));
        }
        "EdgionPlugins/RegionRoute" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/RegionRoute tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RegionRouteTestSuite));
        }
        "EdgionPlugins/EdgionConfigDataManaged" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/EdgionConfigDataManaged tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::EdgionConfigDataManagedTestSuite));
        }
        "EdgionPlugins/GlobalConnectionIpRestriction" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/GlobalConnectionIpRestriction tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GlobalConnectionIpRestrictionTestSuite));
        }
        "EdgionPlugins/GeoRestriction" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/GeoRestriction tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GeoRestrictionTestSuite));
        }
        "EdgionPlugins/EdgionConfigDataStatus" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/EdgionConfigDataStatus tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::EdgionConfigDataStatusTestSuite));
        }
        "EdgionPlugins/PluginValidationStatus" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PluginValidationStatus tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PluginValidationStatusTestSuite));
        }
        "EdgionPlugins/EdgionConfigDataStatus/RefFlap" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/EdgionConfigDataStatus/RefFlap tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::EdgionConfigDataFlapInitialTestSuite));
                }
                Some("invalid") => {
                    runner.add_suite(Box::new(suites::EdgionConfigDataFlapInvalidTestSuite));
                }
                Some("valid") => {
                    runner.add_suite(Box::new(suites::EdgionConfigDataFlapValidTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::EdgionConfigDataFlapInitialTestSuite));
                    runner.add_suite(Box::new(suites::EdgionConfigDataFlapInvalidTestSuite));
                    runner.add_suite(Box::new(suites::EdgionConfigDataFlapValidTestSuite));
                }
            }
        }
        "EdgionPlugins/ResolvedRefs" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/ResolvedRefs tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ResolvedRefsTestSuite));
        }
        "LinkSys/Status" => {
            if !gateway {
                eprintln!("Error: LinkSys/Status tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LinkSysStatusTestSuite));
        }
        "EdgionPlugins/AllEndpointStatus" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/AllEndpointStatus tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllEndpointStatusTestSuite));
        }
        "EdgionPlugins/OpenidConnect" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/OpenidConnect tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::OpenidConnectTestSuite));
        }
        "EdgionPlugins/PathNormalization" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PathNormalization tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PathNormalizationTestSuite));
        }
        "EdgionPlugins/PathNormalization/PreserveParam" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PathNormalization/PreserveParam tests require --gateway flag");
                std::process::exit(1);
            }
            // NOTE: This suite requires the gateway to be started with
            // preserve_path_parameters: true in the `path_normalization` YAML config.
            // TC-C is excluded from automation (manual curl verifies preserve-mode bypass
            // behaviour, see fixture); TC-B is mode-agnostic and passes in both strip and
            // preserve modes.
            runner.add_suite(Box::new(suites::PathNormalizationPreserveParamTestSuite));
        }
        "EdgionPlugins/PathNormalization/ForwardView" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PathNormalization/ForwardView tests require --gateway flag");
                std::process::exit(1);
            }
            // Verifies which path the gateway forwards to upstream based on:
            //   TC-A: default (no annotation) → normalized path forwarded
            //   TC-B: annotation forward-raw-path: "true" → raw path forwarded
            //   TC-C: annotation forward-raw-path: "false" → normalized path forwarded
            // Scenarios that require gateway YAML restart (global forward_normalized_path: false,
            // region_route with no annotation) are documented in the fixture YAMLs and suite
            // header comment for manual verification.
            runner.add_suite(Box::new(suites::PathNormalizationForwardViewTestSuite));
        }
        "EdgionPlugins/BandwidthLimit" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/BandwidthLimit tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BandwidthLimitTestSuite));
        }
        "EdgionPlugins/Cors" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/Cors tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CorsPluginTestSuite));
        }
        "EdgionPlugins/GlobalPluginsSnapshot" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/GlobalPluginsSnapshot tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GlobalPluginsSnapshotTestSuite));
        }
        "EdgionPlugins/Csrf" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/Csrf tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CsrfTestSuite));
        }
        "EdgionPlugins/CtxSet" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/CtxSet tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CtxSetTestSuite));
        }
        "EdgionPlugins/MatchedRouteMeta" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/MatchedRouteMeta tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::MatchedRouteMetaTestSuite));
        }
        "EdgionPlugins/AccessLogExtern" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/AccessLogExtern tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AccessLogExternTestSuite));
        }
        "EdgionPlugins/WebhookKeyGet" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/WebhookKeyGet tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::WebhookKeyGetTestSuite));
        }
        "EdgionPlugins/Dsl" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/Dsl tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DslTestSuite));
        }
        "EdgionPlugins/DslBody" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DslBody tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DslBodyTestSuite));
        }
        "EdgionPlugins/BodyPlugins" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/BodyPlugins tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BodyPluginsTestSuite));
        }
        "EdgionPlugins/JsonSchemaValidation" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/JsonSchemaValidation tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::JsonSchemaValidationTestSuite));
        }
        "EdgionPlugins/BodyDynamic" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/BodyDynamic tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::BodyDynamicInitialTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::BodyDynamicUpdateTestSuite));
                }
                None => {
                    runner.add_suite(Box::new(suites::BodyDynamicInitialTestSuite));
                    runner.add_suite(Box::new(suites::BodyDynamicUpdateTestSuite));
                }
                _ => {
                    eprintln!("Error: Invalid phase '{}'. Use 'initial' or 'update'", phase.unwrap());
                    std::process::exit(1);
                }
            }
        }
        "EdgionPlugins/DslDynamic" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DslDynamic tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::DslDynamicInitialTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::DslDynamicUpdateTestSuite));
                }
                None => {
                    runner.add_suite(Box::new(suites::DslDynamicInitialTestSuite));
                    runner.add_suite(Box::new(suites::DslDynamicUpdateTestSuite));
                }
                _ => {
                    eprintln!("Error: Invalid phase '{}'. Use 'initial' or 'update'", phase.unwrap());
                    std::process::exit(1);
                }
            }
        }
        "EdgionPlugins/Waf" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/Waf tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::WafDynamicInitialTestSuite));
        }
        "EdgionPlugins/DslGrpcError" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DslGrpcError tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::DslGrpcErrorTestSuite));
        }
        "Gateway/ListenerHostname" => {
            if !gateway {
                eprintln!("Error: Gateway/ListenerHostname tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ListenerHostnameTestSuite));
        }
        "Gateway/AllowedRoutes/Same" => {
            if !gateway {
                eprintln!("Error: Gateway/AllowedRoutes/Same tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllowedRoutesSameNamespaceTestSuite));
        }
        "Gateway/AllowedRoutes/All" => {
            if !gateway {
                eprintln!("Error: Gateway/AllowedRoutes/All tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllowedRoutesAllNamespacesTestSuite));
        }
        "Gateway/AllowedRoutes/Kinds" => {
            if !gateway {
                eprintln!("Error: Gateway/AllowedRoutes/Kinds tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllowedRoutesKindsTestSuite));
        }
        "Gateway/AllowedRoutes/Selector" => {
            if !gateway {
                eprintln!("Error: Gateway/AllowedRoutes/Selector tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllowedRoutesSelectorNamespaceTestSuite));
        }
        "Gateway/Combined" => {
            if !gateway {
                eprintln!("Error: Gateway/Combined tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CombinedScenariosTestSuite));
        }
        "Gateway/StreamPlugins" => {
            if !gateway {
                eprintln!("Error: Gateway/StreamPlugins tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::StreamPluginsTestSuite));
        }
        "Gateway/PortConflict" => {
            if !gateway {
                eprintln!("Error: Gateway/PortConflict tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PortConflictTestSuite));
        }
        "Gateway/Dynamic" => {
            if !gateway {
                eprintln!("Error: Gateway/Dynamic tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::InitialPhaseTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::UpdatePhaseTestSuite));
                }
                None => {
                    runner.add_suite(Box::new(suites::InitialPhaseTestSuite));
                    runner.add_suite(Box::new(suites::UpdatePhaseTestSuite));
                }
                _ => {
                    eprintln!("Error: Invalid phase '{}'. Use 'initial' or 'update'", phase.unwrap());
                    std::process::exit(1);
                }
            }
        }
        // EdgionTls
        "EdgionTls" => {
            if !gateway {
                eprintln!("Error: EdgionTls tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpsTestSuite));
            runner.add_suite(Box::new(suites::GrpcTlsTestSuite));
            runner.add_suite(Box::new(suites::MtlsTestSuite));
            runner.add_suite(Box::new(suites::MtlsFailcloseTestSuite));
            runner.add_suite(Box::new(suites::PortOnlyEdgionTlsTestSuite));
            runner.add_suite(Box::new(suites::FallbackSniTestSuite));
            runner.add_suite(Box::new(suites::WildcardSingleLabelTestSuite));
            runner.add_suite(Box::new(suites::SniCollisionTestSuite));
        }
        "EdgionTls/https" => {
            if !gateway {
                eprintln!("Error: EdgionTls/https tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpsTestSuite));
        }
        "EdgionTls/grpctls" => {
            if !gateway {
                eprintln!("Error: EdgionTls/grpctls tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcTlsTestSuite));
        }
        "EdgionTls/mTLS" => {
            if !gateway {
                eprintln!("Error: EdgionTls/mTLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::MtlsTestSuite));
        }
        "EdgionTls/cipher" => {
            if !gateway {
                eprintln!("Error: EdgionTls/cipher tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CipherTestSuite));
        }
        "EdgionTls/mtls_failclose" => {
            if !gateway {
                eprintln!("Error: EdgionTls/mtls_failclose tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::MtlsFailcloseTestSuite));
        }
        "EdgionTls/fallback_sni" => {
            if !gateway {
                eprintln!("Error: EdgionTls/fallback_sni tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::FallbackSniTestSuite));
        }
        "EdgionTls/wildcard_single_label" => {
            if !gateway {
                eprintln!("Error: EdgionTls/wildcard_single_label tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::WildcardSingleLabelTestSuite));
        }
        "EdgionTls/sni_collision" => {
            if !gateway {
                eprintln!("Error: EdgionTls/sni_collision tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::SniCollisionTestSuite));
        }
        // Status tests
        "Status/RefGrant" => {
            if !gateway {
                eprintln!("Error: Status/RefGrant tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RefGrantStatusTestSuite));
        }
        "Status/HostnameIntersection" => {
            if !gateway {
                eprintln!("Error: Status/HostnameIntersection tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HostnameIntersectionStatusTestSuite));
        }
        "Status/BackendTLSPolicy" => {
            if !gateway {
                eprintln!("Error: Status/BackendTLSPolicy tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BackendTlsStatusTestSuite));
        }
        "Status/EdgionBackendTrafficPolicy" => {
            if !gateway {
                eprintln!("Error: Status/EdgionBackendTrafficPolicy tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::EbtpAncestorStatusTestSuite));
        }
        "Status/BackendTLSPolicy/RefFlap" => {
            if !gateway {
                eprintln!("Error: Status/BackendTLSPolicy/RefFlap tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::BtlspFlapInitialTestSuite));
                }
                Some("invalid") => {
                    runner.add_suite(Box::new(suites::BtlspFlapInvalidTestSuite));
                }
                Some("valid") => {
                    runner.add_suite(Box::new(suites::BtlspFlapValidTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::BtlspFlapInitialTestSuite));
                    runner.add_suite(Box::new(suites::BtlspFlapInvalidTestSuite));
                    runner.add_suite(Box::new(suites::BtlspFlapValidTestSuite));
                }
            }
        }
        "Status/GatewayFrontendValidation" => {
            if !gateway {
                eprintln!("Error: Status/GatewayFrontendValidation tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GatewayFrontendValidationStatusTestSuite));
        }
        "Status/TLSListenerMode" => {
            if !gateway {
                eprintln!("Error: Status/TLSListenerMode tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsListenerModeStatusTestSuite));
        }
        "Status/GRPCRoute" => {
            if !gateway {
                eprintln!("Error: Status/GRPCRoute tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcRouteStatusTestSuite));
        }
        "Status/TLSRoute" => {
            if !gateway {
                eprintln!("Error: Status/TLSRoute tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsRouteStatusTestSuite));
        }
        "Status/ObservedGeneration" => {
            if !gateway {
                eprintln!("Error: Status/ObservedGeneration tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ObservedGenerationStatusTestSuite));
        }
        "Status/ObservedGeneration/HighFrequency" => {
            if !gateway {
                eprintln!("Error: Status/ObservedGeneration/HighFrequency tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::HighFreqInitialTestSuite));
                }
                Some("final") => {
                    runner.add_suite(Box::new(suites::HighFreqFinalTestSuite));
                }
                _ => {
                    runner.add_suite(Box::new(suites::HighFreqInitialTestSuite));
                    runner.add_suite(Box::new(suites::HighFreqFinalTestSuite));
                }
            }
        }
        "Status/RefGrant/TCPRouteCrossNsSvc" => {
            if !gateway {
                eprintln!("Error: Status/RefGrant/TCPRouteCrossNsSvc tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TcpRouteCrossNsSvcTestSuite));
        }
        "Status/RefGrant/TLSRouteCrossNsSvc" => {
            if !gateway {
                eprintln!("Error: Status/RefGrant/TLSRouteCrossNsSvc tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TlsRouteCrossNsSvcTestSuite));
        }
        "Status/RefGrant/EdgionConfigData" => {
            if !gateway {
                eprintln!("Error: Status/RefGrant/EdgionConfigData tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::EdgionConfigDataRefGrantTestSuite));
        }
        // Services tests (ACME, etc.)
        "Services/acme" => {
            runner.add_suite(Box::new(suites::AcmeTestSuite));
        }
        "all" => {
            use port_config::PortKind::{Grpc, Http, Https, Tcp, Udp};
            runner.add_suite(framework::with_listener(
                Box::new(suites::HttpTestSuite),
                "HTTPRoute/Basic",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::GrpcTestSuite),
                "GRPCRoute/Basic",
                Grpc,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::WebSocketTestSuite::new(gateway)),
                "HTTPRoute/Protocol/WebSocket",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::H2cTestSuite),
                "HTTPRoute/Protocol/H2C",
                Http,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::TcpTestSuite),
                "TCPRoute/Basic",
                Tcp,
            ));
            runner.add_suite(framework::with_listener(
                Box::new(suites::UdpTestSuite),
                "UDPRoute/Basic",
                Udp,
            ));
            if gateway {
                runner.add_suite(framework::with_listener(
                    Box::new(suites::SingleProviderTestSuite),
                    "EdgionBackend/SingleProvider",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::AnthropicProviderTestSuite),
                    "EdgionBackend/AnthropicProvider",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::ResilienceTestSuite),
                    "EdgionBackend/Resilience",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::CredentialExhaustionTestSuite),
                    "EdgionBackend/CredentialExhaustion",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::TcpIdleTimeoutTestSuite),
                    "TCPRoute/Basic",
                    Tcp,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::L4DuplexTestSuite),
                    "TCPRoute/DuplexBackpressure",
                    Tcp,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::TcpKeepaliveSmokeTestSuite),
                    "TCPRoute/Basic",
                    Tcp,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::HttpMatchTestSuite),
                    "HTTPRoute/Match",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::HttpsTestSuite),
                    "EdgionTls/https",
                    Https,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::GrpcMatchTestSuite),
                    "GRPCRoute/Match",
                    Grpc,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::RealIpTestSuite),
                    "Gateway/RealIP",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::SecurityTestSuite),
                    "Gateway/Security",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::HttpSecurityTestSuite),
                    "HTTPRoute/Filters/Security",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::HttpRedirectTestSuite),
                    "HTTPRoute/Filters/Redirect",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::PluginLogsTestSuite),
                    "EdgionPlugins",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::LBRoundRobinTestSuite),
                    "HTTPRoute/Backend/LBRoundRobin",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::LBConsistentHashTestSuite),
                    "HTTPRoute/Backend/LBConsistentHash",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::LBLeastConnTestSuite),
                    "HTTPRoute/Backend/LBLeastConn",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::LBLeastConnRetryOnStatusTestSuite),
                    "HTTPRoute/Backend/LBLeastConnRetryOnStatus",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::LBEwmaTestSuite),
                    "HTTPRoute/Backend/LBEwma",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::LBEwmaRetryOnStatusTestSuite),
                    "HTTPRoute/Backend/LBEwmaRetryOnStatus",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::WeightedBackendTestSuite),
                    "HTTPRoute/Backend/WeightedBackend",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::TimeoutTestSuite),
                    "HTTPRoute/Backend/Timeout",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::HealthCheckTestSuite),
                    "HTTPRoute/Backend/HealthCheck",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::GrpcHealthCheckTestSuite),
                    "HTTPRoute/Backend/GrpcHealthCheck",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::RetryOnStatusTestSuite),
                    "HTTPRoute/Backend/RetryOnStatus",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::PerTryTimeoutTestSuite),
                    "HTTPRoute/Backend/PerTryTimeout",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::CircuitBreakerTestSuite),
                    "HTTPRoute/Backend/CircuitBreaker",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::DegradeThresholdTestSuite),
                    "HTTPRoute/Backend/DegradeThreshold",
                    Http,
                ));
                runner.add_suite(framework::with_listener(
                    Box::new(suites::MtlsTestSuite),
                    "EdgionTls/mTLS",
                    Https,
                ));
            }
        }
        _ => {
            eprintln!("Error: Unknown suite: {}", suite);
            std::process::exit(1);
        }
    }
}

async fn run_dynamic_scenario(cli: &Cli, sid: &str) -> Result<()> {
    use anyhow::Context;

    let scenario =
        dynamic::registry::find(sid).ok_or_else(|| anyhow::anyhow!("Unknown dynamic scenario id: {}", sid))?;

    let project_root = std::env::current_dir().context("Failed to get current directory")?;
    let conf_root = project_root.join("edgion-tests/integration/conf");

    // Dynamic scenarios declare one exact primary listener contract. Individual
    // phase suites may additionally resolve an HTTP port_key through TestRunner.
    let (http_port, grpc_port, tcp_port, udp_port, https_port, grpc_https_port, http_host, grpc_host) = if cli.gateway {
        let cfg = port_config::PortConfig::load().map_err(anyhow::Error::msg)?;
        let ports = match dynamic_port_contract(scenario.id)? {
            Some((key, kind)) => {
                cfg.require_port(key, kind).map_err(anyhow::Error::msg)?;
                cfg.get_ports(key).map_err(anyhow::Error::msg)?.clone()
            }
            None => port_config::SuitePorts::default(),
        };
        (
            ports.http.unwrap_or(cli.http_port),
            ports.grpc.unwrap_or(cli.grpc_port),
            ports.tcp.or(ports.tls).unwrap_or(cli.tcp_port),
            ports.udp.unwrap_or(cli.udp_port),
            ports.https.unwrap_or(cli.https_port),
            ports.grpc_tls.unwrap_or(cli.grpc_https_port),
            Some("test.example.com".to_string()),
            Some("grpc.example.com".to_string()),
        )
    } else {
        (
            cli.http_port,
            cli.grpc_port,
            cli.tcp_port,
            cli.udp_port,
            cli.https_port,
            cli.grpc_https_port,
            None,
            None,
        )
    };

    let access_log_path =
        std::env::var("EDGION_TEST_ACCESS_LOG_PATH").unwrap_or_else(|_| "logs/edgion_access.log".to_string());

    let test_ctx = framework::TestContext::new(
        cli.target_host.clone(),
        http_port,
        grpc_port,
        cli.websocket_port,
        tcp_port,
        cli.tcp_port + 1, // tcp_filtered_port is by convention tcp+1
        udp_port,
        https_port,
        grpc_https_port,
        cli.admin_port,
        http_host,
        grpc_host,
        cli.gateway,
        cli.verbose,
        PathBuf::from(access_log_path),
    );

    let ctx = dynamic::ScenarioCtx {
        test_ctx,
        conf_root,
        conf_path: scenario.conf_path.to_string(),
        controller_url: std::env::var("EDGION_TEST_ADMIN_API_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
            .map(|url| url.trim_end_matches('/').to_string())
            .unwrap_or_else(|| format!("http://127.0.0.1:{}", cli.admin_port)),
        gateway_url: std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
            .map(|url| url.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "http://127.0.0.1:12001".to_string()),
        project_root,
    };

    eprintln!("▶ Running dynamic scenario: {} ({})", scenario.id, scenario.conf_path);
    (scenario.run)(ctx).await?;
    eprintln!("✓ Dynamic scenario {} passed", scenario.id);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install the default rustls crypto provider (once).
    INIT.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install rustls crypto provider");
    });

    let cli = Cli::parse();

    if cli.verbose {
        tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).init();
    }

    // --list-dynamic-scenarios: print the id list and exit.
    if cli.list_dynamic_scenarios {
        for s in validated_dynamic_scenarios()? {
            println!("{}", s.id);
        }
        return Ok(());
    }

    // --dynamic-scenario <id>: run a single scenario and exit.
    if let Some(ref sid) = cli.dynamic_scenario {
        if let Some(capability) = missing_capability("dynamic", sid)? {
            if std::env::var("EDGION_INTEGRATION_EXPLICIT_TARGET").as_deref() != Ok("0") {
                anyhow::bail!("dynamic scenario `{sid}` requires Gateway capability `{capability}`");
            }
            println!("SKIPPED: dynamic scenario `{sid}` requires Gateway capability `{capability}`");
            return Ok(());
        }
        return run_dynamic_scenario(&cli, sid).await;
    }

    // Resolve suite name.
    let suite = resolve_suite(
        cli.resource.as_deref(),
        cli.item.as_deref(),
        cli.legacy_command.as_deref(),
    );

    if let Some(capability) = missing_capability("suite", &suite)? {
        if std::env::var("EDGION_INTEGRATION_EXPLICIT_TARGET").as_deref() != Ok("0") {
            anyhow::bail!("suite `{suite}` requires Gateway capability `{capability}`");
        }
        println!("SKIPPED: suite `{suite}` requires Gateway capability `{capability}`");
        return Ok(());
    }

    // Port config key
    let port_key = suite_to_port_key(&suite);

    // Determine ports and host based on gateway flag
    let (
        http_port,
        grpc_port,
        tcp_port,
        tcp_filtered_port,
        udp_port,
        websocket_port,
        https_port,
        grpc_https_port,
        http_host,
        grpc_host,
    ) = if cli.gateway {
        // Gateway mode requires the exact local listener mapping. K8s test
        // services expose the same declared listener ports, so they consume the
        // same fail-closed registry.
        {
            let config = port_config::PortConfig::load().map_err(anyhow::Error::msg)?;
            let ports = if let Some(kind) = required_port_kind(port_key) {
                config.require_port(port_key, kind).map_err(anyhow::Error::msg)?;
                config.get_ports(port_key).map_err(anyhow::Error::msg)?.clone()
            } else {
                port_config::SuitePorts::default()
            };
            // Select http_host based on suite
            let http_host = match suite.as_str() {
                "Gateway/TLS/GatewayTLS" => "gateway-tls.test.com",
                "Gateway/TLS/NoHostnameListener" => "no-hostname-gateway-tls.test.com",
                "Gateway/TLS/MisdirectedRequest" => "sni421-exact.test.com",
                "EdgionTls/port_only" => "port-only.example.com",
                "EdgionTls/BothAbsentParentRef" => "both-absent-tls.example.com",
                _ => "test.example.com",
            };
            let http_port = ports.http.unwrap_or(cli.http_port);
            (
                http_port,
                ports.grpc.unwrap_or(cli.grpc_port),
                ports.tcp.or(ports.tls).unwrap_or(cli.tcp_port),
                ports.tcp_filtered.unwrap_or(cli.tcp_port + 1),
                ports.udp.unwrap_or(cli.udp_port),
                http_port,
                ports.https.unwrap_or(cli.https_port),
                ports.grpc_tls.unwrap_or(cli.grpc_https_port),
                Some(http_host.to_string()),
                Some("grpc.example.com".to_string()),
            )
        }
    } else {
        // Direct mode: use CLI provided ports
        (
            cli.http_port,
            cli.grpc_port,
            cli.tcp_port,
            cli.tcp_port + 1,
            cli.udp_port,
            cli.websocket_port,
            cli.https_port,
            cli.grpc_https_port,
            None,
            None,
        )
    };

    let mode_name = if cli.gateway { "Gateway" } else { "Direct" };

    println!("\n========================================");
    println!("Edgion ");
    println!("========================================");
    println!("Mode: {}", mode_name);
    println!("Suite: {}", suite);
    println!("Target: {}:{}", cli.target_host, http_port);
    println!("========================================\n");

    // Get access log path from environment variable
    let access_log_path =
        std::env::var("EDGION_TEST_ACCESS_LOG_PATH").unwrap_or_else(|_| "logs/edgion_access.log".to_string());

    let context = TestContext::new(
        cli.target_host.clone(),
        http_port,
        grpc_port,
        websocket_port,
        tcp_port,
        tcp_filtered_port,
        udp_port,
        https_port,
        grpc_https_port,
        cli.admin_port,
        http_host.clone(),
        grpc_host,
        cli.gateway,
        cli.verbose,
        PathBuf::from(access_log_path),
    );

    let mut runner = TestRunner::new(context);

    // Enable per-suite port resolution from ports.json
    if cli.gateway {
        let port_config = port_config::PortConfig::load().map_err(anyhow::Error::msg)?;
        runner.set_port_resolver(Box::new(move |key: &str, kind| {
            port_config.require_port(key, kind)?;
            port_config.get_ports(key).cloned()
        }));
    }

    add_suites_for_suite(&mut runner, &suite, cli.gateway, cli.phase.as_deref(), cli.full_test);

    let start_time = Instant::now();
    let results = runner.run().await;
    let total_duration = start_time.elapsed();

    let console_reporter = ConsoleReporter::new();
    console_reporter.report(&results, total_duration);

    if cli.json {
        let json_reporter = JsonReporter::new();
        json_reporter.save_to_file(&results, total_duration, &cli.json_output)?;
        println!("\n✓ JSON : {}", cli.json_output);
    }

    if results.has_failures() {
        std::process::exit(1);
    }

    Ok(())
}
