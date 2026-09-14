//! Live regression for delegation parentRefs that share a namespace/name with a Gateway.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::grpc_route::test::test_service_client::TestServiceClient;
use crate::suites::grpc_route::test::{HelloRequest, NumberRequest};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::net::IpAddr;
use std::time::{Duration, Instant};

const NS: &str = "edgion-test";
const HTTP_NAME: &str = "parentref-collision-http";
const HTTP_CHILD: &str = "parentref-collision-http-child";
const HTTP_PARENT_HOST: &str = "parent.parentref-collision-http.test";
const HTTP_DIRECT_HOST: &str = "direct.parentref-collision-http.test";
const GRPC_NAME: &str = "parentref-collision-grpc";
const GRPC_CHILD: &str = "parentref-collision-grpc-child";
const GRPC_PARENT_HOST: &str = "parent.parentref-collision-grpc.test";
const GRPC_DIRECT_HOST: &str = "direct.parentref-collision-grpc.test";
const TIMEOUT: Duration = Duration::from_secs(20);

fn k8s_mode() -> bool {
    std::env::var("EDGION_TEST_K8S_MODE")
        .is_ok_and(|value| matches!(value.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
}

#[derive(Clone, Copy)]
enum Generation {
    Initial,
    Updated,
}

impl Generation {
    fn number(self) -> i64 {
        match self {
            Self::Initial => 1,
            Self::Updated => 2,
        }
    }
}

struct CollisionSuite {
    grpc: bool,
    generation: Generation,
    absent: bool,
}

impl TestSuite for CollisionSuite {
    fn name(&self) -> &str {
        if self.grpc {
            "GRPCRoute delegation ParentRef Gateway collision"
        } else {
            "HTTPRoute delegation ParentRef Gateway collision"
        }
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let grpc = self.grpc;
        let generation = self.generation;
        let absent = self.absent;
        vec![TestCase::new(
            "delegation_parentref_gateway_collision",
            "Assert exact parent identity, publication, direct denial, delegated payload, and access-log owner",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = if absent {
                        check_absent(&ctx, grpc).await
                    } else if grpc {
                        check_grpc(&ctx, generation).await
                    } else {
                        check_http(&ctx, generation).await
                    };
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(15))]
    }
}

macro_rules! phase_factory {
    ($fn_name:ident, $grpc:expr, $generation:ident, $absent:expr) => {
        fn $fn_name() -> Box<dyn TestSuite> {
            Box::new(CollisionSuite {
                grpc: $grpc,
                generation: Generation::$generation,
                absent: $absent,
            })
        }
    };
}

phase_factory!(http_initial, false, Initial, false);
phase_factory!(http_updated, false, Updated, false);
phase_factory!(http_absent, false, Updated, true);
phase_factory!(grpc_initial, true, Initial, false);
phase_factory!(grpc_updated, true, Updated, false);
phase_factory!(grpc_absent, true, Updated, true);

static HTTP_PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: http_initial,
    },
    PhaseSuite {
        name: "updated",
        factory: http_updated,
    },
    PhaseSuite {
        name: "absent",
        factory: http_absent,
    },
];

static GRPC_PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "initial",
        factory: grpc_initial,
    },
    PhaseSuite {
        name: "updated",
        factory: grpc_updated,
    },
    PhaseSuite {
        name: "absent",
        factory: grpc_absent,
    },
];

