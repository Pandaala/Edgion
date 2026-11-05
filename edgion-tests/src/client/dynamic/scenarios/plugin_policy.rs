//! EGC policy migration on the existing Controller/Gateway processes.

use anyhow::{ensure, Context, Result};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites::plugin_policy::{udp_echo, udp_socket, AbsentSuite, Outcome, PolicySuite};

macro_rules! suite {
    ($name:ident, $a:expr, $invalid:expr) => {
        fn $name() -> Box<dyn TestSuite> {
            Box::new(PolicySuite {
                a: $a,
                b: Outcome::Allowed,
                invalid_sources: $invalid,
            })
        }
    };
}
suite!(allowed, Outcome::Allowed, false);
suite!(blocked, Outcome::Block(451), false);
suite!(changed_status, Outcome::Block(429), false);
suite!(bypassed, Outcome::Bypass, false);
suite!(invalid_blocked, Outcome::Block(451), true);
suite!(invalid_bypassed, Outcome::Bypass, true);
suite!(missing, Outcome::Unavailable, false);
fn absent() -> Box<dyn TestSuite> {
    Box::new(AbsentSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "clean",
        factory: absent,
    },
    PhaseSuite {
        name: "initial",
        factory: allowed,
    },
    PhaseSuite {
        name: "block",
        factory: blocked,
    },
    PhaseSuite {
        name: "status-change",
        factory: changed_status,
    },
    PhaseSuite {
        name: "bypass",
        factory: bypassed,
    },
    PhaseSuite {
        name: "relaxed",
        factory: allowed,
    },
    PhaseSuite {
        name: "invalid-block",
        factory: invalid_blocked,
    },
    PhaseSuite {
        name: "invalid-bypass",
        factory: invalid_bypassed,
    },
    PhaseSuite {
        name: "recovered",
        factory: blocked,
    },
    PhaseSuite {
        name: "missing-egc",
        factory: missing,
    },
    PhaseSuite {
        name: "recreated-egc",
        factory: blocked,
    },
    PhaseSuite {
        name: "rebound",
        factory: allowed,
    },
    PhaseSuite {
        name: "binding-restored",
        factory: blocked,
    },
];

async fn phase(ctx: &ScenarioCtx, name: &str) -> Result<()> {
    ctx.wait_phase(SCENARIO, name, Duration::from_secs(30)).await
}

async fn apply_value(ctx: &ScenarioCtx, value: serde_json::Value) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    crate::dynamic::apply::apply_yaml_documents(
        &client,
        &ctx.resolve("generated-phase.yaml"),
        &yaml_serde::to_string(&value)?,
    )
    .await
}

async fn policy(ctx: &ScenarioCtx, action: Option<&str>, status: u16) -> Result<()> {
    let mut value: serde_json::Value =
        yaml_serde::from_str(&tokio::fs::read_to_string(ctx.resolve("initial/EdgionGatewayConfig_pp-a.yaml")).await?)?;
    if let Some(action) = action {
        value["spec"]["pluginPolicy"] = json!({
            "blockStatus": status,
            "deny": (["http/Mock", "http/ResponseHeaderModifier", "http/BandwidthLimit", "stream/IpRestriction", "tls-route/IpRestriction"]
                .into_iter().map(|name| json!({"name": name, "action": action})).collect::<Vec<_>>())
        });
    }
    apply_value(ctx, value).await
}

const INVALID_SOURCE_TARGETS: &[(&str, &str, &str)] = &[
    (
        "EdgionPlugins_pp-dsl.yaml",
        "/spec/requestPlugins/1/config/source",
        "let invalid =",
    ),
    (
        "EdgionConfigData_pp-waf-policy.yaml",
        "/spec/data/config/profiles/baseline/bundleRefs/0/expectedDigest",
        "0000000000000000000000000000000000000000000000000000000000000000",
    ),
];

fn invalidate_source(input: &str, pointer: &str, invalid: &str) -> Result<serde_json::Value> {
    let mut value: serde_json::Value = yaml_serde::from_str(input)?;
    let target = value
        .pointer_mut(pointer)
        .with_context(|| format!("fixture missing {pointer}"))?;
    ensure!(target.is_string(), "fixture {pointer} must be a string");
    *target = json!(invalid);
    Ok(value)
}

async fn invalid_sources(ctx: &ScenarioCtx) -> Result<()> {
    for (file, pointer, invalid) in INVALID_SOURCE_TARGETS {
        let input = tokio::fs::read_to_string(ctx.resolve(&format!("initial/{file}"))).await?;
        let value = invalidate_source(&input, pointer, invalid)?;
        apply_value(ctx, value).await?;
    }
    Ok(())
}

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let identities = tokio::fs::read_to_string(ctx.resolve("delete/resources.txt")).await?;
    let mut failures = Vec::new();
    for identity in identities.lines().filter(|line| !line.trim().is_empty()) {
        let parts: Vec<_> = identity.split('/').collect();
        ensure!(parts.len() == 3, "invalid policy cleanup identity");
        let namespace = (!parts[1].is_empty()).then_some(parts[1]);
        match client.delete(parts[0], namespace, parts[2]).await {
            Ok(response) if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND => {}
            Ok(response) => failures.push(format!("{identity}: {}", response.status())),
            Err(error) => failures.push(format!("{identity}: {error}")),
        }
    }
    ensure!(failures.is_empty(), "policy cleanup failed: {}", failures.join("; "));
    phase(ctx, "clean").await
}

