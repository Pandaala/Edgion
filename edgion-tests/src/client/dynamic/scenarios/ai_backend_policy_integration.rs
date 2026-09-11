//! Cross-component AI backend TLS and traffic-policy lifecycle coverage.

use crate::dynamic::scenario::{Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use anyhow::{Context, Result};
use futures::StreamExt;
use serde_json::{json, Value};
use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;

const NS: &str = "edgion-test";
const BACKEND_A: &str = "policy-integration-a";
const BACKEND_B: &str = "policy-integration-b";
const CREDENTIAL_A: &str = "policy-integration-key-a";
const CREDENTIAL_B: &str = "policy-integration-key-b";
const PLUGIN: &str = "policy-integration-ai-proxy";
const ROUTE: &str = "policy-integration-route";
const TLS_POLICY: &str = "policy-integration-tls";
const TRAFFIC_POLICY: &str = "policy-integration-traffic";
const MISSING_ROUTE: &str = "policy-integration-missing-route";
const HOST: &str = "ai-backend-policy-integration.example.com";
const AI_GATEWAY_PORT: u16 = 31945;

#[derive(Clone, Debug)]
struct ProviderBases {
    http: String,
    https: String,
    mtls: String,
    sni: String,
}

impl ProviderBases {
    fn from_env() -> Self {
        Self {
            http: env_base("EDGION_TEST_AI_PROVIDER_HTTP_BASE", "http://127.0.0.1:30001"),
            https: env_base("EDGION_TEST_AI_PROVIDER_HTTPS_BASE", "https://127.0.0.1:30051"),
            mtls: env_base("EDGION_TEST_AI_PROVIDER_MTLS_BASE", "https://127.0.0.1:30052"),
            sni: env_base("EDGION_TEST_AI_PROVIDER_SNI_BASE", "https://127.0.0.1:30026"),
        }
    }
}

fn env_base(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn gateway_name() -> String {
    std::env::var("EDGION_TEST_AI_GATEWAY_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ai-gateway".to_string())
}

fn secret_yaml(name: &str, key: &str) -> String {
    format!(
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: {name}\n  namespace: {NS}\ntype: Opaque\nstringData:\n  api-key: {key}\n"
    )
}

fn backend_yaml(name: &str, secret: &str, endpoint: &str) -> String {
    backend_yaml_with_max_output(name, secret, endpoint, None)
}

fn backend_yaml_with_max_output(name: &str, secret: &str, endpoint: &str, max_output: Option<u64>) -> String {
    let insecure_http = endpoint
        .starts_with("http://")
        .then_some("    allowInsecureHttp: true\n")
        .unwrap_or_default();
    let defaults = max_output.map_or(String::new(), |value| {
        format!("    defaults:\n      maxOutputTokens: {value}\n")
    });
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackend\nmetadata:\n  name: {name}\n  namespace: {NS}\nspec:\n  ai:\n    provider: OpenAICompatible\n    endpoint: {endpoint}\n{insecure_http}    credentialPool:\n      credentials:\n        - name: primary\n          secretRef:\n            name: {secret}\n    models:\n      - name: fixture-model\n{defaults}"
    )
}

fn plugin_yaml(selection: &str) -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionPlugins\nmetadata:\n  name: {PLUGIN}\n  namespace: {NS}\nspec:\n  requestPlugins:\n    - enable: true\n      type: AiProxy\n      config:\n        backendSelection:\n          type: {selection}\n"
    )
}

fn capture_route_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: {ROUTE}\n  namespace: {NS}\n  annotations:\n    edgion.io/retry-non-idempotent: \"true\"\nspec:\n  parentRefs:\n    - name: {}\n  hostnames:\n    - {HOST}\n  rules:\n    - filters:\n        - type: ExtensionRef\n          extensionRef:\n            group: edgion.io\n            kind: EdgionPlugins\n            name: {PLUGIN}\n      retry:\n        attempts: 1\n        codes:\n          - 503\n      backendRefs:\n        - group: edgion.io\n          kind: EdgionBackend\n          name: {BACKEND_A}\n        - group: edgion.io\n          kind: EdgionBackend\n          name: {BACKEND_B}\n",
        gateway_name()
    )
}

fn missing_route_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: {MISSING_ROUTE}\n  namespace: {NS}\nspec:\n  parentRefs:\n    - name: {}\n  hostnames:\n    - missing-ai-backend-policy-integration.example.com\n  rules:\n    - backendRefs:\n        - group: edgion.io\n          kind: EdgionBackend\n          name: policy-integration-absent\n",
        gateway_name()
    )
}

fn route_yaml(two_targets: bool) -> String {
    let second = if two_targets {
        format!(
            "        - group: edgion.io\n          kind: EdgionBackend\n          name: {BACKEND_B}\n          weight: 3\n"
        )
    } else {
        String::new()
    };
    format!(
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: {ROUTE}\n  namespace: {NS}\nspec:\n  parentRefs:\n    - name: {}\n  hostnames:\n    - {HOST}\n  rules:\n    - filters:\n        - type: ExtensionRef\n          extensionRef:\n            group: edgion.io\n            kind: EdgionPlugins\n            name: {PLUGIN}\n      backendRefs:\n        - group: edgion.io\n          kind: EdgionBackend\n          name: {BACKEND_A}\n          weight: 1\n{second}",
        gateway_name()
    )
}

