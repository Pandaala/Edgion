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
const SECRET_B: &str = "lifecycle-ai-key-b";
const ROUTE: &str = "lifecycle-ai-route";
const PLUGIN: &str = "lifecycle-ai-proxy";
const ROUTE_GRANT: &str = "allow-lifecycle-route-backends";
const SECRET_GRANT: &str = "allow-lifecycle-backend-secret";
const WINNER_POLICY: &str = "lifecycle-ai-policy-a";
const SIBLING_POLICY: &str = "lifecycle-ai-policy-b";
const SERVICE_POLICY: &str = "lifecycle-service-policy";
const HOST: &str = "ai-backend-lifecycle.example.com";
const AI_GATEWAY_PORT: u16 = 31945;

fn secret_yaml() -> String {
    secret_yaml_named(SECRET, "lifecycle-test-key")
}

fn secret_yaml_named(name: &str, value: &str) -> String {
    format!(
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: {name}\n  namespace: {SECRET_NS}\ntype: Opaque\nstringData:\n  api-key: {value}\n"
    )
}

fn service_yaml(name: &str) -> String {
    format!(
        "apiVersion: v1\nkind: Service\nmetadata:\n  name: {name}\n  namespace: {BACKEND_NS}\nspec:\n  ports:\n    - port: 8080\n      protocol: TCP\n"
    )
}

fn backend_yaml(name: &str) -> String {
    backend_yaml_with_secrets(name, &[SECRET])
}

fn backend_yaml_with_secrets(name: &str, secrets: &[&str]) -> String {
    let credentials = secrets
        .iter()
        .enumerate()
        .map(|(index, secret)| {
            format!(
                "        - name: credential-{index}\n          secretRef:\n            name: {secret}\n            namespace: {SECRET_NS}\n"
            )
        })
        .collect::<String>();
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackend\nmetadata:\n  name: {name}\n  namespace: {BACKEND_NS}\nspec:\n  ai:\n    provider: OpenAICompatible\n    endpoint: http://127.0.0.1:30001/ai/prefix\n    allowInsecureHttp: true\n    credentialPool:\n      credentials:\n{credentials}    models:\n      - name: fixture-model\n"
    )
}

fn hot_backend_yaml(name: &str, endpoint_path: &str) -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackend\nmetadata:\n  name: {name}\n  namespace: {BACKEND_NS}\nspec:\n  ai:\n    provider: OpenAICompatible\n    endpoint: http://127.0.0.1:30001/ai/{endpoint_path}\n    allowInsecureHttp: true\n    credentialPool:\n      credentials:\n        - name: primary\n          secretRef:\n            name: {SECRET}\n            namespace: {SECRET_NS}\n    models:\n      - name: economy-model\n        aliases:\n          - fixture-model\n    defaults:\n      streaming: false\n      maxOutputTokens: 1234\n"
    )
}

fn plugin_yaml() -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionPlugins\nmetadata:\n  name: {PLUGIN}\n  namespace: {ROUTE_NS}\nspec:\n  requestPlugins:\n    - enable: true\n      type: AiProxy\n      config:\n        backendSelection:\n          type: Weighted\n"
    )
}

fn routed_plugin_yaml() -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionPlugins\nmetadata:\n  name: {PLUGIN}\n  namespace: {ROUTE_NS}\nspec:\n  requestPlugins:\n    - enable: true\n      type: AiProxy\n      config:\n        backendSelection:\n          type: Weighted\n        modelRoutes:\n          - match:\n              model: virtual-*\n            action:\n              rewrite: economy-model\n              candidates:\n                - backend:\n                    group: edgion.io\n                    kind: EdgionBackend\n                    namespace: {BACKEND_NS}\n                    name: {BACKEND_A}\n                    occurrence: 0\n                  weight: 1\n          - match:\n              model: header-only\n              headers:\n                - name: x-model-tier\n                  value: paid\n            action:\n              rewrite: economy-model\n          - match:\n              model: blocked-model\n            action:\n              rewrite: economy-model\n              candidates:\n                - backend:\n                    group: edgion.io\n                    kind: EdgionBackend\n                    namespace: {BACKEND_NS}\n                    name: {BACKEND_A}\n                    occurrence: 0\n                  weight: 0\n"
    )
}

