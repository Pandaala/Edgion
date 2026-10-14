//! Real endpoint verification of dynamic idle renewal and lifetime admission.
//! Integration logging uses the fallback path; cached matrices have unit coverage.
use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::metrics_helper::counter_value;
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};

const HOST: &str = "metrics-lifetime.example.com";
const COUNTER: &str = "edgion_backend_requests_total";

async fn request_host(ctx: &TestContext, host: &str) -> Result<()> {
    let response = ctx
        .http_client
        .get(format!("{}/echo", ctx.http_url()))
        .header("host", host)
        .timeout(Duration::from_secs(3))
        .send()
        .await?;
    ensure!(
        response.status() == 200,
        "metrics lifetime route returned {}",
        response.status()
    );
    response.bytes().await?;
    Ok(())
}

async fn completed_counter(ctx: &TestContext, labels: &[(&str, &str)]) -> Result<String> {
    let client = ctx.metrics_client();
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let raw =
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), client.fetch_raw_metrics()).await??;
        let count = counter_value(&raw, COUNTER, labels);
        let duration_count = counter_value(&raw, "edgion_gateway_request_duration_ms_count", labels);
        if count == 1. && duration_count == 1. {
            return Ok(raw);
        }
        ensure!(
            (count == 0. || count == 1.) && (duration_count == 0. || duration_count == 1.),
            "completed request produced unexpected counts: counter={count}, duration={duration_count}"
        );
        ensure!(
            Instant::now() < deadline,
            "completed request was not recorded before the deadline"
        );
        // Scraping observes the already completed request; it never refreshes activity.
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn gateway_stats(ctx: &TestContext) -> Result<serde_json::Value> {
    let base =
        std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL").unwrap_or_else(|_| format!("http://{}:12001", ctx.target_host));
    let mut request = ctx.http_client.get(format!("{base}/api/v1/debug/store-stats"));
    if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
        request = request.bearer_auth(token);
    }
    let body: serde_json::Value = request.send().await?.error_for_status()?.json().await?;
    ensure!(body["success"] == true, "gateway diagnostics reported failure");
    ensure!(body["data"].is_object(), "gateway diagnostics omitted object data");
    Ok(body["data"].clone())
}

async fn stats(ctx: &TestContext) -> Result<serde_json::Value> {
    let data = gateway_stats(ctx).await?;
    ensure!(data["metrics"].is_object(), "missing canonical metric diagnostics");
    Ok(data["metrics"].clone())
}

