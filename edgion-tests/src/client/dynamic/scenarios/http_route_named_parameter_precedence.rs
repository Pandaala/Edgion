//! Named-parameter precedence and capture names must survive route publication.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const ALIAS_HOST: &str = "named-alias-capacity.example.com";
const ALIAS_OBSERVER: &str = "named-alias-capture-observer";
const ALIAS_SUPPORT: &str = "named-alias-support";

struct NamedParameterSuite(Option<(u16, &'static str)>);

impl TestSuite for NamedParameterSuite {
    fn name(&self) -> &str {
        "HttpRouteNamedParameterPrecedence"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/NamedParameterPrecedence")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let generation = self.0;
        let backend_cases = [
            (
                "static_over_named_parameter",
                "A static Exact path outranks a matching named-parameter Exact path",
                "/named-rank/fixed",
                false,
            ),
            (
                "exact_over_prefix",
                "A named-parameter Exact path outranks a more static matching Prefix path",
                "/named-exact/a/b",
                false,
            ),
            (
                "aggregate_static_constraints",
                "Aggregate static character constraints outrank parameter spelling",
                "/named-aggregate/long/a",
                false,
            ),
            (
                "escaped_colon_literal",
                "An escaped colon literal outranks a named-parameter segment",
                "/named-colon/:literal",
                false,
            ),
            (
                "multi_parameter_overlap",
                "Static segments outrank an overlapping path with more parameters",
                "/named-multi/a/b/c",
                false,
            ),
            (
                "non_path_precedence",
                "Method precedence wins after equal named-path specificity",
                "/named-nonpath/value",
                false,
            ),
            (
                "live_static_constraint_update",
                "A live static-constraint change rebuilds the winning route",
                "/named-live/a/b/c",
                true,
            ),
        ];
        let mut cases: Vec<_> = backend_cases
            .into_iter()
            .map(|(name, description, path, follows_generation)| {
                TestCase::new(name, description, move |ctx: TestContext| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let expected_port = generation.map(|(port, _)| if follows_generation { port } else { 30001 });
                        let result = check_backend(&ctx, path, expected_port).await;
                        match result {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                        }
                    })
                })
                .with_timeout(Duration::from_secs(15))
            })
            .collect();
        cases.push(
            TestCase::new(
                "equal_specificity_capture_generation",
                "Equal path keys use route precedence and expose only current capture names",
                move |ctx: TestContext| {
                    Box::pin(async move {
                        let start = Instant::now();
                        let result = check_capture_generation(&ctx, generation).await;
                        match result {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                        }
                    })
                },
            )
            .with_timeout(Duration::from_secs(15)),
        );
        cases.push(
            TestCase::new(
                "alias_capacity_and_capture",
                "All 255/256 aliases and unrelated routes survive publication with selected capture names",
                move |ctx: TestContext| {
                    Box::pin(async move {
                        let start = Instant::now();
                        match check_alias_capacity(&ctx, generation).await {
                            Ok(()) => TestResult::passed(start.elapsed()),
                            Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                        }
                    })
                },
            )
            .with_timeout(Duration::from_secs(60)),
        );
        cases
    }
}

async fn response_body(ctx: &TestContext, path: &str) -> Result<(u16, String)> {
    let response = ctx
        .http_client
        .get(format!("{}{path}", ctx.http_url()))
        .header("host", "named-parameter-precedence.example.com")
        .header("x-precedence", "one")
        .timeout(Duration::from_secs(3))
        .send()
        .await?;
    let status = response.status().as_u16();
    let body = response.text().await?;
    Ok((status, body))
}

async fn check_backend(ctx: &TestContext, path: &str, expected_port: Option<u16>) -> Result<()> {
    let (status, body) = response_body(ctx, path).await?;
    match expected_port {
        Some(port) => {
            ensure!(status == 200, "{path}: expected success, got HTTP {status}");
            ensure!(
                body.starts_with(&format!("Server: 0.0.0.0:{port}\n")),
                "{path}: expected backend {port}, got {body:?}"
            );
        }
        None => ensure!(status == 404, "{path}: expected no route, got HTTP {status}"),
    }
    Ok(())
}

