use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites;
use anyhow::{ensure, Context, Result};
use std::time::Duration;

fn initial() -> Box<dyn TestSuite> {
    Box::new(suites::ConditionRefsDynamicInitialTestSuite)
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(suites::ConditionRefsDynamicUpdatedTestSuite)
}

fn deleted() -> Box<dyn TestSuite> {
    Box::new(suites::ConditionRefsDynamicDeletedTestSuite)
}

fn ip_initial() -> Box<dyn TestSuite> {
    Box::new(suites::ConditionRefsLiveIpInitialTestSuite)
}
fn ip_updated() -> Box<dyn TestSuite> {
    Box::new(suites::ConditionRefsLiveIpUpdatedTestSuite)
}
fn ip_deleted() -> Box<dyn TestSuite> {
    Box::new(suites::ConditionRefsLiveIpDeletedTestSuite)
}

fn real_ip_trusted() -> Box<dyn TestSuite> {
    Box::new(suites::ConditionRefsRealIpTrustedTestSuite)
}
fn real_ip_plugin_fallback() -> Box<dyn TestSuite> {
    Box::new(suites::ConditionRefsRealIpPluginFallbackTestSuite)
}
fn real_ip_global_fallback() -> Box<dyn TestSuite> {
    Box::new(suites::ConditionRefsRealIpGlobalFallbackTestSuite)
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "secondary-restored",
        factory: secondary_restored,
    },
    PhaseSuite {
        name: "realip-trusted",
        factory: real_ip_trusted,
    },
    PhaseSuite {
        name: "realip-plugin-fallback",
        factory: real_ip_plugin_fallback,
    },
    PhaseSuite {
        name: "realip-global-fallback",
        factory: real_ip_global_fallback,
    },
    PhaseSuite {
        name: "ip-initial",
        factory: ip_initial,
    },
    PhaseSuite {
        name: "ip-updated",
        factory: ip_updated,
    },
    PhaseSuite {
        name: "ip-deleted",
        factory: ip_deleted,
    },
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
    },
    PhaseSuite {
        name: "deleted",
        factory: deleted,
    },
];

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            let scenario = SCENARIO;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await?;
            ctx.wait_phase(scenario, "ip-initial", Duration::from_secs(15)).await?;
            ctx.apply_dir("updates/01_EdgionConfigData_condition-refs-live-ips.yaml")
                .await?;
            ctx.wait_phase(scenario, "ip-updated", Duration::from_secs(15)).await?;
            ctx.delete_resources("delete/ip-list.txt").await?;
            ctx.wait_phase(scenario, "ip-deleted", Duration::from_secs(15)).await?;
            ctx.apply_dir("initial/00_EdgionConfigData_condition-refs-live-ips.yaml")
                .await?;
            ctx.wait_phase(scenario, "ip-initial", Duration::from_secs(15)).await?;

            ctx.wait_phase(scenario, "realip-trusted", Duration::from_secs(15))
                .await?;
            ctx.apply_dir("updates/02_EdgionConfigData_realip-primary.yaml").await?;
            ctx.wait_phase(scenario, "realip-plugin-fallback", Duration::from_secs(15))
                .await?;
            ctx.apply_dir("updates/03_EdgionConfigData_realip-secondary.yaml")
                .await?;
            ctx.wait_phase(scenario, "realip-trusted", Duration::from_secs(15))
                .await?;
            ctx.delete_resources("delete/realip-primary.txt").await?;
            ctx.wait_phase(scenario, "realip-plugin-fallback", Duration::from_secs(15))
                .await?;
            ctx.apply_dir("initial/00_EdgionConfigData_realip-primary.yaml").await?;
            ctx.wait_phase(scenario, "realip-trusted", Duration::from_secs(15))
                .await?;
            for mutation in [
                "updates/04_EdgionConfigData_global-realip-untrusted.yaml",
                "updates/05_EdgionConfigData_global-realip-private.yaml",
            ] {
                ctx.apply_dir(mutation).await?;
                ctx.wait_phase(scenario, "realip-global-fallback", Duration::from_secs(15))
                    .await?;
                ctx.apply_dir("initial/00_EdgionConfigData_global-realip-proxies.yaml")
                    .await?;
                ctx.wait_phase(scenario, "realip-trusted", Duration::from_secs(15))
                    .await?;
            }
            ctx.delete_resources("delete/global-realip.txt").await?;
            ctx.wait_phase(scenario, "realip-global-fallback", Duration::from_secs(15))
                .await?;
            ctx.apply_dir("initial/00_EdgionConfigData_global-realip-proxies.yaml")
                .await?;
            ctx.wait_phase(scenario, "realip-trusted", Duration::from_secs(15))
                .await?;

            ctx.apply_dir("updates/00_Secret_condition-refs-dynamic.yaml").await?;
            ctx.wait_phase(scenario, "updated", Duration::from_secs(15)).await?;

            ctx.delete_resources("delete/secret.txt").await?;
            ctx.wait_phase(scenario, "deleted", Duration::from_secs(15)).await?;

            ctx.apply_dir("initial/00_Secret_condition-refs-dynamic.yaml").await?;
            ctx.wait_phase(scenario, "initial", Duration::from_secs(15)).await
        }
        .await;
        let mut errors = Vec::new();
        let mut restored_files = [false; 5];
        for (index, fixture) in [
            "initial/00_EdgionConfigData_global-realip-proxies.yaml",
            "initial/00_EdgionConfigData_realip-primary.yaml",
            "initial/00_EdgionConfigData_realip-secondary.yaml",
            "initial/00_EdgionConfigData_condition-refs-live-ips.yaml",
            "initial/00_Secret_condition-refs-dynamic.yaml",
        ]
        .into_iter()
        .enumerate()
        {
            match ctx.apply_dir(fixture).await {
                Ok(()) => restored_files[index] = true,
                Err(error) => errors.push(format!("restore {fixture}: {error:#}")),
            }
        }
        for (ready, phase) in [
            (restored_files[4], "initial"),
            (restored_files[3], "ip-initial"),
            (restored_files[..3].iter().all(|restored| *restored), "realip-trusted"),
            (restored_files[2], "secondary-restored"),
        ] {
            if ready {
                if let Err(error) = ctx.wait_phase(SCENARIO, phase, Duration::from_secs(15)).await {
                    errors.push(format!("verify {phase}: {error:#}"));
                }
            }
        }
        let restored = if errors.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(errors.join("; ")))
        };
        match (result, restored) {
            (Ok(()), restored) => restored,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(restored)) => Err(anyhow::anyhow!("{error:#}; fixture restoration failed: {restored:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "ConditionRefsDynamic",
    conf_path: "EdgionPlugins/ConditionRefsDynamic/DynamicTest",
    run,
    phases: PHASES,
};

struct SecondaryRestoredSuite;

fn secondary_restored() -> Box<dyn TestSuite> {
    Box::new(SecondaryRestoredSuite)
}

fn secondary_spec_restored(resource: &serde_json::Value) -> bool {
    let expected: serde_yaml::Value = serde_yaml::from_str(include_str!(
        "../../../../integration/conf/EdgionPlugins/ConditionRefsDynamic/DynamicTest/initial/00_EdgionConfigData_realip-secondary.yaml"
    )).expect("tracked initial secondary fixture is valid YAML");
    let expected = serde_json::to_value(expected).expect("fixture contains JSON-compatible YAML");
    let Some(actual) = resource.get("spec").and_then(serde_json::Value::as_object) else {
        return false;
    };
    let mut actual = actual.clone();
    let mut expected = expected["spec"]
        .as_object()
        .expect("tracked fixture has an object spec")
        .clone();
    // CRD defaulting may materialize enable, and currentStatus is controller-owned.
    actual.remove("currentStatus");
    actual.entry("enable").or_insert(serde_json::Value::Bool(true));
    expected.entry("enable").or_insert(serde_json::Value::Bool(true));
    actual == expected
}

async fn check_secondary_restored(ctx: &TestContext) -> Result<()> {
    let mut request = ctx.http_client.get(format!(
        "{}/api/v1/namespaced/edgionconfigdata/edgion-default/realip-secondary",
        ctx.admin_api_url()
    ));
    if let Some(token) = &ctx.admin_token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.context("read restored secondary list")?;
    ensure!(
        response.status().is_success(),
        "restored secondary list read failed: {}",
        response.status()
    );
    let resource: serde_json::Value = response.json().await.context("decode restored secondary list")?;
    ensure!(
        secondary_spec_restored(&resource),
        "secondary list has not returned to its original authored spec"
    );
    Ok(())
}

impl TestSuite for SecondaryRestoredSuite {
    fn name(&self) -> &str {
        "Condition References Secondary Restoration"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "condition_refs_secondary_restored",
            "Original secondary list is not masked by primary trust",
            |ctx| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    match check_secondary_restored(&ctx).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )]
    }
}

