//! Real ReferenceGrant denial through both named-jump publishers.
//!
//! Registered alongside the existing DIU suite in local mode only. K8s dispatch
//! excludes this suite before any local fixture can be applied.
//! Apply the dedicated RefGrant
//! fixtures here too, so a narrow DIU run has the same prerequisites as a full
//! run. These are static resources; no existing test resource is changed.
//! Selector-only invalid Internal jumps remain unit-tested: both production
//! publishers validate before publishing, so their rejection is a plugin 403.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::time::{Duration, Instant};

const HOST: &str = "named-jump-denied.example.com";
const ROUTE: &str = "named-jump-denied";
const DENIED: &str = "named-jump-denied";
const FIXTURES: &[&str] = &[
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/integration/conf/EdgionPlugins/DynamicInternalUpstream/01_Service_default_diu-backend-stable.yaml")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/integration/conf/EdgionPlugins/DynamicInternalUpstream/02_EndpointSlice_default_diu-backend-stable-slice.yaml")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/integration/conf/Status/RefGrant/Service_named-jump-target_named-jump-denied.yaml")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/integration/conf/Status/RefGrant/EndpointSlice_named-jump-target_named-jump-denied.yaml")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/integration/conf/EdgionPlugins/DynamicInternalUpstream/03_EdgionPlugins_default_named-jump-diu-reject.yaml")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/integration/conf/EdgionPlugins/DynamicInternalUpstream/03_EdgionPlugins_default_named-jump-diu-fallback.yaml")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/integration/conf/EdgionPlugins/DynamicInternalUpstream/03_EdgionPlugins_default_named-jump-canary-reject.yaml")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/integration/conf/EdgionPlugins/DynamicInternalUpstream/03_EdgionPlugins_default_named-jump-canary-fallback.yaml")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/integration/conf/EdgionPlugins/DynamicInternalUpstream/04_HTTPRoute_default_named-jump-denied.yaml")),
];

pub struct DeniedNamedJumpTestSuite;

impl TestSuite for DeniedNamedJumpTestSuite {
    fn name(&self) -> &str {
        "Denied Named Jumps (DIU and Canary)"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionPlugins")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "denied_named_jump_policies_and_weighted_denial",
            "ReferenceGrant denial, plugin policies, Dye, upstream receipts and typed access logs",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match run(&ctx).await {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(120))]
    }
}