async fn wait_published_route(ctx: &TestContext, name: &str, present: bool, timeout: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    let expected = format!("public-gateway/edgion-test/{name}");
    // This inventory shares the published routing snapshot. Unique fixture names
    // prove membership here, not general route eligibility or backend health.
    tokio::time::timeout_at(deadline, async {
        loop {
            let data = gateway_stats(ctx).await?;
            let routes = data["plugin_publication_inventory"]["ai_routes"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("missing published route inventory array"))?;
            ensure!(
                routes.iter().all(|route| route.is_string()),
                "invalid published route inventory entry"
            );
            if routes.iter().any(|route| route.as_str() == Some(expected.as_str())) == present {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    Ok(())
}

fn dynamic_admissions(stats: &serde_json::Value) -> u64 {
    stats["admission_count"].as_u64().unwrap() - stats["fixed_initialized_count"].as_u64().unwrap()
}

async fn check(ctx: &TestContext) -> Result<()> {
    let client = ctx.metrics_client();
    request_host(ctx, HOST).await?;
    let labels = [("route_name", "metrics-lifetime")];
    let raw = completed_counter(ctx, &labels).await?;
    ensure!(
        counter_value(&raw, COUNTER, &labels) > 0.,
        "route counter was not admitted"
    );
    ensure!(
        raw.contains("edgion_gateway_request_duration_ms_count"),
        "duration missing"
    );
    let baseline = stats(ctx).await?;
    let admitted = dynamic_admissions(&baseline);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let expired = client.fetch_raw_metrics().await?;
    ensure!(
        counter_value(&expired, COUNTER, &labels) == 0.,
        "idle route counter survived TTL"
    );
    ensure!(
        expired.contains("edgion_ctx_created_total"),
        "fixed metric expired with dynamic state"
    );
    request_host(ctx, HOST).await?;
    let renewed = completed_counter(ctx, &labels).await?;
    ensure!(
        counter_value(&renewed, COUNTER, &labels) == 1.,
        "same unchanged route did not start a fresh epoch"
    );
    ensure!(
        dynamic_admissions(&stats(ctx).await?) == admitted,
        "renewal spent lifetime quota"
    );
    let admin = crate::dynamic::apply::build_client(&ctx.admin_api_url())?;
    let template =
        include_str!("../../../../integration/conf/HTTPRoute/MetricsLifetime/HTTPRoute_default_metrics-lifetime.yaml");
    for index in 0..40 {
        let name = format!("metrics-lifetime-epoch-{index}");
        let host = format!("{name}.example.com");
        let yaml = template
            .replacen("name: metrics-lifetime", &format!("name: {name}"), 1)
            .replace(HOST, &host);
        ctx.apply_yaml(&yaml).await.map_err(anyhow::Error::msg)?;
        ctx.wait_for_listener_projection(
            "HTTPRoute",
            "edgion-test",
            &name,
            "resolvedListeners",
            &format!("0.0.0.0:{}/HTTP", ctx.http_port),
            Duration::from_secs(10),
        )
        .await
        .map_err(anyhow::Error::msg)?;
        wait_published_route(ctx, &name, true, Duration::from_secs(10)).await?;
        // Every request must succeed even after new dynamic series are rejected.
        request_host(ctx, &host).await?;
        let response = admin.delete("HTTPRoute", Some("edgion-test"), &name).await?;
        ensure!(response.status().is_success(), "churn route delete failed");
        wait_published_route(ctx, &name, false, Duration::from_secs(10)).await?;
    }
    let saturated = client.fetch_raw_metrics().await?;
    ensure!(
        counter_value(&saturated, "edgion_metrics_series_rejected_total", &[]) > 0.,
        "churn did not reach the shared cap"
    );
    ensure!(
        saturated.contains("edgion_ctx_created_total"),
        "fixed series disappeared at saturation"
    );
    let history = stats(ctx).await?;
    let count = history["admission_count"].as_u64().unwrap();
    let fixed = history["fixed_initialized_count"].as_u64().unwrap();
    ensure!(
        count <= 64 + fixed,
        "serial churn exceeded soft cap plus fixed exceptions"
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    client.fetch_raw_metrics().await?;
    request_host(ctx, HOST).await?;
    let final_raw = completed_counter(ctx, &labels).await?;
    ensure!(
        counter_value(&final_raw, COUNTER, &labels) == 1.,
        "previously admitted route failed to renew at saturation"
    );
    ensure!(
        dynamic_admissions(&stats(ctx).await?) == count - fixed,
        "renewal refunded or spent historical quota"
    );
    Ok(())
}

struct Suite;
impl TestSuite for Suite {
    fn name(&self) -> &str {
        "MetricsLifetime"
    }
    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/MetricsLifetime")
    }
    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "idle_renewal",
            "Idle dynamic epochs disappear and the same route renews",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match check(&ctx).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(120))]
    }
}
static PHASES: &[PhaseSuite] = &[PhaseSuite {
    name: "live",
    factory: || Box::new(Suite),
}];
fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
        crate::dynamic::apply::apply_path(&client, &ctx.scenario_dir()).await?;
        // Wait for actual route synchronization before the one-shot TTL check.
        ctx.test_ctx
            .wait_for_listener_projection(
                "HTTPRoute",
                "edgion-test",
                "metrics-lifetime",
                "resolvedListeners",
                &format!("0.0.0.0:{}/HTTP", ctx.test_ctx.http_port),
                Duration::from_secs(20),
            )
            .await
            .map_err(anyhow::Error::msg)?;
        wait_published_route(&ctx.test_ctx, "metrics-lifetime", true, Duration::from_secs(20)).await?;
        ctx.assert_phase(SCENARIO, "live").await
    })
}
pub static SCENARIO: &Scenario = &Scenario {
    id: "MetricsLifetime",
    conf_path: "HTTPRoute/MetricsLifetime",
    run,
    phases: PHASES,
};
