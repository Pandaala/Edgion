//! Hot-reload regression using the shared BackendTLS Gateway and HTTPS backend.

use crate::framework::{test_backend_hosts, TestCase, TestContext, TestResult};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const NS: &str = "edgion-test";

pub(super) fn test_case() -> TestCase {
    TestCase::new(
        "backend_tls_pool_isolation_on_ca_reload",
        "Prove live connection reuse, isolate a changed CA bundle, reject untrusted CA, and recover",
        |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                match run(&ctx).await {
                    Ok(()) => TestResult::passed(start.elapsed()),
                    Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                }
            })
        },
    )
    .with_timeout(Duration::from_secs(120))
}

async fn apply(ctx: &TestContext, resource: &Value) -> Result<()> {
    ctx.apply_yaml(&resource.to_string()).await.map_err(anyhow::Error::msg)
}

fn policy(name: &str, ca_names: &[&str]) -> Value {
    json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "BackendTLSPolicy",
        "metadata": {"name": name, "namespace": NS},
        "spec": {
            "targetRefs": [{"group": "", "kind": "Service", "name": name}],
            "validation": {
                "hostname": "backend.example.com",
                "caCertificateRefs": ca_names.iter().map(|ca| json!({
                    "group": "", "kind": "Secret", "name": ca
                })).collect::<Vec<_>>()
            }
        }
    })
}