async fn run(ctx: &TestContext) -> Result<()> {
    for yaml in FIXTURES {
        ctx.apply_yaml(yaml).await.map_err(anyhow::Error::msg)?;
    }

    // Assert the real Controller decision, not a hand-written refDenied field.
    let route = helpers::wait_resource_status(
        ctx,
        "HTTPRoute",
        "default",
        ROUTE,
        Duration::from_secs(15),
        |resource| {
            helpers::has_condition(resource, "Accepted", "True")
                && resource["status"]["parents"].as_array().is_some_and(|parents| {
                    parents.iter().any(|parent| {
                        parent["conditions"].as_array().is_some_and(|conditions| {
                            conditions.iter().any(|condition| {
                                condition["type"] == "ResolvedRefs"
                                    && condition["status"] == "False"
                                    && condition["reason"] == "RefNotPermitted"
                            })
                        })
                    })
                })
        },
    )
    .await
    .map_err(anyhow::Error::msg)?;
    let rules = route["spec"]["rules"]
        .as_array()
        .context("Controller route rules missing")?;
    ensure!(rules.len() == 6, "Expected all six route rules");
    for rule in rules {
        let refs = rule["backendRefs"].as_array().context("backendRefs missing")?;
        ensure!(refs.len() == 2, "Each rule must retain both siblings");
        ensure!(refs[0]["refDenied"].is_null(), "Same-namespace sibling must be legal");
        ensure!(
            refs[1]["refDenied"]["targetNamespace"] == "named-jump-target"
                && refs[1]["refDenied"]["targetName"] == DENIED,
            "Controller must deny the existing cross-namespace Service: {refs:?}"
        );
    }

    // Wait for the exact processed specs, including refDenied, at the Gateway.
    // This is state polling, independent of the behavior assertions below.
    for yaml in FIXTURES {
        let fixture: Value = serde_yaml::from_str(yaml)?;
        let kind = fixture["kind"].as_str().context("fixture kind")?;
        let ns = fixture["metadata"]["namespace"].as_str().context("fixture namespace")?;
        let name = fixture["metadata"]["name"].as_str().context("fixture name")?;
        let processed = helpers::wait_resource_status(ctx, kind, ns, name, Duration::from_secs(15), |resource| {
            resource["metadata"]["name"] == name
                && (kind != "EdgionPlugins" || helpers::has_condition(resource, "Accepted", "True"))
        })
        .await
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("Controller did not publish {kind}/{ns}/{name}"))?;
        wait_gateway_resource(ctx, kind, ns, name, &processed).await?;
    }

    // Positive controls prove both real backend listeners and the receipt store
    // work before any zero-receipt assertion. Unique IDs avoid stale evidence.
    for port in [30001, 30002] {
        let trace = format!("named-jump-control-{}", uuid::Uuid::new_v4());
        let response = ctx
            .http_client
            .get(format!("http://{}:{port}/mirror/capture", ctx.target_host))
            .header("x-trace-id", crate::access_log_client::trace_id_for_marker(&trace))
            .send()
            .await?;
        ensure!(response.status() == 200, "Backend {port} control failed");
        ensure!(response.text().await? == "mirror-captured", "Unexpected control body");
        ensure!(
            receipts(ctx, &trace).await?["count"] == 1,
            "Receipt store control failed"
        );
    }

    check_case(ctx, "legal-control", DENIED, 200, None, None).await?;
    for (case, dye) in [
        ("diu-reject", "dynamic_internal_upstream:reject:on-invalid"),
        ("diu-fallback", "dynamic_internal_upstream:fallback:on-invalid"),
        ("canary-reject", "canary:reject:on-invalid"),
        ("canary-fallback", "canary:fallback:on-invalid"),
    ] {
        let status = if case.ends_with("reject") { 403 } else { 200 };
        check_case(ctx, case, DENIED, status, Some(dye), Some("denied"))
            .await
            .with_context(|| format!("Case {case}"))?;
    }
    // This is the plugin's missing-target fallback, not the selector's
    // BackendNotFound arm (which cannot be reached from these publishers).
    check_case(
        ctx,
        "diu-fallback",
        "named-jump-absent",
        200,
        Some("dynamic_internal_upstream:fallback:on-invalid"),
        Some("missing"),
    )
    .await
    .context("Missing target fallback")?;
    // No plugin: force weighted selection onto the denied slot. The legal
    // sibling remains present with weight zero, so a silent fallback is visible.
    check_case(ctx, "weighted-denied", DENIED, 500, None, None).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::FIXTURES;
    use edgion_gateway::plugins::http::compile_plugins;
    use edgion_resources::resources::EdgionPlugins;
    use serde_json::Value;

    #[test]
    fn named_jump_fixtures_validate_and_compile_dye_plans() {
        let mut checked = 0;
        for yaml in FIXTURES {
            let fixture: Value = serde_yaml::from_str(yaml).expect("valid fixture YAML");
            if fixture["kind"] != "EdgionPlugins" {
                continue;
            }
            let plugin: EdgionPlugins = serde_yaml::from_str(yaml).expect("valid plugin resource");
            let name = plugin.metadata.name.as_deref().expect("fixture name");
            let errors = plugin.validate_plugins();
            assert!(errors.is_empty(), "{name}: resource validation failed: {errors:?}");
            let compiled = compile_plugins(&plugin);
            assert!(
                compiled.preparse_errors.is_empty(),
                "{name}: Gateway compilation failed: {:?}",
                compiled.preparse_errors
            );
            assert!(compiled.is_activatable, "{name}: plugin must activate");
            assert_eq!(compiled.plugin_runtime.request_plugins_count(), 1, "{name}");
            assert!(
                compiled
                    .plugin_runtime
                    .request_plugins_iter()
                    .all(|entry| entry.dye_plan().is_some()),
                "{name}: compiled request entry must retain its dye plan"
            );
            checked += 1;
        }
        assert_eq!(checked, 4, "validate every named-jump plugin fixture");
    }
}