fn echoed_header<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    body.lines().find_map(|line| {
        let (header, value) = line.trim().split_once(':')?;
        header.eq_ignore_ascii_case(name).then_some(value.trim())
    })
}

async fn alias_response(ctx: &TestContext, path: &str, selector: usize) -> Result<(u16, String)> {
    let response = ctx
        .http_client
        .get(format!("{}{path}", ctx.http_url()))
        .header("host", ALIAS_HOST)
        .header("x-alias", selector.to_string())
        .timeout(Duration::from_secs(3))
        .send()
        .await?;
    Ok((response.status().as_u16(), response.text().await?))
}

async fn check_alias_capacity(ctx: &TestContext, generation: Option<(u16, &'static str)>) -> Result<()> {
    for path in ["/capacity-health", "/prefix/child", "/regex", "/escaped/:literal/tail"] {
        let (status, body) = alias_response(ctx, path, 0).await?;
        if generation.is_some() {
            ensure!(status == 200, "{path}: expected success, got HTTP {status}");
            ensure!(
                body.starts_with("Server: 0.0.0.0:30001\n"),
                "{path}: wrong backend: {body:?}"
            );
        } else {
            ensure!(status == 404, "{path}: expected absence, got HTTP {status}");
        }
    }
    let (status, _) = alias_response(ctx, "/tail", 0).await?;
    ensure!(status == 404, "escaped path lost its static prefix: HTTP {status}");

    // Probe every selector so passing a phase proves the entire alias set is
    // serving, rather than just observing the last resource to reach the Gateway.
    let alias_count = generation.map_or(0, |(port, _)| if port == 30002 { 256 } else { 255 });
    for selector in 0..256 {
        let (status, body) = alias_response(ctx, "/alias/value", selector).await?;
        if selector >= alias_count {
            ensure!(status == 404, "alias {selector}: expected absence, got HTTP {status}");
            continue;
        }
        let port = if selector == 255 { 30002 } else { 30001 };
        ensure!(status == 200, "alias {selector}: expected success, got HTTP {status}");
        ensure!(
            body.starts_with(&format!("Server: 0.0.0.0:{port}\n")),
            "alias {selector}: wrong backend: {body:?}"
        );
        if selector >= 254 {
            let (selected, rejected) = if selector == 254 {
                ("x-alias-p254", "x-alias-p255")
            } else {
                ("x-alias-p255", "x-alias-p254")
            };
            ensure!(
                echoed_header(&body, selected) == Some("value"),
                "alias {selector}: missing selected capture: {body:?}"
            );
            let unresolved = if selector == 254 { "$p255" } else { "$p254" };
            ensure!(
                echoed_header(&body, rejected) == Some(unresolved),
                "alias {selector}: another candidate's capture leaked: {body:?}"
            );
        }
    }
    Ok(())
}

fn alias_route(name: &str, rules: Value) -> Value {
    json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"name": name, "namespace": "edgion-test"},
        "spec": {
            "parentRefs": [{"name": "named-parameter-precedence-gateway"}],
            "hostnames": [ALIAS_HOST], "rules": rules
        }
    })
}

fn alias_name(index: usize) -> String {
    format!("named-capacity-alias-{index:03}")
}

fn alias_document(index: usize) -> Value {
    let port = if index == 255 { 30002 } else { 30001 };
    alias_route(
        &alias_name(index),
        json!([{
            "matches": [{
                "path": {"type": "Exact", "value": format!("/alias/:p{index:03}")},
                "method": "GET",
                "headers": [{"type": "Exact", "name": "x-alias", "value": index.to_string()}]
            }],
            "filters": [{"type": "ExtensionRef", "extensionRef": {
                "group": "edgion.io", "kind": "EdgionPlugins", "name": ALIAS_OBSERVER
            }}],
            "backendRefs": [{"name": format!("named-precedence-{port}"), "port": port}]
        }]),
    )
}

