//! Shared-topology policy phase assertions, including real upstream controls.

use anyhow::{ensure, Context, Result};
use std::io::ErrorKind;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::counter_value;
use crate::suites::status::helpers::{controller_local_status, fetch_resource_status};
use crate::suites::tls_route::probe_tls_policy;

pub const MOCK_BODY: &str = "PP-MOCK-TERMINATED";
const BLOCK_BODY: &str = r#"{"message":"plugin disabled by policy"}"#;

#[derive(Clone, Copy)]
pub enum Outcome {
    Allowed,
    Block(u16),
    Bypass,
    Unavailable,
}

#[derive(Clone, Copy)]
pub struct PolicySuite {
    pub a: Outcome,
    pub b: Outcome,
    pub invalid_sources: bool,
}

fn counter(raw: &str, bypass: bool) -> f64 {
    counter_value(
        raw,
        if bypass {
            "edgion_gateway_plugin_policy_bypassed_total"
        } else {
            "edgion_gateway_plugin_policy_blocked_total"
        },
        &[("plugin", "http/Mock")],
    )
}

async fn http_probe(ctx: &TestContext, label: &str, port: u16, path: &str, outcome: Outcome) -> Result<()> {
    let trace = uuid::Uuid::new_v4().simple().to_string();
    let metric_before = match outcome {
        Outcome::Block(_) | Outcome::Bypass if path == "mock" => Some(counter(
            &ctx.metrics_client().fetch_raw_metrics().await?,
            matches!(outcome, Outcome::Bypass),
        )),
        _ => None,
    };
    let response = ctx
        .http_client
        .get(format!("http://{}:{port}/{path}", ctx.target_host))
        .header("host", format!("pp-{label}.example.com"))
        .header("x-trace-id", &trace)
        .header("access_log", "test_store")
        .header("traceparent", format!("00-{trace}-0123456789abcdef-01"))
        .timeout(Duration::from_secs(3))
        .send()
        .await;
    let response = response?;
    let status = response.status().as_u16();
    if path == "response" {
        ensure!(
            response.headers().contains_key("x-policy-response") == matches!(outcome, Outcome::Allowed),
            "response-stage plugin effect disagrees with scope policy"
        );
    }
    let body = response.text().await?;
    match outcome {
        Outcome::Block(expected) => {
            ensure!(status == expected, "{label}/{path}: expected {expected}, got {status}");
            ensure!(body == BLOCK_BODY, "policy block must have only its generic body");
        }
        Outcome::Allowed if path == "async" => {
            ensure!(
                status == 500,
                "wrong-stage declaration must fail closed before policy restriction"
            );
        }
        Outcome::Allowed if matches!(path, "mock" | "dsl" | "waf") => {
            ensure!(
                status == 200 && body == MOCK_BODY,
                "{label}/{path}: allowed or retained Mock did not execute ({status})"
            );
        }
        Outcome::Allowed | Outcome::Bypass => {
            ensure!(status == 200, "{label}/{path}: expected upstream success, got {status}");
            ensure!(
                body.starts_with("Server: 0.0.0.0:30001\n"),
                "{label}/{path}: real upstream was not reached"
            );
            ensure!(!body.contains(MOCK_BODY), "bypassed Mock still executed");
        }
        Outcome::Unavailable => {
            ensure!(status == 503, "unavailable scope must return HTTP 503, got {status}");
            ensure!(
                body.is_empty(),
                "unavailable scope must return the generic empty error body"
            );
        }
    }
    if let Some(before) = metric_before {
        let bypass = matches!(outcome, Outcome::Bypass);
        let entry = ctx
            .access_log_client()
            .get_access_log_with_retry(&trace, 20, 100)
            .await?;
        let encoded = serde_json::to_string(&entry.data)?;
        let action = if bypass {
            "PluginPolicyBypass"
        } else {
            "PluginPolicyBlock"
        };
        ensure!(
            encoded.contains(action) && encoded.contains("http/Mock"),
            "missing policy PluginLog ownership"
        );
        ensure!(
            entry.status_code() == Some(status.into()),
            "access log status differs from response"
        );
        if !bypass {
            ensure!(entry.upstream_count() == 0, "blocked request reached upstream");
        }
        let after = counter(&ctx.metrics_client().fetch_raw_metrics().await?, bypass);
        ensure!(after > before, "policy decision did not increment its existing counter");
        ctx.access_log_client().delete_access_log(&trace).await?;
    }
    Ok(())
}

