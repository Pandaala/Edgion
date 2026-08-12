//! Focused hot-reload coverage for EdgionBackend references and multi-target AI policies.

use crate::dynamic::scenario::{Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;

const ROUTE_NS: &str = "edgion-test";
const BACKEND_NS: &str = "edgion-default";
const SECRET_NS: &str = "edgion-lifecycle-secrets";
const BACKEND_A: &str = "lifecycle-ai-a";
const BACKEND_B: &str = "lifecycle-ai-b";
const SECRET: &str = "lifecycle-ai-key";
const ROUTE: &str = "lifecycle-ai-route";
const PLUGIN: &str = "lifecycle-ai-proxy";
const ROUTE_GRANT: &str = "allow-lifecycle-route-backends";
const SECRET_GRANT: &str = "allow-lifecycle-backend-secret";
const WINNER_POLICY: &str = "lifecycle-ai-policy-a";
const SIBLING_POLICY: &str = "lifecycle-ai-policy-b";
const HOST: &str = "ai-backend-lifecycle.example.com";
const AI_GATEWAY_PORT: u16 = 31945;

fn secret_yaml() -> String {
    format!(
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: {SECRET}\n  namespace: {SECRET_NS}\ntype: Opaque\nstringData:\n  api-key: lifecycle-test-key\n"
    )
}

fn backend_yaml(name: &str) -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackend\nmetadata:\n  name: {name}\n  namespace: {BACKEND_NS}\nspec:\n  ai:\n    provider: OpenAICompatible\n    endpoint: http://127.0.0.1:30001/ai/prefix\n    allowInsecureHttp: true\n    credentialPool:\n      credentials:\n        - name: primary\n          secretRef:\n            name: {SECRET}\n            namespace: {SECRET_NS}\n    models:\n      - name: fixture-model\n"
    )
}

fn plugin_yaml() -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionPlugins\nmetadata:\n  name: {PLUGIN}\n  namespace: {ROUTE_NS}\nspec:\n  requestPlugins:\n    - enable: true\n      type: AiProxy\n      config:\n        backendSelection:\n          type: Weighted\n"
    )
}

fn route_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: {ROUTE}\n  namespace: {ROUTE_NS}\nspec:\n  parentRefs:\n    - name: ai-gateway\n  hostnames:\n    - {HOST}\n  rules:\n    - filters:\n        - type: ExtensionRef\n          extensionRef:\n            group: edgion.io\n            kind: EdgionPlugins\n            name: {PLUGIN}\n      backendRefs:\n        - group: edgion.io\n          kind: EdgionBackend\n          name: {BACKEND_A}\n          namespace: {BACKEND_NS}\n"
    )
}

fn route_grant_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1beta1\nkind: ReferenceGrant\nmetadata:\n  name: {ROUTE_GRANT}\n  namespace: {BACKEND_NS}\nspec:\n  from:\n    - group: gateway.networking.k8s.io\n      kind: HTTPRoute\n      namespace: {ROUTE_NS}\n  to:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_A}\n"
    )
}

fn secret_grant_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1beta1\nkind: ReferenceGrant\nmetadata:\n  name: {SECRET_GRANT}\n  namespace: {SECRET_NS}\nspec:\n  from:\n    - group: edgion.io\n      kind: EdgionBackend\n      namespace: {BACKEND_NS}\n  to:\n    - group: \"\"\n      kind: Secret\n      name: {SECRET}\n"
    )
}

fn policy_yaml(name: &str, second_target: &str, consecutive_errors: u32) -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackendTrafficPolicy\nmetadata:\n  name: {name}\n  namespace: {BACKEND_NS}\nspec:\n  targetRefs:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_A}\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {second_target}\n  outlierDetection:\n    consecutiveErrors: {consecutive_errors}\n    ejectionTime: 2s\n"
    )
}

fn mixed_policy_yaml() -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackendTrafficPolicy\nmetadata:\n  name: lifecycle-ai-policy-mixed\n  namespace: {BACKEND_NS}\nspec:\n  targetRefs:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_A}\n    - kind: Service\n      name: lifecycle-service\n  outlierDetection:\n    consecutiveErrors: 3\n"
    )
}

fn incompatible_policy_yaml() -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackendTrafficPolicy\nmetadata:\n  name: lifecycle-ai-policy-incompatible\n  namespace: {BACKEND_NS}\nspec:\n  targetRefs:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_A}\n  loadBalancer:\n    type: LeastConn\n"
    )
}

