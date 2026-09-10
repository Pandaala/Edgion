//! Shared-topology ReferenceGrant lifecycle for both mirror publishers.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const NAME: &str = "mirror-grant-dynamic";
const BUFFER_NAME: &str = "mirror-grant-body-buffer";
const NS: &str = "edgion-default";
const TARGET_NS: &str = "edgion-backend";
const HOST: &str = "mirror-grant-dynamic.example.com";
const BODY: &str = "mirror authorization body";

fn resource(kind: &str, ns: &str, spec: Value) -> Value {
    let api = match kind {
        "Service" => "v1",
        "EndpointSlice" => "discovery.k8s.io/v1",
        "EdgionPlugins" => "edgion.io/v1",
        "ReferenceGrant" => "gateway.networking.k8s.io/v1beta1",
        _ => "gateway.networking.k8s.io/v1",
    };
    json!({"apiVersion":api,"kind":kind,"metadata":{"name":NAME,"namespace":ns},"spec":spec})
}

async fn apply(ctx: &TestContext, value: Value) -> Result<()> {
    ctx.apply_yaml(&serde_yaml::to_string(&value)?)
        .await
        .map_err(anyhow::Error::msg)
}

async fn setup(ctx: &TestContext) -> Result<()> {
    if k8s_mode() {
        // Selector-owned Services and their single-recorder Deployments are
        // applied from the K8s-only fixture tree by the scenario driver.
    } else {
        for (ns, port) in [(NS, 30001), (TARGET_NS, 30002)] {
            apply(
                ctx,
                resource(
                    "Service",
                    ns,
                    json!({"ports":[{"name":"http","port":port,"targetPort":port}]}),
                ),
            )
            .await?;
            let mut slice = resource("EndpointSlice", ns, Value::Null);
            slice.as_object_mut().context("slice object")?.remove("spec");
            slice["metadata"]["labels"] = json!({"kubernetes.io/service-name":NAME});
            slice["addressType"] = json!("IPv4");
            slice["ports"] = json!([{"name":"http","port":port,"protocol":"TCP"}]);
            slice["endpoints"] = json!([{"addresses":["127.0.0.1"],"conditions":{"ready":true}}]);
            apply(ctx, slice).await?;
        }
    }
    let mirror = json!({"backendRef":{"name":NAME,"namespace":TARGET_NS,"port":30002}});
    apply_plugin(ctx, false).await?;
    let mut buffer = resource(
        "EdgionPlugins",
        NS,
        json!({"requestPlugins":[{"type":"RequestBodyBuffer","config":{},"body":{"maxBodySize":"1MiB"}}]}),
    );
    buffer["metadata"]["name"] = json!(BUFFER_NAME);
    apply(ctx, buffer).await?;
    let mut rules = Vec::new();
    for case in ["native", "backend", "plugin"] {
        let filter = if case == "plugin" {
            json!({"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":NAME}})
        } else {
            json!({"type":"RequestMirror","requestMirror":mirror})
        };
        let mut rule = json!({"matches":[{"headers":[{"name":"x-mirror-grant-case","value":case}]}],"backendRefs":[{"name":NAME,"port":30001}]});
        if case == "backend" {
            rule["backendRefs"][0]["filters"] = json!([filter]);
            // Backend filters run after the streaming body policy freezes. Use
            // the existing committed snapshot path to test positive POST delivery.
            rule["filters"] = json!([{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":BUFFER_NAME}}]);
        } else {
            rule["filters"] = json!([filter]);
        }
        rules.push(rule);
    }
    apply(ctx, resource("HTTPRoute", NS, json!({"parentRefs":[{"name":"edgion-plugins-gateway","namespace":"edgion-test","sectionName":"http"}],"hostnames":[HOST],"rules":rules}))).await
}

async fn apply_plugin(ctx: &TestContext, invalid_dsl: bool) -> Result<()> {
    let source = if invalid_dsl { "invalid @@@" } else { "let a = 1" };
    apply(
        ctx,
        resource(
            "EdgionPlugins",
            NS,
            json!({"requestPlugins":[
                {"type":"RequestMirror","config":{"backendRef":{"name":NAME,"namespace":TARGET_NS,"port":30002}}},
                {"type":"Dsl","config":{"name":"mirror-grant-lkg","source":source}}
            ]}),
        ),
    )
    .await
}

async fn grant(ctx: &TestContext, include_plugin: bool) -> Result<()> {
    let mut from = vec![json!({"group":"gateway.networking.k8s.io","kind":"HTTPRoute","namespace":NS})];
    if include_plugin {
        from.push(json!({"group":"edgion.io","kind":"EdgionPlugins","namespace":NS}));
    }
    apply(
        ctx,
        resource(
            "ReferenceGrant",
            TARGET_NS,
            json!({"from":from,"to":[{"group":"","kind":"Service","name":NAME}]}),
        ),
    )
    .await
}

async fn gateway(ctx: &TestContext, kind: &str) -> Result<Value> {
    let base = std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| format!("http://{}:12001", ctx.target_host));
    let mut request = ctx
        .http_client
        .get(format!("{}/configclient/{kind}", base.trim_end_matches('/')))
        .query(&[("namespace", NS), ("name", NAME)])
        .timeout(Duration::from_secs(3));
    if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        request = request.bearer_auth(token);
    }
    let response: Value = request.send().await?.error_for_status()?.json().await?;
    ensure!(response["success"] == true, "Gateway resource unavailable: {response}");
    Ok(response["data"].clone())
}