async fn wait_policy(ctx: &TestContext, expected: &Value) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let token = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN").context("Gateway admin token is required")?;
    loop {
        let response = ctx
            .http_client
            .get(format!(
                "http://{}:12001/configclient/backendtlspolicy/list",
                ctx.target_host
            ))
            .bearer_auth(&token)
            .timeout(Duration::from_secs(3))
            .send()
            .await?
            .error_for_status()?;
        let body: Value = response.json().await?;
        ensure!(body["success"] == true, "Gateway policy query failed");
        let policies = body["data"].as_array().context("missing policy list")?;
        if policies.iter().any(|actual| {
            actual["metadata"]["name"] == expected["metadata"]["name"]
                && actual["metadata"]["namespace"] == NS
                && actual["spec"]["resolvedTargetClass"] == "service"
                && actual["spec"]["validation"] == expected["spec"]["validation"]
        }) {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "timed out waiting for the new CA policy on Gateway"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn request(ctx: &TestContext, host: &str) -> Result<(u16, String)> {
    let response = ctx
        .http_client
        .get(format!("{}/echo", ctx.http_url()))
        .header("host", host)
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    let status = response.status().as_u16();
    Ok((status, response.text().await?))
}

fn connection(status: u16, body: &str) -> Result<SocketAddr> {
    ensure!(status == 200, "expected successful TLS echo, got HTTP {status}");
    ensure!(
        body.lines().any(|line| line == "Server: 0.0.0.0:30051"),
        "wrong backend"
    );
    body.lines()
        .find_map(|line| line.strip_prefix("Client: "))
        .context("backend did not report its observed client socket")?
        .parse()
        .context("invalid backend client socket")
}

async fn warm_pool(ctx: &TestContext, host: &str) -> Result<SocketAddr> {
    wait_reused_connection(ctx, host, None).await
}

async fn wait_reused_connection(
    ctx: &TestContext,
    host: &str,
    previous_generation: Option<SocketAddr>,
) -> Result<SocketAddr> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut previous = None;
    loop {
        let (status, body) = request(ctx, host).await?;
        if status == 200 {
            let current = connection(status, &body)?;
            if previous == Some(current) && previous_generation != Some(current) {
                return Ok(current);
            }
            previous = Some(current);
        }
        ensure!(
            Instant::now() < deadline,
            "did not observe upstream keepalive reuse for the expected CA generation"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn cleanup(ctx: &TestContext, name: &str) -> Result<()> {
    let mut failures = Vec::new();
    for kind in ["HTTPRoute", "BackendTLSPolicy", "EndpointSlice", "Service", "Secret"] {
        let mut request = ctx
            .http_client
            .delete(format!("{}/api/v1/namespaced/{kind}/{NS}/{name}", ctx.admin_api_url()))
            .timeout(Duration::from_secs(3));
        if let Some(token) = &ctx.admin_token {
            request = request.bearer_auth(token);
        }
        match request.send().await {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
            Ok(response) => failures.push(format!("{kind}: HTTP {}", response.status())),
            Err(error) => failures.push(format!("{kind}: {error}")),
        }
    }
    ensure!(failures.is_empty(), "cleanup failed: {}", failures.join("; "));
    Ok(())
}

async fn run(ctx: &TestContext) -> Result<()> {
    let name = format!("tls-pool-{}", uuid::Uuid::new_v4().simple());
    let host = format!("{name}.example.com");
    let key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(vec!["untrusted.example.com".into()])?;
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = params.self_signed(&key)?.pem();
    let backend_ip = test_backend_hosts(ctx)
        .into_iter()
        .next()
        .context("no backend address")?;
    let initial = policy(&name, &["backend-ca"]);
    // Leave time for cleanup before TestCase's outer timeout drops the future.
    let result = tokio::time::timeout(Duration::from_secs(90), async {
        for resource in [
            json!({"apiVersion": "v1", "kind": "Secret",
                "metadata": {"name": name, "namespace": NS}, "stringData": {"ca.crt": ca}}),
            json!({"apiVersion": "v1", "kind": "Service",
                "metadata": {"name": name, "namespace": NS},
                "spec": {"ports": [{"name": "https", "port": 30051, "targetPort": 30051}]}}),
            json!({"apiVersion": "discovery.k8s.io/v1", "kind": "EndpointSlice",
                "metadata": {"name": name, "namespace": NS,
                    "labels": {"kubernetes.io/service-name": name}},
                "addressType": "IPv4", "ports": [{"name": "https", "port": 30051, "protocol": "TCP"}],
                "endpoints": [{"addresses": [backend_ip], "conditions": {"ready": true}}]}),
            initial.clone(),
            json!({"apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
                "metadata": {"name": name, "namespace": NS},
                "spec": {"parentRefs": [{"name": "backend-tls-gateway"}], "hostnames": [host],
                    "rules": [{"backendRefs": [{"name": name, "port": 30051}]}]}}),
        ] {
            apply(ctx, &resource).await?;
        }
        wait_policy(ctx, &initial).await?;
        let old_connection = warm_pool(ctx, &host).await?;

        // configclient observes receipt, not the debounced runtime publication.
        // Keep sending traffic until a different connection is itself reused.
        // A missing discriminator keeps the old connection active throughout
        // this short window instead of passing through idle expiration.
        let expanded = policy(&name, &["backend-ca", &name]);
        apply(ctx, &expanded).await?;
        wait_policy(ctx, &expanded).await?;
        wait_reused_connection(ctx, &host, Some(old_connection)).await?;

        // Remove the backend's trust anchor while keeping the same address
        // and SNI. Missing/invalid resource rejection is not sufficient:
        // the valid unrelated CA must force a fresh, failing TLS handshake.
        let untrusted = policy(&name, &[&name]);
        apply(ctx, &untrusted).await?;
        wait_policy(ctx, &untrusted).await?;
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut rejections = 0;
        while rejections < 3 {
            let (status, _) = request(ctx, &host).await?;
            rejections = if status == 502 { rejections + 1 } else { 0 };
            ensure!(
                Instant::now() < deadline,
                "untrusted CA did not reject after runtime publication: HTTP {status}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        apply(ctx, &initial).await?;
        wait_policy(ctx, &initial).await?;
        warm_pool(ctx, &host).await?;
        Ok(())
    })
    .await
    .context("CA rotation phases exceeded their deadline")
    .and_then(|result| result);
    let cleanup = cleanup(ctx, &name).await;
    match (result, cleanup) {
        (Ok(()), cleanup) => cleanup,
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; {cleanup:#}")),
    }
}
