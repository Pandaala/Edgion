//! Dynamic conformance coverage for weighted EdgionBackend route slots.

use crate::dynamic::scenario::{Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;

const ROUTE_NS: &str = "edgion-test";
const BACKEND_NS: &str = "edgion-backend";
const ROUTE: &str = "weighted-ai-route";
const CHILD_ROUTE: &str = "weighted-ai-child";
const PLUGIN: &str = "weighted-ai-proxy";
const BACKEND_A: &str = "weighted-ai-a";
const BACKEND_B: &str = "weighted-ai-b";
const SECRET_A: &str = "weighted-ai-key-a";
const SECRET_B: &str = "weighted-ai-key-b";
const ZERO_SERVICE: &str = "weighted-zero-service";
const ROUTE_GRANT: &str = "allow-weighted-ai-route";
const HOST: &str = "ai-backend-weighted.example.com";
const AI_GATEWAY_PORT: u16 = 31945;
const SAMPLE_SIZE: u128 = 128;
const WEIGHTED_A_MIN: usize = 16;
const WEIGHTED_A_MAX: usize = 48;

fn provider_endpoint(endpoint_path: &str) -> String {
    provider_endpoint_for(
        std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true"),
        endpoint_path,
    )
}

fn provider_endpoint_for(k8s_mode: bool, endpoint_path: &str) -> String {
    let authority = if k8s_mode {
        "edgion-test-server.edgion-test.svc.cluster.local:30001"
    } else {
        "127.0.0.1:30001"
    };
    format!("http://{authority}/ai/{endpoint_path}")
}

#[derive(Debug, Default, PartialEq, Eq)]
struct TrafficSample {
    backend_a: usize,
    backend_b: usize,
    failed: usize,
}

impl TrafficSample {
    fn matches_weighted(&self, expected: &Self) -> bool {
        let backend_a_matches = (WEIGHTED_A_MIN..=WEIGHTED_A_MAX).contains(&self.backend_a);
        backend_a_matches
            && self.backend_a + self.backend_b + self.failed == SAMPLE_SIZE as usize
            && (expected.backend_b == 0) == (self.backend_b == 0)
            && (expected.failed == 0) == (self.failed == 0)
    }
}

fn secret_yaml(name: &str) -> String {
    format!(
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: {name}\n  namespace: {BACKEND_NS}\ntype: Opaque\nstringData:\n  api-key: ai-provider-test-key\n"
    )
}

fn backend_yaml(name: &str, secret: &str, endpoint_path: &str) -> String {
    let endpoint = provider_endpoint(endpoint_path);
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackend\nmetadata:\n  name: {name}\n  namespace: {BACKEND_NS}\nspec:\n  ai:\n    provider: OpenAICompatible\n    endpoint: {endpoint}\n    allowInsecureHttp: true\n    credentialPool:\n      credentials:\n        - name: primary\n          secretRef:\n            name: {secret}\n    models:\n      - name: fixture-model\n"
    )
}

fn plugin_yaml() -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionPlugins\nmetadata:\n  name: {PLUGIN}\n  namespace: {ROUTE_NS}\nspec:\n  requestPlugins:\n    - enable: true\n      type: AiProxy\n      config:\n        backendSelection:\n          type: Weighted\n"
    )
}

fn zero_service_yaml() -> String {
    format!(
        "apiVersion: v1\nkind: Service\nmetadata:\n  name: {ZERO_SERVICE}\n  namespace: {ROUTE_NS}\nspec:\n  ports:\n    - name: http\n      port: 30001\n      targetPort: 30001\n"
    )
}

fn route_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: {ROUTE}\n  namespace: {ROUTE_NS}\nspec:\n  parentRefs:\n    - name: ai-gateway\n  hostnames:\n    - {HOST}\n  rules:\n    - filters:\n        - type: ExtensionRef\n          extensionRef:\n            group: edgion.io\n            kind: EdgionPlugins\n            name: {PLUGIN}\n      backendRefs:\n        - group: edgion.io\n          kind: EdgionBackend\n          name: {BACKEND_A}\n          namespace: {BACKEND_NS}\n          weight: 1\n        - group: edgion.io\n          kind: EdgionBackend\n          name: {BACKEND_B}\n          namespace: {BACKEND_NS}\n          weight: 3\n        - kind: Service\n          name: {ZERO_SERVICE}\n          port: 30001\n          weight: 0\n"
    )
}

