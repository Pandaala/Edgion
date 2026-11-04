//! Full-tier TLS policy recovery on the shared Controller/Gateway topology.
//! The independent processor observes both TCP accepts and Process streams.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers::controller_local_status;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const NS: &str = "edgion-default";
const GLOBAL: &str = "ext-proc-tls-recovery-default";
const GLOBAL_CA_GRANT: &str = "ext-proc-tls-recovery-global-ca";
const NAMES: [&str; 2] = ["ext-proc-tls-recovery-close", "ext-proc-tls-recovery-open"];
const MARKER: &str = "edgion.io/test-tls-phase";
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Absent,
    Initial,
    LocalHostname,
    LocalSan,
    Recovered,
    GlobalHostname,
    GlobalSan,
    Override,
    Restored,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Initial => "initial",
            Self::LocalHostname => "local-hostname",
            Self::LocalSan => "local-san",
            Self::Recovered => "recovered",
            Self::GlobalHostname => "global-hostname",
            Self::GlobalSan => "global-san",
            Self::Override => "override",
            Self::Restored => "restored",
        }
    }

    fn rejected(self) -> bool {
        matches!(
            self,
            Self::LocalHostname | Self::LocalSan | Self::GlobalHostname | Self::GlobalSan
        )
    }

    fn inherited(self) -> bool {
        matches!(self, Self::GlobalHostname | Self::GlobalSan | Self::Restored)
    }

    fn validation(self) -> Value {
        let mut validation = json!({"caCertificateRefs": [{
            "group": "", "kind": "Secret", "name": "ext-proc-external-server-ca", "namespace": NS
        }]});
        match self {
            Self::LocalHostname | Self::GlobalHostname => validation["hostname"] = json!("processor.example.com"),
            Self::LocalSan | Self::GlobalSan => {
                validation["subjectAltNames"] = json!([{"type": "Hostname", "hostname": "processor.example.com"}]);
            }
            // A present block, including an empty SAN list, replaces global identity constraints.
            _ => validation["subjectAltNames"] = json!([]),
        }
        validation
    }
}

struct PolicySuite(Phase);

impl TestSuite for PolicySuite {
    fn name(&self) -> &str {
        "ExtProcTlsPolicyRecovery"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.0;
        vec![TestCase::new(
            "publication",
            "Observe the exact TLS policy generation",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check_publication(&ctx, phase).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{phase:?}: {error:#}")),
                    }
                })
            },
        )]
    }
}

