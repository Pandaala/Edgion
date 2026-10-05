//! A partial LinkSys update must replace the callable provider for one key.
//! Uses the shared Gateway, test_server, and Redis fixture in each test environment.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::time::{Duration, Instant};

const NS: &str = "edgion-test";
const NAME: &str = "linksys-provider-replacement";
const AUTH: &str = "linksys-provider-replacement-auth";
const KEY: &str = "edgion-test/linksys-provider-replacement";
const HOST: &str = "linksys-provider-replacement.example.com";
const TIMEOUT: Duration = Duration::from_secs(45);
const STABILITY_WINDOW: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Absent,
    WebhookV1,
    Redis,
    Unsupported,
    WebhookV2,
    Deleted,
}

impl Phase {
    fn provider(self) -> Option<&'static str> {
        match self {
            Self::WebhookV1 | Self::WebhookV2 => Some("webhook"),
            Self::Redis => Some("redis"),
            Self::Absent | Self::Deleted | Self::Unsupported => None,
        }
    }
}

struct ReplacementSuite(Phase, Duration);

impl TestSuite for ReplacementSuite {
    fn name(&self) -> &str {
        "LinkSysProviderReplacement"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.0;
        let stability_window = self.1;
        vec![TestCase::new(
            "current_provider",
            "Check current runtime keys and live Webhook/Redis calls from an unchanged consumer",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        let mut probes = tokio::time::interval(Duration::from_millis(250));
                        probes.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        let mut stable_since = None;
                        loop {
                            probes.tick().await;
                            check(&ctx, phase).await?;
                            let first_success = *stable_since.get_or_insert_with(Instant::now);
                            if Instant::now().saturating_duration_since(first_success) >= stability_window {
                                return Ok::<(), anyhow::Error>(());
                            }
                        }
                    }
                    .await;
                    let elapsed = Instant::now().saturating_duration_since(start);
                    match result {
                        Ok(()) => TestResult::passed(elapsed),
                        Err(error) => TestResult::failed(elapsed, format!("{phase:?}: {error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(30))]
    }
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(ReplacementSuite(Phase::Absent, Duration::ZERO))
}
fn webhook_v1() -> Box<dyn TestSuite> {
    Box::new(ReplacementSuite(Phase::WebhookV1, Duration::ZERO))
}
fn unsupported() -> Box<dyn TestSuite> {
    Box::new(ReplacementSuite(Phase::Unsupported, Duration::ZERO))
}
fn redis() -> Box<dyn TestSuite> {
    Box::new(ReplacementSuite(Phase::Redis, Duration::ZERO))
}
fn webhook_v2() -> Box<dyn TestSuite> {
    Box::new(ReplacementSuite(Phase::WebhookV2, Duration::ZERO))
}
fn webhook_v2_stable() -> Box<dyn TestSuite> {
    Box::new(ReplacementSuite(Phase::WebhookV2, STABILITY_WINDOW))
}
fn deleted() -> Box<dyn TestSuite> {
    Box::new(ReplacementSuite(Phase::Deleted, Duration::ZERO))
}

static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "unsupported",
        factory: unsupported,
    },
    PhaseSuite {
        name: "absent",
        factory: absent,
    },
    PhaseSuite {
        name: "webhook_v1",
        factory: webhook_v1,
    },
    PhaseSuite {
        name: "redis",
        factory: redis,
    },
    PhaseSuite {
        name: "webhook_v2",
        factory: webhook_v2,
    },
    PhaseSuite {
        name: "deleted",
        factory: deleted,
    },
    PhaseSuite {
        name: "webhook_v2_stable",
        factory: webhook_v2_stable,
    },
];

fn gateway_url(ctx: &TestContext) -> String {
    std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| format!("http://{}:12001", ctx.target_host))
        .trim_end_matches('/')
        .to_owned()
}

fn authed(ctx: &TestContext, request: reqwest::RequestBuilder, gateway: bool) -> reqwest::RequestBuilder {
    let gateway_token = gateway
        .then(|| std::env::var("EDGION_GATEWAY_ADMIN_TOKEN").ok())
        .flatten();
    let request = request.timeout(Duration::from_secs(3));
    match gateway_token.as_deref().or(ctx.admin_token.as_deref()) {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

async fn resource(ctx: &TestContext, kind: &str, name: &str, gateway: bool) -> Result<Option<Value>> {
    let base = if gateway { gateway_url(ctx) } else { ctx.admin_api_url() };
    let prefix = if gateway { "configclient" } else { "configserver" };
    let response = authed(
        ctx,
        ctx.http_client
            .get(format!("{base}/{prefix}/{kind}"))
            .query(&[("namespace", NS), ("name", name)]),
        gateway,
    )
    .send()
    .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = response.status();
    let endpoint = response.url().clone();
    let bytes = response.bytes().await?;
    let body: Value = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "{endpoint} returned a non-JSON response: status={status}, bytes={}",
            bytes.len()
        )
    })?;
    if gateway && status.is_success() && body["success"] == false && body["error"] == format!("{kind} not found") {
        return Ok(None);
    }
    ensure!(
        status.is_success() && body["success"] == true,
        "{kind} query failed: {status}"
    );
    Ok(body.get("data").filter(|data| !data.is_null()).cloned())
}

