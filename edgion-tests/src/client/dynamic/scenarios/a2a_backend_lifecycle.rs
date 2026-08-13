//! Focused lifecycle coverage for transparent A2A EdgionBackend targets.

use crate::dynamic::scenario::{Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use anyhow::{Context, Result};
use serde_json::Value;
use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;

const NAMESPACE: &str = "edgion-test";
const BACKEND: &str = "a2a-lifecycle-agent";
const ROUTE: &str = "a2a-lifecycle-route";
const SECRET: &str = "a2a-lifecycle-key";
const HOST: &str = "a2a-backend-lifecycle.example.com";
const GATEWAY_PORT: u16 = 31945;

fn service_backend_yaml(service: &str) -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackend\nmetadata:\n  name: {BACKEND}\n  namespace: {NAMESPACE}\nspec:\n  a2a:\n    target:\n      serviceRef:\n        name: {service}\n        port: 30001\n"
    )
}

fn external_backend_yaml() -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackend\nmetadata:\n  name: {BACKEND}\n  namespace: {NAMESPACE}\nspec:\n  a2a:\n    target:\n      endpoint: http://127.0.0.1:30002\n    allowInsecureHttp: true\n"
    )
}

fn ai_backend_yaml() -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackend\nmetadata:\n  name: {BACKEND}\n  namespace: {NAMESPACE}\nspec:\n  ai:\n    provider: OpenAICompatible\n    endpoint: http://127.0.0.1:30001/ai/prefix\n    allowInsecureHttp: true\n    credentialPool:\n      credentials:\n        - name: primary\n          secretRef:\n            name: {SECRET}\n    models:\n      - name: fixture-model\n"
    )
}

fn secret_yaml() -> String {
    format!(
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: {SECRET}\n  namespace: {NAMESPACE}\ntype: Opaque\nstringData:\n  api-key: a2a-lifecycle-test-key\n"
    )
}

fn route_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: {ROUTE}\n  namespace: {NAMESPACE}\nspec:\n  parentRefs:\n    - name: ai-gateway\n  hostnames:\n    - {HOST}\n  rules:\n    - backendRefs:\n        - group: edgion.io\n          kind: EdgionBackend\n          name: {BACKEND}\n"
    )
}

fn authed(ctx: &ScenarioCtx, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match ctx.test_ctx.admin_token.as_deref() {
        Some(token) if !token.is_empty() => request.bearer_auth(token),
        _ => request,
    }
}