fn policy_close(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe
    )
}

async fn tcp_probe(ctx: &TestContext, port: u16) -> Result<bool> {
    // Refusal/timeouts are environment failures, never evidence of a policy deny.
    let mut stream = tokio::time::timeout(
        Duration::from_secs(3),
        TcpStream::connect((ctx.target_host.as_str(), port)),
    )
    .await
    .context("TCP connect timed out")??;
    let payload = b"policy-echo";
    if let Err(error) = stream.write_all(payload).await {
        if policy_close(&error) {
            return Ok(false);
        }
        return Err(error.into());
    }
    let mut received = [0; 11];
    match tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut received)).await {
        Ok(Ok(_)) => {
            ensure!(&received == payload, "TCP echo mismatch");
            Ok(true)
        }
        Ok(Err(error)) if policy_close(&error) => Ok(false),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => anyhow::bail!("TCP response timed out"),
    }
}

pub async fn udp_socket(ctx: &TestContext, port: u16) -> Result<UdpSocket> {
    let target = tokio::net::lookup_host((ctx.target_host.as_str(), port))
        .await?
        .next()
        .context("UDP target resolved to no addresses")?;
    let socket = UdpSocket::bind(if target.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }).await?;
    socket.connect(target).await?;
    Ok(socket)
}

pub async fn udp_echo(socket: &UdpSocket, allowed: bool) -> Result<()> {
    let payload = uuid::Uuid::new_v4().to_string();
    socket.send(payload.as_bytes()).await?;
    let mut received = [0; 128];
    match tokio::time::timeout(Duration::from_millis(750), socket.recv(&mut received)).await {
        Ok(Ok(length)) => {
            ensure!(allowed, "denied new UDP session reached upstream");
            ensure!(&received[..length] == payload.as_bytes(), "UDP echo mismatch");
            Ok(())
        }
        Ok(Err(error)) => Err(error.into()),
        Err(_) => {
            ensure!(!allowed, "allowed UDP session did not echo");
            Ok(())
        }
    }
}

async fn source_status(ctx: &TestContext, invalid: bool) -> Result<()> {
    for (name, expected) in [
        ("pp-mock", true),
        ("pp-dsl", !invalid),
        ("pp-waf", !invalid),
        ("pp-response", true),
        ("pp-body", true),
        ("pp-async", false),
    ] {
        let resource = fetch_resource_status(ctx, "EdgionPlugins", "edgion-test", name)
            .await
            .map_err(anyhow::Error::msg)?;
        let local_status = controller_local_status(&resource["status"], "edgion.io/gateway-controller")
            .context("source status has no Edgion controller observation")?;
        let conditions = local_status["conditions"]
            .as_array()
            .context("source status has no conditions")?;
        ensure!(
            !conditions
                .iter()
                .any(|condition| condition["type"] == "PluginPolicyRestricted"),
            "shared source carries a global policy marker"
        );
        ensure!(
            conditions.iter().any(|condition| condition["type"] == "Accepted"
                && condition["status"] == if expected { "True" } else { "False" }),
            "unexpected source Accepted condition for {name}"
        );
    }
    Ok(())
}