async fn gateway_data(ctx: &TestContext, path: &str) -> Result<Value> {
    let response = authed(ctx, ctx.http_client.get(format!("{}{path}", gateway_url(ctx))), true)
        .send()
        .await?;
    let status = response.status();
    let endpoint = response.url().clone();
    let bytes = response.bytes().await?;
    ensure!(
        status.is_success(),
        "Gateway inventory query failed: {endpoint}: {status}"
    );
    let body: Value = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "{endpoint} returned a non-JSON response: status={status}, bytes={}",
            bytes.len()
        )
    })?;
    ensure!(body["success"] == true, "Gateway inventory query failed: {path}");
    Ok(body["data"].clone())
}

async fn check_inventory(ctx: &TestContext, phase: Phase) -> Result<()> {
    let stats = gateway_data(ctx, "/api/v1/debug/store-stats").await?;
    let keys = stats["link_sys_runtime"]["keys"]
        .as_array()
        .context("missing current LinkSys runtime keys")?;
    let occurrences = keys.iter().filter(|key| key.as_str() == Some(KEY)).count();
    ensure!(
        occurrences == usize::from(phase.provider().is_some()),
        "expected one current entry when configured, none when removed; got {occurrences} for {KEY}"
    );
    let inventory = gateway_data(ctx, "/api/v1/testing/link-sys/redis/clients").await?;
    let clients = inventory.as_array().context("missing Redis client inventory")?;
    ensure!(
        clients.iter().any(|key| key.as_str() == Some(KEY)) == (phase == Phase::Redis),
        "Redis inventory does not match the current provider"
    );
    Ok(())
}

async fn check_traffic(ctx: &TestContext, phase: Phase) -> Result<()> {
    // Fresh markers prevent a cached or previous-generation result from passing.
    // The fixed plugin does a real Redis SET/GET/DEL and a real Webhook exchange.
    let marker = uuid::Uuid::new_v4().to_string();
    let response = ctx
        .http_client
        .get(format!("{}/echo/provider-replacement", ctx.http_url()))
        .header("Host", HOST)
        .header("X-Replacement-Marker", &marker)
        .timeout(Duration::from_secs(8))
        .send()
        .await?;
    ensure!(
        response.status().as_u16() == 200,
        "business request failed: {}",
        response.status()
    );
    let expected_webhook = match phase {
        Phase::WebhookV1 => format!("v1-{marker}"),
        Phase::WebhookV2 => format!("v2-{marker}"),
        _ => "unavailable".to_owned(),
    };
    let actual_webhook = response
        .headers()
        .get("x-replacement-webhook")
        .and_then(|value| value.to_str().ok());
    ensure!(
        actual_webhook == Some(expected_webhook.as_str()),
        "x-replacement-webhook: expected {expected_webhook:?}, got {actual_webhook:?}"
    );

    let actual_redis = response
        .headers()
        .get("x-replacement-redis")
        .and_then(|value| value.to_str().ok());
    if phase == Phase::Redis {
        ensure!(
            actual_redis == Some(marker.as_str()),
            "x-replacement-redis: expected {marker:?}, got {actual_redis:?}"
        );
    } else {
        // The DSL may expose a non-current typed client as either an unavailable
        // operation or a failed round trip. That presentation is not part of
        // the LinkSys lifecycle contract; only a successful marker would prove
        // that a stale Redis generation remained callable.
        ensure!(
            actual_redis != Some(marker.as_str()),
            "stale Redis generation remained callable: {actual_redis:?}"
        );
    }
    Ok(())
}

fn local_conditions(resource: &Value) -> Result<&Vec<Value>> {
    // Published Kubernetes status retains all deployment observations; currentStatus
    // carries the observation computed by the Controller serving this connection.
    resource
        .pointer("/spec/currentStatus/conditions")
        .or_else(|| resource.pointer("/status/conditions"))
        .and_then(Value::as_array)
        .context("missing current LinkSys conditions")
}

