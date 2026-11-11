// DnsResolutionFlap dynamic scenario.
//
// Proves an ExternalName Service's DNS resolver is hot-reloadable through the
// Gateway endpoint table: flipping the `edgion.io/dns-resolver-ref` annotation
// or updating the referenced LinkSys at runtime re-keys the table entry and
// takes effect without a restart.
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

fn family_v4() -> Box<dyn TestSuite> {
    Box::new(suites::DnsFlapFamilyTestSuite(false))
}
fn family_v6() -> Box<dyn TestSuite> {
    Box::new(suites::DnsFlapFamilyTestSuite(true))
}
fn family_all() -> Box<dyn TestSuite> {
    Box::new(suites::DnsFlapDualStackTestSuite)
}

/// Wait until the Gateway itself holds the flap Gateway's family annotation:
/// a phase that checks the annotation has no effect would otherwise pass on
/// the previous state before the change arrived.
async fn wait_gateway_family_annotation(ctx: &ScenarioCtx, value: &str) -> Result<()> {
    let token = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty());
    let url = format!(
        "{}/configclient/gateway?name=dns-flap-gateway&namespace=edgion-test",
        ctx.gateway_url.trim_end_matches('/')
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let mut request = ctx.test_ctx.http_client.get(&url).timeout(Duration::from_secs(2));
        if let Some(token) = &token {
            request = request.bearer_auth(token);
        }
        if let Ok(response) = request.send().await {
            if let Ok(body) = response.json::<serde_json::Value>().await {
                let observed = body
                    .pointer("/data/metadata/annotations/edgion.io~1dns-lookup-family")
                    .and_then(serde_json::Value::as_str);
                if observed == Some(value) {
                    return Ok(());
                }
            }
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "the Gateway never observed dns-lookup-family={value}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "family-v4",
        factory: family_v4,
    },
    PhaseSuite {
        name: "family-v6",
        factory: family_v6,
    },
    PhaseSuite {
        name: "family-all",
        factory: family_all,
    },
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
        let result = async {
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
            // The local backend has both loopback listeners; Kubernetes has a separate driver.
            if std::env::var("EDGION_TEST_K8S_MODE").as_deref() != Ok("true") {
                for (update, phase) in [
                    ("family_default", "family-v4"),
                    ("family_service_v6", "family-v6"),
                    // Every answer address (IPv4 and IPv6) is a load-balanced endpoint.
                    ("family_service_all", "family-all"),
                    // Observe inherited IPv4 before checking the Gateway annotation;
                    // an old Service IPv6 snapshot cannot satisfy the next phase.
                    ("family_default", "family-v4"),
                    // A Gateway-level family annotation does not apply to
                    // ExternalName Services (Service -> EGC -> ipv4Only only).
                    ("family_gateway_v6", "family-v4"),
                    ("family_service_v4", "family-v4"),
                    ("family_egc_v6", "family-v6"),
                    ("family_egc_v4", "family-v4"),
                    ("family_invalid", "failing"),
                    ("family_restore", "family-v4"),
                ] {
                    ctx.apply_dir(&format!("updates/{update}")).await?;
                    if update == "family_gateway_v6" {
                        wait_gateway_family_annotation(&ctx, "ipv6Only").await?;
                    }
                    ctx.wait_phase(scenario, phase, Duration::from_secs(15)).await?;
                }
            } else {
                println!("DEFERRED: DNS family/cache phases require the local dual-stack backend; Kubernetes keeps its existing DNS recovery phases");
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let mut errors = Vec::new();
        for fixture in [
            "00_EdgionGatewayConfig_dns-flap-egc.yaml",
            "02_Gateway_edgion-test_dns-flap-gateway.yaml",
            "01_Service_edgion-default_dns-flap.yaml",
            "00_LinkSys_edgion-default_dns-flap-local.yaml",
        ] {
            if let Err(error) = ctx.apply_dir(&format!("initial/{fixture}")).await {
                errors.push(format!("restore {fixture}: {error:#}"));
            }
        }
        if errors.is_empty() {
            if let Err(error) = ctx.wait_phase(scenario, "failing", Duration::from_secs(15)).await {
                errors.push(format!("verify restored state: {error:#}"));
            }
        }
        match (result, errors.is_empty()) {
            (Ok(()), true) => Ok(()),
            (Err(error), true) => Err(error),
            (result, false) => anyhow::bail!("transition: {result:?}; restoration: {}", errors.join("; ")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "DnsResolutionFlap",
    conf_path: "HTTPRoute/Backend/DnsResolutionFlap/DynamicTest",
    run,
    phases: PHASES,
};
