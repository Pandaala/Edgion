use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::TestSuite;
use crate::suites;
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
        let restored = async {
            ctx.apply_dir("initial/00_EdgionConfigData_global-realip-proxies.yaml")
                .await?;
            ctx.apply_dir("initial/00_EdgionConfigData_realip-primary.yaml").await?;
            ctx.apply_dir("initial/00_EdgionConfigData_realip-secondary.yaml")
                .await?;
            ctx.apply_dir("initial/00_EdgionConfigData_condition-refs-live-ips.yaml")
                .await?;
            ctx.apply_dir("initial/00_Secret_condition-refs-dynamic.yaml").await
        }
        .await;
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