async fn check(ctx: &TestContext, phase: Phase) -> Result<()> {
    for gateway in [false, true] {
        let current = resource(ctx, "LinkSys", NAME, gateway).await?;
        if let Some(provider) = phase
            .provider()
            .or((phase == Phase::Unsupported).then_some("unsupported"))
        {
            let current = current.context("current LinkSys is absent")?;
            ensure!(
                current["spec"]["type"] == provider,
                "LinkSys provider has not converged"
            );
            if !gateway {
                let conditions = local_conditions(&current)?;
                let accepted = conditions
                    .iter()
                    .find(|condition| condition["type"] == "Accepted")
                    .context("missing Accepted condition")?;
                let expected = if phase == Phase::Unsupported { "False" } else { "True" };
                ensure!(accepted["status"] == expected, "unexpected Accepted status");
                if phase == Phase::Unsupported {
                    ensure!(
                        accepted["reason"] == "Invalid",
                        "unsupported LinkSys must report Invalid"
                    );
                }
                if let Some(generation) = current["metadata"]["generation"].as_i64() {
                    ensure!(
                        accepted["observedGeneration"] == generation,
                        "stale Accepted generation"
                    );
                }
            }
            if phase == Phase::Unsupported {
                ensure!(
                    current["spec"]["config"] == serde_json::json!({}),
                    "unknown payload survived publication"
                );
            }
            if matches!(phase, Phase::WebhookV1 | Phase::WebhookV2) {
                let version = if phase == Phase::WebhookV1 { "v1" } else { "v2" };
                ensure!(
                    current["spec"]["config"]["request"]["headers"]["custom"][0]["template"]
                        == format!("{version}-${{header:X-Replacement-Marker}}"),
                    "stale Webhook generation"
                );
            }
        } else {
            ensure!(current.is_none(), "removed LinkSys remains in config cache");
        }
    }
    check_inventory(ctx, phase).await?;
    if phase == Phase::Absent {
        for kind in ["HTTPRoute", "EdgionPlugins"] {
            for gateway in [false, true] {
                ensure!(
                    resource(ctx, kind, NAME, gateway).await?.is_none(),
                    "{kind} remains after cleanup"
                );
            }
        }
        // Secret is intentionally excluded from ConfigSync, so it cannot be
        // inspected through /configserver. cleanup() still deletes the owned
        // fixture; LinkSys absence proves that no resolved credential-bearing
        // resource remains published to the Gateway.
        return Ok(());
    }
    check_traffic(ctx, phase).await
}

async fn delete_resource(ctx: &ScenarioCtx, kind: &str, name: &str) -> Result<()> {
    let response = authed(
        &ctx.test_ctx,
        ctx.test_ctx.http_client.delete(format!(
            "{}/api/v1/namespaced/{kind}/{NS}/{name}",
            ctx.controller_url.trim_end_matches('/')
        )),
        false,
    )
    .send()
    .await?;
    ensure!(
        response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND,
        "delete {kind}/{name} failed: {}",
        response.status()
    );
    Ok(())
}

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let mut errors = Vec::new();
    for (kind, name) in [
        ("HTTPRoute", NAME),
        ("EdgionPlugins", NAME),
        ("LinkSys", NAME),
        ("Secret", AUTH),
    ] {
        if let Err(error) = delete_resource(ctx, kind, name).await {
            errors.push(format!("{error:#}"));
        }
    }
    if let Err(error) = ctx.wait_phase(SCENARIO, "absent", TIMEOUT).await {
        errors.push(format!("{error:#}"));
    }
    ensure!(errors.is_empty(), "cleanup failed: {errors:?}");
    Ok(())
}

async fn apply_unsupported(ctx: &ScenarioCtx) -> Result<()> {
    // This topology-independent fixture also runs when conf_root points at k8s/conf.
    ctx.test_ctx
        .apply_yaml(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/integration/conf/LinkSys/Redis/ProviderReplacement/DynamicTest/updates/unsupported.yaml"
        )))
        .await
        .map_err(anyhow::Error::msg)
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            // Static loaders may preload either generation. Own only these names.
            cleanup(&ctx).await?;
            ctx.apply_dir("initial/").await?;
            ctx.wait_phase(SCENARIO, "webhook_v1", TIMEOUT).await?;
            apply_unsupported(&ctx).await?;
            ctx.wait_phase(SCENARIO, "unsupported", TIMEOUT).await?;
            ctx.apply_dir("initial/01_LinkSys.yaml").await?;
            ctx.wait_phase(SCENARIO, "webhook_v1", TIMEOUT).await?;

            // Only one LinkSys changes per transition: no full-set/reload or
            // consumer re-apply is allowed to repair a stale provider entry.
            ctx.apply_dir("updates/redis.yaml").await?;
            ctx.wait_phase(SCENARIO, "redis", TIMEOUT).await?;
            apply_unsupported(&ctx).await?;
            ctx.wait_phase(SCENARIO, "unsupported", TIMEOUT).await?;
            ctx.apply_dir("updates/redis.yaml").await?;
            ctx.wait_phase(SCENARIO, "redis", TIMEOUT).await?;
            ctx.apply_dir("updates/webhook-v2.yaml").await?;
            ctx.wait_phase(SCENARIO, "webhook_v2", TIMEOUT).await?;

            delete_resource(&ctx, "LinkSys", NAME).await?;
            ctx.wait_phase(SCENARIO, "deleted", TIMEOUT).await?;
            ctx.apply_dir("updates/redis.yaml").await?;
            ctx.wait_phase(SCENARIO, "redis", TIMEOUT).await?;

            // Recreate without awaiting cleanup: a late old-provider task must
            // not remove or republish over the new Webhook generation.
            delete_resource(&ctx, "LinkSys", NAME).await?;
            ctx.apply_dir("updates/webhook-v2.yaml").await?;
            ctx.wait_phase(SCENARIO, "webhook_v2", TIMEOUT).await?;
            // Observe repeated fresh traffic after convergence, without another
            // configuration event. Fail on any regression; do not retry this
            // phase and hide a late failure. The bounded window is evidence of
            // stability, not proof that every old SDK task has terminated.
            ctx.assert_phase(SCENARIO, "webhook_v2_stable").await
        }
        .await;
        let cleanup_result = cleanup(&ctx).await;
        match (result, cleanup_result) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "LinkSysProviderReplacement",
    conf_path: "LinkSys/Redis/ProviderReplacement/DynamicTest",
    run,
    phases: PHASES,
};