impl TestSuite for PolicySuite {
    fn name(&self) -> &str {
        "PluginPolicyDynamic"
    }
    fn port_key(&self) -> Option<&str> {
        Some("PluginPolicy")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let phase = *self;
        vec![TestCase::new(
            "scoped_policy",
            "Observe both classes, all HTTP stages and new stream/TLS admissions",
            move |ctx| {
                Box::pin(async move {
                    let started = Instant::now();
                    let result = async {
                        for (label, http, tcp, tls, passthrough, outcome, host) in [
                            ("a", 58330, 58331, 58334, 58335, phase.a, "pp-a.example.com"),
                            ("b", 58332, 58333, 58336, 58337, phase.b, "pp-b.example.com"),
                        ] {
                            for path in ["mock", "dsl", "waf", "response", "body", "async"] {
                                http_probe(&ctx, label, http, path, outcome).await?;
                            }
                            let allowed = matches!(outcome, Outcome::Allowed | Outcome::Bypass);
                            ensure!(
                                tcp_probe(&ctx, tcp).await? == allowed,
                                "{label}: incorrect TCP admission"
                            );
                            let udp = udp_socket(&ctx, if label == "a" { 58338 } else { 58339 }).await?;
                            udp_echo(&udp, allowed).await?;
                            if crate::suites::tls_route::tls_route_terminate_tests_enabled() {
                                ensure!(
                                    probe_tls_policy(&ctx, tls, host, false).await? == allowed,
                                    "{label}: incorrect TLS terminate admission"
                                );
                            }
                            ensure!(
                                probe_tls_policy(&ctx, passthrough, host, true).await? == allowed,
                                "{label}: incorrect TLS passthrough admission"
                            );
                        }
                        source_status(&ctx, phase.invalid_sources).await
                    }
                    .await;
                    match result {
                        Ok(()) => TestResult::passed(started.elapsed()),
                        Err(error) => TestResult::failed(started.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(45))]
    }
}

pub struct AbsentSuite;

impl TestSuite for AbsentSuite {
    fn name(&self) -> &str {
        "PluginPolicyCleanup"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "owned_resources_absent",
            "Old policy generations and bindings leave the Gateway",
            |ctx| {
                Box::pin(async move {
                    let started = Instant::now();
                    let result = async {
                        let gateway = std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL")
                            .unwrap_or_else(|_| format!("http://{}:12001", ctx.target_host));
                        for kind in [
                            "gateway",
                            "gatewayclass",
                            "edgiongatewayconfig",
                            "edgionplugins",
                            "edgionstreamplugins",
                            "httproute",
                            "tcproute",
                            "udproute",
                            "tlsroute",
                            "edgiontls",
                            "service",
                            "endpointslice",
                        ] {
                            let mut request = ctx.http_client.get(format!("{gateway}/configclient/{kind}/list"));
                            if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
                                request = request.bearer_auth(token);
                            }
                            let response: serde_json::Value = request.send().await?.error_for_status()?.json().await?;
                            ensure!(response["success"] == true, "Gateway cache query failed");
                            let objects = response["data"]
                                .as_array()
                                .context("Gateway cache query has no resource array")?;
                            ensure!(
                                !objects.iter().any(|object| {
                                    let name = object["metadata"]["name"].as_str().unwrap_or_default();
                                    (matches!(
                                        name,
                                        "pp-backend"
                                            | "pp-a"
                                            | "pp-b"
                                            | "pp-mock"
                                            | "pp-dsl"
                                            | "pp-waf"
                                            | "pp-response"
                                            | "pp-body"
                                            | "pp-async"
                                            | "pp-stream"
                                            | "pp-a-terminate"
                                            | "pp-b-terminate"
                                            | "pp-a-passthrough"
                                            | "pp-b-passthrough"
                                    ) || object["metadata"]["labels"]["kubernetes.io/service-name"] == "pp-backend")
                                        && object["metadata"]["namespace"]
                                            .as_str()
                                            .is_none_or(|ns| ns == "edgion-test")
                                }),
                                "owned {kind} remained after cleanup"
                            );
                        }
                        Ok::<_, anyhow::Error>(())
                    }
                    .await;
                    match result {
                        Ok(()) => TestResult::passed(started.elapsed()),
                        Err(error) => TestResult::failed(started.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )]
    }
}