fn route_yaml() -> String {
    route_yaml_target(BACKEND_A)
}

fn route_yaml_target(target: &str) -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: {ROUTE}\n  namespace: {ROUTE_NS}\nspec:\n  parentRefs:\n    - name: ai-gateway\n  hostnames:\n    - {HOST}\n  rules:\n    - filters:\n        - type: ExtensionRef\n          extensionRef:\n            group: edgion.io\n            kind: EdgionPlugins\n            name: {PLUGIN}\n      backendRefs:\n        - group: edgion.io\n          kind: EdgionBackend\n          name: {target}\n          namespace: {BACKEND_NS}\n"
    )
}

fn route_grant_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1beta1\nkind: ReferenceGrant\nmetadata:\n  name: {ROUTE_GRANT}\n  namespace: {BACKEND_NS}\nspec:\n  from:\n    - group: gateway.networking.k8s.io\n      kind: HTTPRoute\n      namespace: {ROUTE_NS}\n  to:\n    - group: edgion.io\n      kind: EdgionBackend\n"
    )
}

fn secret_grant_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1beta1\nkind: ReferenceGrant\nmetadata:\n  name: {SECRET_GRANT}\n  namespace: {SECRET_NS}\nspec:\n  from:\n    - group: edgion.io\n      kind: EdgionBackend\n      namespace: {BACKEND_NS}\n  to:\n    - group: \"\"\n      kind: Secret\n"
    )
}

fn denying_secret_grant_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1beta1\nkind: ReferenceGrant\nmetadata:\n  name: {SECRET_GRANT}\n  namespace: {SECRET_NS}\nspec:\n  from:\n    - group: edgion.io\n      kind: EdgionBackend\n      namespace: {BACKEND_NS}\n  to:\n    - group: \"\"\n      kind: Secret\n      name: unrelated-secret\n"
    )
}

fn policy_yaml(name: &str, second_target: &str, consecutive_errors: u32) -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackendTrafficPolicy\nmetadata:\n  name: {name}\n  namespace: {BACKEND_NS}\nspec:\n  targetRefs:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_A}\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {second_target}\n  outlierDetection:\n    consecutiveErrors: {consecutive_errors}\n    ejectionTime: 2s\n"
    )
}

fn mixed_policy_yaml(name: &str) -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackendTrafficPolicy\nmetadata:\n  name: {name}\n  namespace: {BACKEND_NS}\nspec:\n  targetRefs:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_A}\n    - kind: Service\n      name: lifecycle-service\n  outlierDetection:\n    consecutiveErrors: 3\n"
    )
}

fn incompatible_policy_yaml(name: &str) -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackendTrafficPolicy\nmetadata:\n  name: {name}\n  namespace: {BACKEND_NS}\nspec:\n  targetRefs:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_A}\n  loadBalancer:\n    type: LeastConn\n"
    )
}

fn service_policy_yaml(name: &str, target: &str) -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackendTrafficPolicy\nmetadata:\n  name: {name}\n  namespace: {BACKEND_NS}\nspec:\n  targetRefs:\n    - kind: Service\n      name: {target}\n  loadBalancer:\n    type: LeastConn\n"
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