async fn receipts(ctx: &TestContext, trace: &str) -> Result<Vec<Value>> {
    let hosts: Vec<String> = if k8s_mode() {
        vec![
            format!("{NAME}.{NS}.svc.cluster.local:30001"),
            format!("{NAME}.{TARGET_NS}.svc.cluster.local:30002"),
        ]
    } else {
        vec![format!("{}:30001", ctx.target_host)]
    };
    let mut captures = Vec::new();
    for host in hosts {
        let response: Value = ctx
            .http_client
            .get(format!("http://{host}/mirror/query/{trace}"))
            .timeout(Duration::from_secs(3))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        captures.extend(response["captures"].as_array().cloned().unwrap_or_default());
    }
    Ok(captures)
}

fn k8s_mode() -> bool {
    std::env::var("EDGION_TEST_K8S_MODE")
        .is_ok_and(|value| matches!(value.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
}

async fn traffic(ctx: &TestContext, allowed: bool, case: &str, invalid: bool) -> Result<()> {
    let trace = format!("mirror-grant-{case}-{}", uuid::Uuid::new_v4());
    let response = ctx
        .http_client
        .post(format!("{}/mirror/capture", ctx.http_url()))
        .header("host", HOST)
        .header("x-mirror-grant-case", case)
        .header("x-trace-id", &trace)
        .header("access_log", "test_store")
        .body(BODY)
        .timeout(Duration::from_secs(3))
        .send()
        .await?;
    if invalid {
        ensure!(
            response.status() == 500,
            "invalid plugin retained a serving runtime: {}",
            response.status()
        );
        let _ = response.bytes().await?;
        let log = ctx
            .access_log_client()
            .get_access_log_with_retry(&trace, 30, 100)
            .await?;
        ensure!(log.route_name() == Some(NAME), "wrong route selected");
        ensure!(
            receipts(ctx, &trace).await?.is_empty(),
            "invalid plugin sent primary or mirror traffic"
        );
        return Ok(());
    }
    ensure!(response.status() == 200, "{case}: primary status {}", response.status());
    ensure!(
        response.text().await? == "mirror-captured",
        "{case}: wrong primary body"
    );
    let log = ctx
        .access_log_client()
        .get_access_log_with_retry(&trace, 30, 100)
        .await?;
    ensure!(log.route_name() == Some(NAME), "wrong route selected");
    let stage = serde_json::to_string(&log.data["stage_logs"])?;
    ensure!(
        stage.contains("mirror ref denied") != allowed,
        "{case}: wrong denial log: {stage}"
    );
    // A denied mirror returns before spawning any task; its terminal denial log
    // is the completion barrier. Positive sends are asynchronous and need receipt polling.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    loop {
        interval.tick().await;
        let captures = receipts(ctx, &trace).await?;
        let mirrors: Vec<_> = captures.iter().filter(|c| c["headers"]["x-mirror"] == "true").collect();
        ensure!(
            captures
                .iter()
                .any(|c| c["headers"]["x-mirror"] != "true" && c["body"] == BODY),
            "{case}: primary receipt missing"
        );
        if !allowed {
            ensure!(
                mirrors.is_empty() && captures.len() == 1,
                "{case}: denied mirror reached backend: {captures:?}"
            );
            return Ok(());
        }
        if !mirrors.is_empty() {
            ensure!(
                mirrors.len() == 1
                    && captures.len() == 2
                    && mirrors[0]["body"] == BODY
                    && (if k8s_mode() {
                        mirrors[0]["headers"]["host"]
                            .as_str()
                            .is_some_and(|host| host.ends_with(":30002"))
                    } else {
                        mirrors[0]["headers"]["host"] == "127.0.0.1:30002"
                    }),
                "{case}: unexpected mirror receipts: {captures:?}"
            );
            return Ok(());
        }
        ensure!(Instant::now() < deadline, "{case}: authorized mirror never arrived");
    }
}

#[derive(Clone, Copy)]
enum Expected {
    Absent,
    Denied,
    NativeAllowed,
    Allowed,
    InvalidAllowed,
    InvalidDenied,
}
struct MirrorSuite(Expected);

async fn check(ctx: &TestContext, expected: Expected) -> Result<()> {
    if matches!(expected, Expected::Absent) {
        let response = ctx
            .http_client
            .get(format!("{}/mirror/capture", ctx.http_url()))
            .header("host", HOST)
            .timeout(Duration::from_secs(3))
            .send()
            .await?;
        ensure!(response.status() == 404, "route remains after cleanup");
        return Ok(());
    }
    for kind in ["HTTPRoute", "EdgionPlugins"] {
        let allowed = matches!(expected, Expected::Allowed | Expected::InvalidAllowed)
            || (kind == "HTTPRoute" && matches!(expected, Expected::NativeAllowed));
        let processed = helpers::fetch_resource_status(ctx, kind, NS, NAME)
            .await
            .map_err(anyhow::Error::msg)?;
        let conditions = if kind == "HTTPRoute" {
            &processed["status"]["parents"][0]["conditions"]
        } else {
            &processed["status"]["conditions"]
        };
        ensure!(
            conditions
                .as_array()
                .is_some_and(|cs| cs.iter().any(|c| c["type"] == "ResolvedRefs"
                    && c["status"] == if allowed { "True" } else { "False" }
                    && (allowed || c["reason"] == "RefNotPermitted"))),
            "{kind}: wrong reference status: {conditions}"
        );
        if kind == "EdgionPlugins" {
            let invalid = matches!(expected, Expected::InvalidAllowed | Expected::InvalidDenied);
            ensure!(
                conditions.as_array().is_some_and(|cs| cs
                    .iter()
                    .any(|c| c["type"] == "Accepted" && c["status"] == if invalid { "False" } else { "True" })),
                "wrong plugin validation status: {conditions}"
            );
        }
        let synced = gateway(ctx, kind).await?;
        ensure!(
            synced["spec"] == processed["spec"],
            "{kind}: Gateway generation has not converged"
        );
        let refs = if kind == "HTTPRoute" {
            vec![
                &synced["spec"]["rules"][0]["filters"][0]["requestMirror"]["backendRef"],
                &synced["spec"]["rules"][1]["backendRefs"][0]["filters"][0]["requestMirror"]["backendRef"],
            ]
        } else {
            vec![&synced["spec"]["requestPlugins"][0]["config"]["backendRef"]]
        };
        for target in refs {
            ensure!(
                target["name"] == NAME && target["namespace"] == TARGET_NS,
                "mirror target missing: {target}"
            );
            ensure!(
                target["refDenied"].is_null() == allowed,
                "wrong denial result: {target}"
            );
        }
    }
    for case in ["native", "backend", "plugin"] {
        let allowed = matches!(expected, Expected::Allowed | Expected::InvalidAllowed)
            || (case != "plugin" && matches!(expected, Expected::NativeAllowed));
        traffic(
            ctx,
            allowed,
            case,
            case == "plugin" && matches!(expected, Expected::InvalidAllowed | Expected::InvalidDenied),
        )
        .await?;
    }
    Ok(())
}

impl TestSuite for MirrorSuite {
    fn name(&self) -> &str {
        "RequestMirror ReferenceGrant lifecycle"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let expected = self.0;
        vec![TestCase::new(
            "mirror_grant_generation",
            "Reference status, Gateway decision, primary and mirror receipts",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check(&ctx, expected).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(25))]
    }
}
fn absent() -> Box<dyn TestSuite> {
    Box::new(MirrorSuite(Expected::Absent))
}
fn denied() -> Box<dyn TestSuite> {
    Box::new(MirrorSuite(Expected::Denied))
}
fn allowed() -> Box<dyn TestSuite> {
    Box::new(MirrorSuite(Expected::Allowed))
}
fn native_allowed() -> Box<dyn TestSuite> {
    Box::new(MirrorSuite(Expected::NativeAllowed))
}
fn invalid_allowed() -> Box<dyn TestSuite> {
    Box::new(MirrorSuite(Expected::InvalidAllowed))
}
fn invalid_denied() -> Box<dyn TestSuite> {
    Box::new(MirrorSuite(Expected::InvalidDenied))
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "invalid-allowed",
        factory: invalid_allowed,
    },
    PhaseSuite {
        name: "invalid-denied",
        factory: invalid_denied,
    },
    PhaseSuite {
        name: "absent",
        factory: absent,
    },
    PhaseSuite {
        name: "denied",
        factory: denied,
    },
    PhaseSuite {
        name: "allowed",
        factory: allowed,
    },
    PhaseSuite {
        name: "native-allowed",
        factory: native_allowed,
    },
];

