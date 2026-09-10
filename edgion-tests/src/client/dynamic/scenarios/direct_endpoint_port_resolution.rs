//! Exercise section authorization and discovery generations through the shared proxy.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};

const HOST: &str = "direct-port-dynamic.example.com";

fn k8s_mode() -> bool {
    std::env::var("EDGION_TEST_K8S_MODE")
        .is_ok_and(|value| matches!(value.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
}

fn endpoint_ip() -> Result<String> {
    if k8s_mode() {
        let value = std::env::var("EDGION_TEST_DIRECT_ENDPOINT_IP")?;
        ensure!(!value.trim().is_empty(), "Kubernetes endpoint IP is empty");
        Ok(value)
    } else {
        Ok("127.0.0.1".to_string())
    }
}

fn unready_endpoint_ip() -> Result<String> {
    if k8s_mode() {
        let value = std::env::var("EDGION_TEST_DIRECT_UNREADY_ENDPOINT_IP")?;
        ensure!(!value.trim().is_empty(), "Kubernetes unready endpoint IP is empty");
        Ok(value)
    } else {
        endpoint_ip()
    }
}

#[derive(Clone, Copy)]
enum Expected {
    Absent,
    Ready(u16),
    Unready,
    Missing,
    InvalidTls,
}

struct DirectSuite(Expected);

async fn request(ctx: &TestContext, target: Option<&str>) -> Result<reqwest::Response> {
    let mut request = ctx
        .http_client
        .get(format!("{}/echo", ctx.http_url()))
        .header("host", HOST);
    if let Some(target) = target {
        request = request.header("x-target-ip", target);
    }
    Ok(request.timeout(Duration::from_secs(3)).send().await?)
}

async fn check(ctx: &TestContext, expected: Expected) -> Result<()> {
    let endpoint_ip = if matches!(expected, Expected::Unready) {
        unready_endpoint_ip()?
    } else {
        endpoint_ip()?
    };
    if matches!(expected, Expected::Absent) {
        ensure!(
            request(ctx, None).await?.status().as_u16() == 404,
            "route still present"
        );
        return Ok(());
    }
    if matches!(expected, Expected::Missing) {
        for target in [
            endpoint_ip.clone(),
            format!("{endpoint_ip}:30002"),
            format!("{endpoint_ip}:30003"),
        ] {
            ensure!(
                request(ctx, Some(&target)).await?.status().as_u16() == 403,
                "missing authorized section must reject {target}"
            );
        }
        return Ok(());
    }
    if matches!(expected, Expected::InvalidTls) {
        // Plaintext is a live listener: an invalid policy must never degrade to HTTP.
        let response = request(ctx, Some(&endpoint_ip)).await?;
        ensure!(
            response.status().as_u16() == 403,
            "invalid TLS policy unexpectedly succeeded"
        );
        return Ok(());
    }
    let port = match expected {
        Expected::Ready(port) => port,
        Expected::Unready => 30002,
        _ => unreachable!(),
    };
    for target in [endpoint_ip.clone(), format!("{endpoint_ip}:{port}")] {
        let response = request(ctx, Some(&target)).await?;
        ensure!(
            response.status().as_u16() == 200,
            "authorized endpoint failed: {}",
            response.status()
        );
        ensure!(
            response
                .headers()
                .get("x-direct-endpoint")
                .is_some_and(|value| value == format!("{endpoint_ip}:{port}").as_str()),
            "direct selection marker has the wrong serving port"
        );
        let body = response.text().await?;
        ensure!(
            body.starts_with(&format!("Server: 0.0.0.0:{port}\n")),
            "request reached the wrong listener"
        );
    }
    // This port exists on the same IP and slice, but belongs to another Service section.
    ensure!(
        request(ctx, Some(&format!("{endpoint_ip}:30003")))
            .await?
            .status()
            .as_u16()
            == 403,
        "sibling section escaped route authorization"
    );
    if port != 30001 {
        ensure!(
            request(ctx, Some(&format!("{endpoint_ip}:30001")))
                .await?
                .status()
                .as_u16()
                == 403,
            "obsolete serving port remained authorized"
        );
    }
    let fallback = request(ctx, None).await?;
    if matches!(expected, Expected::Unready) {
        ensure!(
            fallback.status().is_server_error(),
            "normal LB must not select an unready endpoint"
        );
    } else {
        ensure!(fallback.status().as_u16() == 200, "normal LB fallback failed");
        ensure!(
            fallback.headers().get("x-direct-endpoint").is_none(),
            "fallback used direct preset"
        );
        ensure!(
            fallback.text().await?.starts_with(&format!("Server: 0.0.0.0:{port}\n")),
            "normal LB fallback reached another section"
        );
    }
    Ok(())
}

impl TestSuite for DirectSuite {
    fn name(&self) -> &str {
        "DirectEndpointPortResolution"
    }
    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let expected = self.0;
        vec![TestCase::new(
            "authorized_section",
            "Resolve only the authorized section in the current generation",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check(&ctx, expected).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(20))]
    }
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(DirectSuite(Expected::Absent))
}
fn initial() -> Box<dyn TestSuite> {
    Box::new(DirectSuite(Expected::Ready(30001)))
}
fn changed() -> Box<dyn TestSuite> {
    Box::new(DirectSuite(Expected::Ready(30002)))
}
fn unready() -> Box<dyn TestSuite> {
    Box::new(DirectSuite(Expected::Unready))
}
fn missing() -> Box<dyn TestSuite> {
    Box::new(DirectSuite(Expected::Missing))
}
fn tls() -> Box<dyn TestSuite> {
    Box::new(DirectSuite(Expected::Ready(30051)))
}
fn invalid_tls() -> Box<dyn TestSuite> {
    Box::new(DirectSuite(Expected::InvalidTls))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "absent",
        factory: absent,
    },
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "changed",
        factory: changed,
    },
    PhaseSuite {
        name: "unready",
        factory: unready,
    },
    PhaseSuite {
        name: "missing",
        factory: missing,
    },
    PhaseSuite {
        name: "tls",
        factory: tls,
    },
    PhaseSuite {
        name: "invalid_tls",
        factory: invalid_tls,
    },
];

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let token = edgion_cli::cli::auto_auth::resolve_token(None);
    let mut failures = Vec::new();
    for kind in [
        "HTTPRoute",
        "EdgionPlugins",
        "BackendTLSPolicy",
        "EndpointSlice",
        "Service",
    ] {
        let mut request = ctx
            .test_ctx
            .http_client
            .delete(format!(
                "{}/api/v1/namespaced/{kind}/edgion-test/direct-port-dynamic",
                ctx.controller_url.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(5));
        if let Some(token) = &token {
            request = request.bearer_auth(token.as_str());
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

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup(&ctx).await?;
            ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await?;
            let transitions: &[(&str, &str)] = if k8s_mode() {
                &[
                    ("updates/changed.yaml", "changed"),
                    ("updates/unready.yaml", "unready"),
                    ("updates/missing.yaml", "missing"),
                    ("updates/unready.yaml", "unready"),
                    ("initial/00_Service.yaml", "initial"),
                    ("updates/tls/", "tls"),
                ]
            } else {
                &[
                    ("updates/changed.yaml", "changed"),
                    ("updates/unready.yaml", "unready"),
                    ("updates/missing.yaml", "missing"),
                    ("initial/00_Service.yaml", "unready"),
                    ("initial/01_EndpointSlice.yaml", "initial"),
                    ("updates/tls/", "tls"),
                ]
            };
            for (manifest, phase) in transitions {
                ctx.apply_dir(manifest).await?;
                ctx.wait_phase(SCENARIO, phase, Duration::from_secs(20)).await?;
            }
            ctx.apply_dir("updates/invalid_tls.yaml").await?;
            // Restore a live plaintext endpoint while the effective TLS policy is invalid.
            ctx.apply_dir(if k8s_mode() {
                "initial/00_Service.yaml"
            } else {
                "initial/01_EndpointSlice.yaml"
            })
            .await?;
            ctx.wait_phase(SCENARIO, "invalid_tls", Duration::from_secs(20)).await?;
            ctx.apply_dir("updates/tls/").await?;
            ctx.wait_phase(SCENARIO, "tls", Duration::from_secs(20)).await?;
            Ok(())
        }
        .await;
        let cleanup_result = async {
            cleanup(&ctx).await?;
            ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await
        }
        .await;
        match (result, cleanup_result) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "DirectEndpointPortResolution",
    conf_path: "EdgionPlugins/DirectEndpoint/DynamicTest",
    run,
    phases: PHASES,
};

#[cfg(test)]
mod tests {
    #[test]
    fn k8s_unready_fixture_keeps_a_running_backend_out_of_ready_inventory() {
        let deployment: serde_yaml::Value = serde_yaml::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/k8s/conf/EdgionPlugins/DirectEndpoint/DynamicTest/bootstrap/01_Deployment_unready.yaml"
        )))
        .unwrap();
        let service: serde_yaml::Value = serde_yaml::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/k8s/conf/EdgionPlugins/DirectEndpoint/DynamicTest/updates/unready.yaml"
        )))
        .unwrap();

        assert_eq!(
            deployment["spec"]["template"]["metadata"]["labels"]["app"],
            "direct-port-unready"
        );
        assert_eq!(
            deployment["metadata"]["labels"]["edgion.io/test-server-rollout-ready"],
            "false"
        );
        assert_eq!(deployment["spec"]["strategy"]["type"], "Recreate");
        assert_eq!(
            deployment["spec"]["template"]["spec"]["containers"][0]["ports"][0]["containerPort"],
            30002
        );
        assert_eq!(
            deployment["spec"]["template"]["spec"]["containers"][0]["readinessProbe"]["tcpSocket"]["port"],
            39999
        );
        assert_eq!(service["spec"]["selector"]["app"], "direct-port-unready");
        assert_eq!(service["spec"]["ports"][0]["targetPort"], 30002);
    }
}