#[cfg(test)]
mod restoration_spec_tests {
    use super::*;
    #[test]
    fn secondary_restoration_rejects_masked_updated_and_missing_spec() {
        let initial: serde_yaml::Value = serde_yaml::from_str(include_str!("../../../../integration/conf/EdgionPlugins/ConditionRefsDynamic/DynamicTest/initial/00_EdgionConfigData_realip-secondary.yaml")).unwrap();
        let updated: serde_yaml::Value = serde_yaml::from_str(include_str!("../../../../integration/conf/EdgionPlugins/ConditionRefsDynamic/DynamicTest/updates/03_EdgionConfigData_realip-secondary.yaml")).unwrap();
        let mut initial = serde_json::to_value(initial).unwrap();
        assert!(secondary_spec_restored(&initial));
        initial["spec"]["enable"] = serde_json::json!(true);
        initial["spec"]["currentStatus"] = serde_json::json!({"conditions": []});
        assert!(secondary_spec_restored(&initial));
        for invalid in [
            serde_json::json!(false),
            serde_json::Value::Null,
            serde_json::json!("true"),
        ] {
            initial["spec"]["enable"] = invalid;
            assert!(!secondary_spec_restored(&initial));
        }
        assert!(!secondary_spec_restored(&serde_json::to_value(updated).unwrap()));
        assert!(!secondary_spec_restored(&serde_json::json!({})));
        assert!(!secondary_spec_restored(&serde_json::json!({"spec": null})));
    }
}