async fn apply_alias_documents(ctx: &ScenarioCtx, documents: Vec<Value>) -> Result<()> {
    let yaml = documents
        .iter()
        .map(yaml_serde::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .join("\n---\n");
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    crate::dynamic::apply::apply_yaml_documents(&client, &ctx.resolve("generated-alias-capacity.yaml"), &yaml).await
}

async fn apply_initial_aliases(ctx: &ScenarioCtx) -> Result<()> {
    let observer = json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"name": ALIAS_OBSERVER, "namespace": "edgion-test"},
        "spec": {"requestPlugins": [{"enable": true, "type": "ProxyRewrite", "config": {
            "headers": {"set": [
                {"name": "X-Alias-P254", "value": "$p254"},
                {"name": "X-Alias-P255", "value": "$p255"}
            ]}
        }}]}
    });
    let rules = [
        ("Exact", "/capacity-health"),
        ("PathPrefix", "/prefix"),
        ("RegularExpression", "^/regex$"),
        ("Exact", "/escaped/::literal/tail"),
    ]
    .into_iter()
    .map(|(kind, path)| {
        json!({
            "matches": [{"path": {"type": kind, "value": path}}],
            "backendRefs": [{"name": "named-precedence-30001", "port": 30001}]
        })
    })
    .collect::<Vec<_>>();
    let mut documents = vec![observer, alias_route(ALIAS_SUPPORT, json!(rules))];
    // Sequential creation plus lexical names place lower selectors ahead of the
    // selected p254/p255 candidates; their mismatching headers must be rejected.
    documents.extend((0..255).map(alias_document));
    apply_alias_documents(ctx, documents).await
}

async fn check_capture_generation(ctx: &TestContext, generation: Option<(u16, &'static str)>) -> Result<()> {
    let path = "/named-capture/value";
    let Some((port, current_name)) = generation else {
        return check_backend(ctx, path, None).await;
    };
    let (status, body) = response_body(ctx, path).await?;
    ensure!(status == 200, "{path}: expected success, got HTTP {status}");
    ensure!(
        body.starts_with(&format!("Server: 0.0.0.0:{port}\n")),
        "{path}: expected backend {port}, got {body:?}"
    );

    let (current_header, stale_header) = if current_name == "short" {
        ("x-capture-short", "x-capture-customer-identifier")
    } else {
        ("x-capture-customer-identifier", "x-capture-short")
    };
    ensure!(
        echoed_header(&body, current_header) == Some("value"),
        "{path}: current capture {current_name:?} was not resolved: {body:?}"
    );
    ensure!(
        echoed_header(&body, stale_header).is_some_and(|value| value.starts_with('$')),
        "{path}: stale capture unexpectedly remained available: {body:?}"
    );
    Ok(())
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(NamedParameterSuite(None))
}

fn initial() -> Box<dyn TestSuite> {
    Box::new(NamedParameterSuite(Some((30001, "short"))))
}

fn updated() -> Box<dyn TestSuite> {
    Box::new(NamedParameterSuite(Some((30002, "customerIdentifier"))))
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
        name: "updated",
        factory: updated,
    },
];

async fn delete_resources(ctx: &ScenarioCtx, resources: &[(&str, &str)]) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let k8s_mode = std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true");
    let mut failures = Vec::new();
    for &(kind, name) in resources {
        if k8s_mode && kind == "EndpointSlice" {
            continue;
        }
        match client.delete(kind, Some("edgion-test"), name).await {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
            Ok(response) => failures.push(format!("{kind}/{name}: HTTP {}", response.status())),
            Err(error) => failures.push(format!("{kind}/{name}: {error}")),
        }
    }
    ensure!(failures.is_empty(), "cleanup failed: {}", failures.join("; "));
    Ok(())
}

