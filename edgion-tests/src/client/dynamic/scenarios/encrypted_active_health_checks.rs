//! Independent encrypted probes against the shared local TLS fixture listeners.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant};

const NAME: &str = "encrypted-active-health";
const HOST: &str = "encrypted-active-health.example.com";
const NS: &str = "edgion-test";
const CA: &str = "encrypted-active-health-ca";
const CLIENT: &str = "encrypted-active-health-client";

struct TrafficSuite(u16);

impl TestSuite for TrafficSuite {
    fn name(&self) -> &str {
        "EncryptedActiveHealthChecks"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let expected = self.0;
        vec![TestCase::new(
            "active_health_selection",
            "Observe stable health-filtered application traffic across probe generations",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        // Observe at least two full one-second probe intervals. A transient
                        // default-healthy snapshot must not satisfy a successful phase.
                        for observation in 0..4 {
                            let response = ctx
                                .http_client
                                .get(format!("{}/echo", ctx.http_url()))
                                .header("host", HOST)
                                .timeout(Duration::from_secs(2))
                                .send()
                                .await?;
                            ensure!(
                                response.status().as_u16() == expected,
                                "expected {expected}, got {}",
                                response.status()
                            );
                            if expected == 200 {
                                ensure!(
                                    response.text().await?.starts_with("Server: 0.0.0.0:30001\n"),
                                    "application traffic must remain on its independent plaintext endpoint"
                                );
                            }
                            if observation < 3 {
                                tokio::time::sleep(Duration::from_secs(1)).await;
                            }
                        }
                        Ok::<_, anyhow::Error>(())
                    }
                    .await;
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(15))]
    }
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(TrafficSuite(404))
}
fn healthy() -> Box<dyn TestSuite> {
    Box::new(TrafficSuite(200))
}
fn unhealthy() -> Box<dyn TestSuite> {
    Box::new(TrafficSuite(503))
}
fn traffic_tls_failed() -> Box<dyn TestSuite> {
    Box::new(TrafficSuite(502))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "absent",
        factory: absent,
    },
    PhaseSuite {
        name: "healthy",
        factory: healthy,
    },
    PhaseSuite {
        name: "unhealthy",
        factory: unhealthy,
    },
    PhaseSuite {
        name: "traffic_tls_failed",
        factory: traffic_tls_failed,
    },
];

async fn phase(ctx: &ScenarioCtx, name: &str) -> Result<()> {
    ctx.wait_phase(SCENARIO, name, Duration::from_secs(30)).await
}

async fn apply(ctx: &ScenarioCtx, document: Value) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    crate::dynamic::apply::apply_yaml_documents(
        &client,
        Path::new("encrypted-active-health-runtime.yaml"),
        &serde_yaml::to_string(&document)?,
    )
    .await
}

fn policy(protocol: &str, port: u16, mtls: bool) -> Value {
    let mut active = json!({"type": protocol, "port": port, "path": "/health",
        "interval": "1s", "timeout": "1s", "healthyThreshold": 1, "unhealthyThreshold": 1,
        "expectedStatuses": [200]});
    if matches!(protocol, "https" | "grpcs") {
        active["tls"] = json!({"validation": {"hostname": "backend.example.com",
            "caCertificateRefs": [{"kind": "Secret", "name": CA}]}});
        if mtls {
            active["tls"]["clientCertificateRef"] = json!({"name": CLIENT});
        }
    }
    json!({"apiVersion":"edgion.io/v1", "kind":"EdgionBackendTrafficPolicy",
        "metadata":{"name":NAME,"namespace":NS},
        "spec":{"targetRefs":[{"kind":"Service","name":NAME}], "healthCheck":{"active":active}}})
}

async fn delete(ctx: &ScenarioCtx, kind: &str, name: &str) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let response = client.delete(kind, Some(NS), name).await?;
    ensure!(
        response.status().is_success() || response.status().as_u16() == 404,
        "cleanup {kind}/{name}: HTTP {}",
        response.status()
    );
    Ok(())
}

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let mut failures = Vec::new();
    for (kind, name) in [
        ("HTTPRoute", NAME),
        ("EdgionBackendTrafficPolicy", NAME),
        ("BackendTLSPolicy", NAME),
        ("EndpointSlice", NAME),
        ("Service", NAME),
        ("Secret", CA),
        ("ConfigMap", CA),
        ("Secret", CLIENT),
    ] {
        if let Err(error) = delete(ctx, kind, name).await {
            failures.push(error.to_string());
        }
    }
    ensure!(failures.is_empty(), "cleanup failed: {}", failures.join("; "));
    Ok(())
}

