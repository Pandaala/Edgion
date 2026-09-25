//! TLSRoute Stage-2 hot reload on the shared Terminate and Passthrough listeners.
//! Every target probe has a separate healthy control on the same listener.
//! Each update changes the expected decision, so stale generations cannot pass.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestResult, TestSuite};
use crate::suites::tls_route::probe_tls_policy;
use anyhow::{Context, Result};
use std::time::{Duration, Instant};

struct AdmissionSuite {
    allowed: bool,
}

impl TestSuite for AdmissionSuite {
    fn name(&self) -> &str {
        "TLSRoute StreamPlugins"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let allowed = self.allowed;
        [
            (
                "terminate",
                31282,
                "reload.sp-allow.example.com",
                "reload-control.sp-allow.example.com",
                false,
            ),
            (
                "passthrough",
                31290,
                "reload.sandbox.example.com",
                "reload-control.sandbox.example.com",
                true,
            ),
        ]
        .into_iter()
        .filter(|(_, _, _, _, passthrough)| {
            *passthrough || crate::suites::tls_route::tls_route_terminate_tests_enabled()
        })
        .map(|(name, port, hostname, control, passthrough)| {
            TestCase::new(
                name,
                "TLSRoute policy update with an independent healthy control",
                move |ctx| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let result = async {
                            anyhow::ensure!(
                                probe_tls_policy(&ctx, port, control, passthrough).await?,
                                "healthy control route was denied"
                            );
                            anyhow::ensure!(
                                probe_tls_policy(&ctx, port, hostname, passthrough).await? == allowed,
                                "target route did not converge to allowed={allowed}"
                            );
                            Ok::<_, anyhow::Error>(())
                        }
                        .await;
                        match result {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                        }
                    })
                },
            )
        })
        .collect()
    }
}

fn allow() -> Box<dyn TestSuite> {
    Box::new(AdmissionSuite { allowed: true })
}

fn deny() -> Box<dyn TestSuite> {
    Box::new(AdmissionSuite { allowed: false })
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "allow",
        factory: allow,
    },
    PhaseSuite {
        name: "deny",
        factory: deny,
    },
];

/// Do not let recursively preloaded fixture generations survive scenario reset.
async fn wait_absent(ctx: &ScenarioCtx) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let mut absent = true;
        for kind in ["edgionstreamplugins", "tlsroute"] {
            let url = format!("{}/configclient/{kind}/list", ctx.gateway_url.trim_end_matches('/'));
            let mut request = ctx.test_ctx.http_client.get(&url).timeout(Duration::from_secs(3));
            if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
                if !token.trim().is_empty() {
                    request = request.bearer_auth(token);
                }
            }
            let body: serde_json::Value = request.send().await?.error_for_status()?.json().await?;
            anyhow::ensure!(body["success"] == true, "gateway resource list failed");
            let items = body["data"]
                .as_array()
                .context("gateway resource list has no data array")?;
            absent &= !items.iter().any(|item| {
                item["metadata"]["namespace"] == "edgion-test"
                    && item["metadata"]["name"]
                        .as_str()
                        .is_some_and(|name| name == "tls-sp-reload" || name.starts_with("tls-sp-reload-"))
            });
        }
        if absent {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "dynamic resources remained in the gateway after cleanup"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let resources = tokio::fs::read_to_string(ctx.resolve("delete/resources.txt")).await?;
    let mut errors = Vec::new();
    for line in resources.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split('/').collect();
        anyhow::ensure!(fields.len() == 3, "invalid dynamic cleanup identity");
        match client.delete(fields[0], Some(fields[1]), fields[2]).await {
            Ok(response) if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND => {}
            Ok(response) => errors.push(format!("cleanup {line}: {}", response.status())),
            Err(error) => errors.push(format!("cleanup {line}: {error}")),
        }
    }
    anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
    wait_absent(ctx).await
}

async fn phase(ctx: &ScenarioCtx, allowed: bool) -> Result<()> {
    ctx.wait_phase(
        SCENARIO,
        if allowed { "allow" } else { "deny" },
        Duration::from_secs(15),
    )
    .await
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        if !crate::suites::tls_route::tls_route_terminate_tests_enabled() {
            crate::suites::tls_route::report_terminate_deferred(
                "TlsRoutePluginsReload/terminate target and healthy control",
            );
        }
        let result = async {
            cleanup(&ctx).await?;
            ctx.apply_dir("routes").await?;
            phase(&ctx, false).await?;
            ctx.apply_dir("initial").await?;
            phase(&ctx, true).await?;

            // Empty and disabled Stage-2 configurations must not run Stage-1 deny.
            for update in ["empty", "disabled", "no_plugins"] {
                ctx.apply_dir("updates/deny").await?;
                phase(&ctx, false).await?;
                ctx.apply_dir(&format!("updates/{update}")).await?;
                phase(&ctx, true).await?;
            }
            // An omitted Stage-2 declaration with Stage-1 entries fails closed.
            ctx.apply_dir("updates/stage1_only").await?;
            phase(&ctx, false).await?;
            ctx.apply_dir("initial").await?;
            phase(&ctx, true).await?;

            ctx.delete_resources("delete/plugin.txt").await?;
            phase(&ctx, false).await?;
            ctx.apply_dir("initial").await?;
            phase(&ctx, true).await
        }
        .await;

        let cleaned = cleanup(&ctx).await;
        match (result, cleaned) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error.context("dynamic TLS policy cleanup failed")),
            (Err(error), Err(cleanup_error)) => Err(error.context(format!("cleanup also failed: {cleanup_error:#}"))),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "TlsRoutePluginsReload",
    conf_path: "TLSRoute/StreamPlugins/DynamicTest",
    run,
    phases: PHASES,
};