async fn gateway_store_count(ctx: &ScenarioCtx, field: &str) -> Result<u64> {
    let url = format!("{}/api/v1/debug/store-stats", ctx.gateway_url.trim_end_matches('/'));
    let response = gateway_authed(ctx.test_ctx.http_client.get(&url))
        .send()
        .await
        .with_context(|| format!("GET {url} failed"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .with_context(|| format!("GET {url} returned invalid JSON"))?;
    anyhow::ensure!(status.is_success(), "GET {url} returned {status}: {body}");
    body.pointer(&format!("/data/{field}/count"))
        .and_then(Value::as_u64)
        .with_context(|| format!("GET {url} response has no data.{field}.count: {body}"))
}

async fn wait_backend_presence(ctx: &ScenarioCtx, name: &str, present: bool) -> Result<()> {
    wait_for(
        &format!("gateway EdgionBackend {BACKEND_NS}/{name} presence={present}"),
        Duration::from_secs(15),
        || async {
            let items = gateway_list(ctx, "/configclient/edgionbackend/list").await?;
            Ok(items.iter().any(|item| resource_named(item, BACKEND_NS, name)) == present)
        },
    )
    .await
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

async fn wait_policy_marker_class(ctx: &ScenarioCtx, name: &str, class: Option<&str>) -> Result<()> {
    wait_for(
        &format!("policy {BACKEND_NS}/{name} target class={class:?}"),
        Duration::from_secs(15),
        || async {
            let items = gateway_list(ctx, "/configclient/edgionbackendtrafficpolicy/list").await?;
            let found = items.iter().find(|item| resource_named(item, BACKEND_NS, name));
            Ok(match class {
                Some(expected) => found.is_some_and(|item| {
                    item.pointer("/spec/resolvedTargetClass").and_then(Value::as_str) == Some(expected)
                }),
                None => found.is_none_or(|item| {
                    item.pointer("/spec/resolvedTargetClass")
                        .and_then(Value::as_str)
                        .is_none()
                }),
            })
        },
    )
    .await
}

async fn wait_policy_marker(ctx: &ScenarioCtx, name: &str, present: bool) -> Result<()> {
    wait_policy_marker_class(ctx, name, present.then_some("ai")).await
}

async fn ai_request_status(ctx: &ScenarioCtx) -> Result<reqwest::StatusCode> {
    Ok(ai_request(ctx).await?.status())
}

async fn ai_request(ctx: &ScenarioCtx) -> Result<reqwest::Response> {
    ai_request_model(ctx, "fixture-model").await
}

async fn ai_request_model(ctx: &ScenarioCtx, model: &str) -> Result<reqwest::Response> {
    ai_request_model_with_header(ctx, model, None).await
}

async fn ai_request_model_with_header(ctx: &ScenarioCtx, model: &str, tier: Option<&str>) -> Result<reqwest::Response> {
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
    let mut request = client
        .post(format!("http://{HOST}:{AI_GATEWAY_PORT}/v1/chat/completions"))
        .json(&json!({"model": model, "messages": [{"role": "user", "content": "hello"}]}));
    if let Some(tier) = tier {
        request = request.header("x-model-tier", tier);
    }
    request.send().await.context("AI lifecycle request failed")
}

async fn model_list(ctx: &ScenarioCtx) -> Result<Vec<String>> {
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
        .context("failed to build model-list request client")?;
    let response = client
        .get(format!("http://{HOST}:{AI_GATEWAY_PORT}/v1/models"))
        .send()
        .await
        .context("AI model-list request failed")?
        .error_for_status()
        .context("AI model-list returned an error")?;
    let value: Value = response.json().await.context("AI model-list response is not JSON")?;
    Ok(value
        .pointer("/data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
        .collect())
}

async fn wait_hot_backend_observation(ctx: &ScenarioCtx, economy: bool, streaming: bool) -> Result<()> {
    wait_for(
        &format!("hot backend observation economy={economy} streaming={streaming}"),
        Duration::from_secs(15),
        || async {
            let response = ai_request(ctx).await?;
            if !response.status().is_success() {
                return Ok(false);
            }
            let body: Value = response.json().await.context("hot backend response is not JSON")?;
            Ok(
                body.pointer("/mock/model_is_economy").and_then(Value::as_bool) == Some(economy)
                    && body.pointer("/mock/stream_requested").and_then(Value::as_bool) == Some(streaming),
            )
        },
    )
    .await
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
        ("EdgionBackendTrafficPolicy", BACKEND_NS, SERVICE_POLICY),
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
        ("Service", BACKEND_NS, "lifecycle-service"),
        ("Service", BACKEND_NS, BACKEND_A),
        ("Secret", SECRET_NS, SECRET),
        ("Secret", SECRET_NS, SECRET_B),
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

    // Verify the current Controller and Gateway representations before
    // exercising dependency transitions.
    wait_for(
        "Controller nested EdgionBackend spec.ai",
        Duration::from_secs(15),
        || async {
            let backend = controller_resource(ctx, "EdgionBackend", BACKEND_NS, BACKEND_A).await?;
            Ok(backend.pointer("/spec/ai").is_some())
        },
    )
    .await?;
    wait_backend_presence(ctx, BACKEND_A, true).await?;

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

    // Updating an existing grant must requeue the same dependency edge; this
    // is distinct from the delete/recreate lifecycle below.
    apply(ctx, denying_secret_grant_yaml()).await?;
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

    // A partially-resolved credential pool remains usable. Removing its only
    // good Secret must fail closed; adding the previously missing Secret
    // repairs traffic while preserving PartiallyInvalid until both resolve.
    apply(ctx, backend_yaml_with_secrets(BACKEND_A, &[SECRET, SECRET_B])).await?;
    wait_condition(
        ctx,
        "EdgionBackend",
        BACKEND_NS,
        BACKEND_A,
        "PartiallyInvalid",
        "True",
        Some("InvalidCredentials"),
    )
    .await?;
    wait_traffic(ctx, true).await?;
    ctx.test_ctx
        .delete_resource("Secret", SECRET_NS, SECRET)
        .await
        .map_err(anyhow::Error::msg)?;
    wait_condition(
        ctx,
        "EdgionBackend",
        BACKEND_NS,
        BACKEND_A,
        "PartiallyInvalid",
        "False",
        Some("AllRulesValid"),
    )
    .await?;
    wait_traffic(ctx, false).await?;
    apply(ctx, secret_yaml_named(SECRET_B, "ai-provider-rotated-test-key")).await?;
    wait_condition(
        ctx,
        "EdgionBackend",
        BACKEND_NS,
        BACKEND_A,
        "PartiallyInvalid",
        "True",
        Some("InvalidCredentials"),
    )
    .await?;
    wait_traffic(ctx, true).await?;
    apply(ctx, secret_yaml()).await?;
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

    // Repoint A→B, then remove and recreate A. The dropped Secret dependency
    // must stay inert while requests continue through B.
    apply(ctx, backend_yaml_with_secrets(BACKEND_A, &[SECRET_B])).await?;
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
    ctx.test_ctx
        .delete_resource("Secret", SECRET_NS, SECRET)
        .await
        .map_err(anyhow::Error::msg)?;
    wait_traffic(ctx, true).await?;
    apply(ctx, secret_yaml_named(SECRET, "old-secret-must-stay-inert")).await?;
    wait_traffic(ctx, true).await?;
    apply(ctx, backend_yaml(BACKEND_A)).await?;
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

    // Update endpoint, model/alias and defaults in place and observe the new
    // immutable Gateway snapshot through a fresh request. The configclient
    // check pins maxOutputTokens, which the mock provider cannot observe.
    apply(ctx, hot_backend_yaml(BACKEND_A, "prefix")).await?;
    wait_hot_backend_observation(ctx, true, false).await?;
    wait_for("hot backend maxOutputTokens=1234", Duration::from_secs(15), || async {
        let items = gateway_list(ctx, "/configclient/edgionbackend/list").await?;
        Ok(items
            .iter()
            .find(|item| resource_named(item, BACKEND_NS, BACKEND_A))
            .is_some_and(|item| {
                item.pointer("/spec/ai/defaults/maxOutputTokens")
                    .and_then(Value::as_u64)
                    == Some(1234)
            }))
    })
    .await?;
    wait_for(
        "public model list after backend reload",
        Duration::from_secs(15),
        || async { Ok(model_list(ctx).await? == vec!["economy-model".to_string(), "fixture-model".to_string()]) },
    )
    .await?;
    apply(ctx, routed_plugin_yaml()).await?;
    wait_for(
        "model route rewrite after plugin reload",
        Duration::from_secs(15),
        || async {
            let response = ai_request_model(ctx, "virtual-chat").await?;
            if !response.status().is_success() {
                return Ok(false);
            }
            let body: Value = response.json().await.context("routed backend response is not JSON")?;
            Ok(body.pointer("/mock/model_is_economy").and_then(Value::as_bool) == Some(true))
        },
    )
    .await?;
    wait_for("virtual model visibility", Duration::from_secs(15), || async {
        Ok(model_list(ctx).await?.iter().any(|model| model == "virtual-*"))
    })
    .await?;
    wait_for("header model route", Duration::from_secs(15), || async {
        Ok(ai_request_model_with_header(ctx, "header-only", Some("paid"))
            .await?
            .status()
            .is_success())
    })
    .await?;
    anyhow::ensure!(
        ai_request_model_with_header(ctx, "header-only", Some("free"))
            .await?
            .status()
            == reqwest::StatusCode::NOT_FOUND,
        "header mismatch must not select the model route"
    );
    anyhow::ensure!(
        !ai_request_model(ctx, "blocked-model").await?.status().is_success(),
        "an all-zero candidate filter must exhaust routing"
    );
    apply(ctx, plugin_yaml()).await?;
    wait_for("model route removal", Duration::from_secs(15), || async {
        Ok(ai_request_model(ctx, "virtual-chat").await?.status() == reqwest::StatusCode::NOT_FOUND)
    })
    .await?;
    apply(ctx, hot_backend_yaml(BACKEND_A, "fail-503")).await?;
    wait_traffic(ctx, false).await?;
    apply(ctx, backend_yaml(BACKEND_A)).await?;
    wait_hot_backend_observation(ctx, false, false).await?;

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
    wait_backend_presence(ctx, BACKEND_A, false).await?;
    apply(ctx, backend_yaml(BACKEND_A)).await?;
    wait_condition(ctx, "HTTPRoute", ROUTE_NS, ROUTE, "ResolvedRefs", "True", None).await?;
    wait_traffic(ctx, true).await?;
    wait_backend_presence(ctx, BACKEND_A, true).await?;

    // Replace the route target A→B. Deleting the dropped target must not
    // perturb the route or traffic, proving old dependency edges were cleared.
    apply(ctx, route_yaml_target(BACKEND_B)).await?;
    wait_condition(ctx, "HTTPRoute", ROUTE_NS, ROUTE, "ResolvedRefs", "True", None).await?;
    wait_traffic(ctx, true).await?;
    ctx.test_ctx
        .delete_resource("EdgionBackend", BACKEND_NS, BACKEND_A)
        .await
        .map_err(anyhow::Error::msg)?;
    wait_traffic(ctx, true).await?;
    apply(ctx, backend_yaml(BACKEND_A)).await?;
    apply(ctx, route_yaml()).await?;
    wait_condition(ctx, "HTTPRoute", ROUTE_NS, ROUTE, "ResolvedRefs", "True", None).await?;
    wait_traffic(ctx, true).await
}

async fn policy_lifecycle(ctx: &ScenarioCtx) -> Result<()> {
    // A real Service target makes the mixed-capability rejection distinct from
    // the already-covered missing-target path. A same-name Service and
    // EdgionBackend also prove that policy arbitration keeps typed identities.
    apply(ctx, service_yaml("lifecycle-service")).await?;
    apply(ctx, service_yaml(BACKEND_A)).await?;
    apply(ctx, service_policy_yaml(SERVICE_POLICY, BACKEND_A)).await?;
    wait_policy_marker_class(ctx, SERVICE_POLICY, Some("service")).await?;

    apply(ctx, policy_yaml(WINNER_POLICY, BACKEND_B, 3)).await?;
    wait_policy_marker(ctx, WINNER_POLICY, true).await?;

    ctx.test_ctx
        .delete_resource("EdgionBackend", BACKEND_NS, BACKEND_B)
        .await
        .map_err(anyhow::Error::msg)?;
    wait_policy_marker(ctx, WINNER_POLICY, false).await?;
    apply(ctx, backend_yaml(BACKEND_B)).await?;
    wait_policy_marker(ctx, WINNER_POLICY, true).await?;

    // Mutate the already-published policy in place. Lenient admission retains
    // the raw resource for status reporting, while clearing the transport
    // marker must withdraw the previous generation from every target.
    apply(ctx, incompatible_policy_yaml(WINNER_POLICY)).await?;
    wait_condition(
        ctx,
        "EdgionBackendTrafficPolicy",
        BACKEND_NS,
        WINNER_POLICY,
        "Accepted",
        "False",
        Some("Invalid"),
    )
    .await?;
    wait_policy_marker(ctx, WINNER_POLICY, false).await?;
    apply(ctx, policy_yaml(WINNER_POLICY, BACKEND_B, 3)).await?;
    wait_condition(
        ctx,
        "EdgionBackendTrafficPolicy",
        BACKEND_NS,
        WINNER_POLICY,
        "Accepted",
        "True",
        Some("Accepted"),
    )
    .await?;
    wait_policy_marker(ctx, WINNER_POLICY, true).await?;

    apply(ctx, mixed_policy_yaml(WINNER_POLICY)).await?;
    wait_condition(
        ctx,
        "EdgionBackendTrafficPolicy",
        BACKEND_NS,
        WINNER_POLICY,
        "Accepted",
        "False",
        Some("Invalid"),
    )
    .await?;
    wait_policy_marker(ctx, WINNER_POLICY, false).await?;
    apply(ctx, policy_yaml(WINNER_POLICY, BACKEND_B, 3)).await?;
    wait_policy_marker(ctx, WINNER_POLICY, true).await?;
    // The same-name Service policy must remain independently published across
    // every AI policy transition above.
    wait_policy_marker_class(ctx, SERVICE_POLICY, Some("service")).await?;

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
    reference_lifecycle(ctx).await?;
    // The AI backend store is shared by the whole harness, so publication is
    // proven by this scenario's own pair rather than by a global total.
    wait_backend_presence(ctx, BACKEND_A, true).await?;
    wait_backend_presence(ctx, BACKEND_B, true).await?;
    policy_lifecycle(ctx).await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        cleanup(&ctx).await;
        // The store is shared with every other scenario, so the baseline is
        // only captured once the Gateway has dropped this scenario's own
        // backends.
        let baseline = async {
            wait_backend_presence(&ctx, BACKEND_A, false).await?;
            wait_backend_presence(&ctx, BACKEND_B, false).await?;
            gateway_store_count(&ctx, "edgion_backend_store").await
        }
        .await?;
        let result = run_inner(&ctx).await;
        cleanup(&ctx).await;
        // Cleanup is proven by this scenario's own entries leaving the runtime
        // store. The unified count only has to stay at or below the baseline:
        // an unrelated scenario's pending removal may still land here, but a
        // leaked lifecycle backend would push the total above it.
        let cleanup_result = async {
            wait_backend_presence(&ctx, BACKEND_A, false).await?;
            wait_backend_presence(&ctx, BACKEND_B, false).await?;
            wait_for(
                "lifecycle AI runtime-store cleanup",
                Duration::from_secs(15),
                || async { Ok(gateway_store_count(&ctx, "edgion_backend_store").await? <= baseline) },
            )
            .await
        }
        .await;
        cleanup_result?;
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
            routed_plugin_yaml(),
            route_yaml(),
            route_grant_yaml(),
            secret_grant_yaml(),
            policy_yaml(WINNER_POLICY, BACKEND_B, 3),
            policy_yaml(SIBLING_POLICY, BACKEND_B, 7),
            incompatible_policy_yaml(WINNER_POLICY),
            mixed_policy_yaml(WINNER_POLICY),
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