async fn controller_resource(ctx: &ScenarioCtx, kind: &str, name: &str) -> Result<Value> {
    let url = format!(
        "{}/configserver/{kind}?namespace={NAMESPACE}&name={name}",
        ctx.controller_url.trim_end_matches('/')
    );
    let response = authed(ctx, ctx.test_ctx.http_client.get(&url))
        .send()
        .await
        .with_context(|| format!("GET {url} failed"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .with_context(|| format!("GET {url} returned invalid JSON"))?;
    anyhow::ensure!(
        status.is_success() && body["success"] == true,
        "GET {url} returned {status}: {body}"
    );
    Ok(body["data"].clone())
}

fn has_condition(value: &Value, type_: &str, status: &str, reason: Option<&str>) -> bool {
    match value {
        Value::Array(items) => items.iter().any(|item| has_condition(item, type_, status, reason)),
        Value::Object(map) => {
            let direct = map.get("type").and_then(Value::as_str) == Some(type_)
                && map.get("status").and_then(Value::as_str) == Some(status)
                && reason.is_none_or(|expected| map.get("reason").and_then(Value::as_str) == Some(expected));
            direct || map.values().any(|item| has_condition(item, type_, status, reason))
        }
        _ => false,
    }
}

async fn wait_for<F, Fut>(label: &str, mut check: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last_error = None;
    loop {
        match check().await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => last_error = Some(error),
        }
        if Instant::now() >= deadline {
            match last_error {
                Some(error) => anyhow::bail!("timed out waiting for {label}; last error: {error:#}"),
                None => anyhow::bail!("timed out waiting for {label}"),
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn wait_condition(
    ctx: &ScenarioCtx,
    kind: &str,
    name: &str,
    type_: &str,
    status: &str,
    reason: Option<&str>,
) -> Result<()> {
    wait_for(
        &format!("{kind}/{NAMESPACE}/{name} {type_}={status}/{reason:?}"),
        || async {
            let resource = controller_resource(ctx, kind, name).await?;
            Ok(has_condition(&resource, type_, status, reason))
        },
    )
    .await
}

async fn apply(ctx: &ScenarioCtx, label: &str, yaml: String) -> Result<()> {
    let path = std::env::temp_dir().join(format!(
        "edgion-a2a-backend-lifecycle-{label}-{}.yaml",
        std::process::id()
    ));
    tokio::fs::write(&path, yaml)
        .await
        .with_context(|| format!("failed to write {label} apply manifest"))?;
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let result = crate::dynamic::apply::apply_path(&client, &path).await;
    let _ = tokio::fs::remove_file(&path).await;
    result
}

async fn proxy_request(ctx: &ScenarioCtx) -> Result<(reqwest::StatusCode, String)> {
    let target = ctx
        .test_ctx
        .target_host
        .parse::<std::net::IpAddr>()
        .with_context(|| format!("test target {} is not an IP address", ctx.test_ctx.target_host))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .resolve(HOST, std::net::SocketAddr::new(target, GATEWAY_PORT))
        .build()
        .context("failed to build A2A lifecycle request client")?;
    let response = client
        .get(format!("http://{HOST}:{GATEWAY_PORT}/echo"))
        .send()
        .await
        .context("A2A lifecycle proxy request failed")?;
    let status = response.status();
    let body = response.text().await.context("failed to read A2A lifecycle response")?;
    Ok((status, body))
}

async fn wait_traffic(ctx: &ScenarioCtx, expected_port: Option<u16>) -> Result<()> {
    wait_for(&format!("A2A traffic target {expected_port:?}"), || async {
        let (status, body) = proxy_request(ctx).await?;
        Ok(match expected_port {
            Some(port) => status.is_success() && body.contains(&format!(":{port}")),
            None => !status.is_success(),
        })
    })
    .await
}

async fn cleanup(ctx: &ScenarioCtx) {
    for (kind, name) in [("HTTPRoute", ROUTE), ("EdgionBackend", BACKEND), ("Secret", SECRET)] {
        let _ = ctx.test_ctx.delete_resource(kind, NAMESPACE, name).await;
    }
}

async fn run_inner(ctx: &ScenarioCtx) -> Result<()> {
    apply(ctx, "secret", secret_yaml()).await?;
    apply(ctx, "route", route_yaml()).await?;
    wait_condition(
        ctx,
        "HTTPRoute",
        ROUTE,
        "ResolvedRefs",
        "False",
        Some("BackendNotFound"),
    )
    .await?;
    wait_traffic(ctx, None).await?;

    apply(ctx, "service-backend", service_backend_yaml("test-http")).await?;
    wait_condition(ctx, "EdgionBackend", BACKEND, "Accepted", "True", Some("Accepted")).await?;
    wait_condition(ctx, "EdgionBackend", BACKEND, "ResolvedRefs", "True", None).await?;
    wait_condition(ctx, "HTTPRoute", ROUTE, "ResolvedRefs", "True", None).await?;
    wait_traffic(ctx, Some(30001)).await?;

    apply(ctx, "external-backend", external_backend_yaml()).await?;
    wait_condition(ctx, "EdgionBackend", BACKEND, "ResolvedRefs", "True", None).await?;
    wait_traffic(ctx, Some(30002)).await?;

    apply(
        ctx,
        "missing-service-backend",
        service_backend_yaml("a2a-missing-service"),
    )
    .await?;
    wait_condition(
        ctx,
        "EdgionBackend",
        BACKEND,
        "ResolvedRefs",
        "False",
        Some("TargetNotFound"),
    )
    .await?;
    wait_traffic(ctx, None).await?;

    apply(ctx, "wrong-ai-variant", ai_backend_yaml()).await?;
    wait_condition(ctx, "EdgionBackend", BACKEND, "Accepted", "True", Some("Accepted")).await?;
    wait_condition(ctx, "HTTPRoute", ROUTE, "Accepted", "False", None).await?;
    wait_traffic(ctx, None).await?;

    apply(ctx, "restored-external-backend", external_backend_yaml()).await?;
    wait_condition(ctx, "HTTPRoute", ROUTE, "Accepted", "True", None).await?;
    wait_condition(ctx, "HTTPRoute", ROUTE, "ResolvedRefs", "True", None).await?;
    wait_traffic(ctx, Some(30002)).await?;

    ctx.test_ctx
        .delete_resource("EdgionBackend", NAMESPACE, BACKEND)
        .await
        .map_err(anyhow::Error::msg)?;
    wait_condition(
        ctx,
        "HTTPRoute",
        ROUTE,
        "ResolvedRefs",
        "False",
        Some("BackendNotFound"),
    )
    .await?;
    wait_traffic(ctx, None).await?;

    apply(ctx, "recreated-service-backend", service_backend_yaml("test-http")).await?;
    wait_condition(ctx, "HTTPRoute", ROUTE, "ResolvedRefs", "True", None).await?;
    wait_traffic(ctx, Some(30001)).await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        cleanup(&ctx).await;
        let result = run_inner(&ctx).await;
        cleanup(&ctx).await;
        result
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "A2aBackendLifecycle",
    conf_path: "EdgionBackend/Lifecycle/DynamicTest",
    run,
    phases: &[],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_manifests_are_valid_and_keep_the_target_union_strict() {
        for yaml in [
            secret_yaml(),
            route_yaml(),
            service_backend_yaml("test-http"),
            external_backend_yaml(),
            ai_backend_yaml(),
        ] {
            serde_yaml::from_str::<Value>(&yaml)
                .unwrap_or_else(|error| panic!("invalid A2A lifecycle YAML: {error}\n{yaml}"));
        }

        let service: Value = serde_yaml::from_str(&service_backend_yaml("test-http")).unwrap();
        assert!(service.pointer("/spec/a2a/target/serviceRef").is_some());
        assert!(service.pointer("/spec/a2a/target/endpoint").is_none());

        let external: Value = serde_yaml::from_str(&external_backend_yaml()).unwrap();
        assert!(external.pointer("/spec/a2a/target/serviceRef").is_none());
        assert_eq!(
            external.pointer("/spec/a2a/target/endpoint"),
            Some(&Value::String("http://127.0.0.1:30002".into()))
        );
    }
}
