//! Real TLS probes through the shared HTTPS and mTLS fixture listeners.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

const HOST: &str = "aes-tls-dynamic.example.com";

#[derive(Clone, Copy)]
enum Expected {
    Absent,
    Both,
    Neither,
    TlsOnly,
    InvalidPolicy,
    PlaintextRejected,
    PlaintextOnly,
}

struct ProbeSuite(Expected);

impl TestSuite for ProbeSuite {
    fn name(&self) -> &str {
        "AllEndpointStatusTls"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let expected = self.0;
        vec![TestCase::new(
            "tls_endpoint_results",
            "Probe each exact endpoint with the current TLS policy generation",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check_probe(&ctx, expected).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(12))]
    }
}

async fn check_probe(ctx: &TestContext, expected: Expected) -> Result<()> {
    let mut response = ctx
        .http_client
        .get(format!("{}/echo", ctx.http_url()))
        .header("host", HOST)
        .send()
        .await?;
    // Respect the plugin's global probe gate, independently of the dynamic
    // framework's 200 ms state polling. Keep each assertion attempt bounded.
    for _ in 0..2 {
        if response.status().as_u16() != 429 {
            break;
        }
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .context("rate-limited probe has no numeric Retry-After")?;
        ensure!((1..=2).contains(&retry_after), "unexpected probe retry interval");
        drop(response);
        tokio::time::sleep(Duration::from_secs(retry_after)).await;
        response = ctx
            .http_client
            .get(format!("{}/echo", ctx.http_url()))
            .header("host", HOST)
            .send()
            .await?;
    }
    let status = response.status().as_u16();
    if matches!(expected, Expected::Absent) {
        ensure!(status == 404, "expected removed route, got {status}");
        return Ok(());
    }
    ensure!(status == 200, "expected probe aggregation, got {status}");
    let value: serde_json::Value = response.json().await?;
    let backends = value["backends"].as_array().context("missing backends")?;
    ensure!(backends.len() == 1, "expected exactly one Service");
    ensure!(backends[0]["port"] == 443, "must retain the Service port");
    let endpoints = backends[0]["endpoints"].as_array().context("missing endpoints")?;
    ensure!(endpoints.len() == 2, "expected both serving endpoints");
    let mut ports = BTreeSet::new();
    let mut successes = 0;
    for endpoint in endpoints {
        let address = endpoint["address"].as_str().context("missing endpoint address")?;
        let port: u16 = address.rsplit(':').next().context("missing endpoint port")?.parse()?;
        let expected_ports = if matches!(expected, Expected::PlaintextRejected | Expected::PlaintextOnly) {
            [30001, 30052]
        } else {
            [30051, 30052]
        };
        ensure!(expected_ports.contains(&port), "unexpected endpoint port");
        ensure!(ports.insert(port), "duplicate endpoint result");
        let success = matches!(expected, Expected::Both)
            || matches!(expected, Expected::TlsOnly) && port == 30051
            || matches!(expected, Expected::PlaintextRejected) && port == 30052
            || matches!(expected, Expected::PlaintextOnly) && port == 30001;
        if success {
            ensure!(endpoint["status"] == 200, "TLS probe failed for port {port}");
            ensure!(endpoint["error"].is_null(), "successful endpoint has an error");
            let body = endpoint["body"].as_str().context("missing echo body")?;
            ensure!(
                body.starts_with(&format!("Server: 0.0.0.0:{port}\n")),
                "probe result must come from its exact reported endpoint"
            );
            successes += 1;
        } else {
            ensure!(
                endpoint["status"].is_null(),
                "untrusted endpoint unexpectedly succeeded"
            );
            ensure!(endpoint["body"].is_null(), "failed endpoint exposed a response body");
            ensure!(endpoint["error"].as_str().is_some(), "failed endpoint lacks an error");
            if matches!(expected, Expected::InvalidPolicy) {
                ensure!(
                    endpoint["error"] == "probe configuration unavailable",
                    "invalid policy must fail before dialing"
                );
            }
        }
    }
    ensure!(value["summary"]["success_count"] == successes, "wrong success summary");
    ensure!(
        value["summary"]["failure_count"] == 2 - successes,
        "wrong failure summary"
    );
    Ok(())
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(ProbeSuite(Expected::Absent))
}
fn both() -> Box<dyn TestSuite> {
    Box::new(ProbeSuite(Expected::Both))
}
fn neither() -> Box<dyn TestSuite> {
    Box::new(ProbeSuite(Expected::Neither))
}
fn tls_only() -> Box<dyn TestSuite> {
    Box::new(ProbeSuite(Expected::TlsOnly))
}
fn invalid() -> Box<dyn TestSuite> {
    Box::new(ProbeSuite(Expected::InvalidPolicy))
}
fn plaintext_rejected() -> Box<dyn TestSuite> {
    Box::new(ProbeSuite(Expected::PlaintextRejected))
}
fn plaintext_only() -> Box<dyn TestSuite> {
    Box::new(ProbeSuite(Expected::PlaintextOnly))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "absent",
        factory: absent,
    },
    PhaseSuite {
        name: "valid",
        factory: both,
    },
    PhaseSuite {
        name: "rejected",
        factory: neither,
    },
    PhaseSuite {
        name: "tls_only",
        factory: tls_only,
    },
    PhaseSuite {
        name: "invalid",
        factory: invalid,
    },
    PhaseSuite {
        name: "plaintext_rejected",
        factory: plaintext_rejected,
    },
    PhaseSuite {
        name: "plaintext_only",
        factory: plaintext_only,
    },
];