// Local shared-topology runs expose these canonical PID files. Kubernetes has
// separate Pod UID/restartCount checks in the live deployment verification.
async fn process_identity() -> Result<Option<Vec<(PathBuf, String, Vec<u8>)>>> {
    if std::env::var("EDGION_TEST_K8S_MODE").is_ok_and(|mode| matches!(mode.as_str(), "true" | "1" | "yes")) {
        return Ok(None);
    }
    let work = PathBuf::from(
        std::env::var("EDGION_WORK_DIR").context("shared topology must expose EDGION_WORK_DIR for PID checks")?,
    );
    let mut result = Vec::new();
    for role in ["controller", "gateway"] {
        let path = work.join(format!("pids/{role}.pid"));
        let pid = tokio::fs::read_to_string(&path).await?;
        ensure!(pid.trim().parse::<u32>().is_ok(), "invalid shared process PID");
        let output = tokio::process::Command::new("ps")
            .args(["-p", pid.trim(), "-o", "lstart="])
            .output()
            .await?;
        ensure!(
            output.status.success() && !output.stdout.is_empty(),
            "shared {role} process is not alive"
        );
        result.push((path, pid, output.stdout));
    }
    Ok(Some(result))
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        if !crate::suites::tls_route::tls_route_terminate_tests_enabled() {
            crate::suites::tls_route::report_terminate_deferred(
                "PluginPolicyDynamic/TLS terminate admission across both classes and all phases",
            );
        }
        let before = process_identity().await?;
        let result = async {
            // The shared harness preloads listeners before Gateway startup.
            // Reset policy and sources in place; listener recreation is a separate lifecycle.
            // Reserve all source ports before any admission. Holding these sockets
            // prevents the OS from reusing a previous phase's established session.
            let established_udp = udp_socket(&ctx.test_ctx, 58338).await?;
            let blocked_udp = udp_socket(&ctx.test_ctx, 58338).await?;
            let changed_udp = udp_socket(&ctx.test_ctx, 58338).await?;
            let bypassed_udp = udp_socket(&ctx.test_ctx, 58338).await?;
            ctx.apply_dir("initial").await?;
            phase(&ctx, "initial").await?;
            udp_echo(&established_udp, true).await?;
            {
                let transitions = async {
                    policy(&ctx, Some("block"), 451).await?;
                    phase(&ctx, "block").await?;
                    udp_echo(&blocked_udp, false).await?;
                    policy(&ctx, Some("block"), 429).await?;
                    phase(&ctx, "status-change").await?;
                    udp_echo(&changed_udp, false).await?;
                    policy(&ctx, Some("bypass"), 503).await?;
                    phase(&ctx, "bypass").await?;
                    udp_echo(&bypassed_udp, true).await
                };
                tokio::pin!(transitions);
                let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
                loop {
                    tokio::select! {
                        result = &mut transitions => { result?; break; }
                        _ = heartbeat.tick() => udp_echo(&established_udp, true).await?,
                    }
                }
            }
            udp_echo(&established_udp, true).await?;
            drop((established_udp, blocked_udp, changed_udp, bypassed_udp));
            policy(&ctx, None, 503).await?;
            phase(&ctx, "relaxed").await?;

            // Both classes retain real valid generations before malformed input.
            // These Admin API updates are deltas. Full/relist behavior is covered
            // by the publisher's production unit tests, without process restarts.
            invalid_sources(&ctx).await?;
            policy(&ctx, Some("block"), 451).await?;
            phase(&ctx, "invalid-block").await?;
            policy(&ctx, Some("bypass"), 503).await?;
            phase(&ctx, "invalid-bypass").await?;
            ctx.apply_dir("initial/EdgionPlugins_pp-dsl.yaml").await?;
            ctx.apply_dir("initial/EdgionConfigData_pp-waf-policy.yaml").await?;
            ctx.apply_dir("initial/EdgionPlugins_pp-waf.yaml").await?;
            policy(&ctx, Some("block"), 451).await?;
            phase(&ctx, "recovered").await?;

            let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
            client
                .delete("EdgionGatewayConfig", None, "pp-a")
                .await?
                .error_for_status()?;
            phase(&ctx, "missing-egc").await?;
            policy(&ctx, Some("block"), 451).await?;
            phase(&ctx, "recreated-egc").await?;
            let mut class: serde_json::Value =
                yaml_serde::from_str(&tokio::fs::read_to_string(ctx.resolve("initial/GatewayClass_pp-a.yaml")).await?)?;
            class["spec"]["parametersRef"]["name"] = json!("pp-b");
            apply_value(&ctx, class).await?;
            phase(&ctx, "rebound").await?;
            ctx.apply_dir("initial/GatewayClass_pp-a.yaml").await?;
            phase(&ctx, "binding-restored").await
        }
        .await;
        let cleaned = cleanup(&ctx).await;
        let identity = process_identity().await.and_then(|after| {
            ensure!(
                before == after,
                "Controller or Gateway process identity changed during policy edits"
            );
            Ok(())
        });
        result.and(cleaned).and(identity)
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "PluginPolicyDynamic",
    conf_path: "PluginPolicy/DynamicTest",
    run,
    phases: PHASES,
};