fn authed(ctx: &TestContext, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match ctx.admin_token.as_deref() {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

fn gateway_admin_url(ctx: &TestContext) -> String {
    std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .map(|url| url.trim_end_matches('/').to_string())
        .unwrap_or_else(|| format!("http://{}:12001", ctx.target_host))
}

async fn resource(ctx: &TestContext, kind: &str, name: &str, gateway: bool) -> Result<Option<Value>> {
    let (base, prefix) = if gateway {
        (gateway_admin_url(ctx), "configclient")
    } else {
        (ctx.admin_api_url(), "configserver")
    };
    let mut request = ctx
        .http_client
        .get(format!("{base}/{prefix}/{kind}?namespace={NS}&name={name}"))
        .timeout(Duration::from_secs(3));
    request = if gateway {
        match std::env::var("EDGION_GATEWAY_ADMIN_TOKEN").ok() {
            Some(token) => request.bearer_auth(token),
            None => authed(ctx, request),
        }
    } else {
        authed(ctx, request)
    };
    let response = request.send().await?;
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
        "GET {kind}/{NS}/{name}: {status} {body}"
    );
    Ok(body.get("data").filter(|data| !data.is_null()).cloned())
}

fn normalized_group(parent_ref: &Value) -> &str {
    parent_ref["group"].as_str().unwrap_or("gateway.networking.k8s.io")
}

fn normalized_kind(parent_ref: &Value) -> &str {
    parent_ref["kind"].as_str().unwrap_or("Gateway")
}

fn assert_status_identity(resource: &Value, expected_kind: &str, expected_generation: i64) -> Result<()> {
    ensure!(resource.pointer("/metadata/generation").and_then(Value::as_i64) == Some(expected_generation));
    let parents = resource
        .pointer("/status/parents")
        .and_then(Value::as_array)
        .context("missing status.parents")?;
    ensure!(parents.len() == 1, "expected exactly one parent status: {parents:?}");
    let parent = &parents[0];
    let parent_ref = &parent["parentRef"];
    ensure!(
        normalized_group(parent_ref) == "gateway.networking.k8s.io"
            && normalized_kind(parent_ref) == expected_kind
            && parent_ref["namespace"] == NS,
        "wrong parent type: {parent_ref}"
    );
    let expected_name = if expected_kind == "Gateway" {
        resource["metadata"]["name"].as_str().context("route name")?
    } else if expected_kind == "HTTPRoute" {
        HTTP_NAME
    } else {
        GRPC_NAME
    };
    ensure!(parent_ref["name"] == expected_name, "wrong parent name: {parent_ref}");
    let accepted = parent["conditions"]
        .as_array()
        .context("missing parent conditions")?
        .iter()
        .find(|condition| condition["type"] == "Accepted")
        .context("missing Accepted condition")?;
    ensure!(
        accepted["status"] == "True" && accepted["observedGeneration"] == expected_generation,
        "stale or rejected parent status: {accepted}"
    );
    Ok(())
}

fn assert_child_has_no_gateway_identity(resource: &Value, coordinate: &str) -> Result<()> {
    let parents = resource
        .pointer("/status/parents")
        .and_then(Value::as_array)
        .context("missing parents")?;
    ensure!(
        !parents.iter().any(|parent| {
            let parent_ref = &parent["parentRef"];
            normalized_kind(parent_ref) == "Gateway"
                && parent_ref["namespace"] == NS
                && parent_ref["name"] == coordinate
        }),
        "child acquired a same-coordinate Gateway parent status"
    );
    Ok(())
}

fn assert_bootstrap_gateway(resource: &Value, grpc: bool) -> Result<()> {
    let (name, listener_name, hostname, port) = if grpc {
        (GRPC_NAME, "grpc", "*.parentref-collision-grpc.test", 31971)
    } else {
        (HTTP_NAME, "http", "*.parentref-collision-http.test", 31970)
    };
    ensure!(
        resource["metadata"]["name"] == name
            && resource["metadata"]["namespace"] == NS
            && resource["metadata"]["generation"] == 1,
        "wrong bootstrap Gateway identity: {resource}"
    );
    let listeners = resource["spec"]["listeners"]
        .as_array()
        .context("bootstrap Gateway has no listeners")?;
    ensure!(listeners.len() == 1, "bootstrap Gateway listener count changed");
    let listener = &listeners[0];
    ensure!(
        listener["name"] == listener_name
            && listener["hostname"] == hostname
            && listener["protocol"] == "HTTP"
            && listener["port"] == port,
        "wrong bootstrap Gateway listener: {listener}"
    );
    Ok(())
}

async fn assert_publication(ctx: &TestContext, grpc: bool, generation: Generation) -> Result<()> {
    let (kind, parent_name, child_name, route_kind) = if grpc {
        ("GRPCRoute", GRPC_NAME, GRPC_CHILD, "GRPCRoute")
    } else {
        ("HTTPRoute", HTTP_NAME, HTTP_CHILD, "HTTPRoute")
    };
    let expected_generation = generation.number();
    let parent = resource(ctx, kind, parent_name, false)
        .await?
        .context("Controller parent absent")?;
    let child = resource(ctx, kind, child_name, false)
        .await?
        .context("Controller child absent")?;
    assert_status_identity(&parent, "Gateway", 1)?;
    assert_status_identity(&child, route_kind, expected_generation)?;
    assert_child_has_no_gateway_identity(&child, parent_name)?;

    let published_parent = resource(ctx, kind, parent_name, true)
        .await?
        .context("Gateway parent absent")?;
    let published_child = resource(ctx, kind, child_name, true)
        .await?
        .context("Gateway child absent")?;
    ensure!(published_parent["metadata"]["generation"] == 1);
    ensure!(published_child["metadata"]["generation"] == expected_generation);
    ensure!(published_parent["spec"]["hostnames"]
        .as_array()
        .is_some_and(|hosts| hosts.len() == 1));
    let child_parent = &published_child["spec"]["parentRefs"][0];
    ensure!(
        published_child["spec"]["parentRefs"]
            .as_array()
            .is_some_and(|refs| refs.len() == 1)
            && normalized_group(child_parent) == "gateway.networking.k8s.io"
            && normalized_kind(child_parent) == route_kind
            && child_parent["namespace"] == NS
            && child_parent["name"] == parent_name,
        "Gateway child payload lost exact delegation identity: {published_child}"
    );
    if grpc {
        let expected_method = if matches!(generation, Generation::Initial) {
            "SayHello"
        } else {
            "StreamNumbers"
        };
        ensure!(
            published_child.pointer("/spec/rules/0/matches/0/method/method") == Some(&Value::from(expected_method))
        );
    } else {
        let expected_path = if matches!(generation, Generation::Initial) {
            "/delegated/v1"
        } else {
            "/delegated/v2"
        };
        ensure!(published_child.pointer("/spec/rules/0/matches/0/path/value") == Some(&Value::from(expected_path)));
    }
    Ok(())
}

async fn http_request(
    ctx: &TestContext,
    host: &str,
    path: &str,
    logged: bool,
) -> Result<(u16, String, Option<String>)> {
    let trace = logged.then(|| format!("parentref-collision-http-{}", uuid::Uuid::new_v4()));
    let mut request = ctx
        .http_client
        .get(format!("{}{}", ctx.http_url(), path))
        .header("host", host)
        .timeout(Duration::from_secs(3));
    if let Some(trace) = &trace {
        request = request
            .header("traceparent", crate::access_log_client::traceparent_for_marker(trace))
            .header("access_log", "test_store");
    }
    let response = request.send().await?;
    let status = response.status().as_u16();
    let body = response.text().await?;
    Ok((status, body, trace))
}

async fn check_http(ctx: &TestContext, generation: Generation) -> Result<()> {
    assert_publication(ctx, false, generation).await?;
    let (active, inactive) = if matches!(generation, Generation::Initial) {
        ("/delegated/v1", "/delegated/v2")
    } else {
        ("/delegated/v2", "/delegated/v1")
    };
    for path in [active, inactive] {
        let (status, _, _) = http_request(ctx, HTTP_DIRECT_HOST, path, false).await?;
        ensure!(
            status == 404,
            "delegation-only child attached directly for {path}: {status}"
        );
    }
    let (inactive_status, _, _) = http_request(ctx, HTTP_PARENT_HOST, inactive, false).await?;
    ensure!(
        inactive_status == 404,
        "stale delegated path remained active: {inactive_status}"
    );
    let (status, body, trace) = http_request(ctx, HTTP_PARENT_HOST, active, true).await?;
    ensure!(status == 200, "delegated request returned {status}");
    ensure!(
        body.contains("Server: 0.0.0.0:30001"),
        "unexpected delegated HTTP payload: {body}"
    );
    let trace = trace.context("missing trace id")?;
    let log = ctx
        .access_log_client()
        .get_access_log_with_retry(&trace, 5, 100)
        .await?;
    ensure!(
        log.route_kind() == Some("HTTPRoute")
            && log.route_namespace() == Some(NS)
            && log.route_name() == Some(HTTP_NAME)
            && log.request_host() == Some(HTTP_PARENT_HOST)
            && log.request_path() == Some(active)
            && log.status_code() == Some(200),
        "wrong HTTP access-log owner or payload: {:?}",
        log.data
    );
    Ok(())
}

fn grpc_endpoint(ctx: &TestContext, authority: &str) -> Result<tonic::transport::Endpoint> {
    let url = format!("http://{}:{}", ctx.target_host, ctx.grpc_port);
    let origin: http::Uri = format!("http://{authority}:{}", ctx.grpc_port).parse()?;
    Ok(tonic::transport::Endpoint::from_shared(url)?.origin(origin))
}

async fn grpc_say_hello(
    ctx: &TestContext,
    authority: &str,
    logged: bool,
) -> Result<(tonic::Code, Option<String>, Option<String>)> {
    let mut client = TestServiceClient::connect(grpc_endpoint(ctx, authority)?).await?;
    let trace = logged.then(|| format!("parentref-collision-grpc-{}", uuid::Uuid::new_v4()));
    let mut request = tonic::Request::new(HelloRequest {
        name: "ParentRefCollision".to_string(),
    });
    if let Some(trace) = &trace {
        request.metadata_mut().insert(
            "traceparent",
            crate::access_log_client::traceparent_for_marker(trace).parse()?,
        );
        request.metadata_mut().insert("access_log", "test_store".parse()?);
    }
    match client.say_hello(request).await {
        Ok(response) => {
            let response = response.into_inner();
            ensure!(
                response.server_addr == "0.0.0.0:30021",
                "unexpected gRPC backend: {}",
                response.server_addr
            );
            Ok((tonic::Code::Ok, Some(response.message), trace))
        }
        Err(status) => Ok((status.code(), None, trace)),
    }
}

async fn grpc_stream_numbers(
    ctx: &TestContext,
    authority: &str,
    logged: bool,
) -> Result<(tonic::Code, Option<Vec<i32>>, Option<String>)> {
    let mut client = TestServiceClient::connect(grpc_endpoint(ctx, authority)?).await?;
    let trace = logged.then(|| format!("parentref-collision-grpc-{}", uuid::Uuid::new_v4()));
    let mut request = tonic::Request::new(NumberRequest { count: 2 });
    if let Some(trace) = &trace {
        request.metadata_mut().insert(
            "traceparent",
            crate::access_log_client::traceparent_for_marker(trace).parse()?,
        );
        request.metadata_mut().insert("access_log", "test_store".parse()?);
    }
    match client.stream_numbers(request).await {
        Ok(response) => {
            let mut stream = response.into_inner();
            let mut values = Vec::new();
            while let Some(item) = stream.message().await? {
                values.push(item.number);
            }
            Ok((tonic::Code::Ok, Some(values), trace))
        }
        Err(status) => Ok((status.code(), None, trace)),
    }
}

fn validate_say_hello_metadata(message: &str, trace: &str, k8s: bool) -> Result<()> {
    let metadata = message
        .strip_prefix("Hello, ParentRefCollision! metadata=(")
        .and_then(|value| value.strip_suffix(')'))
        .context("unexpected SayHello envelope")?;
    let entries = metadata
        .split(',')
        .map(|entry| entry.split_once('=').context("malformed SayHello metadata entry"))
        .collect::<Result<Vec<_>>>()?;
    ensure!(entries.len() == 3, "expected exactly three SayHello metadata entries");
    for key in ["x-trace-id", "x-forwarded-for", "x-real-ip"] {
        ensure!(
            entries.iter().filter(|(actual, _)| *actual == key).count() == 1,
            "SayHello metadata must contain {key} exactly once"
        );
    }
    let value = |key: &str| {
        entries
            .iter()
            .find(|(actual, _)| *actual == key)
            .map(|(_, value)| *value)
            .unwrap()
    };
    ensure!(
        value("x-trace-id") == crate::access_log_client::trace_id_for_marker(trace),
        "SayHello trace metadata mismatch"
    );
    let forwarded = value("x-forwarded-for");
    let real = value("x-real-ip");
    if k8s {
        let forwarded: IpAddr = forwarded.parse().context("invalid x-forwarded-for IP")?;
        let real: IpAddr = real.parse().context("invalid x-real-ip IP")?;
        ensure!(forwarded == real, "forwarded and real client IPs differ");
        ensure!(!real.is_unspecified(), "client IP is unspecified");
        ensure!(!real.is_loopback(), "Kubernetes client IP is loopback");
    } else {
        ensure!(forwarded == "127.0.0.1", "unexpected local x-forwarded-for");
        ensure!(real == "127.0.0.1", "unexpected local x-real-ip");
    }
    Ok(())
}

async fn check_grpc(ctx: &TestContext, generation: Generation) -> Result<()> {
    assert_publication(ctx, true, generation).await?;
    let initial = matches!(generation, Generation::Initial);
    let direct_say = grpc_say_hello(ctx, GRPC_DIRECT_HOST, false).await?.0;
    let direct_stream = grpc_stream_numbers(ctx, GRPC_DIRECT_HOST, false).await?.0;
    ensure!(
        direct_say == tonic::Code::Unimplemented && direct_stream == tonic::Code::Unimplemented,
        "delegation-only gRPC child attached directly: {direct_say:?}, {direct_stream:?}"
    );
    let (trace, path) = if initial {
        ensure!(grpc_stream_numbers(ctx, GRPC_PARENT_HOST, false).await?.0 == tonic::Code::Unimplemented);
        let (code, message, trace) = grpc_say_hello(ctx, GRPC_PARENT_HOST, true).await?;
        ensure!(code == tonic::Code::Ok);
        let message = message.context("missing SayHello payload")?;
        let trace = trace.context("missing trace id")?;
        validate_say_hello_metadata(&message, &trace, k8s_mode())
            .with_context(|| format!("unexpected SayHello metadata: {message}"))?;
        (trace, "/test.TestService/SayHello")
    } else {
        ensure!(grpc_say_hello(ctx, GRPC_PARENT_HOST, false).await?.0 == tonic::Code::Unimplemented);
        let (code, values, trace) = grpc_stream_numbers(ctx, GRPC_PARENT_HOST, true).await?;
        ensure!(
            code == tonic::Code::Ok && values == Some(vec![1, 2]),
            "unexpected StreamNumbers payload: {values:?}"
        );
        (trace.context("missing trace id")?, "/test.TestService/StreamNumbers")
    };
    let log = ctx
        .access_log_client()
        .get_access_log_with_retry(&trace, 5, 100)
        .await?;
    ensure!(
        log.route_kind() == Some("GRPCRoute")
            && log.route_namespace() == Some(NS)
            && log.route_name() == Some(GRPC_NAME)
            && log.request_path() == Some(path)
            && log.status_code() == Some(200),
        "wrong gRPC access-log owner or payload: {:?}",
        log.data
    );
    Ok(())
}

async fn check_absent(ctx: &TestContext, grpc: bool) -> Result<()> {
    let (kind, name, child) = if grpc {
        ("GRPCRoute", GRPC_NAME, GRPC_CHILD)
    } else {
        ("HTTPRoute", HTTP_NAME, HTTP_CHILD)
    };
    for gateway in [false, true] {
        ensure!(resource(ctx, kind, name, gateway).await?.is_none(), "parent remains");
        ensure!(resource(ctx, kind, child, gateway).await?.is_none(), "child remains");
        let bootstrap = resource(ctx, "Gateway", name, gateway)
            .await?
            .context("bootstrap Gateway disappeared")?;
        assert_bootstrap_gateway(&bootstrap, grpc)?;
    }
    Ok(())
}

async fn delete_if_present(ctx: &TestContext, kind: &str, name: &str) -> Result<()> {
    if resource(ctx, kind, name, false).await?.is_some() {
        ctx.delete_resource(kind, NS, name).await.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

async fn cleanup_scenario_resources(ctx: &TestContext, grpc: bool) -> Result<()> {
    let (kind, name, child) = if grpc {
        ("GRPCRoute", GRPC_NAME, GRPC_CHILD)
    } else {
        ("HTTPRoute", HTTP_NAME, HTTP_CHILD)
    };
    delete_if_present(ctx, kind, child).await?;
    delete_if_present(ctx, kind, name).await
}

async fn run_scenario(ctx: ScenarioCtx, scenario: &'static Scenario, grpc: bool) -> Result<()> {
    // The Gateway is bootstrapped before process startup because listener ports
    // are not dynamically bound. Own only the two Route resources here so a
    // prior or broad fixture load cannot choose their initial generation.
    cleanup_scenario_resources(&ctx.test_ctx, grpc).await?;
    ctx.wait_phase(scenario, "absent", TIMEOUT).await?;

    let result = async {
        ctx.apply_dir("initial/").await?;
        ctx.wait_phase(scenario, "initial", TIMEOUT).await?;
        ctx.apply_dir("updates/").await?;
        ctx.wait_phase(scenario, "updated", TIMEOUT).await
    }
    .await;

    let cleanup = async {
        cleanup_scenario_resources(&ctx.test_ctx, grpc).await?;
        ctx.wait_phase(scenario, "absent", TIMEOUT).await
    }
    .await;
    result.and(cleanup)
}

fn run_http(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move { run_scenario(ctx, HTTP_SCENARIO, false).await })
}

fn run_grpc(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move { run_scenario(ctx, GRPC_SCENARIO, true).await })
}

pub static HTTP_SCENARIO: &Scenario = &Scenario {
    id: "DelegationParentRefGatewayCollisionHttp",
    conf_path: "HTTPRoute/Delegation/ParentRefGatewayCollision/DynamicTest",
    run: run_http,
    phases: HTTP_PHASES,
};

pub static GRPC_SCENARIO: &Scenario = &Scenario {
    id: "DelegationParentRefGatewayCollisionGrpc",
    conf_path: "GRPCRoute/Delegation/ParentRefGatewayCollision/DynamicTest",
    run: run_grpc,
    phases: GRPC_PHASES,
};

#[cfg(test)]
mod tests {
    use super::{validate_say_hello_metadata, GRPC_SCENARIO, HTTP_SCENARIO};
    use std::path::Path;

    #[test]
    fn registered_scenario_paths_resolve_lifecycle_fixtures() {
        for topology in ["integration", "k8s"] {
            let conf_root = Path::new(env!("CARGO_MANIFEST_DIR")).join(topology).join("conf");
            for (scenario, kind) in [(HTTP_SCENARIO, "HTTPRoute"), (GRPC_SCENARIO, "GRPCRoute")] {
                let scenario_dir = conf_root.join(scenario.conf_path);
                for relative in [
                    "../bootstrap/01_Gateway.yaml".to_string(),
                    format!("initial/02_{kind}_parent.yaml"),
                    format!("initial/03_{kind}_child.yaml"),
                    format!("updates/03_{kind}_child.yaml"),
                ] {
                    let fixture = scenario_dir.join(relative);
                    assert!(fixture.is_file(), "{} is missing {}", scenario.id, fixture.display());
                }
            }
        }
    }

    fn message(metadata: &str) -> String {
        let metadata = metadata.replace("trace-1", &crate::access_log_client::trace_id_for_marker("trace-1"));
        format!("Hello, ParentRefCollision! metadata=({metadata})")
    }

    #[test]
    fn say_hello_metadata_is_topology_aware_and_rejects_invalid_shapes() {
        let trace = "trace-1";
        assert!(validate_say_hello_metadata(
            &message("x-trace-id=trace-1,x-forwarded-for=10.0.0.8,x-real-ip=10.0.0.8"),
            trace,
            true
        )
        .is_ok());
        assert!(validate_say_hello_metadata(
            &message("x-trace-id=trace-1,x-forwarded-for=127.0.0.1,x-real-ip=127.0.0.1"),
            trace,
            false
        )
        .is_ok());

        for metadata in [
            "x-trace-id=trace-1,x-forwarded-for=10.0.0.8,x-forwarded-for=10.0.0.8",
            "x-trace-id=trace-1,x-forwarded-for=10.0.0.8",
            "x-trace-id=trace-1,x-forwarded-for=10.0.0.8,x-real-ip=10.0.0.8,x-extra=value",
            "x-trace-id=trace-1,x-forwarded-for=bad,x-real-ip=bad",
            "x-trace-id=trace-1,x-forwarded-for=10.0.0.8,x-real-ip=10.0.0.9",
            "x-trace-id=trace-1,x-forwarded-for=0.0.0.0,x-real-ip=0.0.0.0",
            "x-trace-id=trace-1,x-forwarded-for=127.0.0.1,x-real-ip=127.0.0.1",
        ] {
            assert!(validate_say_hello_metadata(&message(metadata), trace, true).is_err());
        }
    }
}