fn child_route_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: {CHILD_ROUTE}\n  namespace: {ROUTE_NS}\nspec:\n  rules:\n    - filters:\n        - type: ExtensionRef\n          extensionRef:\n            group: edgion.io\n            kind: EdgionPlugins\n            name: {PLUGIN}\n      backendRefs:\n        - group: edgion.io\n          kind: EdgionBackend\n          name: {BACKEND_A}\n          namespace: {BACKEND_NS}\n          weight: 1\n        - group: edgion.io\n          kind: EdgionBackend\n          name: {BACKEND_B}\n          namespace: {BACKEND_NS}\n          weight: 3\n        - kind: Service\n          name: {ZERO_SERVICE}\n          port: 30001\n          weight: 0\n"
    )
}

fn delegated_parent_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: {ROUTE}\n  namespace: {ROUTE_NS}\nspec:\n  parentRefs:\n    - name: ai-gateway\n  hostnames:\n    - {HOST}\n  rules:\n    - backendRefs:\n        - group: gateway.networking.k8s.io\n          kind: HTTPRoute\n          name: {CHILD_ROUTE}\n"
    )
}

fn route_grant_yaml(allow_backend_b: bool) -> String {
    let backend_b = if allow_backend_b {
        format!("    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_B}\n")
    } else {
        String::new()
    };
    format!(
        "apiVersion: gateway.networking.k8s.io/v1beta1\nkind: ReferenceGrant\nmetadata:\n  name: {ROUTE_GRANT}\n  namespace: {BACKEND_NS}\nspec:\n  from:\n    - group: gateway.networking.k8s.io\n      kind: HTTPRoute\n      namespace: {ROUTE_NS}\n  to:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_A}\n{backend_b}"
    )
}