#[cfg(test)]
mod tests {
    #[test]
    fn invalid_source_targets_match_current_fixtures() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("integration/conf/PluginPolicy/DynamicTest/initial");
        for (file, pointer, invalid) in super::INVALID_SOURCE_TARGETS {
            let input = std::fs::read_to_string(root.join(file)).unwrap();
            let original: serde_json::Value = yaml_serde::from_str(&input).unwrap();
            let changed = super::invalidate_source(&input, pointer, invalid).unwrap();
            assert_ne!(original.pointer(pointer), changed.pointer(pointer), "{file}");
            assert_eq!(
                changed.pointer(pointer).and_then(serde_json::Value::as_str),
                Some(*invalid)
            );
        }
        assert!(super::invalidate_source("{}", "/missing/0", "invalid").is_err());
    }

    #[test]
    fn phase_fixtures_use_real_schema_validation() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("integration/conf/PluginPolicy/DynamicTest/initial");
        for name in ["mock", "dsl", "waf", "response", "body", "async"] {
            let input = std::fs::read_to_string(root.join(format!("EdgionPlugins_pp-{name}.yaml"))).unwrap();
            let mut resource: edgion_resources::EdgionPlugins = yaml_serde::from_str(&input).unwrap();
            if name == "waf" {
                use edgion_resources::resources::edgion_config_data::{WafPolicyData, WafRuleBundleData};
                use edgion_resources::resources::edgion_plugins::plugin_configs::ResolvedWafBundle;

                let read_fixture = |name: &str| -> serde_json::Value {
                    yaml_serde::from_str(&std::fs::read_to_string(root.join(name)).unwrap()).unwrap()
                };
                let policy = read_fixture("EdgionConfigData_pp-waf-policy.yaml");
                let bundle = read_fixture("EdgionConfigData_pp-waf-rules.yaml");
                let namespace = resource.metadata.namespace.as_deref().unwrap();
                let waf = resource.spec.waf.as_mut().unwrap();
                assert_eq!(waf.policy_ref.name, policy["metadata"]["name"].as_str().unwrap());
                assert_eq!(waf.policy_ref.namespace.as_deref().unwrap_or(namespace), namespace);
                assert_eq!(policy["metadata"]["namespace"], namespace);
                assert_eq!(policy["spec"]["data"]["type"], "WafPolicy");
                assert_eq!(bundle["spec"]["data"]["type"], "WafRuleBundle");
                let policy: WafPolicyData = serde_json::from_value(policy["spec"]["data"]["config"].clone()).unwrap();
                assert_eq!(policy.default_profile, "baseline");
                assert!(waf.active_profile.is_none());
                assert!(waf.active_profile_ref.is_none());
                let references = &policy.profiles[&policy.default_profile].bundle_refs;
                assert_eq!(references.len(), 1);
                let reference = &references[0];
                assert!(!reference.optional);
                assert_eq!(reference.target.name, bundle["metadata"]["name"].as_str().unwrap());
                assert_eq!(reference.target.namespace.as_deref().unwrap_or(namespace), namespace);
                assert_eq!(bundle["metadata"]["namespace"], namespace);
                let data: WafRuleBundleData = serde_json::from_value(bundle["spec"]["data"]["config"].clone()).unwrap();
                let digest = data.compute_digest();
                assert_eq!(data.digest.as_deref(), Some(digest.as_str()));
                assert_eq!(reference.expected_digest.as_deref(), Some(digest.as_str()));
                // Schema validation consumes the Controller-resolved projection,
                // so resolve the real sibling fixtures rather than inline rules.
                waf.owner_namespace = Some(namespace.into());
                waf.selected_profile = Some(policy.default_profile.clone());
                waf.resolved_ref_indices = vec![0];
                waf.resolved_bundles = Some(vec![ResolvedWafBundle {
                    identity: format!("{namespace}/{}", reference.target.name),
                    digest,
                    data,
                }]);
                waf.resolved_policy = Some(policy);
            }
            let errors = resource.validate_plugins();
            if name == "async" {
                assert!(
                    !errors.is_empty(),
                    "the unsupported fourth-stage control must be invalid"
                );
            } else {
                assert!(errors.is_empty(), "{name}: {errors:?}");
            }
        }
    }
}