async fn wait_gateway_resource(ctx: &TestContext, kind: &str, ns: &str, name: &str, expected: &Value) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    loop {
        interval.tick().await;
        let mut request = ctx
            .http_client
            .get(format!("http://{}:12001/configclient/{kind}", ctx.target_host))
            .query(&[("namespace", ns), ("name", name)])
            .timeout(Duration::from_secs(3));
        if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?.error_for_status()?.json::<Value>().await?;
        // EndpointSlice has no spec, so compare its endpoint and port payloads.
        let data = &response["data"];
        if response["success"] == true
            && data["metadata"]["name"] == name
            && data["spec"] == expected["spec"]
            && data["endpoints"] == expected["endpoints"]
            && data["ports"] == expected["ports"]
        {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "Gateway did not sync {kind}/{ns}/{name}: {response}"
        );
    }
}

async fn receipts(ctx: &TestContext, trace: &str) -> Result<Value> {
    Ok(ctx
        .http_client
        .get(format!(
            "http://{}:30001/mirror/query/{}",
            ctx.target_host,
            crate::access_log_client::trace_id_for_marker(trace)
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn check_case(
    ctx: &TestContext,
    case: &str,
    target: &str,
    status: u16,
    outcome: Option<&str>,
    invalid_reason: Option<&str>,
) -> Result<()> {
    let trace = format!("named-jump-{case}-{}", uuid::Uuid::new_v4());
    let mut request = ctx
        .http_client
        .get(format!("{}/mirror/capture", ctx.http_url()))
        .header("host", HOST)
        .header("x-named-jump-case", case)
        .header("x-backend-target", target)
        .header("traceparent", crate::access_log_client::traceparent_for_marker(&trace))
        .header("access_log", "test_store");
    if outcome.is_some() {
        request = request.header("x-named-jump-success", "spoofed");
    }
    let response = request.send().await?;
    ensure!(
        response.status().as_u16() == status,
        "{case}: expected {status}, got {}",
        response.status()
    );
    ensure!(
        response
            .headers()
            .get("x-named-jump-outcome")
            .and_then(|h| h.to_str().ok())
            == outcome,
        "{case}: wrong Dye outcome: {:?}",
        response.headers()
    );
    ensure!(
        !response.headers().contains_key("x-named-jump-success"),
        "{case}: success Dye leaked"
    );
    let body = response.text().await?;
    if status == 403 {
        ensure!(
            body == "Target backend not allowed",
            "{case}: changed rejection body: {body}"
        );
    } else if status == 200 {
        ensure!(body == "mirror-captured", "{case}: unexpected backend body: {body}");
    }

    let log = ctx
        .access_log_client()
        .get_access_log_with_retry(&trace, 30, 100)
        .await?;
    ensure!(
        log.status_code() == Some(u64::from(status)),
        "{case}: wrong logged status"
    );
    ensure!(
        log.route_name() == Some(ROUTE) && log.route_namespace() == Some("default"),
        "{case}: wrong route"
    );
    ensure!(
        log.has_diagnostic("error", "RefDenied") == (case == "weighted-denied"),
        "{case}: unexpected RefDenied diagnostic: {:?}",
        log.data
    );
    if case.starts_with("diu-") {
        let expected = match invalid_reason {
            Some("denied") => "backend ref denied",
            Some("missing") => "backend ref not found",
            _ => anyhow::bail!("DIU assertion needs an invalid reason"),
        };
        let logged = serde_json::to_string(&log.data["stage_logs"])?;
        ensure!(
            logged.contains(expected),
            "{case}: missing plugin diagnostic {expected}: {logged}"
        );
    }
    let captured = receipts(ctx, &trace).await?;
    if status == 200 {
        ensure!(
            log.backend_name() == Some("diu-backend-stable"),
            "{case}: selected wrong sibling"
        );
        ensure!(
            log.backend_namespace() == Some("default"),
            "{case}: selected wrong namespace"
        );
        ensure!(log.upstream_count() == 1, "{case}: expected one upstream attempt");
        ensure!(
            captured["count"] == 1,
            "{case}: expected exactly one backend receipt: {captured}"
        );
        let headers = captured["captures"][0]["headers"]
            .as_object()
            .context("capture headers missing")?;
        ensure!(
            !headers.contains_key("x-named-jump-success"),
            "{case}: successful/spoofed Dye reached backend"
        );
    } else {
        ensure!(log.upstream_count() == 0, "{case}: rejection attempted an upstream");
        ensure!(log.backend_name().is_none(), "{case}: rejection selected a backend");
        // The terminal access log above is the completion barrier. These are
        // ordinary proxied requests, with no asynchronous mirror plugin.
        ensure!(
            captured["count"] == 0,
            "{case}: rejected request reached a backend: {captured}"
        );
    }
    Ok(())
}