/// The local fixture certificates are generated by the shared stack preparation.
/// Kubernetes needs its own mounted certificate fixture and reachable TLS Services.
struct LocalCertificates {
    ca: Vec<u8>,
    wrong_ca: Vec<u8>,
    valid_cert: Vec<u8>,
    valid_key: Vec<u8>,
    rotated_cert: Vec<u8>,
    rotated_key: Vec<u8>,
    expired_cert: Vec<u8>,
}

impl LocalCertificates {
    async fn read(ctx: &ScenarioCtx) -> Result<Self> {
        let root = ctx.project_root.join("edgion-tests/certs");
        async fn read(root: &Path, name: &str) -> Result<Vec<u8>> {
            tokio::fs::read(root.join(name)).await.with_context(|| {
                format!("missing local certificate fixture {name}; run shared-stack certificate preparation")
            })
        }
        Ok(Self {
            ca: read(&root, "backend/ca.crt").await?,
            wrong_ca: read(&root, "mtls/client-ca.crt").await?,
            valid_cert: read(&root, "mtls/valid-client.crt").await?,
            valid_key: read(&root, "mtls/valid-client.key").await?,
            rotated_cert: read(&root, "mtls/nonmatching-client.crt").await?,
            rotated_key: read(&root, "mtls/nonmatching-client.key").await?,
            expired_cert: read(&root, "mtls/expired-client.crt").await?,
        })
    }
}

async fn ca_secret(ctx: &ScenarioCtx, pem: &[u8]) -> Result<()> {
    apply(
        ctx,
        json!({"apiVersion":"v1", "kind":"Secret", "metadata":{"name":CA,"namespace":NS},
        "type":"Opaque", "data":{"ca.crt":base64::engine::general_purpose::STANDARD.encode(pem)}}),
    )
    .await
}

async fn identity(ctx: &ScenarioCtx, cert: &[u8], key: &[u8]) -> Result<()> {
    apply(
        ctx,
        json!({"apiVersion":"v1", "kind":"Secret", "metadata":{"name":CLIENT,"namespace":NS},
        "type":"kubernetes.io/tls", "data":{
            "tls.crt":base64::engine::general_purpose::STANDARD.encode(cert),
            "tls.key":base64::engine::general_purpose::STANDARD.encode(key)}}),
    )
    .await
}

async fn reject_and_restore(ctx: &ScenarioCtx, broken: Value, valid: &Value) -> Result<()> {
    apply(ctx, broken).await?;
    phase(ctx, "unhealthy").await?;
    apply(ctx, valid.clone()).await?;
    phase(ctx, "healthy").await
}