fn authed(ctx: &ScenarioCtx, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match ctx.test_ctx.admin_token.as_deref() {
        Some(token) if !token.is_empty() => request.bearer_auth(token),
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

async fn wait_route_condition(ctx: &ScenarioCtx, status: &str, reason: Option<&str>) -> Result<()> {
    wait_for(
        &format!("HTTPRoute {ROUTE} ResolvedRefs={status}/{reason:?}"),
        || async {
            let route = controller_resource(ctx, "HTTPRoute", ROUTE_NS, ROUTE).await?;
            Ok(has_condition(&route, "ResolvedRefs", status, reason))
        },
    )
    .await
}

async fn apply(ctx: &ScenarioCtx, yaml: String) -> Result<()> {
    let path = crate::scratch::scratch_dir()
        .await?
        .join(format!("edgion-ai-backend-weighted-{}.yaml", std::process::id()));
    tokio::fs::write(&path, yaml)
        .await
        .context("failed to write weighted AI apply manifest")?;
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let result = crate::dynamic::apply::apply_path(&client, &path).await;
    let _ = tokio::fs::remove_file(&path).await;
    result
}

async fn request(ctx: &ScenarioCtx, entropy: u128, header_probe: bool) -> Result<(reqwest::StatusCode, Value)> {
    let target = ctx.target_socket_addr(AI_GATEWAY_PORT).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .resolve(HOST, target)
        .build()
        .context("failed to build weighted AI request client")?;
    let query = if header_probe { "?mock_mode=header_modifier" } else { "" };
    let mut request = client
        .post(format!("http://{HOST}:{AI_GATEWAY_PORT}/v1/chat/completions{query}"))
        .header(
            "traceparent",
            crate::access_log_client::traceparent_for_marker(uuid::Uuid::from_u128(entropy).to_string()),
        );
    if header_probe {
        request = request
            .header("x-header-modifier-set", "client")
            .header("x-header-modifier-add", "client-a")
            .header("x-header-modifier-add", "client-b")
            .header("x-header-modifier-remove", "client");
    }
    let response = request
        .json(&json!({"model": "fixture-model", "messages": []}))
        .send()
        .await
        .context("weighted AI request failed")?;
    let status = response.status();
    if !status.is_success() {
        return Ok((status, Value::Null));
    }
    let body: Value = response.json().await.context("weighted AI response was not JSON")?;
    let path_has_prefix = body.pointer("/mock/path_has_prefix").and_then(Value::as_bool);
    anyhow::ensure!(
        path_has_prefix.is_some(),
        "weighted AI response has no backend observation: {body}"
    );
    Ok((status, body))
}

async fn sample(ctx: &ScenarioCtx) -> Result<TrafficSample> {
    let mut sample = TrafficSample::default();
    for entropy in 0..SAMPLE_SIZE {
        let (status, body) = request(ctx, entropy, false).await?;
        match (
            status.is_success(),
            body.pointer("/mock/path_has_prefix").and_then(Value::as_bool),
        ) {
            (false, _) => sample.failed += 1,
            (_, Some(true)) => sample.backend_a += 1,
            (_, Some(false)) => sample.backend_b += 1,
            _ => unreachable!("successful responses always carry a backend observation"),
        }
    }
    Ok(sample)
}

async fn wait_sample(ctx: &ScenarioCtx, expected: &TrafficSample, label: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let actual = sample(ctx).await?;
        if actual.matches_weighted(expected) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for {label}; expected backend A in {WEIGHTED_A_MIN}..={WEIGHTED_A_MAX} of {SAMPLE_SIZE}, backend B present: {}, failures present: {}; got {actual:?}",
                expected.backend_b > 0,
                expected.failed > 0
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn cleanup(ctx: &ScenarioCtx) {
    for (kind, namespace, name) in [
        ("ReferenceGrant", BACKEND_NS, ROUTE_GRANT),
        ("HTTPRoute", ROUTE_NS, ROUTE),
        ("HTTPRoute", ROUTE_NS, CHILD_ROUTE),
        ("EdgionPlugins", ROUTE_NS, PLUGIN),
        ("Service", ROUTE_NS, ZERO_SERVICE),
        ("EdgionBackend", BACKEND_NS, BACKEND_A),
        ("EdgionBackend", BACKEND_NS, BACKEND_B),
        ("Secret", BACKEND_NS, SECRET_A),
        ("Secret", BACKEND_NS, SECRET_B),
    ] {
        let _ = ctx.test_ctx.delete_resource(kind, namespace, name).await;
    }
}

async fn check_header_modifier_updates(ctx: &ScenarioCtx) -> Result<()> {
    // Change the first equivalent operation, restore it, then remove the filter.
    for first in [Some("first"), Some("updated"), Some("first"), None] {
        let mut route: Value = serde_yaml::from_str(&route_yaml())?;
        if let Some(first) = first {
            for index in 0..2 {
                route["spec"]["rules"][0]["backendRefs"][index]["filters"] = json!([{
                    "type": "RequestHeaderModifier",
                    "requestHeaderModifier": {
                        "set": [
                            {"name": "X-Header-Modifier-Set", "value": first},
                            {"name": "x-header-modifier-set", "value": "ignored"}
                        ],
                        "add": [
                            {"name": "X-Header-Modifier-Add", "value": first},
                            {"name": "x-header-modifier-add", "value": "ignored"}
                        ],
                        "remove": ["X-Header-Modifier-Remove", "x-header-modifier-remove"]
                    }
                }]);
            }
        }
        apply(ctx, serde_yaml::to_string(&route)?).await?;
        let expected = match first {
            Some(first) => json!({"set": [first], "add": ["client-a", "client-b", first], "remove": []}),
            None => json!({"set": ["client"], "add": ["client-a", "client-b"], "remove": ["client"]}),
        };
        wait_for("AI backend first-equivalent header generation", || async {
            // A complete weighted cycle observes both backends.
            for entropy in 0..4 {
                let (status, body) = request(ctx, entropy, true).await?;
                if !status.is_success() || body["mock"]["header_modifier"] != expected {
                    return Ok(false);
                }
            }
            Ok(true)
        })
        .await?;
    }
    Ok(())
}

async fn run_inner(ctx: &ScenarioCtx) -> Result<()> {
    apply(ctx, secret_yaml(SECRET_A)).await?;
    apply(ctx, secret_yaml(SECRET_B)).await?;
    apply(ctx, backend_yaml(BACKEND_A, SECRET_A, "prefix")).await?;
    apply(ctx, backend_yaml(BACKEND_B, SECRET_B, "weighted-b")).await?;
    apply(ctx, zero_service_yaml()).await?;
    apply(ctx, plugin_yaml()).await?;
    apply(ctx, route_grant_yaml(true)).await?;
    apply(ctx, route_yaml()).await?;

    let healthy = TrafficSample {
        backend_a: 32,
        backend_b: 96,
        failed: 0,
    };
    let invalid_b = TrafficSample {
        backend_a: 32,
        backend_b: 0,
        failed: 96,
    };

    wait_route_condition(ctx, "True", None).await?;
    wait_sample(ctx, &healthy, "1:3 weighted AI distribution with a zero-weight Service").await?;

    check_header_modifier_updates(ctx).await?;

    ctx.test_ctx
        .delete_resource("EdgionBackend", BACKEND_NS, BACKEND_B)
        .await
        .map_err(anyhow::Error::msg)?;
    wait_route_condition(ctx, "False", Some("BackendNotFound")).await?;
    wait_sample(
        ctx,
        &invalid_b,
        "missing positive-weight slot preserved without redistribution",
    )
    .await?;
    apply(ctx, backend_yaml(BACKEND_B, SECRET_B, "weighted-b")).await?;
    wait_route_condition(ctx, "True", None).await?;
    wait_sample(ctx, &healthy, "weighted distribution restored after backend recreation").await?;

    apply(ctx, route_grant_yaml(false)).await?;
    wait_route_condition(ctx, "False", Some("RefNotPermitted")).await?;
    wait_sample(
        ctx,
        &invalid_b,
        "denied positive-weight slot preserved without redistribution",
    )
    .await?;
    apply(ctx, route_grant_yaml(true)).await?;
    wait_route_condition(ctx, "True", None).await?;
    wait_sample(
        ctx,
        &healthy,
        "weighted distribution restored after ReferenceGrant update",
    )
    .await?;

    apply(ctx, child_route_yaml()).await?;
    apply(ctx, delegated_parent_yaml()).await?;
    wait_route_condition(ctx, "True", None).await?;
    wait_sample(ctx, &healthy, "delegation preserves AI slot order and weights").await?;
    apply(ctx, route_grant_yaml(false)).await?;
    wait_route_condition(ctx, "False", Some("RefNotPermitted")).await?;
    wait_sample(
        ctx,
        &invalid_b,
        "delegation preserves denied positive-weight slot identity",
    )
    .await?;
    apply(ctx, route_grant_yaml(true)).await?;
    wait_route_condition(ctx, "True", None).await?;
    wait_sample(
        ctx,
        &healthy,
        "delegated weighted distribution restored after authorization",
    )
    .await
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
    id: "AiBackendWeighted",
    conf_path: "EdgionBackend/Weighted/DynamicTest",
    run,
    phases: &[],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_endpoint_uses_the_k8s_service_only_in_k8s_mode() {
        assert_eq!(
            provider_endpoint_for(false, "weighted-b"),
            "http://127.0.0.1:30001/ai/weighted-b"
        );
        assert_eq!(
            provider_endpoint_for(true, "weighted-b"),
            "http://edgion-test-server.edgion-test.svc.cluster.local:30001/ai/weighted-b"
        );
    }

    #[test]
    fn manifests_are_valid_and_preserve_weighted_slot_shape() {
        for yaml in [
            secret_yaml(SECRET_A),
            secret_yaml(SECRET_B),
            backend_yaml(BACKEND_A, SECRET_A, "prefix"),
            backend_yaml(BACKEND_B, SECRET_B, "weighted-b"),
            zero_service_yaml(),
            plugin_yaml(),
            route_grant_yaml(false),
            route_grant_yaml(true),
            route_yaml(),
            child_route_yaml(),
            delegated_parent_yaml(),
        ] {
            serde_yaml::from_str::<Value>(&yaml)
                .unwrap_or_else(|error| panic!("invalid weighted AI YAML: {error}\n{yaml}"));
        }

        let route: Value = serde_yaml::from_str(&child_route_yaml()).unwrap();
        let refs = route
            .pointer("/spec/rules/0/backendRefs")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(refs[0]["weight"], 1);
        assert_eq!(refs[1]["weight"], 3);
        assert_eq!(refs[2]["kind"], "Service");
        assert_eq!(refs[2]["weight"], 0);

        let child: Value = serde_yaml::from_str(&child_route_yaml()).unwrap();
        assert_eq!(
            child.pointer("/spec/rules/0/backendRefs"),
            Some(&Value::Array(refs.clone()))
        );
        let parent: Value = serde_yaml::from_str(&delegated_parent_yaml()).unwrap();
        assert_eq!(
            parent.pointer("/spec/rules/0/backendRefs/0/kind"),
            Some(&Value::String("HTTPRoute".into()))
        );

        let denied: Value = serde_yaml::from_str(&route_grant_yaml(false)).unwrap();
        let allowed: Value = serde_yaml::from_str(&route_grant_yaml(true)).unwrap();
        assert_eq!(denied.pointer("/spec/to").and_then(Value::as_array).unwrap().len(), 1);
        assert_eq!(allowed.pointer("/spec/to").and_then(Value::as_array).unwrap().len(), 2);
    }
}