async fn cleanup_routes(ctx: &ScenarioCtx) -> Result<()> {
    let names: Vec<_> = (0..256).map(alias_name).collect();
    let mut resources = vec![
        ("HTTPRoute", "named-static-z-winner"),
        ("HTTPRoute", "named-static-a-loser"),
        ("HTTPRoute", "named-equal-a-winner"),
        ("HTTPRoute", "named-equal-z-loser"),
        ("HTTPRoute", "named-exact-z-winner"),
        ("HTTPRoute", "named-prefix-a-loser"),
        ("HTTPRoute", "named-aggregate-z-winner"),
        ("HTTPRoute", "named-aggregate-a-loser"),
        ("HTTPRoute", "named-colon-z-winner"),
        ("HTTPRoute", "named-colon-a-loser"),
        ("HTTPRoute", "named-multi-z-winner"),
        ("HTTPRoute", "named-multi-a-loser"),
        ("HTTPRoute", "named-nonpath-z-winner"),
        ("HTTPRoute", "named-nonpath-a-loser"),
        ("HTTPRoute", "named-live-z-winner"),
        ("HTTPRoute", "named-live-a-loser"),
        ("HTTPRoute", ALIAS_SUPPORT),
    ];
    resources.extend(names.iter().map(|name| ("HTTPRoute", name.as_str())));
    delete_resources(ctx, &resources).await
}

async fn cleanup_supporting_resources(ctx: &ScenarioCtx) -> Result<()> {
    let mut resources = vec![
        ("HTTPRoute", "named-sentinel"),
        ("EdgionPlugins", "named-capture-observer"),
        ("EdgionPlugins", ALIAS_OBSERVER),
        ("Service", "named-precedence-30001"),
        ("Service", "named-precedence-30002"),
        ("Service", "named-precedence-30003"),
    ];
    if std::env::var("EDGION_TEST_K8S_MODE").as_deref() != Ok("true") {
        resources.extend([
            ("EndpointSlice", "named-precedence-30001"),
            ("EndpointSlice", "named-precedence-30002"),
            ("EndpointSlice", "named-precedence-30003"),
        ]);
    }
    delete_resources(ctx, &resources).await
}

async fn cleanup_gateway(ctx: &ScenarioCtx) -> Result<()> {
    if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
        return Ok(());
    }
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let response = client
        .delete("Gateway", Some("edgion-test"), "named-parameter-precedence-gateway")
        .await?;
    ensure!(
        response.status().is_success() || response.status().as_u16() == 404,
        "gateway cleanup failed: {}",
        response.status()
    );
    Ok(())
}

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup_routes(&ctx).await?;
            if std::env::var("EDGION_TEST_K8S_MODE").as_deref() != Ok("true") {
                ctx.apply_dir("bootstrap/").await?;
            }
            ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(60)).await?;
            ctx.apply_dir("initial/").await?;
            apply_initial_aliases(&ctx).await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(60)).await?;
            let updated_path = if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
                "updates/updated/"
            } else {
                "updated/"
            };
            ctx.apply_dir(updated_path).await?;
            apply_alias_documents(&ctx, vec![alias_document(255)]).await?;
            ctx.wait_phase(SCENARIO, "updated", Duration::from_secs(60)).await?;
            ctx.apply_dir("initial/").await?;
            delete_resources(&ctx, &[("HTTPRoute", &alias_name(255))]).await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(60)).await
        }
        .await;
        let cleanup = async {
            let mut failures = Vec::new();
            match cleanup_routes(&ctx).await {
                Ok(()) => {
                    if let Err(error) = ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(60)).await {
                        failures.push(format!("route absence: {error:#}"));
                    }
                }
                Err(error) => failures.push(format!("route cleanup: {error:#}")),
            }
            if let Err(error) = cleanup_supporting_resources(&ctx).await {
                failures.push(format!("supporting cleanup: {error:#}"));
            }
            if let Err(error) = cleanup_gateway(&ctx).await {
                failures.push(format!("gateway cleanup: {error:#}"));
            }
            ensure!(failures.is_empty(), "{}", failures.join("; "));
            Ok(())
        }
        .await;
        match (result, cleanup) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "HttpRouteNamedParameterPrecedence",
    conf_path: "HTTPRoute/NamedParameterPrecedence/DynamicTest",
    run,
    phases: PHASES,
};