async fn exercise(ctx: &ScenarioCtx, certs: &LocalCertificates) -> Result<()> {
    ctx.apply_dir("initial/").await?;
    ca_secret(ctx, &certs.ca).await?;
    identity(ctx, &certs.valid_cert, &certs.valid_key).await?;

    for (protocol, port, mtls_port, plaintext_port) in [("https", 30051, 30052, 30001), ("grpcs", 30042, 30043, 30021)]
    {
        println!("EncryptedActiveHealthChecks: {protocol} trust and authority");
        let valid = policy(protocol, port, false);
        // First force a real negative observation, including the initial generation;
        // recovery must then come from successful active probes.
        let mut broken = valid.clone();
        broken["spec"]["healthCheck"]["active"]["tls"]["validation"]["hostname"] = json!("wrong.example.com");
        reject_and_restore(ctx, broken, &valid).await?;
        if protocol == "grpcs" {
            let mut not_serving = valid.clone();
            not_serving["spec"]["healthCheck"]["active"]["grpcServiceName"] = json!("unhealthy");
            reject_and_restore(ctx, not_serving, &valid).await?;
        }

        let mut broken = valid.clone();
        broken["spec"]["healthCheck"]["active"]["tls"]["validation"]["subjectAltNames"] =
            json!([{"type":"Hostname", "hostname":"wrong.example.com"}]);
        reject_and_restore(ctx, broken, &valid).await?;

        for pem in [&certs.wrong_ca[..], b"malformed certificate" as &[u8]] {
            ca_secret(ctx, pem).await?;
            phase(ctx, "unhealthy").await?;
            ca_secret(ctx, &certs.ca).await?;
            phase(ctx, "healthy").await?;
        }
        delete(ctx, "Secret", CA).await?;
        phase(ctx, "unhealthy").await?;
        ca_secret(ctx, &certs.ca).await?;
        phase(ctx, "healthy").await?;

        // A ConfigMap is independently registered and requeued on data changes.
        apply(
            ctx,
            json!({"apiVersion":"v1", "kind":"ConfigMap", "metadata":{"name":CA,"namespace":NS},
            "data":{"ca.crt":std::str::from_utf8(&certs.ca)?}}),
        )
        .await?;
        let mut configmap = valid.clone();
        configmap["spec"]["healthCheck"]["active"]["tls"]["validation"]["caCertificateRefs"][0]["kind"] =
            json!("ConfigMap");
        apply(ctx, configmap).await?;
        phase(ctx, "healthy").await?;
        delete(ctx, "ConfigMap", CA).await?;
        phase(ctx, "unhealthy").await?;
        apply(ctx, valid.clone()).await?;
        phase(ctx, "healthy").await?;

        // TLS to a known live plaintext listener must fail without fallback.
        let mut plaintext = valid.clone();
        plaintext["spec"]["healthCheck"]["active"]["port"] = json!(plaintext_port);
        reject_and_restore(ctx, plaintext, &valid).await?;

        println!("EncryptedActiveHealthChecks: {protocol} identity lifecycle");
        let mtls = policy(protocol, mtls_port, true);
        reject_and_restore(ctx, policy(protocol, mtls_port, false), &mtls).await?;
        delete(ctx, "Secret", CLIENT).await?;
        phase(ctx, "unhealthy").await?;
        identity(ctx, &certs.valid_cert, &certs.valid_key).await?;
        phase(ctx, "healthy").await?;
        for cert in [&certs.expired_cert[..], b"malformed identity" as &[u8]] {
            identity(ctx, cert, &certs.valid_key).await?;
            phase(ctx, "unhealthy").await?;
            identity(ctx, &certs.rotated_cert, &certs.rotated_key).await?;
            phase(ctx, "healthy").await?;
        }
        identity(ctx, &certs.valid_cert, &certs.valid_key).await?;
        phase(ctx, "healthy").await?;
    }

    println!("EncryptedActiveHealthChecks: BackendTLSPolicy independence");
    // A traffic TLS policy changes application transport but cannot change the
    // active checker, its local trust, or health-state publication.
    apply(ctx, policy("https", 30051, false)).await?;
    phase(ctx, "healthy").await?;
    for hostname in ["backend.example.com", "wrong.example.com"] {
        apply(
            ctx,
            json!({"apiVersion":"gateway.networking.k8s.io/v1", "kind":"BackendTLSPolicy",
            "metadata":{"name":NAME,"namespace":NS}, "spec":{
                "targetRefs":[{"group":"","kind":"Service","name":NAME,"sectionName":"http"}],
                "validation":{"hostname":hostname,"caCertificateRefs":[{"group":"","kind":"Secret","name":CA}]}}}),
        )
        .await?;
        phase(ctx, "traffic_tls_failed").await?;
    }
    apply(ctx, policy("https", 30001, false)).await?;
    phase(ctx, "unhealthy").await?;
    apply(ctx, policy("https", 30051, false)).await?;
    phase(ctx, "traffic_tls_failed").await?;
    delete(ctx, "BackendTLSPolicy", NAME).await?;
    phase(ctx, "healthy").await?;

    println!("EncryptedActiveHealthChecks: plaintext protocol preservation");
    // Existing explicit plaintext and TCP meanings remain unchanged. The gRPC
    // NOT_SERVING case also verifies the standard Health response is inspected.
    for (protocol, port) in [("http", 30001), ("tcp", 30010), ("grpc", 30021)] {
        let valid = policy(protocol, port, false);
        let mut broken = valid.clone();
        if protocol == "grpc" {
            broken["spec"]["healthCheck"]["active"]["grpcServiceName"] = json!("unhealthy");
        } else if protocol == "http" {
            broken["spec"]["healthCheck"]["active"]["path"] = json!("/status/503");
        } else {
            // Force failure using the prior encrypted checker on plaintext.
            broken = policy("https", 30001, false);
        }
        reject_and_restore(ctx, broken, &valid).await?;
    }
    Ok(())
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            let certs = LocalCertificates::read(&ctx).await?;
            cleanup(&ctx).await?;
            phase(&ctx, "absent").await?;
            exercise(&ctx, &certs).await
        }
        .await;
        let cleanup_result = async {
            cleanup(&ctx).await?;
            phase(&ctx, "absent").await
        }
        .await;
        match (result, cleanup_result) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "EncryptedActiveHealthChecks",
    conf_path: "HTTPRoute/Backend/EncryptedActiveHealthChecks/DynamicTest",
    run,
    phases: PHASES,
};