fn tls_policy_yaml(name: &str, target: &str, hostname: &str, san_hostname: &str, client_cert: bool) -> String {
    let option = if client_cert {
        "  options:\n    edgion.io/client-certificate-ref: backend-client-cert\n"
    } else {
        ""
    };
    format!(
        "apiVersion: gateway.networking.k8s.io/v1\nkind: BackendTLSPolicy\nmetadata:\n  name: {name}\n  namespace: {NS}\nspec:\n  targetRefs:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {target}\n  validation:\n    caCertificateRefs:\n      - group: \"\"\n        kind: Secret\n        name: backend-ca\n    hostname: {hostname}\n    subjectAltNames:\n      - type: Hostname\n        hostname: {san_hostname}\n{option}"
    )
}

fn wrong_variant_tls_policy_yaml() -> String {
    format!(
        "apiVersion: gateway.networking.k8s.io/v1\nkind: BackendTLSPolicy\nmetadata:\n  name: policy-integration-wrong-variant\n  namespace: {NS}\nspec:\n  targetRefs:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_A}\n      sectionName: unsupported-port\n  validation:\n    caCertificateRefs:\n      - group: \"\"\n        kind: Secret\n        name: backend-ca\n    hostname: backend.example.com\n"
    )
}

fn traffic_policy_yaml(consecutive_errors: u32) -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackendTrafficPolicy\nmetadata:\n  name: {TRAFFIC_POLICY}\n  namespace: {NS}\nspec:\n  targetRefs:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_A}\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_B}\n  outlierDetection:\n    consecutiveErrors: {consecutive_errors}\n    ejectionTime: 2s\n"
    )
}

fn incompatible_traffic_policy_yaml() -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackendTrafficPolicy\nmetadata:\n  name: policy-integration-incompatible\n  namespace: {NS}\nspec:\n  targetRefs:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: {BACKEND_A}\n  loadBalancer:\n    type: LeastConn\n"
    )
}

fn missing_traffic_policy_yaml() -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackendTrafficPolicy\nmetadata:\n  name: policy-integration-missing\n  namespace: {NS}\nspec:\n  targetRefs:\n    - group: edgion.io\n      kind: EdgionBackend\n      name: policy-integration-absent\n  outlierDetection:\n    consecutiveErrors: 3\n"
    )
}

fn denied_backend_yaml(endpoint: &str) -> String {
    format!(
        "apiVersion: edgion.io/v1\nkind: EdgionBackend\nmetadata:\n  name: policy-integration-denied\n  namespace: {NS}\nspec:\n  ai:\n    provider: OpenAICompatible\n    endpoint: {endpoint}\n    credentialPool:\n      credentials:\n        - name: primary\n          secretRef:\n            name: policy-integration-denied-key\n            namespace: edgion-default\n    models:\n      - name: fixture-model\n"
    )
}