#[cfg(test)]
mod tests {
    use super::{local_conditions, AUTH, KEY, NAME, NS};
    use edgion_resources::{EdgionPlugins, HTTPRoute, LinkSys};
    use serde_json::Value;

    macro_rules! fixture {
        ($path:literal) => {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/integration/conf/LinkSys/Redis/ProviderReplacement/DynamicTest/",
                $path
            ))
        };
    }

    #[test]
    fn conditions_use_current_projection_and_native_fallback() {
        let resource = serde_json::json!({
            "spec": {"currentStatus": {"conditions": [{"type": "Accepted", "status": "False"}]}},
            "status": {"controllers": [{"controllerName": "foreign", "status": {
                "conditions": [{"type": "Accepted", "status": "True"}]
            }}]}
        });
        assert_eq!(local_conditions(&resource).unwrap()[0]["status"], "False");
        let native = serde_json::json!({"status": {"conditions": [{"type": "Accepted", "status": "True"}]}});
        assert_eq!(local_conditions(&native).unwrap()[0]["status"], "True");
        let mut missing_projection = resource;
        missing_projection["spec"] = serde_json::json!({});
        assert!(local_conditions(&missing_projection).is_err());
    }

    #[test]
    fn tracked_fixtures_validate_and_keep_one_replacement_key() {
        for (source, provider) in [
            (fixture!("initial/01_LinkSys.yaml"), "webhook"),
            (fixture!("updates/redis.yaml"), "redis"),
            (fixture!("updates/webhook-v2.yaml"), "webhook"),
        ] {
            let resource: LinkSys = serde_yaml::from_str(source).expect("tracked LinkSys fixture must deserialize");
            let errors = resource.validate_config();
            assert!(errors.is_empty(), "invalid LinkSys fixture: {errors:?}");
            let value: Value = serde_yaml::from_str(source).unwrap();
            assert_eq!(value["metadata"]["name"], NAME);
            assert_eq!(value["metadata"]["namespace"], NS);
            assert_eq!(value["spec"]["type"], provider);
        }
        let plugins: EdgionPlugins = serde_yaml::from_str(fixture!("initial/02_EdgionPlugins.yaml")).unwrap();
        let errors = plugins.validate_plugins();
        assert!(errors.is_empty(), "invalid replacement consumer: {errors:?}");
        let plugin: Value = serde_yaml::from_str(fixture!("initial/02_EdgionPlugins.yaml")).unwrap();
        assert_eq!(
            plugin["spec"]["requestPlugins"][0]["config"]["vars"][0]["webhookRef"],
            KEY
        );
        let route: HTTPRoute = serde_yaml::from_str(fixture!("initial/03_HTTPRoute.yaml")).unwrap();
        assert_eq!(route.metadata.name.as_deref(), Some(NAME));
        let secret: Value = serde_yaml::from_str(fixture!("initial/00_Secret.yaml")).unwrap();
        assert_eq!(secret["metadata"]["name"], AUTH);
        assert_eq!(secret["stringData"]["password"], "edgion-test-pwd");
        let redis: Value = serde_yaml::from_str(fixture!("updates/redis.yaml")).unwrap();
        assert_eq!(redis["spec"]["config"]["endpoints"][0], "redis://127.0.0.1:16379");
        assert_eq!(redis["spec"]["config"]["auth"]["secretRef"]["name"], AUTH);
    }
}