fn admin(ctx: &TestContext, request: reqwest::RequestBuilder, gateway: bool) -> reqwest::RequestBuilder {
    let token = gateway
        .then(|| std::env::var("EDGION_GATEWAY_ADMIN_TOKEN").ok())
        .flatten();
    let request = request.timeout(Duration::from_secs(3));
    match token.as_deref().or(ctx.admin_token.as_deref()) {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

async fn resource(ctx: &TestContext, kind: &str, name: &str, gateway: bool) -> Result<Option<Value>> {
    let base = if gateway {
        std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL").unwrap_or_else(|_| format!("http://{}:12001", ctx.target_host))
    } else {
        ctx.admin_api_url()
    };
    let prefix = if gateway { "configclient" } else { "configserver" };
    let mut request = ctx
        .http_client
        .get(format!("{}/{prefix}/{kind}", base.trim_end_matches('/')))
        .query(&[("name", name)]);
    if kind != "EdgionGatewayConfig" {
        request = request.query(&[("namespace", NS)]);
    }
    let response = admin(ctx, request, gateway).send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = response.status();
    let body: Value = response.json().await?;
    if gateway && status.is_success() && body["success"] == false && body["error"] == format!("{kind} not found") {
        return Ok(None);
    }
    ensure!(
        status.is_success() && body["success"] == true,
        "resource query failed: {kind}/{name}: {status}"
    );
    Ok(body.get("data").filter(|value| !value.is_null()).cloned())
}

async fn check_publication(ctx: &TestContext, phase: Phase) -> Result<()> {
    for gateway in [false, true] {
        for name in NAMES {
            let value = resource(ctx, "EdgionPlugins", name, gateway).await?;
            if phase == Phase::Absent {
                ensure!(value.is_none(), "plugin {name} remains published");
                ensure!(
                    resource(ctx, "HTTPRoute", name, gateway).await?.is_none(),
                    "route {name} remains published"
                );
                continue;
            }
            let value = value.context("plugin not published")?;
            let route = resource(ctx, "HTTPRoute", name, gateway)
                .await?
                .context("route not published")?;
            ensure!(
                route["spec"]["rules"][0]["filters"][0]["extensionRef"]["name"] == name,
                "wrong route attachment"
            );
            ensure!(
                value["metadata"]["annotations"][MARKER] == phase.name(),
                "stale plugin generation"
            );
            let config = &value["spec"]["requestPlugins"][0]["config"];
            ensure!(
                config["failureModeAllow"] == (name == NAMES[1]),
                "failure policy changed"
            );
            let expected = if phase.inherited() {
                Value::Null
            } else {
                phase.validation()
            };
            ensure!(
                config["grpcService"]["tls"]["validation"] == expected,
                "wrong local validation policy"
            );
            if !gateway {
                let local_status = controller_local_status(&value["status"], "edgion.io/gateway-controller")
                    .context("missing Edgion controller observation")?;
                let accepted = local_status["conditions"]
                    .as_array()
                    .context("missing conditions")?
                    .iter()
                    .find(|condition| condition["type"] == "Accepted")
                    .context("missing Accepted")?;
                let expected = if matches!(phase, Phase::LocalHostname | Phase::LocalSan) {
                    "False"
                } else {
                    "True"
                };
                ensure!(
                    accepted["status"] == expected,
                    "unexpected Controller capability status"
                );
                if let Some(generation) = value["metadata"]["generation"].as_i64() {
                    ensure!(accepted["observedGeneration"] == generation, "stale Controller status");
                }
            }
        }
        if phase == Phase::Absent {
            ensure!(
                resource(ctx, "EdgionGatewayConfig", GLOBAL, gateway).await?.is_none(),
                "global policy remains published"
            );
        }
    }
    Ok(())
}

async fn fixture(ctx: &TestContext, path: &str) -> Result<Value> {
    let base = std::env::var("EDGION_EXT_PROC_EXTERNAL_ADMIN_URL").context("external fixture admin URL is required")?;
    Ok(ctx
        .http_client
        .get(format!("{}{path}", base.trim_end_matches('/')))
        .timeout(Duration::from_secs(3))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn runtime_stats(ctx: &ScenarioCtx) -> Result<Value> {
    let response = admin(
        &ctx.test_ctx,
        ctx.test_ctx.http_client.get(format!(
            "{}/api/v1/debug/store-stats",
            ctx.gateway_url.trim_end_matches('/')
        )),
        true,
    )
    .send()
    .await?
    .error_for_status()?;
    let body: Value = response.json().await?;
    ensure!(body["success"] == true, "runtime inventory query failed");
    Ok(body["data"].clone())
}

fn generations(stats: &Value) -> Result<Vec<String>> {
    let all = stats["plugin_publication_inventory"]["http_generations"]
        .as_array()
        .context("missing runtime generations")?;
    Ok(all
        .iter()
        .filter_map(Value::as_str)
        .filter(|id| NAMES.iter().any(|name| id.starts_with(&format!("{NS}/{name}@"))))
        .map(str::to_owned)
        .collect())
}

fn route_present(stats: &Value, name: &str) -> Result<bool> {
    let routes = stats["plugin_publication_inventory"]["ai_routes"]
        .as_array()
        .context("missing runtime route inventory")?;
    Ok(routes
        .iter()
        .filter_map(Value::as_str)
        .any(|id| id.ends_with(&format!("/{NS}/{name}"))))
}

fn global_count(stats: &Value) -> Result<u64> {
    stats["edgion_gateway_config_store"]["count"]
        .as_u64()
        .context("missing runtime gateway-config count")
}

async fn wait_runtime(ctx: &ScenarioCtx, check: impl Fn(&Value) -> Result<()>) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let stats = runtime_stats(ctx).await?;
        let result = check(&stats);
        if result.is_ok() {
            return Ok(());
        }
        ensure!(Instant::now() < deadline, "runtime publication timed out: {result:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn accepted_connections(ctx: &TestContext) -> Result<(u64, u64)> {
    let snapshot = fixture(ctx, "/v1/tls").await?;
    ensure!(snapshot["tls"]["ready"] == true, "TLS fixture is not ready");
    let accepted = snapshot["tls"]["accepted_connections"]
        .as_u64()
        .context("fixture lacks cumulative TCP accept observation")?;
    let calls = snapshot["tls"]["rpc_calls"]
        .as_u64()
        .context("fixture lacks cumulative RPC observation")?;
    Ok((accepted, calls))
}

fn case_id(phase: Phase, name: &str) -> String {
    format!(
        "tls-recovery-{}-{}",
        phase.name(),
        if name == NAMES[1] { "open" } else { "close" }
    )
}

async fn probe(ctx: &TestContext, phase: Phase) -> Result<()> {
    for name in NAMES {
        let response = ctx
            .http_client
            .get(format!("{}/headers", ctx.http_url()))
            .header("host", format!("{name}.example.com"))
            .header("x-ext-proc-case-id", case_id(phase, name))
            .header("x-ext-proc-test-mode", "interop")
            .timeout(Duration::from_secs(5))
            .send()
            .await?;
        let status = response.status().as_u16();
        let response_marker = response
            .headers()
            .get("x-ext-proc-resp")
            .and_then(|value| value.to_str().ok())
            == Some("external-go");
        let body = response.text().await?;
        if phase.rejected() {
            ensure!(
                status == 500 && body == r#"{"message":"plugin configuration error"}"#,
                "invalid policy did not seal {name}: status={status}"
            );
            ensure!(!response_marker, "invalid policy reached the processor");
        } else {
            ensure!(
                status == 200 && response_marker,
                "supported policy did not recover {name}: phase={}, status={status}",
                phase.name()
            );
            let echoed: Value = serde_json::from_str(&body)?;
            ensure!(
                echoed["headers"]["x-ext-proc-req"] == "external-go",
                "request mutation absent"
            );
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum SupportedAttempt {
    Ready,
    Sealed,
}

fn classify_supported_response(status: u16, marker: Option<&str>, body: &str) -> Result<SupportedAttempt> {
    if status == 500 {
        ensure!(
            marker.is_none() && body == r#"{"message":"plugin configuration error"}"#,
            "supported policy returned a noncanonical rejection"
        );
        return Ok(SupportedAttempt::Sealed);
    }
    ensure!(
        status == 200 && marker == Some("external-go"),
        "supported policy did not recover"
    );
    let echoed: Value = serde_json::from_str(body)?;
    ensure!(
        echoed["headers"]["x-ext-proc-req"] == "external-go",
        "request mutation absent"
    );
    Ok(SupportedAttempt::Ready)
}

fn verify_retry_evidence(baseline: (u64, u64), current: (u64, u64), case_status: u16) -> Result<()> {
    ensure!(case_status == 404, "sealed policy created a Process observation");
    ensure!(current == baseline, "sealed policy dialed or called the processor");
    Ok(())
}

async fn wait_supported_owner<F, Fut>(mut attempt: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<SupportedAttempt>>,
{
    loop {
        match attempt().await? {
            SupportedAttempt::Ready => return Ok(()),
            SupportedAttempt::Sealed => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

fn supported_case_complete(snapshot: &Value) -> Result<bool> {
    let streams = snapshot["streams"]
        .as_array()
        .context("missing processor stream inventory")?;
    ensure!(streams.len() == 1, "recovery must produce exactly one Process stream");
    Ok(streams[0]["client_half_closed"] == true && streams[0]["active"] == false)
}

async fn probe_supported(ctx: &TestContext, phase: Phase) -> Result<()> {
    // One deadline covers both owners, request bodies, retry evidence and final
    // stream completion. Cancellation never starts another attempt.
    tokio::time::timeout(TIMEOUT, async {
        for name in NAMES {
            // The preceding owner's successful stream is already in these
            // cumulative counters. Never compare the next owner to phase start.
            let baseline = accepted_connections(ctx).await?;
            wait_supported_owner(|| async {
                let response = ctx
                    .http_client
                    .get(format!("{}/headers", ctx.http_url()))
                    .header("host", format!("{name}.example.com"))
                    .header("x-ext-proc-case-id", case_id(phase, name))
                    .header("x-ext-proc-test-mode", "interop")
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await?;
                let status = response.status().as_u16();
                let marker = response
                    .headers()
                    .get("x-ext-proc-resp")
                    .map(|value| value.to_str())
                    .transpose()?;
                let marker = marker.map(str::to_owned);
                let body = response.text().await?;
                let outcome = classify_supported_response(status, marker.as_deref(), &body)
                    .with_context(|| format!("supported phase {} owner {name}", phase.name()))?;
                if outcome == SupportedAttempt::Sealed {
                    let base = std::env::var("EDGION_EXT_PROC_EXTERNAL_ADMIN_URL")?;
                    let response = ctx
                        .http_client
                        .get(format!(
                            "{}/v1/cases/{}",
                            base.trim_end_matches('/'),
                            case_id(phase, name)
                        ))
                        .timeout(Duration::from_secs(3))
                        .send()
                        .await?;
                    let case_status = response.status().as_u16();
                    let current = accepted_connections(ctx).await?;
                    verify_retry_evidence(baseline, current, case_status)?;
                }
                Ok(outcome)
            })
            .await?;
        }
        loop {
            let mut complete = true;
            for name in NAMES {
                let snapshot = fixture(ctx, &format!("/v1/cases/{}", case_id(phase, name))).await?;
                complete &= supported_case_complete(&snapshot)?;
            }
            if complete {
                // Keep the original exact TLS transport, phase and stream assertions.
                return observe_cases(ctx, phase).await;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .with_context(|| format!("supported phase {} convergence timed out", phase.name()))?
}

async fn observe_cases(ctx: &TestContext, phase: Phase) -> Result<()> {
    for name in NAMES {
        if phase.rejected() {
            let base = std::env::var("EDGION_EXT_PROC_EXTERNAL_ADMIN_URL")?;
            let response = ctx
                .http_client
                .get(format!(
                    "{}/v1/cases/{}",
                    base.trim_end_matches('/'),
                    case_id(phase, name)
                ))
                .timeout(Duration::from_secs(3))
                .send()
                .await?;
            // Reset removes the case. A received Process request creates it.
            ensure!(
                response.status() == reqwest::StatusCode::NOT_FOUND,
                "invalid policy created a Process observation"
            );
            continue;
        }
        let snapshot = fixture(ctx, &format!("/v1/cases/{}", case_id(phase, name))).await?;
        let streams = snapshot["streams"]
            .as_array()
            .context("missing processor stream inventory")?;
        ensure!(streams.len() == 1, "recovery must produce exactly one Process stream");
        let stream = &streams[0];
        ensure!(
            stream["transport"] == "tls" && stream["client_half_closed"] == true && stream["active"] == false,
            "processor stream did not complete over TLS"
        );
        ensure!(
            stream["phases"] == json!(["request_headers", "response_headers"]),
            "wrong processor phases"
        );
    }
    Ok(())
}

async fn apply(ctx: &TestContext, value: Value) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.admin_api_url())?;
    crate::dynamic::apply::apply_yaml_documents(
        &client,
        std::path::Path::new("ext-proc-tls-recovery-phase.yaml"),
        &yaml_serde::to_string(&value)?,
    )
    .await
}

async fn publish_plugins(ctx: &ScenarioCtx, phase: Phase, target: &str) -> Result<()> {
    let previous = generations(&runtime_stats(ctx).await?)?;
    let template = std::fs::read_to_string(ctx.resolve("plugin.yaml.tmpl"))?;
    for name in NAMES {
        let mut value: Value = yaml_serde::from_str(&template)?;
        value["metadata"]["name"] = json!(name);
        value["metadata"]["annotations"] = json!({MARKER: phase.name()});
        let config = &mut value["spec"]["requestPlugins"][0]["config"];
        config["failureModeAllow"] = json!(name == NAMES[1]);
        config["grpcService"]["target"] = json!(target);
        // Force a new source entry even when only inherited policy changes.
        config["grpcService"]["timeoutMs"] = json!(2000 + phase as u64);
        if !phase.inherited() {
            config["grpcService"]["tls"]["validation"] = phase.validation();
        }
        apply(&ctx.test_ctx, value).await?;
    }
    ctx.wait_phase(SCENARIO, phase.name(), TIMEOUT).await?;
    // ConfigClient exposes raw arrivals before its debounced handler runs.
    // New publisher IDs show rebuild/publication progress, not acknowledgment
    // of this source or its effective TLS policy. Supported traffic supplies that evidence.
    wait_runtime(ctx, |stats| {
        let current = generations(stats)?;
        ensure!(
            current.len() == NAMES.len() && current.iter().all(|id| !previous.contains(id)),
            "waiting for new plugin runtimes"
        );
        for name in NAMES {
            ensure!(
                route_present(stats, name)?,
                "route is not in the published plugin scope"
            );
        }
        Ok(())
    })
    .await
}

async fn delete_global(ctx: &ScenarioCtx) -> Result<()> {
    let response = admin(
        &ctx.test_ctx,
        ctx.test_ctx.http_client.delete(format!(
            "{}/api/v1/cluster/EdgionGatewayConfig/{GLOBAL}",
            ctx.controller_url.trim_end_matches('/')
        )),
        false,
    )
    .send()
    .await?;
    ensure!(
        response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND,
        "global policy delete failed: {}",
        response.status()
    );
    Ok(())
}

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let mut errors = Vec::new();
    for (kind, name) in [
        ("HTTPRoute", NAMES[0]),
        ("HTTPRoute", NAMES[1]),
        ("EdgionPlugins", NAMES[0]),
        ("EdgionPlugins", NAMES[1]),
        ("ReferenceGrant", GLOBAL_CA_GRANT),
        ("EdgionGatewayConfig", GLOBAL),
    ] {
        let scope = if kind == "EdgionGatewayConfig" {
            format!("cluster/{kind}")
        } else {
            format!("namespaced/{kind}/{NS}")
        };
        let result = admin(
            &ctx.test_ctx,
            ctx.test_ctx.http_client.delete(format!(
                "{}/api/v1/{scope}/{name}",
                ctx.controller_url.trim_end_matches('/')
            )),
            false,
        )
        .send()
        .await;
        match result {
            Ok(response) if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND => {}
            Ok(response) => errors.push(format!("delete {kind}/{name}: {}", response.status())),
            Err(error) => errors.push(format!("delete {kind}/{name}: {error}")),
        }
    }
    ensure!(errors.is_empty(), "cleanup failed: {errors:?}");
    ctx.wait_phase(SCENARIO, "absent", TIMEOUT).await?;
    wait_runtime(ctx, |stats| {
        ensure!(generations(stats)?.is_empty(), "plugin runtime remains after deletion");
        for name in NAMES {
            ensure!(!route_present(stats, name)?, "route runtime remains after deletion");
        }
        Ok(())
    })
    .await
}

const RUN_PHASES: &[Phase] = &[
    Phase::Initial,
    Phase::LocalHostname,
    Phase::LocalSan,
    Phase::Recovered,
    Phase::GlobalHostname,
    Phase::GlobalSan,
    Phase::Override,
    Phase::Restored,
];

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        ensure!(
            std::env::var("EDGION_EXT_PROC_EXTERNAL_REQUIRED").as_deref() == Ok("1"),
            "run with the full-test external ExtProc fixture"
        );
        let ready = fixture(&ctx.test_ctx, "/readyz").await?;
        ensure!(
            ready["fixture"] == "edgion-ext-proc-external"
                && ready["protocol_version"] == "v1"
                && ready["go_control_plane_envoy"] == "v1.40.0",
            "wrong external processor fixture"
        );
        accepted_connections(&ctx.test_ctx).await?;
        // Read the startup-rendered target, so the scenario uses the same managed listener.
        let source = resource(&ctx.test_ctx, "EdgionPlugins", "ext-proc-external-tls", true)
            .await?
            .context("external TLS fixture configuration absent")?;
        let target = source["spec"]["requestPlugins"][0]["config"]["grpcService"]["target"]
            .as_str()
            .context("missing TLS fixture target")?
            .to_string();
        cleanup(&ctx).await?;
        // The shared topology is serial. Count the raw configs after our cleanup,
        // then wait for the actual handler store to agree before freezing baselines.
        let response = admin(
            &ctx.test_ctx,
            ctx.test_ctx.http_client.get(format!(
                "{}/configclient/EdgionGatewayConfig/list",
                ctx.gateway_url.trim_end_matches('/')
            )),
            true,
        )
        .send()
        .await?
        .error_for_status()?;
        let configs: Value = response.json().await?;
        ensure!(configs["success"] == true, "global config inventory failed");
        let baseline_global_count = configs["data"]
            .as_array()
            .context("missing global config inventory")?
            .len() as u64;
        wait_runtime(&ctx, |stats| {
            ensure!(
                global_count(stats)? == baseline_global_count,
                "global runtime cleanup pending"
            );
            Ok(())
        })
        .await?;
        let baseline_routes = runtime_stats(&ctx).await?["http_routes"]["http_routes"]
            .as_u64()
            .context("missing runtime route count")?;
        let result: Result<()> = async {
            let route_template = std::fs::read_to_string(ctx.resolve("route.yaml.tmpl"))?;
            for name in NAMES {
                let mut route: Value = yaml_serde::from_str(&route_template)?;
                route["metadata"]["name"] = json!(name);
                route["spec"]["hostnames"] = json!([format!("{name}.example.com")]);
                route["spec"]["rules"][0]["filters"][0]["extensionRef"]["name"] = json!(name);
                apply(&ctx.test_ctx, route).await?;
            }
            // Cluster-scoped owners use `default` as their ReferenceGrant
            // source namespace in the shared Secret resolver.
            apply(
                &ctx.test_ctx,
                json!({
                    "apiVersion": "gateway.networking.k8s.io/v1beta1",
                    "kind": "ReferenceGrant",
                    "metadata": {"name": GLOBAL_CA_GRANT, "namespace": NS},
                    "spec": {
                        "from": [{
                            "group": "edgion.io",
                            "kind": "EdgionGatewayConfig",
                            "namespace": "default"
                        }],
                        "to": [{
                            "group": "",
                            "kind": "Secret",
                            "name": "ext-proc-external-server-ca"
                        }]
                    }
                }),
            )
            .await?;
            wait_runtime(&ctx, |stats| {
                ensure!(
                    stats["http_routes"]["http_routes"] == baseline_routes + 2,
                    "route tables are not published"
                );
                Ok(())
            })
            .await?;
            for &phase in RUN_PHASES {
                // Reset only our bounded IDs; other full-tier cases retain their evidence.
                let base = std::env::var("EDGION_EXT_PROC_EXTERNAL_ADMIN_URL")?;
                for name in NAMES {
                    ctx.test_ctx
                        .http_client
                        .post(format!(
                            "{}/v1/cases/{}/reset",
                            base.trim_end_matches('/'),
                            case_id(phase, name)
                        ))
                        .timeout(Duration::from_secs(3))
                        .send()
                        .await?
                        .error_for_status()?;
                }
                let before = accepted_connections(&ctx.test_ctx).await?;
                if phase.inherited() {
                    // Delete/recreate our isolated global config so the existing
                    // runtime count proves recomputation, not merely raw receipt.
                    delete_global(&ctx).await?;
                    wait_runtime(&ctx, |stats| {
                        ensure!(
                            global_count(stats)? == baseline_global_count,
                            "previous global runtime remains"
                        );
                        Ok(())
                    })
                    .await?;
                    apply(
                        &ctx.test_ctx,
                        json!({"apiVersion":"edgion.io/v1alpha1", "kind":"EdgionGatewayConfig",
                        "metadata":{"name":GLOBAL}, "spec":{"outboundTls":{"validation":phase.validation()}}}),
                    )
                    .await?;
                    // Wait for the global resource before publishing a new plugin generation.
                    let deadline = Instant::now() + TIMEOUT;
                    loop {
                        let value = resource(&ctx.test_ctx, "EdgionGatewayConfig", GLOBAL, true).await?;
                        if value.as_ref().is_some_and(|value| {
                            value["spec"]["outboundTls"]["validation"] == phase.validation()
                                && value["spec"]["outboundTls"]["resolvedCaCertificates"] == "[redacted]"
                        }) {
                            break;
                        }
                        ensure!(Instant::now() < deadline, "global policy publication timed out");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    wait_runtime(&ctx, |stats| {
                        ensure!(
                            global_count(stats)? == baseline_global_count + 1,
                            "new global runtime is not published"
                        );
                        Ok(())
                    })
                    .await?;
                }
                // A local whole-block override recovers while the unsupported global block remains.
                publish_plugins(&ctx, phase, &target).await?;
                if !phase.rejected() {
                    probe_supported(&ctx.test_ctx, phase).await?;
                    continue;
                }
                probe(&ctx.test_ctx, phase).await?;
                let deadline = Instant::now() + TIMEOUT;
                loop {
                    let observation = observe_cases(&ctx.test_ctx, phase).await;
                    if observation.is_ok() {
                        break;
                    }
                    ensure!(
                        Instant::now() < deadline,
                        "processor observation timed out: {observation:?}"
                    );
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                if phase.rejected() {
                    ensure!(
                        accepted_connections(&ctx.test_ctx).await? == before,
                        "invalid generation dialed or called the processor"
                    );
                }
            }
            Ok(())
        }
        .await;
        let cleaned = async {
            cleanup(&ctx).await?;
            wait_runtime(&ctx, |stats| {
                ensure!(
                    global_count(stats)? == baseline_global_count,
                    "global runtime was not restored"
                );
                ensure!(
                    stats["http_routes"]["http_routes"] == baseline_routes,
                    "route tables were not restored"
                );
                Ok(())
            })
            .await
        }
        .await;
        match (result, cleaned) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "ExtProcTlsPolicyRecovery",
    conf_path: "EdgionPlugins/ExtProc/DynamicTest",
    run,
    phases: &[
        PhaseSuite {
            name: "absent",
            factory: || Box::new(PolicySuite(Phase::Absent)),
        },
        PhaseSuite {
            name: "initial",
            factory: || Box::new(PolicySuite(Phase::Initial)),
        },
        PhaseSuite {
            name: "local-hostname",
            factory: || Box::new(PolicySuite(Phase::LocalHostname)),
        },
        PhaseSuite {
            name: "local-san",
            factory: || Box::new(PolicySuite(Phase::LocalSan)),
        },
        PhaseSuite {
            name: "recovered",
            factory: || Box::new(PolicySuite(Phase::Recovered)),
        },
        PhaseSuite {
            name: "global-hostname",
            factory: || Box::new(PolicySuite(Phase::GlobalHostname)),
        },
        PhaseSuite {
            name: "global-san",
            factory: || Box::new(PolicySuite(Phase::GlobalSan)),
        },
        PhaseSuite {
            name: "override",
            factory: || Box::new(PolicySuite(Phase::Override)),
        },
        PhaseSuite {
            name: "restored",
            factory: || Box::new(PolicySuite(Phase::Restored)),
        },
    ],
};

#[cfg(test)]
mod supported_convergence_tests {
    use super::*;
    use std::cell::Cell;
    use std::future::{pending, ready};

    const SEALED: &str = r#"{"message":"plugin configuration error"}"#;
    const SUCCESS: &str = r#"{"headers":{"x-ext-proc-req":"external-go"}}"#;

    #[test]
    fn only_canonical_sealed_rejection_can_wait() {
        assert_eq!(
            classify_supported_response(500, None, SEALED).unwrap(),
            SupportedAttempt::Sealed
        );
        for (status, marker, body) in [
            (500, Some("external-go"), SEALED),
            (500, Some("wrong-marker"), SEALED),
            (500, Some(""), SEALED),
            (500, None, "different error"),
            (503, None, SEALED),
            (200, None, SUCCESS),
            (200, Some("wrong-marker"), SUCCESS),
            (200, Some("external-go"), r#"{"headers":{}}"#),
            (200, Some("external-go"), "invalid JSON"),
        ] {
            assert!(classify_supported_response(status, marker, body).is_err());
        }
        assert_eq!(
            classify_supported_response(200, Some("external-go"), SUCCESS).unwrap(),
            SupportedAttempt::Ready
        );
    }

    #[test]
    fn retry_requires_absent_case_and_unchanged_cumulative_activity() {
        verify_retry_evidence((12, 4), (12, 4), 404).unwrap();
        for (current, case_status) in [
            ((13, 4), 404),
            ((12, 5), 404),
            ((11, 4), 404),
            ((12, 4), 200),
            ((12, 4), 503),
        ] {
            assert!(verify_retry_evidence((12, 4), current, case_status).is_err());
        }
        // The second owner's baseline includes the first owner's successful RPC.
        verify_retry_evidence((13, 5), (13, 5), 404).unwrap();
        assert!(verify_retry_evidence((12, 4), (13, 5), 404).is_err());
    }

    #[tokio::test]
    async fn successful_owner_is_never_replayed_while_next_owner_waits() {
        let close_calls = Cell::new(0);
        let open_calls = Cell::new(0);
        wait_supported_owner(|| {
            close_calls.set(close_calls.get() + 1);
            ready(Ok(SupportedAttempt::Ready))
        })
        .await
        .unwrap();
        wait_supported_owner(|| {
            open_calls.set(open_calls.get() + 1);
            assert_eq!(close_calls.get(), 1);
            ready(Ok(if open_calls.get() == 1 {
                SupportedAttempt::Sealed
            } else {
                SupportedAttempt::Ready
            }))
        })
        .await
        .unwrap();
        assert_eq!((close_calls.get(), open_calls.get()), (1, 2));
    }

    #[tokio::test]
    async fn ambiguous_attempt_error_never_retries() {
        let calls = Cell::new(0);
        let result = wait_supported_owner(|| {
            calls.set(calls.get() + 1);
            ready(Err(anyhow::anyhow!("ambiguous request or observation failure")))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.get(), 1);
    }

    #[tokio::test]
    async fn deadline_cancels_pending_attempt_and_retry_sleep_without_replay() {
        let calls = Cell::new(0);
        let result = tokio::time::timeout(
            Duration::from_millis(10),
            wait_supported_owner(|| {
                calls.set(calls.get() + 1);
                pending::<Result<SupportedAttempt>>()
            }),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.get(), 1);

        let calls = Cell::new(0);
        let result = tokio::time::timeout(
            Duration::from_millis(10),
            wait_supported_owner(|| {
                calls.set(calls.get() + 1);
                ready(Ok(SupportedAttempt::Sealed))
            }),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn completion_requires_exactly_one_stream() {
        assert!(
            !supported_case_complete(&json!({"streams": [{"client_half_closed": false, "active": true}]})).unwrap()
        );
        assert!(supported_case_complete(&json!({"streams": [{"client_half_closed": true, "active": false}]})).unwrap());
        assert!(supported_case_complete(&json!({"streams": []})).is_err());
        assert!(supported_case_complete(&json!({"streams": [{}, {}]})).is_err());
    }
}