fn denied_secret_yaml() -> String {
    "apiVersion: v1\nkind: Secret\nmetadata:\n  name: policy-integration-denied-key\n  namespace: edgion-default\ntype: Opaque\nstringData:\n  api-key: ai-provider-test-key\n".to_string()
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

async fn apply(ctx: &ScenarioCtx, yaml: &str) -> Result<()> {
    ctx.test_ctx.apply_yaml(yaml).await.map_err(anyhow::Error::msg)
}

async fn controller_resource(ctx: &ScenarioCtx, kind: &str, name: &str) -> Result<Value> {
    let url = format!(
        "{}/configserver/{kind}?namespace={NS}&name={name}",
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
        .context("Controller resource response was not JSON")?;
    anyhow::ensure!(
        status.is_success() && body["success"] == true,
        "GET {url} returned {status}: {body}"
    );
    Ok(body["data"].clone())
}

async fn gateway_policies(ctx: &ScenarioCtx, kind: &str) -> Result<Vec<Value>> {
    let url = format!("{}/configclient/{kind}/list", ctx.gateway_url.trim_end_matches('/'));
    let response = gateway_authed(ctx.test_ctx.http_client.get(&url))
        .send()
        .await
        .with_context(|| format!("GET {url} failed"))?;
    let status = response.status();
    let body: Value = response.json().await.context("Gateway policy response was not JSON")?;
    anyhow::ensure!(
        status.is_success() && body["success"] == true,
        "GET {url} returned {status}: {body}"
    );
    body["data"]
        .as_array()
        .cloned()
        .context("Gateway policy response has no data array")
}

fn resource_named(value: &Value, name: &str) -> bool {
    value.pointer("/metadata/namespace").and_then(Value::as_str) == Some(NS)
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

async fn wait_for<F, Fut>(label: &str, mut check: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool>>,
{
    let deadline = Instant::now() + Duration::from_secs(20);
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
    wait_for(&format!("{kind}/{NS}/{name} {type_}={status}/{reason:?}"), || async {
        Ok(has_condition(
            &controller_resource(ctx, kind, name).await?,
            type_,
            status,
            reason,
        ))
    })
    .await
}

async fn wait_policy_class(ctx: &ScenarioCtx, kind: &str, name: &str, class: Option<&str>) -> Result<()> {
    wait_for(
        &format!("Gateway {kind}/{NS}/{name} target class={class:?}"),
        || async {
            let policies = gateway_policies(ctx, kind).await?;
            let found = policies.iter().find(|item| resource_named(item, name));
            Ok(match class {
                Some(expected) => found.is_some_and(|item| {
                    item.pointer("/spec/resolvedTargetClass").and_then(Value::as_str) == Some(expected)
                }),
                None => found.is_some_and(|item| {
                    item.pointer("/spec/resolvedTargetClass")
                        .and_then(Value::as_str)
                        .is_none()
                }),
            })
        },
    )
    .await
}

async fn wait_tls_hostname(ctx: &ScenarioCtx, name: &str, hostname: &str) -> Result<()> {
    wait_for(
        &format!("Gateway BackendTLSPolicy/{NS}/{name} hostname={hostname}"),
        || async {
            Ok(gateway_policies(ctx, "backendtlspolicy")
                .await?
                .iter()
                .find(|item| resource_named(item, name))
                .is_some_and(|item| {
                    item.pointer("/spec/validation/hostname").and_then(Value::as_str) == Some(hostname)
                }))
        },
    )
    .await
}

async fn wait_tls_san_hostname(ctx: &ScenarioCtx, name: &str, hostname: &str) -> Result<()> {
    wait_for(
        &format!("Gateway BackendTLSPolicy/{NS}/{name} SAN hostname={hostname}"),
        || async {
            Ok(gateway_policies(ctx, "backendtlspolicy")
                .await?
                .iter()
                .find(|item| resource_named(item, name))
                .and_then(|item| item.pointer("/spec/validation/subjectAltNames/0/hostname"))
                .and_then(Value::as_str)
                == Some(hostname))
        },
    )
    .await
}

fn provider_control_bases(provider_http_base: &str) -> Result<Vec<String>> {
    let endpoint_bases = std::env::var("EDGION_TEST_BACKEND_ENDPOINT_IPS")
        .ok()
        .into_iter()
        .flat_map(|ips| {
            ips.split(',')
                .map(str::trim)
                .filter(|ip| !ip.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .map(|ip| {
            let address = ip
                .parse::<std::net::IpAddr>()
                .with_context(|| format!("invalid test backend endpoint IP {ip}"))?;
            Ok(format!("http://{}", std::net::SocketAddr::new(address, 30001)))
        })
        .collect::<Result<Vec<_>>>()?;
    if endpoint_bases.is_empty() {
        Ok(vec![provider_http_base.to_string()])
    } else {
        Ok(endpoint_bases)
    }
}

async fn set_resilience_mode(ctx: &ScenarioCtx, bases: &[String], fixture: &str, mode: &str) -> Result<()> {
    for base in bases {
        let url = format!("{base}/ai-resilience/mode/{fixture}/half-open-primary/{mode}");
        let response = ctx
            .test_ctx
            .http_client
            .post(&url)
            .send()
            .await
            .with_context(|| format!("POST {url} failed"))?;
        anyhow::ensure!(
            response.status().is_success(),
            "POST {url} returned {}",
            response.status()
        );
    }
    Ok(())
}

async fn release_resilience_gate(ctx: &ScenarioCtx, bases: &[String], fixture: &str) -> Result<()> {
    for base in bases {
        let url = format!("{base}/ai-resilience/release/{fixture}/half-open-primary");
        let response = ctx
            .test_ctx
            .http_client
            .post(&url)
            .send()
            .await
            .with_context(|| format!("POST {url} failed"))?;
        anyhow::ensure!(
            response.status().is_success(),
            "POST {url} returned {}",
            response.status()
        );
    }
    Ok(())
}

async fn clear_resilience_fixture(ctx: &ScenarioCtx, bases: &[String], fixture: &str) {
    for base in bases {
        let url = format!("{base}/ai-resilience/count/{fixture}");
        let _ = ctx.test_ctx.http_client.get(&url).send().await;
    }
}

async fn wait_resilience_gate(ctx: &ScenarioCtx, bases: &[String], fixture: &str) -> Result<()> {
    wait_for("gated retry primary attempt", || async {
        resilience_gate_seen(ctx, bases, fixture).await
    })
    .await
}

async fn resilience_gate_seen(ctx: &ScenarioCtx, bases: &[String], fixture: &str) -> Result<bool> {
    for base in bases {
        let url = format!("{base}/ai-resilience/counts/{fixture}");
        if let Ok(response) = ctx.test_ctx.http_client.get(&url).send().await {
            if let Ok(value) = response.json::<Value>().await {
                if value.pointer("/half_open_primary").and_then(Value::as_u64).unwrap_or(0) > 0 {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

async fn wait_capture_topology(ctx: &ScenarioCtx, bases: &[String]) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let fixture = format!("policy-readiness-{}", uuid::Uuid::new_v4());
        set_resilience_mode(ctx, bases, &fixture, "fail").await?;
        let response = capture_retry_request(ctx, &fixture).await;
        let primary_seen = resilience_gate_seen(ctx, bases, &fixture).await?;
        clear_resilience_fixture(ctx, bases, &fixture).await;
        if let Ok(response) = response {
            if response.status().is_success() {
                let observation: Value = response.json().await.context("readiness response was not JSON")?;
                if primary_seen
                    && observation.pointer("/mock/credential_generation") == Some(&Value::String("b".into()))
                {
                    return Ok(());
                }
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for one Priority A-to-B fallback request");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn request_client(ctx: &ScenarioCtx) -> Result<reqwest::Client> {
    request_client_with_timeout(ctx, Duration::from_secs(15)).await
}

async fn request_client_with_timeout(ctx: &ScenarioCtx, timeout: Duration) -> Result<reqwest::Client> {
    let target = ctx.target_socket_addr(AI_GATEWAY_PORT).await?;
    reqwest::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .resolve(HOST, target)
        .build()
        .context("failed to build AI backend policy request client")
}

async fn ai_request(ctx: &ScenarioCtx, mode: Option<&str>, streaming: bool) -> Result<reqwest::Response> {
    let query = mode.map_or(String::new(), |mode| format!("?mock_mode={mode}"));
    request_client(ctx)
        .await?
        .post(format!("http://{HOST}:{AI_GATEWAY_PORT}/v1/chat/completions{query}"))
        .json(&json!({"model": "fixture-model", "stream": streaming, "messages": []}))
        .send()
        .await
        .context("AI backend policy request failed")
}

async fn capture_retry_request(ctx: &ScenarioCtx, fixture: &str) -> Result<reqwest::Response> {
    request_client_with_timeout(ctx, Duration::from_secs(135))
        .await?
        .post(format!("http://{HOST}:{AI_GATEWAY_PORT}/v1/chat/completions"))
        .header("x-trace-id", uuid::Uuid::new_v4().to_string())
        .header("x-resilience-fixture-id", fixture)
        .json(&json!({"model": "fixture-model", "stream": false, "messages": []}))
        .send()
        .await
        .context("request-stable retry request failed")
}

async fn wait_unary(ctx: &ScenarioCtx, success: bool) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let response = ai_request(ctx, None, false).await?;
        if response.status().is_success() == success {
            return if success {
                response.json().await.context("AI policy response was not JSON")
            } else {
                Ok(Value::Null)
            };
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for AI policy traffic success={success}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn assert_normal_eos(ctx: &ScenarioCtx) -> Result<()> {
    let response = ai_request(ctx, Some("paced"), true).await?;
    anyhow::ensure!(
        response.status().is_success(),
        "paced request returned {}",
        response.status()
    );
    let body = response.bytes().await.context("paced response body failed")?;
    anyhow::ensure!(
        body.windows(12).any(|window| window == b"data: [DONE]"),
        "paced response missed [DONE]"
    );
    anyhow::ensure!(
        body.windows(7).any(|window| window == b"\"usage\""),
        "paced response missed usage"
    );
    Ok(())
}

async fn wait_unary_path_prefix(ctx: &ScenarioCtx) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let last = match ai_request(ctx, None, false).await {
            Ok(response) if response.status().is_success() => match response.json::<Value>().await {
                Ok(body) if body.pointer("/mock/path_has_prefix") == Some(&Value::Bool(true)) => {
                    return Ok(body);
                }
                Ok(body) => format!("unexpected response body: {body}"),
                Err(error) => format!("response JSON failed: {error:#}"),
            },
            Ok(response) => format!("response status was {}", response.status()),
            Err(error) => format!("request failed: {error:#}"),
        };
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for AI policy prefix endpoint; last observation: {last}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn upstream_abort_sse_summary(response: reqwest::Response) -> Result<(usize, bool)> {
    anyhow::ensure!(
        response.status().is_success(),
        "upstream-abort request returned {}",
        response.status()
    );
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut events = 0usize;
    let mut saw_done = false;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            break;
        };
        pending.extend_from_slice(&chunk);
        while let Some(end) = pending.windows(2).position(|window| window == b"\n\n") {
            let frame = pending.drain(..end + 2).collect::<Vec<_>>();
            if frame.starts_with(b"data:") {
                events += 1;
                saw_done |= frame.windows(6).any(|window| window == b"[DONE]");
            }
        }
    }
    Ok((events, saw_done))
}

async fn wait_upstream_abort_ready(ctx: &ScenarioCtx) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let last = match ai_request(ctx, None, true).await {
            Ok(response) => match upstream_abort_sse_summary(response).await {
                Ok((3, false)) => return Ok(()),
                Ok((events, saw_done)) => format!("events={events}, done={saw_done}"),
                Err(error) => format!("request failed: {error:#}"),
            },
            Err(error) => format!("request failed: {error:#}"),
        };
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for the upstream-abort endpoint; last observation: {last}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn assert_request_stable_capture(ctx: &ScenarioCtx, bases: &ProviderBases) -> Result<()> {
    apply(
        ctx,
        &backend_yaml(
            BACKEND_A,
            CREDENTIAL_A,
            &format!("{}/ai/resilience-half-open-primary", bases.http),
        ),
    )
    .await?;
    apply(
        ctx,
        &backend_yaml(BACKEND_B, CREDENTIAL_B, &format!("{}/ai/prefix", bases.sni)),
    )
    .await?;
    apply(ctx, &plugin_yaml("Priority")).await?;
    apply(ctx, &capture_route_yaml()).await?;
    apply(
        ctx,
        &tls_policy_yaml(
            TLS_POLICY,
            BACKEND_B,
            "hostname-a.example.com",
            "hostname-a.example.com",
            false,
        ),
    )
    .await?;
    wait_tls_hostname(ctx, TLS_POLICY, "hostname-a.example.com").await?;

    let control_bases = provider_control_bases(&bases.http)?;
    wait_capture_topology(ctx, &control_bases).await?;

    let fixture = format!("policy-capture-{}", uuid::Uuid::new_v4());
    set_resilience_mode(ctx, &control_bases, &fixture, "probe-fail").await?;
    let request = capture_retry_request(ctx, &fixture);
    let policy_update = async {
        wait_resilience_gate(ctx, &control_bases, &fixture).await?;
        apply(
            ctx,
            &tls_policy_yaml(
                TLS_POLICY,
                BACKEND_B,
                "wrong.example.com",
                "hostname-a.example.com",
                false,
            ),
        )
        .await?;
        wait_tls_hostname(ctx, TLS_POLICY, "wrong.example.com").await?;
        set_resilience_mode(ctx, &control_bases, &fixture, "fail").await?;
        wait_for("fresh request observes the updated invalid TLS policy", || async {
            Ok(!capture_retry_request(ctx, &fixture).await?.status().is_success())
        })
        .await?;
        release_resilience_gate(ctx, &control_bases, &fixture).await
    };
    let (response, ()) = tokio::try_join!(request, policy_update)?;
    anyhow::ensure!(
        response.status().is_success(),
        "captured retry did not retain the pre-update TLS policy: {}",
        response.status()
    );
    let observation: Value = response.json().await.context("captured retry response was not JSON")?;
    anyhow::ensure!(
        observation.pointer("/mock/credential_generation") == Some(&Value::String("b".into())),
        "captured retry did not reach the TLS fallback backend: {observation}"
    );

    set_resilience_mode(ctx, &control_bases, &fixture, "healthy").await?;
    clear_resilience_fixture(ctx, &control_bases, &fixture).await;

    apply(
        ctx,
        &backend_yaml(BACKEND_A, CREDENTIAL_A, &format!("{}/ai/prefix", bases.sni)),
    )
    .await?;
    apply(
        ctx,
        &backend_yaml(BACKEND_B, CREDENTIAL_B, &format!("{}/ai/policy-b", bases.http)),
    )
    .await?;
    apply(ctx, &plugin_yaml("Weighted")).await?;
    apply(ctx, &route_yaml(false)).await?;
    apply(
        ctx,
        &tls_policy_yaml(
            TLS_POLICY,
            BACKEND_A,
            "hostname-a.example.com",
            "hostname-a.example.com",
            false,
        ),
    )
    .await?;
    let restored = wait_unary(ctx, true).await?;
    anyhow::ensure!(
        restored.pointer("/mock/host_is_expected") == Some(&Value::Bool(true)),
        "upstream Host was not preserved after capture restore: {restored}"
    );

    apply(
        ctx,
        &tls_policy_yaml(
            TLS_POLICY,
            BACKEND_A,
            "hostname-a.example.com",
            "hostname-b.example.com",
            false,
        ),
    )
    .await?;
    wait_tls_san_hostname(ctx, TLS_POLICY, "hostname-b.example.com").await?;
    wait_unary(ctx, false).await?;
    apply(
        ctx,
        &tls_policy_yaml(
            TLS_POLICY,
            BACKEND_A,
            "hostname-a.example.com",
            "hostname-a.example.com",
            false,
        ),
    )
    .await?;
    wait_tls_san_hostname(ctx, TLS_POLICY, "hostname-a.example.com").await?;
    wait_unary(ctx, true).await?;
    Ok(())
}

async fn assert_client_abort(ctx: &ScenarioCtx, provider_http_base: &str) -> Result<()> {
    let trace = uuid::Uuid::new_v4().to_string();
    let response = request_client(ctx)
        .await?
        .post(format!(
            "http://{HOST}:{AI_GATEWAY_PORT}/v1/chat/completions?mock_mode=long"
        ))
        .header("x-trace-id", &trace)
        .json(&json!({"model": "fixture-model", "stream": true, "messages": []}))
        .send()
        .await
        .context("client-abort request failed")?;
    anyhow::ensure!(
        response.status().is_success(),
        "client-abort request returned {}",
        response.status()
    );
    let mut body = response.bytes_stream();
    for _ in 0..3 {
        tokio::time::timeout(Duration::from_secs(3), body.next())
            .await
            .context("client-abort stream stalled")?
            .context("client-abort stream ended early")??;
    }
    drop(body);

    let observation_bases = provider_control_bases(provider_http_base)?;
    wait_for("provider observes downstream client abort", || async {
        for base in &observation_bases {
            let url = format!("{base}/ai-long-stream/observation/{trace}");
            if let Ok(response) = ctx.test_ctx.http_client.get(&url).send().await {
                if let Ok(value) = response.json::<Value>().await {
                    if value.pointer("/observation/upstream_cancelled") == Some(&Value::Bool(true)) {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    })
    .await
}

async fn assert_upstream_abort(ctx: &ScenarioCtx, bases: &ProviderBases) -> Result<()> {
    apply(
        ctx,
        &backend_yaml(
            BACKEND_A,
            CREDENTIAL_A,
            &format!("{}/ai/resilience-stream", bases.https),
        ),
    )
    .await?;
    wait_upstream_abort_ready(ctx).await?;
    apply(
        ctx,
        &backend_yaml(BACKEND_A, CREDENTIAL_A, &format!("{}/ai/prefix", bases.https)),
    )
    .await?;
    wait_unary_path_prefix(ctx).await?;
    Ok(())
}

async fn sample_weighted(ctx: &ScenarioCtx) -> Result<(usize, usize)> {
    let mut a = 0;
    let mut b = 0;
    for sample in 0..512 {
        let response = ai_request(ctx, None, false).await?;
        anyhow::ensure!(
            response.status().is_success(),
            "weighted request {sample} returned {}",
            response.status()
        );
        let body: Value = response.json().await.context("weighted response was not JSON")?;
        if body.pointer("/mock/credential_generation") == Some(&Value::String("a".into())) {
            a += 1;
        } else if body.pointer("/mock/credential_generation") == Some(&Value::String("b".into())) {
            b += 1;
        } else {
            anyhow::bail!("weighted response did not identify a credential generation: {body}");
        }
    }
    Ok((a, b))
}

async fn assert_weighted(ctx: &ScenarioCtx) -> Result<()> {
    // Client correlation IDs no longer select routing entropy. A single bounded
    // sample tests the live 1:3 policy; never retry a failed distribution until it
    // passes. Hoeffding bounds false rejection below 2 * exp(-16) per batch.
    // Exact integer selection remains covered by weighted_has_exact_small_integer_distribution.
    let (a, b) = sample_weighted(ctx).await?;
    anyhow::ensure!(
        a + b == 512 && (64..=192).contains(&a),
        "expected 1:3 weighted routing within 12.5 percentage points, got {a}:{b} over 512 requests"
    );
    Ok(())
}

async fn wait_weighted_generation(ctx: &ScenarioCtx, consecutive_errors: u64) -> Result<()> {
    wait_for("weighted route and traffic policy publication", || async {
        let routes = gateway_policies(ctx, "httproute").await?;
        let policies = gateway_policies(ctx, "edgionbackendtrafficpolicy").await?;
        let route_ready = routes
            .iter()
            .find(|item| resource_named(item, ROUTE))
            .is_some_and(|route| {
                route
                    .pointer("/spec/rules/0/backendRefs/0/weight")
                    .and_then(Value::as_u64)
                    == Some(1)
                    && route
                        .pointer("/spec/rules/0/backendRefs/1/weight")
                        .and_then(Value::as_u64)
                        == Some(3)
            });
        let policy_ready = policies
            .iter()
            .find(|item| resource_named(item, TRAFFIC_POLICY))
            .is_some_and(|policy| {
                policy
                    .pointer("/spec/outlierDetection/consecutiveErrors")
                    .and_then(Value::as_u64)
                    == Some(consecutive_errors)
            });
        Ok(route_ready && policy_ready)
    })
    .await
}

async fn cleanup(ctx: &ScenarioCtx) {
    for (kind, namespace, name) in [
        ("BackendTLSPolicy", NS, TLS_POLICY),
        ("BackendTLSPolicy", NS, "policy-integration-wrong-variant"),
        ("EdgionBackendTrafficPolicy", NS, TRAFFIC_POLICY),
        ("EdgionBackendTrafficPolicy", NS, "policy-integration-incompatible"),
        ("EdgionBackendTrafficPolicy", NS, "policy-integration-missing"),
        ("HTTPRoute", NS, ROUTE),
        ("HTTPRoute", NS, MISSING_ROUTE),
        ("EdgionPlugins", NS, PLUGIN),
        ("EdgionBackend", NS, BACKEND_A),
        ("EdgionBackend", NS, BACKEND_B),
        ("EdgionBackend", NS, "policy-integration-denied"),
        ("Secret", NS, CREDENTIAL_A),
        ("Secret", NS, CREDENTIAL_B),
        ("Secret", "edgion-default", "policy-integration-denied-key"),
    ] {
        let _ = ctx.test_ctx.delete_resource(kind, namespace, name).await;
    }
}

async fn status_boundaries(ctx: &ScenarioCtx, bases: &ProviderBases) -> Result<()> {
    apply(ctx, &missing_route_yaml()).await?;
    wait_condition(
        ctx,
        "HTTPRoute",
        MISSING_ROUTE,
        "ResolvedRefs",
        "False",
        Some("BackendNotFound"),
    )
    .await?;
    apply(ctx, &missing_traffic_policy_yaml()).await?;
    wait_condition(
        ctx,
        "EdgionBackendTrafficPolicy",
        "policy-integration-missing",
        "ResolvedRefs",
        "False",
        Some("TargetNotFound"),
    )
    .await?;
    wait_policy_class(ctx, "edgionbackendtrafficpolicy", "policy-integration-missing", None).await?;

    apply(ctx, &wrong_variant_tls_policy_yaml()).await?;
    wait_condition(
        ctx,
        "BackendTLSPolicy",
        "policy-integration-wrong-variant",
        "Accepted",
        "False",
        Some("Invalid"),
    )
    .await?;
    wait_policy_class(ctx, "backendtlspolicy", "policy-integration-wrong-variant", None).await?;

    apply(ctx, &incompatible_traffic_policy_yaml()).await?;
    wait_condition(
        ctx,
        "EdgionBackendTrafficPolicy",
        "policy-integration-incompatible",
        "Accepted",
        "False",
        Some("Invalid"),
    )
    .await?;
    wait_policy_class(
        ctx,
        "edgionbackendtrafficpolicy",
        "policy-integration-incompatible",
        None,
    )
    .await?;

    apply(ctx, &denied_secret_yaml()).await?;
    apply(ctx, &denied_backend_yaml(&format!("{}/ai/prefix", bases.https))).await?;
    wait_condition(
        ctx,
        "EdgionBackend",
        "policy-integration-denied",
        "ResolvedRefs",
        "False",
        Some("RefNotPermitted"),
    )
    .await
}

async fn run_inner(ctx: &ScenarioCtx) -> Result<()> {
    let bases = ProviderBases::from_env();
    apply(ctx, &secret_yaml(CREDENTIAL_A, "ai-provider-test-key")).await?;
    apply(ctx, &secret_yaml(CREDENTIAL_B, "ai-provider-rotated-test-key")).await?;
    apply(
        ctx,
        &backend_yaml(BACKEND_A, CREDENTIAL_A, &format!("{}/ai/prefix", bases.sni)),
    )
    .await?;
    apply(
        ctx,
        &backend_yaml(BACKEND_B, CREDENTIAL_B, &format!("{}/ai/policy-b", bases.http)),
    )
    .await?;
    apply(ctx, &plugin_yaml("Weighted")).await?;
    apply(ctx, &route_yaml(false)).await?;
    apply(
        ctx,
        &tls_policy_yaml(
            TLS_POLICY,
            BACKEND_A,
            "hostname-a.example.com",
            "hostname-a.example.com",
            false,
        ),
    )
    .await?;
    wait_condition(
        ctx,
        "BackendTLSPolicy",
        TLS_POLICY,
        "Accepted",
        "True",
        Some("Accepted"),
    )
    .await?;
    wait_policy_class(ctx, "backendtlspolicy", TLS_POLICY, Some("ai")).await?;

    let initial = wait_unary(ctx, true).await?;
    anyhow::ensure!(
        initial.pointer("/mock/host_is_expected") == Some(&Value::Bool(true)),
        "HTTPS provider Host was not preserved: {initial}"
    );
    anyhow::ensure!(
        initial.pointer("/mock/path_has_prefix") == Some(&Value::Bool(true)),
        "HTTPS provider did not receive the configured path: {initial}"
    );
    assert_normal_eos(ctx).await?;

    apply(ctx, &secret_yaml(CREDENTIAL_A, "ai-provider-rotated-test-key")).await?;
    wait_for("AI credential Secret rotation", || async {
        let response = ai_request(ctx, None, false).await?;
        if !response.status().is_success() {
            return Ok(false);
        }
        let body: Value = response.json().await.context("rotation response was not JSON")?;
        Ok(body.pointer("/mock/credential_generation") == Some(&Value::String("b".into())))
    })
    .await?;

    assert_request_stable_capture(ctx, &bases).await?;

    apply(ctx, &secret_yaml(CREDENTIAL_A, "ai-provider-test-key")).await?;

    let backend_mtls = backend_yaml(BACKEND_A, CREDENTIAL_A, &format!("{}/ai/prefix", bases.mtls));
    let policy_mtls = tls_policy_yaml(
        TLS_POLICY,
        BACKEND_A,
        "backend.example.com",
        "backend.example.com",
        true,
    );
    tokio::try_join!(apply(ctx, &backend_mtls), apply(ctx, &policy_mtls))?;
    let mtls = wait_unary(ctx, true).await?;
    anyhow::ensure!(
        mtls.pointer("/mock/host_is_expected") == Some(&Value::Bool(true)),
        "mTLS provider Host was not preserved: {mtls}"
    );

    apply(ctx, &traffic_policy_yaml(3)).await?;
    apply(ctx, &route_yaml(true)).await?;
    wait_condition(
        ctx,
        "EdgionBackendTrafficPolicy",
        TRAFFIC_POLICY,
        "Accepted",
        "True",
        Some("Accepted"),
    )
    .await?;
    wait_policy_class(ctx, "edgionbackendtrafficpolicy", TRAFFIC_POLICY, Some("ai")).await?;
    wait_weighted_generation(ctx, 3).await?;
    assert_weighted(ctx).await?;

    let backend_hot =
        backend_yaml_with_max_output(BACKEND_A, CREDENTIAL_A, &format!("{}/ai/prefix", bases.mtls), Some(777));
    let policy_hot = traffic_policy_yaml(7);
    tokio::try_join!(apply(ctx, &backend_hot), apply(ctx, &policy_hot))?;
    wait_weighted_generation(ctx, 7).await?;
    wait_for("backend and traffic policy hot reload", || async {
        let backends = gateway_policies(ctx, "edgionbackend").await?;
        Ok(backends
            .iter()
            .find(|item| resource_named(item, BACKEND_A))
            .is_some_and(|item| {
                item.pointer("/spec/ai/defaults/maxOutputTokens")
                    .and_then(Value::as_u64)
                    == Some(777)
            }))
    })
    .await?;
    assert_weighted(ctx).await?;
    let policies = gateway_policies(ctx, "edgionbackendtrafficpolicy").await?;
    let policy = policies
        .iter()
        .find(|item| resource_named(item, TRAFFIC_POLICY))
        .context("multi-target traffic policy was not published")?;
    anyhow::ensure!(
        policy
            .pointer("/spec/targetRefs")
            .and_then(Value::as_array)
            .is_some_and(|refs| refs.len() == 2),
        "traffic policy did not publish both targets: {policy}"
    );
    anyhow::ensure!(
        policy.pointer("/spec/outlierDetection/consecutiveErrors") == Some(&Value::Number(7.into())),
        "traffic policy hot reload was not published: {policy}"
    );

    apply(
        ctx,
        &backend_yaml(BACKEND_A, CREDENTIAL_A, &format!("{}/ai/prefix", bases.https)),
    )
    .await?;
    apply(
        ctx,
        &tls_policy_yaml(
            TLS_POLICY,
            BACKEND_A,
            "backend.example.com",
            "backend.example.com",
            false,
        ),
    )
    .await?;
    apply(ctx, &route_yaml(false)).await?;
    wait_unary(ctx, true).await?;
    assert_client_abort(ctx, &bases.http).await?;
    assert_upstream_abort(ctx, &bases).await?;
    assert_normal_eos(ctx).await?;

    status_boundaries(ctx, &bases).await
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
    id: "AiBackendPolicyIntegration",
    conf_path: "EdgionBackend/PolicyIntegration/DynamicTest",
    run,
    phases: &[],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_manifests_are_valid_and_keep_typed_targets() {
        let bases = ProviderBases::from_env();
        for yaml in [
            secret_yaml(CREDENTIAL_A, "ai-provider-test-key"),
            backend_yaml(BACKEND_A, CREDENTIAL_A, &format!("{}/ai/prefix", bases.https)),
            plugin_yaml("Weighted"),
            capture_route_yaml(),
            missing_route_yaml(),
            route_yaml(true),
            tls_policy_yaml(
                TLS_POLICY,
                BACKEND_A,
                "backend.example.com",
                "backend.example.com",
                true,
            ),
            traffic_policy_yaml(3),
            wrong_variant_tls_policy_yaml(),
            incompatible_traffic_policy_yaml(),
            missing_traffic_policy_yaml(),
            denied_backend_yaml(&format!("{}/ai/prefix", bases.https)),
            denied_secret_yaml(),
        ] {
            serde_yaml::from_str::<Value>(&yaml).unwrap_or_else(|error| panic!("invalid policy YAML: {error}\n{yaml}"));
        }
        let route: Value = serde_yaml::from_str(&route_yaml(true)).unwrap();
        assert_eq!(
            route.pointer("/spec/rules/0/backendRefs/0/weight"),
            Some(&Value::Number(1.into()))
        );
        assert_eq!(
            route.pointer("/spec/rules/0/backendRefs/1/weight"),
            Some(&Value::Number(3.into()))
        );
        let policy: Value = serde_yaml::from_str(&traffic_policy_yaml(3)).unwrap();
        assert!(policy
            .pointer("/spec/targetRefs")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .all(|target| target["kind"] == "EdgionBackend"));
        let http_backend: Value = serde_yaml::from_str(&backend_yaml(
            BACKEND_B,
            CREDENTIAL_B,
            "http://127.0.0.1:30001/ai/policy-b",
        ))
        .unwrap();
        assert_eq!(
            http_backend.pointer("/spec/ai/allowInsecureHttp"),
            Some(&Value::Bool(true))
        );
    }
}
