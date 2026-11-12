//! Shared HTTP policy conflicts preserve healthy listeners and recover via requeue.
use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::port_config::{PortConfig, PortKind};
use anyhow::{ensure, Context, Result};
use base64::Engine;
use serde_json::Value;
use std::time::{Duration, Instant};

const NAMESPACE: &str = "edgion-test";
const CONVERGENCE: Duration = Duration::from_secs(20);
#[derive(Clone, Copy)]
enum Phase {
    Compatible,
    Rejected,
    Released,
}

async fn probe(ctx: &TestContext, host: &str, port: u16, expected: u16) -> Result<()> {
    let response = ctx
        .http_client
        .get(format!("http://{}:{port}/health", ctx.target_host))
        .header("Host", format!("shared-policy-{host}.sandbox.example.com"))
        .timeout(Duration::from_secs(3))
        .send()
        .await?;
    ensure!(
        response.status().as_u16() == expected,
        "Gateway {host} port {port}: expected {expected}, got {}",
        response.status()
    );
    Ok(())
}

async fn assert_phase(ctx: &TestContext, phase: Phase) -> Result<()> {
    let ports = PortConfig::load().map_err(anyhow::Error::msg)?;
    let shared = ports
        .require_port("Gateway/SharedHttpPolicyAdmission", PortKind::Http)
        .map_err(anyhow::Error::msg)?;
    let healthy = ports
        .require_port("Gateway/SharedHttpPolicyAdmission/Healthy", PortKind::Http)
        .map_err(anyhow::Error::msg)?;
    let rejected = matches!(phase, Phase::Rejected);
    if !matches!(phase, Phase::Released) {
        probe(ctx, "a", shared, 200).await?;
    }
    probe(ctx, "b", shared, if rejected { 404 } else { 200 }).await?;
    probe(ctx, "b", healthy, 200).await?;
    // The data-plane projection is exactly the Controller status shipped by conf_sync.
    let gateway_url =
        std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL").unwrap_or_else(|_| format!("http://{}:12001", ctx.target_host));
    let url = format!("{}/configclient/Gateway/list", gateway_url.trim_end_matches('/'));
    let mut request = ctx.http_client.get(url).timeout(Duration::from_secs(3));
    if let Some(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN")
        .ok()
        .or_else(|| ctx.admin_token.clone())
    {
        request = request.bearer_auth(token);
    }
    let body: Value = request.send().await?.error_for_status()?.json().await?;
    let gateway = body["data"]
        .as_array()
        .context("missing Gateway list")?
        .iter()
        .find(|gateway| {
            gateway["metadata"]["namespace"] == NAMESPACE && gateway["metadata"]["name"] == "shared-policy-b"
        })
        .context("missing B Gateway")?;
    let listeners = gateway["spec"]["currentStatus"]["listeners"]
        .as_array()
        .context("missing published listener status")?;
    let shared_status = listeners
        .iter()
        .find(|listener| listener["name"] == "shared")
        .context("missing shared listener")?;
    let conditions = shared_status["conditions"]
        .as_array()
        .context("missing listener conditions")?;
    let accepted = conditions
        .iter()
        .find(|condition| condition["type"] == "Accepted")
        .context("missing Accepted condition")?;
    ensure!(
        accepted["status"] == if rejected { "False" } else { "True" },
        "unexpected admission: {accepted}"
    );
    if rejected {
        ensure!(
            accepted["reason"] == "SharedListenerPolicyConflict",
            "unexpected rejection: {accepted}"
        );
        ensure!(
            accepted["message"]
                .as_str()
                .unwrap_or_default()
                .contains("shared-policy-a"),
            "missing blocker identity"
        );
        ensure!(
            conditions
                .iter()
                .any(|condition| condition["type"] == "Programmed" && condition["status"] == "False"),
            "rejected listener is programmed"
        );
    }
    // Listener admission only changes traffic ownership. Ordinary Route
    // acceptance and parsed attachments remain valid throughout these phases.
    let url = format!("{}/configclient/HTTPRoute/list", gateway_url.trim_end_matches('/'));
    let mut request = ctx.http_client.get(url).timeout(Duration::from_secs(3));
    if let Some(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN")
        .ok()
        .or_else(|| ctx.admin_token.clone())
    {
        request = request.bearer_auth(token);
    }
    let body: Value = request.send().await?.error_for_status()?.json().await?;
    let route = body["data"]
        .as_array()
        .context("missing HTTPRoute list")?
        .iter()
        .find(|route| route["metadata"]["namespace"] == NAMESPACE && route["metadata"]["name"] == "shared-policy-b")
        .context("missing B Route")?;
    ensure!(
        route["spec"]["resolvedListeners"]
            .as_array()
            .is_some_and(|keys| keys.len() == 2),
        "listener admission changed ordinary Route attachments"
    );
    ensure!(
        route["spec"]["currentStatus"]["parents"]
            .as_array()
            .is_some_and(|parents| {
                parents.iter().any(|parent| {
                    parent["conditions"].as_array().is_some_and(|conditions| {
                        conditions
                            .iter()
                            .any(|condition| condition["type"] == "Accepted" && condition["status"] == "True")
                    })
                })
            }),
        "listener admission rejected the ordinary B Route"
    );
    Ok(())
}
struct AdmissionSuite(Phase);
impl TestSuite for AdmissionSuite {
    fn name(&self) -> &str {
        "SharedHttpPolicyAdmissionPhase"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.0;
        vec![TestCase::new(
            "shared_http_admission",
            "Conflicts withdraw only affected listeners and dependency requeue recovers them",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match assert_phase(&ctx, phase).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(15))]
    }
}
fn compatible() -> Box<dyn TestSuite> {
    Box::new(AdmissionSuite(Phase::Compatible))
}
fn rejected() -> Box<dyn TestSuite> {
    Box::new(AdmissionSuite(Phase::Rejected))
}
fn released() -> Box<dyn TestSuite> {
    Box::new(AdmissionSuite(Phase::Released))
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "compatible",
        factory: compatible,
    },
    PhaseSuite {
        name: "rejected",
        factory: rejected,
    },
    PhaseSuite {
        name: "released",
        factory: released,
    },
];
async fn restore(ctx: &ScenarioCtx) -> Result<()> {
    // `bootstrap` owns Gateways A and B; this copy only restores them (kept
    // outside the initial fixtures, so every object has one initial owner).
    ctx.apply_dir("updates/restore").await?;
    ctx.wait_phase(SCENARIO, "compatible", CONVERGENCE).await
}
async fn recover_initially_missing_certificate(ctx: &ScenarioCtx) -> Result<()> {
    const CERT: &str = "shared-policy-recovery-cert";
    const HOST: &str = "gateway-tls.test.com";
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    ensure!(
        client.get("Secret", Some(NAMESPACE), CERT).await?.status().as_u16() == 404,
        "certificate Secret must be absent when the bootstrap listener starts"
    );
    let port = PortConfig::load()
        .map_err(anyhow::Error::msg)?
        .require_port("Gateway/SharedHttpPolicyAdmission/SecretRecovery", PortKind::Https)
        .map_err(anyhow::Error::msg)?;
    let address = tokio::net::lookup_host((ctx.test_ctx.target_host.as_str(), port))
        .await?
        .next()
        .context("Gateway host has no address")?;
    let https = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .resolve(HOST, address)
        .timeout(Duration::from_secs(3))
        .build()?;
    let url = format!("https://{HOST}:{port}/health");
    let deadline = Instant::now() + CONVERGENCE;
    let status_url = format!(
        "{}/configserver/Gateway?namespace={NAMESPACE}&name=shared-policy-tls",
        ctx.controller_url.trim_end_matches('/')
    );
    // The listener existed in bootstrap with an absent Secret. The socket must
    // remain available while native status and the handshake temporarily reject.
    loop {
        let mut request = ctx
            .test_ctx
            .http_client
            .get(&status_url)
            .timeout(Duration::from_secs(3));
        if let Some(token) = std::env::var("EDGION_ADMIN_TOKEN")
            .ok()
            .or_else(|| ctx.test_ctx.admin_token.clone())
        {
            request = request.bearer_auth(token);
        }
        let body: Value = request.send().await?.error_for_status()?.json().await?;
        let not_programmed = body["data"]["spec"]["currentStatus"]["listeners"]
            .as_array()
            .is_some_and(|listeners| {
                listeners.iter().any(|listener| {
                    listener["name"] == "https"
                        && listener["conditions"].as_array().is_some_and(|conditions| {
                            conditions
                                .iter()
                                .any(|condition| condition["type"] == "Programmed" && condition["status"] == "False")
                        })
                })
            });
        if not_programmed && https.get(&url).send().await.is_err() {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "missing certificate did not reject its native listener"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let local_process = if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
        ensure!(
            std::env::var("EDGION_TEST_GATEWAY_RESTART_GUARD").as_deref() == Ok("true"),
            "Kubernetes certificate recovery requires the runner Gateway restart guard"
        );
        None
    } else {
        let work_dir = std::env::var("EDGION_WORK_DIR").context("missing integration work directory")?;
        let pid_path = std::path::Path::new(&work_dir).join("pids/gateway.pid");
        let pid_before = tokio::fs::read_to_string(&pid_path).await?;
        Some((pid_path, pid_before))
    };
    let recovery = async {
        // Generate an owned certificate in either topology; never read redacted
        // Admin output or depend on a host-local generated fixture.
        let certificate = rcgen::generate_simple_self_signed(vec!["gateway-tls.test.com".into()])?;
        let secret = serde_json::json!({
            "apiVersion": "v1", "kind": "Secret",
            "metadata": {"name": CERT, "namespace": NAMESPACE},
            "type": "kubernetes.io/tls",
            "data": {
                "tls.crt": base64::engine::general_purpose::STANDARD.encode(certificate.cert.pem()),
                "tls.key": base64::engine::general_purpose::STANDARD.encode(certificate.signing_key.serialize_pem())
            }
        });
        let response = client
            .create("Secret", Some(NAMESPACE), yaml_serde::to_string(&secret)?)
            .await?;
        ensure!(response.status().is_success(), "certificate Secret creation failed");
        let deadline = Instant::now() + CONVERGENCE;
        loop {
            if https
                .get(&url)
                .send()
                .await
                .is_ok_and(|response| response.status().as_u16() == 200)
            {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "same-port HTTPS did not recover after Secret arrival"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if let Some((pid_path, pid_before)) = &local_process {
            ensure!(
                tokio::fs::read_to_string(pid_path).await? == *pid_before,
                "certificate recovery restarted the Gateway"
            );
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let cleanup = client.delete("Secret", Some(NAMESPACE), CERT).await?;
    ensure!(
        cleanup.status().is_success() || cleanup.status().as_u16() == 404,
        "temporary certificate Secret deletion failed"
    );
    recovery
}
fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            recover_initially_missing_certificate(&ctx).await?;
            restore(&ctx).await?;
            ctx.apply_dir("updates").await?;
            ctx.wait_phase(SCENARIO, "rejected", CONVERGENCE).await?;
            restore(&ctx).await?;
            ctx.apply_dir("updates").await?;
            ctx.wait_phase(SCENARIO, "rejected", CONVERGENCE).await?;
            let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
            ensure!(
                client
                    .delete("Gateway", Some(NAMESPACE), "shared-policy-a")
                    .await?
                    .status()
                    .is_success(),
                "blocker deletion failed"
            );
            ctx.wait_phase(SCENARIO, "released", CONVERGENCE).await
        }
        .await;
        let restored = restore(&ctx).await;
        match (result, restored) {
            (Ok(()), restored) => restored,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(restore)) => Err(anyhow::anyhow!("{error:#}; restore failed: {restore:#}")),
        }
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "SharedHttpPolicyAdmission",
    conf_path: "Gateway/SharedHttpPolicyAdmission/DynamicTest",
    run,
    phases: PHASES,
};