async fn cleanup_resources(ctx: &ScenarioCtx) -> Result<()> {
    // Cleanup must also work before initial apply and after policy deletion.
    // Inspect the actual response status instead of matching error text.
    let resources = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/integration/conf/EdgionPlugins/AllEndpointStatus/DynamicTest/delete/resources.txt"
    ));
    let token = edgion_cli::cli::auto_auth::resolve_token(None);
    let mut failures = Vec::new();
    for resource in resources.lines().filter(|line| !line.trim().is_empty()) {
        let mut request = ctx
            .test_ctx
            .http_client
            .delete(format!(
                "{}/api/v1/namespaced/{resource}",
                ctx.controller_url.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(5));
        if let Some(token) = token.as_ref() {
            request = request.bearer_auth(token.as_str());
        }
        match request.send().await {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
            Ok(response) => failures.push(format!("delete {resource}: HTTP {}", response.status())),
            Err(error) => failures.push(format!("delete {resource}: {error}")),
        }
    }
    if std::env::var("EDGION_TEST_K8S_MODE")
        .is_ok_and(|value| matches!(value.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
    {
        for name in ["aes-tls-dynamic-tls", "aes-tls-dynamic-mtls"] {
            let mut request = ctx
                .test_ctx
                .http_client
                .delete(format!(
                    "{}/api/v1/namespaced/Deployment/edgion-test/{name}",
                    ctx.controller_url.trim_end_matches('/')
                ))
                .timeout(Duration::from_secs(5));
            if let Some(token) = token.as_ref() {
                request = request.bearer_auth(token.as_str());
            }
            match request.send().await {
                Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
                Ok(response) => failures.push(format!("delete Deployment/{name}: HTTP {}", response.status())),
                Err(error) => failures.push(format!("delete Deployment/{name}: {error}")),
            }
        }
    }
    ensure!(failures.is_empty(), "cleanup failed: {}", failures.join("; "));
    Ok(())
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        // The shared loader may have preloaded any dynamic generation.
        let result = async {
            cleanup_resources(&ctx).await?;
            ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "valid", Duration::from_secs(20)).await?;
            for (manifest, phase) in [
                ("wrong_hostname", "rejected"),
                ("wrong_san", "rejected"),
                ("wrong_ca", "rejected"),
                ("missing_ca", "invalid"),
                ("no_client", "tls_only"),
            ] {
                ctx.apply_dir(&format!("updates/{manifest}.yaml")).await?;
                ctx.wait_phase(SCENARIO, phase, Duration::from_secs(20)).await?;
                // Re-establish trusted connections before each rejection so
                // stale TLS clients cannot make a later generation pass.
                ctx.apply_dir("initial/04_BackendTLSPolicy.yaml").await?;
                ctx.wait_phase(SCENARIO, "valid", Duration::from_secs(20)).await?;
            }
            // A live plaintext listener must not be retried without TLS while
            // policy is effective. Only removing the policy permits HTTP.
            ctx.apply_dir("updates/plaintext_endpoint.yaml").await?;
            ctx.wait_phase(SCENARIO, "plaintext_rejected", Duration::from_secs(20))
                .await?;
            ctx.delete_resources("delete/policy.txt").await?;
            ctx.wait_phase(SCENARIO, "plaintext_only", Duration::from_secs(20))
                .await?;
            Ok(())
        }
        .await;
        let cleanup = async {
            cleanup_resources(&ctx).await?;
            ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await
        }
        .await;
        match (result, cleanup) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup_error)) => {
                Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup_error:#}"))
            }
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "AllEndpointStatusTls",
    conf_path: "EdgionPlugins/AllEndpointStatus/DynamicTest",
    run,
    phases: PHASES,
};