fn authed(ctx: &ScenarioCtx, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match ctx.test_ctx.admin_token.as_deref() {
        Some(token) if !token.is_empty() => request.bearer_auth(token),
        _ => request,
    }
}

fn gateway_authed(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        Ok(token) if !token.trim().is_empty() => request.bearer_auth(token),
        _ => request,
    }
}

async fn controller_resource(ctx: &ScenarioCtx, kind: &str, namespace: &str, name: &str) -> Result<Value> {
    let url = format!(
        "{}/configserver/{kind}?namespace={namespace}&name={name}",
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

async fn gateway_list(ctx: &ScenarioCtx, path: &str) -> Result<Vec<Value>> {
    let url = format!("{}{}", ctx.gateway_url.trim_end_matches('/'), path);
    let response = gateway_authed(ctx.test_ctx.http_client.get(&url))
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
    body["data"]
        .as_array()
        .cloned()
        .with_context(|| format!("GET {url} response has no data array: {body}"))
}

fn resource_named(value: &Value, namespace: &str, name: &str) -> bool {
    value.pointer("/metadata/namespace").and_then(Value::as_str) == Some(namespace)
        && value.pointer("/metadata/name").and_then(Value::as_str) == Some(name)
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

async fn wait_for<F, Fut>(label: &str, timeout: Duration, mut check: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    let deadline = Instant::now() + timeout;
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
    namespace: &str,
    name: &str,
    type_: &str,
    status: &str,
    reason: Option<&str>,
) -> Result<()> {
    wait_for(
        &format!("{kind}/{namespace}/{name} {type_}={status}/{reason:?}"),
        Duration::from_secs(15),
        || async {
            let resource = controller_resource(ctx, kind, namespace, name).await?;
            Ok(has_condition(&resource, type_, status, reason))
        },
    )
    .await
}

async fn wait_policy_marker(ctx: &ScenarioCtx, name: &str, present: bool) -> Result<()> {
    wait_for(
        &format!("AI policy {BACKEND_NS}/{name} publication={present}"),
        Duration::from_secs(15),
        || async {
            let items = gateway_list(ctx, "/configclient/edgionbackendtrafficpolicy/list").await?;
            let found = items.iter().find(|item| resource_named(item, BACKEND_NS, name));
            Ok(if present {
                found
                    .is_some_and(|item| item.pointer("/spec/resolvedTargetClass").and_then(Value::as_str) == Some("ai"))
            } else {
                found.is_none_or(|item| item.pointer("/spec/resolvedTargetClass").and_then(Value::as_str) != Some("ai"))
            })
        },
    )
    .await
}

async fn ai_request_status(ctx: &ScenarioCtx) -> Result<reqwest::StatusCode> {
    let target = ctx
        .test_ctx
        .target_host
        .parse::<std::net::IpAddr>()
        .with_context(|| format!("test target {} is not an IP address", ctx.test_ctx.target_host))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .resolve(HOST, std::net::SocketAddr::new(target, AI_GATEWAY_PORT))
        .build()
        .context("failed to build lifecycle request client")?;
    Ok(client
        .post(format!("http://{HOST}:{AI_GATEWAY_PORT}/v1/chat/completions"))
        .json(&json!({"model": "fixture-model", "messages": [{"role": "user", "content": "hello"}]}))
        .send()
        .await
        .context("AI lifecycle request failed")?
        .status())
}

async fn wait_traffic(ctx: &ScenarioCtx, available: bool) -> Result<()> {
    wait_for(
        &format!("AI lifecycle traffic available={available}"),
        Duration::from_secs(15),
        || async {
            let status = ai_request_status(ctx).await?;
            Ok(status.is_success() == available)
        },
    )
    .await
}

async fn apply(ctx: &ScenarioCtx, yaml: String) -> Result<()> {
    let path = std::env::temp_dir().join(format!("edgion-ai-backend-lifecycle-{}.yaml", std::process::id()));
    tokio::fs::write(&path, yaml)
        .await
        .context("failed to write lifecycle apply manifest")?;
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let result = crate::dynamic::apply::apply_path(&client, &path).await;
    let _ = tokio::fs::remove_file(&path).await;
    result
}

async fn cleanup(ctx: &ScenarioCtx) {
    for (kind, namespace, name) in [
        ("EdgionBackendTrafficPolicy", BACKEND_NS, WINNER_POLICY),
        ("EdgionBackendTrafficPolicy", BACKEND_NS, SIBLING_POLICY),
        ("EdgionBackendTrafficPolicy", BACKEND_NS, "lifecycle-ai-policy-mixed"),
        (
            "EdgionBackendTrafficPolicy",
            BACKEND_NS,
            "lifecycle-ai-policy-incompatible",
        ),
        ("ReferenceGrant", BACKEND_NS, ROUTE_GRANT),
        ("ReferenceGrant", SECRET_NS, SECRET_GRANT),
        ("HTTPRoute", ROUTE_NS, ROUTE),
        ("EdgionPlugins", ROUTE_NS, PLUGIN),
        ("EdgionBackend", BACKEND_NS, BACKEND_A),
        ("EdgionBackend", BACKEND_NS, BACKEND_B),
        ("Secret", SECRET_NS, SECRET),
    ] {
        let _ = ctx.test_ctx.delete_resource(kind, namespace, name).await;
    }
}

async fn reference_lifecycle(ctx: &ScenarioCtx) -> Result<()> {
    apply(ctx, secret_yaml()).await?;
    apply(ctx, backend_yaml(BACKEND_A)).await?;
    apply(ctx, backend_yaml(BACKEND_B)).await?;
    apply(ctx, plugin_yaml()).await?;
    apply(ctx, route_yaml()).await?;

    wait_condition(
        ctx,
        "EdgionBackend",
        BACKEND_NS,
        BACKEND_A,
        "ResolvedRefs",
        "False",
        Some("RefNotPermitted"),
    )
    .await?;
    wait_condition(
        ctx,
        "HTTPRoute",
        ROUTE_NS,
        ROUTE,
        "ResolvedRefs",
        "False",
        Some("RefNotPermitted"),
    )
    .await?;

    apply(ctx, secret_grant_yaml()).await?;
    wait_condition(
        ctx,
        "EdgionBackend",
        BACKEND_NS,
        BACKEND_A,
        "ResolvedRefs",
        "True",
        None,
    )
    .await?;
    apply(ctx, route_grant_yaml()).await?;
    wait_condition(ctx, "HTTPRoute", ROUTE_NS, ROUTE, "ResolvedRefs", "True", None).await?;
    wait_traffic(ctx, true).await?;

    ctx.test_ctx
        .delete_resource("ReferenceGrant", BACKEND_NS, ROUTE_GRANT)
        .await
        .map_err(anyhow::Error::msg)?;
    wait_condition(
        ctx,
        "HTTPRoute",
        ROUTE_NS,
        ROUTE,
        "ResolvedRefs",
        "False",
        Some("RefNotPermitted"),
    )
    .await?;
    wait_traffic(ctx, false).await?;
    apply(ctx, route_grant_yaml()).await?;
    wait_condition(ctx, "HTTPRoute", ROUTE_NS, ROUTE, "ResolvedRefs", "True", None).await?;
    wait_traffic(ctx, true).await?;

    ctx.test_ctx
        .delete_resource("ReferenceGrant", SECRET_NS, SECRET_GRANT)
        .await
        .map_err(anyhow::Error::msg)?;
    wait_condition(
        ctx,
        "EdgionBackend",
        BACKEND_NS,
        BACKEND_A,
        "ResolvedRefs",
        "False",
        Some("RefNotPermitted"),
    )
    .await?;
    wait_traffic(ctx, false).await?;
    apply(ctx, secret_grant_yaml()).await?;
    wait_condition(
        ctx,
        "EdgionBackend",
        BACKEND_NS,
        BACKEND_A,
        "ResolvedRefs",
        "True",
        None,
    )
    .await?;
    wait_traffic(ctx, true).await?;

    ctx.test_ctx
        .delete_resource("EdgionBackend", BACKEND_NS, BACKEND_A)
        .await
        .map_err(anyhow::Error::msg)?;
    wait_condition(
        ctx,
        "HTTPRoute",
        ROUTE_NS,
        ROUTE,
        "ResolvedRefs",
        "False",
        Some("BackendNotFound"),
    )
    .await?;
    wait_traffic(ctx, false).await?;
    apply(ctx, backend_yaml(BACKEND_A)).await?;
    wait_condition(ctx, "HTTPRoute", ROUTE_NS, ROUTE, "ResolvedRefs", "True", None).await?;
    wait_traffic(ctx, true).await
}

async fn policy_lifecycle(ctx: &ScenarioCtx) -> Result<()> {
    apply(ctx, policy_yaml(WINNER_POLICY, BACKEND_B, 3)).await?;
    wait_policy_marker(ctx, WINNER_POLICY, true).await?;

    ctx.test_ctx
        .delete_resource("EdgionBackend", BACKEND_NS, BACKEND_B)
        .await
        .map_err(anyhow::Error::msg)?;
    wait_policy_marker(ctx, WINNER_POLICY, false).await?;
    apply(ctx, backend_yaml(BACKEND_B)).await?;
    wait_policy_marker(ctx, WINNER_POLICY, true).await?;

    // Lenient admission retains invalid resources for status reporting, but
    // the transport marker must be cleared so the Gateway atomically withdraws
    // every target attachment.
    apply(ctx, incompatible_policy_yaml()).await?;
    wait_condition(
        ctx,
        "EdgionBackendTrafficPolicy",
        BACKEND_NS,
        "lifecycle-ai-policy-incompatible",
        "Accepted",
        "False",
        Some("Invalid"),
    )
    .await?;
    wait_policy_marker(ctx, "lifecycle-ai-policy-incompatible", false).await?;
    apply(ctx, mixed_policy_yaml()).await?;
    wait_condition(
        ctx,
        "EdgionBackendTrafficPolicy",
        BACKEND_NS,
        "lifecycle-ai-policy-mixed",
        "Accepted",
        "False",
        Some("Invalid"),
    )
    .await?;
    wait_policy_marker(ctx, "lifecycle-ai-policy-mixed", false).await?;

    // With no Kubernetes creationTimestamp in the filesystem integration
    // center, lexicographic policy name is the deterministic precedence tie
    // breaker. The second policy is initially Conflicted, then is promoted
    // when the older winner is deleted.
    apply(ctx, policy_yaml(SIBLING_POLICY, BACKEND_B, 7)).await?;
    wait_condition(
        ctx,
        "EdgionBackendTrafficPolicy",
        BACKEND_NS,
        SIBLING_POLICY,
        "Accepted",
        "False",
        Some("Conflicted"),
    )
    .await?;
    ctx.test_ctx
        .delete_resource("EdgionBackendTrafficPolicy", BACKEND_NS, WINNER_POLICY)
        .await
        .map_err(anyhow::Error::msg)?;
    wait_condition(
        ctx,
        "EdgionBackendTrafficPolicy",
        BACKEND_NS,
        SIBLING_POLICY,
        "Accepted",
        "True",
        Some("Accepted"),
    )
    .await?;
    wait_policy_marker(ctx, SIBLING_POLICY, true).await
}

async fn run_inner(ctx: &ScenarioCtx) -> Result<()> {
    cleanup(ctx).await;
    reference_lifecycle(ctx).await?;
    policy_lifecycle(ctx).await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = run_inner(&ctx).await;
        cleanup(&ctx).await;
        result
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "AiBackendLifecycle",
    conf_path: "EdgionBackend/Lifecycle/DynamicTest",
    run,
    phases: &[],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_manifests_are_valid_yaml_and_keep_reference_namespaces_explicit() {
        for yaml in [
            secret_yaml(),
            backend_yaml(BACKEND_A),
            backend_yaml(BACKEND_B),
            plugin_yaml(),
            route_yaml(),
            route_grant_yaml(),
            secret_grant_yaml(),
            policy_yaml(WINNER_POLICY, BACKEND_B, 3),
            policy_yaml(SIBLING_POLICY, BACKEND_B, 7),
            incompatible_policy_yaml(),
            mixed_policy_yaml(),
        ] {
            serde_yaml::from_str::<Value>(&yaml)
                .unwrap_or_else(|error| panic!("invalid lifecycle YAML: {error}\n{yaml}"));
        }
        let backend: Value = serde_yaml::from_str(&backend_yaml(BACKEND_A)).unwrap();
        assert_eq!(
            backend.pointer("/spec/ai/credentialPool/credentials/0/secretRef/namespace"),
            Some(&Value::String(SECRET_NS.into()))
        );
        let route: Value = serde_yaml::from_str(&route_yaml()).unwrap();
        assert_eq!(
            route.pointer("/spec/rules/0/backendRefs/0/namespace"),
            Some(&Value::String(BACKEND_NS.into()))
        );
    }
}