async fn cleanup(ctx: &TestContext) -> Result<()> {
    let mut failures = Vec::new();
    for (kind, ns, name) in [
        ("HTTPRoute", NS, NAME),
        ("EdgionPlugins", NS, NAME),
        ("EdgionPlugins", NS, BUFFER_NAME),
        ("ReferenceGrant", TARGET_NS, NAME),
        ("EndpointSlice", NS, NAME),
        ("Service", NS, NAME),
        ("EndpointSlice", TARGET_NS, NAME),
        ("Service", TARGET_NS, NAME),
    ] {
        let mut request = ctx
            .http_client
            .delete(format!("{}/api/v1/namespaced/{kind}/{ns}/{name}", ctx.admin_api_url()))
            .timeout(Duration::from_secs(5));
        if let Some(token) = &ctx.admin_token {
            request = request.bearer_auth(token);
        }
        match request.send().await {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
            Ok(response) => failures.push(format!("{kind}/{ns}: {}", response.status())),
            Err(error) => failures.push(format!("{kind}/{ns}: {error}")),
        }
    }
    ensure!(failures.is_empty(), "cleanup failed: {failures:?}");
    Ok(())
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        cleanup(&ctx.test_ctx).await?;
        let result = async {
            ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(15)).await?;
            if k8s_mode() {
                ctx.apply_dir("DynamicTest/initial/").await?;
            }
            // Prove both listeners and the shared receipt store before testing non-delivery.
            for port in [30001, 30002] {
                let trace = format!("mirror-grant-control-{}", uuid::Uuid::new_v4());
                ctx.test_ctx
                    .http_client
                    .post(if k8s_mode() {
                        let namespace = if port == 30001 { NS } else { TARGET_NS };
                        format!("http://{NAME}.{namespace}.svc.cluster.local:{port}/mirror/capture")
                    } else {
                        format!("http://{}:{port}/mirror/capture", ctx.test_ctx.target_host)
                    })
                    .header("x-trace-id", &trace)
                    .body(BODY)
                    .timeout(Duration::from_secs(3))
                    .send()
                    .await?
                    .error_for_status()?;
                ensure!(
                    receipts(&ctx.test_ctx, &trace).await?.len() == 1,
                    "backend receipt control failed"
                );
            }
            setup(&ctx.test_ctx).await?;
            ctx.wait_phase(SCENARIO, "denied", Duration::from_secs(25)).await?;
            grant(&ctx.test_ctx, false).await?;
            ctx.wait_phase(SCENARIO, "native-allowed", Duration::from_secs(25))
                .await?;
            grant(&ctx.test_ctx, true).await?;
            ctx.wait_phase(SCENARIO, "allowed", Duration::from_secs(25)).await?;
            apply_plugin(&ctx.test_ctx, true).await?;
            ctx.wait_phase(SCENARIO, "invalid-allowed", Duration::from_secs(25))
                .await?;
            ctx.test_ctx
                .delete_resource("ReferenceGrant", TARGET_NS, NAME)
                .await
                .map_err(anyhow::Error::msg)?;
            ctx.wait_phase(SCENARIO, "invalid-denied", Duration::from_secs(25))
                .await?;
            apply_plugin(&ctx.test_ctx, false).await?;
            ctx.wait_phase(SCENARIO, "denied", Duration::from_secs(25)).await?;
            grant(&ctx.test_ctx, true).await?;
            ctx.wait_phase(SCENARIO, "allowed", Duration::from_secs(25)).await
        }
        .await;
        let cleanup_result = cleanup(&ctx.test_ctx).await;
        let absent_result = ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(15)).await;
        result.and(cleanup_result).and(absent_result)
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "RequestMirrorReferenceGrant",
    conf_path: "EdgionPlugins/RequestMirror",
    run,
    phases: PHASES,
};

#[cfg(test)]
mod tests {
    #[test]
    fn k8s_initial_stream_contains_only_controller_supported_services() {
        for namespace in ["edgion-default", "edgion-backend"] {
            let path = format!(
                "{}/k8s/conf/EdgionPlugins/RequestMirror/DynamicTest/initial/{namespace}.yaml",
                env!("CARGO_MANIFEST_DIR")
            );
            let document: serde_yaml::Value = serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(document["kind"], "Service");
        }
    }
}
