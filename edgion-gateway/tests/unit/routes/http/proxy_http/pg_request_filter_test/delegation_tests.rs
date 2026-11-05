//! Delegated chains: every level's filters and plugins run, root first, and
//! each level resolves its ExtensionRefs and templates against its own route.

use super::*;
use crate::plugins::http::compile_plugins;
use crate::plugins::runtime::traits::session::{PluginSession, RouteProto};
use crate::plugins::runtime::PingoraSessionAdapter;
use crate::routes::http::routes_mgr::{DomainRouteRules, GlobalHttpRouteManagers};
use crate::runtime::gateway_info::GatewayInfo;
use crate::runtime::store::listener_gateway_info::{GatewayInfoIndex, ListenerEntry};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::route_delegation::{
    key_for, DelegationKey, DelegationTarget, DelegationTargetName, RouteKindName,
};
use edgion_resources::{EdgionPlugins, HTTPRoute, ListenerKey, ResourceMeta};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::io::AsyncWriteExt;

fn listener(port: u16) -> ListenerKey {
    ListenerKey::new(format!("0.0.0.0:{port}").parse().unwrap(), "HTTP")
}

fn key(parent_ns: &str, parent: &str, child_ns: &str, child: &str) -> DelegationKey {
    key_for(
        RouteKindName::HTTPRoute,
        parent_ns,
        parent,
        &DelegationTarget {
            ns: child_ns.into(),
            name: DelegationTargetName::Named(child.into()),
        },
    )
}

/// One level of a chain: `(namespace, name, path prefix, extra rule fields)`.
struct Level<'a> {
    ns: &'a str,
    name: &'a str,
    prefix: &'a str,
    rule: Value,
}

/// Compile `levels` (root first) as a delegation chain on `port` and return
/// the routes; every non-leaf level delegates to the next one.
fn chain_routes(port: u16, levels: &[Level<'_>]) -> Vec<Arc<HTTPRoute>> {
    let mut out = Vec::new();
    for (i, level) in levels.iter().enumerate() {
        let mut rule = level.rule.clone();
        rule["matches"] = json!([{"path": {"type": "PathPrefix", "value": level.prefix}}]);
        rule["backendRefs"] = match levels.get(i + 1) {
            Some(next) => json!([{
                "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                "namespace": next.ns, "name": next.name
            }]),
            None => json!([{"name": "svc", "port": 80}]),
        };
        let mut parent_refs = Vec::new();
        if i == 0 {
            parent_refs.push(json!({
                "group": "gateway.networking.k8s.io", "kind": "Gateway", "namespace": "default", "name": "gw1"
            }));
        } else {
            let parent = &levels[i - 1];
            parent_refs.push(json!({
                "group": "gateway.networking.k8s.io", "kind": "HTTPRoute",
                "namespace": parent.ns, "name": parent.name
            }));
        }
        let mut route: HTTPRoute = crate::test_current_status::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
            "metadata": {"namespace": level.ns, "name": level.name, "creationTimestamp": "2026-09-20T00:00:00Z",
                "labels": {"level": level.name}},
            "spec": {
                "resolvedStatusController": "edgion.io/gateway-controller",
                "parentRefs": parent_refs,
                "rules": [rule],
            }
        }))
        .unwrap();
        if i == 0 {
            route.spec.resolved_listeners = Some(vec![listener(port)]);
        } else {
            let parent = &levels[i - 1];
            route.spec.resolved_delegation_keys = vec![key(parent.ns, parent.name, level.ns, level.name)];
        }
        route.pre_parse();
        out.push(Arc::new(route));
    }
    out
}

fn index() -> GatewayInfoIndex {
    GatewayInfoIndex::from_entries_for_test(vec![(
        "default/gw1".to_string(),
        ListenerEntry {
            port: 8080,
            info: Arc::new(GatewayInfo::new(
                Some("default".into()),
                "gw1".into(),
                Some("web".into()),
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
            )),
            gateway_proof: None,
            hostname: None,
            allowed_routes: None,
        },
    )])
}

/// Match `path` against the chain and install the result into a fresh context
/// exactly as `request_filter` does. Returns the live session too.
async fn matched(
    port: u16,
    routes: &[Arc<HTTPRoute>],
    path: &str,
) -> (Session, tokio::io::DuplexStream, EdgionHttpContext) {
    let mgr = GlobalHttpRouteManagers::new();
    let data: HashMap<_, _> = routes.iter().map(|r| (r.key_name(), Arc::clone(r))).collect();
    mgr.full_set(&data);
    let table: Arc<DomainRouteRules> =
        Arc::clone(&mgr.get_or_create_listener_manager(&listener(port)).load_route_table());
    let (mut client, server) = tokio::io::duplex(16 * 1024);
    let mut session = Session::new_h1(Box::new(server));
    client
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: api.example.com\r\n\r\n").as_bytes())
        .await
        .unwrap();
    assert!(session.read_request().await.unwrap());
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.hostname = NormalizedHostname::new("api.example.com");
    ctx.request_info.entry_raw_path = path.to_string();
    ctx.request_info.normalized_current_path = path.to_string();
    let result = table
        .match_route(&mut session, &ctx, &index())
        .expect("chain must match");
    install_match_result(&mut ctx, result);
    (session, client, ctx)
}

/// An EdgionPlugins runtime that appends `value` to `x-order` on the request
/// and to `x-resp-order` on the (sync) upstream response.
fn order_plugins(ns: &str, value: &str) -> Arc<crate::plugins::runtime::PluginRuntime> {
    plugins(
        ns,
        "order",
        json!({
            "requestPlugins": [{"type": "RequestHeaderModifier",
                "config": {"add": [{"name": "x-order", "value": value}]}}],
            "upstreamResponseFilterPlugins": [{"type": "ResponseHeaderModifier",
                "config": {"add": [{"name": "x-resp-order", "value": value}]}}],
        }),
    )
}

/// Resolve the leaf's ExtensionRefs (ordinary path) and every ancestor's
/// (delegated path) from `registry`, keyed `namespace/name`.
fn install_all_ext_refs(
    ctx: &mut EdgionHttpContext,
    registry: &HashMap<String, Arc<crate::plugins::runtime::PluginRuntime>>,
) {
    let keys = ctx.route_unit.as_ref().unwrap().plugin_runtime.ext_ref_keys().to_vec();
    ctx.install_resolved_route_ext_refs(
        keys.iter()
            .map(|key| key.as_deref().and_then(|key| registry.get(key).cloned()))
            .collect(),
    );
    install_delegated_ext_refs(ctx, |key| registry.get(key).cloned());
}

fn values(headers: &http::HeaderMap, name: &str) -> Vec<String> {
    headers
        .get_all(name)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

fn ext_ref(name: &str) -> Value {
    json!({"filters": [{"type": "ExtensionRef",
        "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": name}}]})
}

fn plugins(ns: &str, name: &str, spec: Value) -> Arc<crate::plugins::runtime::PluginRuntime> {
    let ep: EdgionPlugins =
        serde_json::from_value(json!({"metadata": {"name": name, "namespace": ns}, "spec": spec})).unwrap();
    Arc::clone(&compile_plugins(&ep).plugin_runtime)
}

fn mock(status: u16) -> Value {
    json!({"requestPlugins": [{"type": "Mock", "config": {"statusCode": status, "body": "denied"}}]})
}

#[tokio::test]
async fn stage_order_root_then_leaf_for_request_and_response() {
    let routes = chain_routes(
        18761,
        &[
            Level {
                ns: "default",
                name: "so-root",
                prefix: "/api",
                rule: ext_ref("order"),
            },
            Level {
                ns: "ns-mid",
                name: "so-mid",
                prefix: "/api/v1",
                rule: ext_ref("order"),
            },
            Level {
                ns: "ns-leaf",
                name: "so-leaf",
                prefix: "/api/v1/x",
                rule: ext_ref("order"),
            },
        ],
    );
    let (mut session, _client, mut ctx) = matched(18761, &routes, "/api/v1/x/1").await;
    assert_eq!(ctx.delegated.as_ref().map(|d| d.ancestors.len()), Some(2));
    let registry: HashMap<_, _> = [("default", "root"), ("ns-mid", "mid"), ("ns-leaf", "leaf")]
        .into_iter()
        .map(|(ns, value)| (format!("{ns}/order"), order_plugins(ns, value)))
        .collect();
    install_all_ext_refs(&mut ctx, &registry);

    run_route_request_plugins(&mut session, &mut ctx).await;
    assert!(!ctx.plugin_terminated());
    assert_eq!(
        values(&session.req_header().headers, "x-order"),
        ["root", "mid", "leaf"],
        "request stage runs root -> mid -> leaf"
    );

    let mut response = pingora_http::ResponseHeader::build(200, None).unwrap();
    super::super::pg_upstream_response_filter::run_route_upstream_response_sync(&mut session, &mut ctx, &mut response);
    assert_eq!(
        values(&response.headers, "x-resp-order"),
        ["root", "mid", "leaf"],
        "response stage is also parent first"
    );
}

#[tokio::test]
async fn parent_auth_plugin_blocks_request_even_when_leaf_has_no_plugins() {
    let routes = chain_routes(
        18762,
        &[
            Level {
                ns: "default",
                name: "auth-root",
                prefix: "/api",
                rule: ext_ref("guard"),
            },
            Level {
                ns: "default",
                name: "auth-leaf",
                prefix: "/api/private",
                rule: json!({}),
            },
        ],
    );
    let (mut session, _client, mut ctx) = matched(18762, &routes, "/api/private/data").await;
    assert_eq!(
        ctx.route_unit.as_ref().unwrap().plugin_runtime.request_plugins_count(),
        0
    );
    let guard = plugins("default", "guard", mock(401));
    install_delegated_ext_refs(&mut ctx, |key| (key == "default/guard").then(|| Arc::clone(&guard)));

    run_route_request_plugins(&mut session, &mut ctx).await;

    assert!(ctx.plugin_terminated(), "the parent's denial must stop the request");
    assert_eq!(session.response_written().unwrap().status.as_u16(), 401);
}

#[tokio::test]
async fn ancestor_extension_ref_resolves_in_ancestor_namespace() {
    let routes = chain_routes(
        18763,
        &[
            Level {
                ns: "default",
                name: "ns-root",
                prefix: "/api",
                rule: ext_ref("guard"),
            },
            Level {
                ns: "child-ns",
                name: "ns-leaf",
                prefix: "/api/t",
                rule: ext_ref("guard"),
            },
        ],
    );
    let (mut session, _client, mut ctx) = matched(18763, &routes, "/api/t/1").await;
    // The leaf's array is installed by the ordinary path, from the leaf's keys.
    let leaf_guard = plugins(
        "child-ns",
        "guard",
        json!({"requestPlugins": [{"type": "RequestHeaderModifier", "config": {"add": [{"name": "x-guard", "value": "child"}]}}]}),
    );
    let leaf_keys = ctx.route_unit.as_ref().unwrap().plugin_runtime.ext_ref_keys().to_vec();
    assert_eq!(leaf_keys, [Some("child-ns/guard".to_string())]);
    ctx.install_resolved_route_ext_refs(vec![Some(Arc::clone(&leaf_guard))]);

    let parent_guard = plugins(
        "default",
        "guard",
        json!({"requestPlugins": [{"type": "RequestHeaderModifier", "config": {"add": [{"name": "x-guard", "value": "parent"}]}}]}),
    );
    let mut asked = Vec::new();
    install_delegated_ext_refs(&mut ctx, |key| {
        asked.push(key.to_string());
        (key == "default/guard").then(|| Arc::clone(&parent_guard))
    });
    assert_eq!(asked, ["default/guard"], "the ancestor resolves in its own namespace");

    run_route_request_plugins(&mut session, &mut ctx).await;
    assert!(!ctx.plugin_terminated());
    assert_eq!(values(&session.req_header().headers, "x-guard"), ["parent", "child"]);
}

#[tokio::test]
async fn matched_route_template_sees_own_level() {
    let routes = chain_routes(
        18764,
        &[
            Level {
                ns: "default",
                name: "tpl-root",
                prefix: "/api",
                rule: json!({}),
            },
            Level {
                ns: "child-ns",
                name: "tpl-leaf",
                prefix: "/api/t",
                rule: json!({}),
            },
        ],
    );
    let (mut session, _client, mut ctx) = matched(18764, &routes, "/api/t/1").await;
    assert_eq!(ctx.ancestor_count(), 1);
    {
        let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx).with_level(Some(0));
        assert_eq!(
            adapter.matched_route_meta(RouteProto::Http, "name").as_deref(),
            Some("tpl-root")
        );
        assert_eq!(
            adapter.matched_route_meta(RouteProto::Http, "namespace").as_deref(),
            Some("default")
        );
        assert_eq!(
            adapter.matched_route_meta(RouteProto::Http, "label.level").as_deref(),
            Some("tpl-root")
        );
    }
    let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert_eq!(
        adapter.matched_route_meta(RouteProto::Http, "name").as_deref(),
        Some("tpl-leaf"),
        "the leaf level (and every ordinary request) sees the leaf route"
    );
}

fn ai_guard() -> Arc<crate::plugins::runtime::PluginRuntime> {
    plugins(
        "default",
        "guard",
        json!({"requestPlugins": [{"type": "AiGuard", "config": {"rules": [{
            "name": "tripwire", "match": {"type": "regex", "pattern": "private"}, "action": "reject"
        }]}}]}),
    )
}

fn graphql_router() -> Arc<crate::plugins::runtime::PluginRuntime> {
    plugins(
        "default",
        "router",
        json!({"requestPlugins": [{"type": "GraphqlOperationRouter", "config": {"rules": [
            {"operationType": "query", "operationName": "GetUser", "backendRef": {"name": "users"}}
        ]}}]}),
    )
}

type Refs = Vec<Option<Arc<crate::plugins::runtime::PluginRuntime>>>;

fn level_refs(refs: Refs) -> Option<crate::routes::delegation::descent::LevelExtRefs> {
    Some(Arc::from(refs))
}

#[test]
fn router_and_ai_across_chain_rejected_500() {
    let (router, guard) = (graphql_router(), ai_guard());
    assert!(router.has_graphql_operation_router());
    assert!(guard.has_ai_conversion_owner());
    // Ordinary route: the same rule applies within one level.
    assert!(route_owner_conflict(
        &[],
        &[],
        None,
        &[Some(router.clone()), Some(guard.clone())],
        false
    ));
    // Delegated: a parent router and a leaf AI owner conflict across levels.
    let ancestors = [level_refs(vec![Some(router.clone())])];
    assert!(route_owner_conflict(&[], &[], Some(&ancestors), &[Some(guard)], false));
    // Defense in depth: routers on two levels are rejected.
    assert!(route_owner_conflict(
        &[],
        &[],
        Some(&ancestors),
        &[Some(router.clone())],
        false
    ));
    // A single router on one level, nothing else, is permitted.
    assert!(!route_owner_conflict(&[], &[], Some(&ancestors), &[], false));
}

#[test]
fn ai_guard_parent_with_ai_proxy_leaf_allowed() {
    let proxy = plugins(
        "default",
        "proxy",
        json!({"requestPlugins": [{"type": "AiProxy", "config": {"api": "openaiChat"}}]}),
    );
    assert!(proxy.has_ai_conversion_owner());
    let ancestors = [level_refs(vec![Some(ai_guard())]), None];
    assert!(
        !route_owner_conflict(&[], &[], Some(&ancestors), &[Some(proxy.clone())], false),
        "AI plugins on several levels are allowed, as on one ordinary rule"
    );
    // The ordinary-route equivalent (AiGuard + AiProxy on one rule) is allowed too.
    assert!(!route_owner_conflict(
        &[],
        &[],
        None,
        &[Some(ai_guard()), Some(proxy)],
        false
    ));
}

/// Records which level's route each body callback saw, in call order.
struct BodyProbe(Arc<parking_lot::Mutex<Vec<String>>>);

impl crate::plugins::runtime::traits::UpstreamResponseBodyFilter for BodyProbe {
    fn name(&self) -> &str {
        "BodyProbe"
    }
    fn run_upstream_response_body_filter(
        &self,
        _body: &Option<bytes::Bytes>,
        _end_of_stream: bool,
        session: &mut dyn PluginSession,
    ) -> Option<std::time::Duration> {
        let name = session.matched_route_meta(RouteProto::Http, "name").unwrap_or_default();
        self.0.lock().push(name);
        None
    }
}

#[tokio::test]
async fn body_stage_runs_ancestors_then_leaf_each_with_its_own_route() {
    let routes = chain_routes(
        18766,
        &[
            Level {
                ns: "default",
                name: "body-root",
                prefix: "/api",
                rule: ext_ref("probe"),
            },
            Level {
                ns: "child-ns",
                name: "body-mid",
                prefix: "/api/v1",
                rule: ext_ref("probe"),
            },
            Level {
                ns: "leaf-ns",
                name: "body-leaf",
                prefix: "/api/v1/x",
                rule: ext_ref("probe"),
            },
        ],
    );
    let (mut session, _client, mut ctx) = matched(18766, &routes, "/api/v1/x/1").await;
    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let probe = |seen: &Arc<parking_lot::Mutex<Vec<String>>>| {
        let mut runtime = crate::plugins::runtime::PluginRuntime::new();
        runtime.add_upstream_response_body_filter(Box::new(BodyProbe(Arc::clone(seen))));
        Arc::new(runtime)
    };
    let registry: HashMap<_, _> = ["default", "child-ns", "leaf-ns"]
        .into_iter()
        .map(|ns| (format!("{ns}/probe"), probe(&seen)))
        .collect();
    install_all_ext_refs(&mut ctx, &registry);

    let chunk = Some(bytes::Bytes::from_static(b"chunk"));
    super::super::pg_upstream_response_body_filter::run_read_only_body_plugins(&mut session, &mut ctx, &chunk, false);

    assert_eq!(
        *seen.lock(),
        ["body-root", "body-mid", "body-leaf"],
        "body callbacks run root -> leaf, each resolving its own level's route"
    );
}

/// A response processor that renders the trusted matched-route name.
struct TemplateProbe(Arc<parking_lot::Mutex<Vec<Option<String>>>>);

#[async_trait::async_trait]
impl crate::plugins::runtime::traits::StreamingResponseProcessor for TemplateProbe {
    async fn on_chunk(
        &mut self,
        _chunk: &mut Option<bytes::Bytes>,
        _end_of_stream: bool,
        _stream_kind: crate::ctx::ResponseStreamKind,
        _sink: &mut pingora_proxy::ResponseBodySink,
        session: &mut dyn crate::plugins::runtime::traits::ResponseStreamSession,
    ) {
        self.0
            .lock()
            .push(session.trusted_matched_route_meta(RouteProto::Http, "name"));
    }
}

#[tokio::test]
async fn parent_installed_stream_processor_renders_parent_route_meta() {
    use crate::link_sys::template::TrustedTemplateVariables;
    let routes = chain_routes(
        18767,
        &[
            Level {
                ns: "default",
                name: "sp-root",
                prefix: "/api",
                rule: json!({}),
            },
            Level {
                ns: "child-ns",
                name: "sp-leaf",
                prefix: "/api/t",
                rule: json!({}),
            },
        ],
    );
    let (mut session, _client, mut ctx) = matched(18767, &routes, "/api/t/1").await;
    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    {
        let mut parent = PingoraSessionAdapter::new(&mut session, &mut ctx).with_level(Some(0));
        PluginSession::install_stream_processor(&mut parent, Box::new(TemplateProbe(Arc::clone(&seen))));
    }
    {
        let mut leaf = PingoraSessionAdapter::new(&mut session, &mut ctx);
        PluginSession::install_stream_processor(&mut leaf, Box::new(TemplateProbe(Arc::clone(&seen))));
    }
    let count = ctx.response_stream_processors_mut().map_or(0, |p| p.len());
    assert_eq!(count, 2);
    for index in 0..count {
        let mut processor = std::mem::replace(
            &mut ctx.response_stream_processors_mut().unwrap()[index],
            Box::new(TemplateProbe(Arc::new(parking_lot::Mutex::new(Vec::new())))),
        );
        let mut sink = pingora_proxy::ResponseBodySink::new();
        let mut adapter = crate::plugins::runtime::response_stream_session::ResponseStreamAdapter::new(&mut ctx);
        let mut chunk = Some(bytes::Bytes::from_static(b"x"));
        processor
            .on_chunk(&mut chunk, false, Default::default(), &mut sink, &mut adapter)
            .await;
        let _ = adapter.trusted_remote_ip();
    }
    assert_eq!(
        *seen.lock(),
        [Some("sp-root".to_string()), Some("sp-leaf".to_string())],
        "the parent-installed processor sees the parent; the leaf's sees the leaf"
    );
}

// ---------------------------------------------------------------------------
// GRPCRoute chains share the same stage runners
// ---------------------------------------------------------------------------

fn grpc_route(
    name: &str,
    gateway_port: Option<u16>,
    parent: Option<&str>,
    rule: Value,
) -> Arc<edgion_resources::GRPCRoute> {
    let mut parent_refs = Vec::new();
    if let Some(parent) = parent {
        parent_refs.push(json!({"group": "gateway.networking.k8s.io", "kind": "GRPCRoute", "name": parent}));
    }
    if gateway_port.is_some() {
        parent_refs.push(json!({"name": "gw1", "namespace": "default", "sectionName": "web"}));
    }
    let mut route: edgion_resources::GRPCRoute = crate::test_current_status::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GRPCRoute",
        "metadata": {"namespace": "default", "name": name, "creationTimestamp": "2026-09-20T00:00:00Z"},
        "spec": {
            "resolvedStatusController": "edgion.io/gateway-controller",
            "parentRefs": parent_refs,
            "rules": [rule],
        }
    }))
    .unwrap();
    if let Some(port) = gateway_port {
        route.spec.resolved_listeners = Some(vec![listener(port)]);
    }
    if let Some(parent) = parent {
        route.spec.resolved_delegation_keys = vec![key_for(
            RouteKindName::GRPCRoute,
            "default",
            parent,
            &DelegationTarget {
                ns: "default".into(),
                name: DelegationTargetName::Named(name.into()),
            },
        )];
    }
    Arc::new(route)
}

#[tokio::test]
async fn grpc_stage_order_root_then_leaf_for_request_and_response() {
    use crate::routes::grpc::GlobalGrpcRouteManagers;
    let ext = |name: &str| {
        json!([{"type": "ExtensionRef",
            "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": name}}])
    };
    let root = grpc_route(
        "gso-root",
        Some(18791),
        None,
        json!({"matches": [{"method": {"service": "svc.Users"}}], "filters": ext("order-root"),
            "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute", "name": "gso-leaf"}]}),
    );
    let leaf = grpc_route(
        "gso-leaf",
        None,
        Some("gso-root"),
        json!({"matches": [{"method": {"method": "Get"}}], "filters": ext("order-leaf"),
            "backendRefs": [{"name": "svc", "port": 80}]}),
    );
    let mgr = GlobalGrpcRouteManagers::new();
    mgr.full_set(&HashMap::from([
        (root.key_name(), Arc::clone(&root)),
        (leaf.key_name(), Arc::clone(&leaf)),
    ]));
    let table = Arc::clone(&mgr.get_or_create_listener_manager(&listener(18791)).load_route_table());

    let (mut client, server) = tokio::io::duplex(16 * 1024);
    let mut session = Session::new_h1(Box::new(server));
    client
        .write_all(b"POST /svc.Users/Get HTTP/1.1\r\nHost: api.example.com\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
    assert!(session.read_request().await.unwrap());
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.hostname = NormalizedHostname::new("api.example.com");
    ctx.request_info.normalized_current_path = "/svc.Users/Get".to_string();
    assert!(
        crate::routes::grpc::try_match_grpc_route(&table, &mut session, &mut ctx, &index())
            .await
            .unwrap()
    );
    assert_eq!(ctx.delegated.as_ref().map(|d| d.grpc_ancestors.len()), Some(1));

    let registry: HashMap<_, _> = [("order-root", "root"), ("order-leaf", "leaf")]
        .into_iter()
        .map(|(name, value)| (format!("default/{name}"), order_plugins("default", value)))
        .collect();
    let keys = ctx
        .grpc_route_unit
        .as_ref()
        .unwrap()
        .plugin_runtime
        .ext_ref_keys()
        .to_vec();
    ctx.install_resolved_route_ext_refs(
        keys.iter()
            .map(|key| key.as_deref().and_then(|key| registry.get(key).cloned()))
            .collect(),
    );
    install_delegated_ext_refs(&mut ctx, |key| registry.get(key).cloned());

    run_route_request_plugins(&mut session, &mut ctx).await;
    assert!(!ctx.plugin_terminated());
    assert_eq!(
        values(&session.req_header().headers, "x-order"),
        ["root", "leaf"],
        "gRPC request stage runs root -> leaf"
    );
    let mut response = pingora_http::ResponseHeader::build(200, None).unwrap();
    super::super::pg_upstream_response_filter::run_route_upstream_response_sync(&mut session, &mut ctx, &mut response);
    assert_eq!(values(&response.headers, "x-resp-order"), ["root", "leaf"]);

    // The access log picks the gRPC chain from the request context.
    let entry = serde_json::to_value(crate::observe::access_log::AccessLogEntry::from_context(&ctx)).unwrap();
    let names: Vec<_> = entry["match_info"]
        .as_array()
        .unwrap()
        .iter()
        .map(|level| level["rsc"]["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["gso-root", "gso-leaf"]);
}

/// An ExtensionRef target whose first request filter is a policy block (451).
fn policy_blocked_runtime() -> Arc<crate::plugins::runtime::PluginRuntime> {
    let mut runtime = crate::plugins::runtime::PluginRuntime::new();
    runtime.record_policy_action(
        "Mock",
        edgion_resources::resources::plugin_policy::PluginPolicyAction::Block { status: 451 },
    );
    runtime.seal_plugin_policy();
    Arc::new(runtime)
}

async fn preflight_status(port: u16, root_rule: Value) -> u16 {
    let routes = chain_routes(
        port,
        &[
            Level {
                ns: "default",
                name: "pf-root",
                prefix: "/api",
                rule: root_rule,
            },
            Level {
                ns: "ns-leaf",
                name: "pf-leaf",
                prefix: "/api/x",
                rule: ext_ref("blocked"),
            },
        ],
    );
    let (mut session, _client, mut ctx) = matched(port, &routes, "/api/x/1").await;
    let registry = HashMap::from([("ns-leaf/blocked".to_string(), policy_blocked_runtime())]);
    install_all_ext_refs(&mut ctx, &registry);
    let route = ctx.resolved_route_ext_refs();
    let ancestors = ctx.delegated.as_ref().unwrap().ext_refs.clone();
    assert!(
        run_preflight_gates(
            &mut session,
            &mut ctx,
            &[],
            &[],
            &ancestors,
            route.as_deref().unwrap_or_default()
        )
        .await,
        "a failing gate terminates the request"
    );
    session.response_written().unwrap().status.as_u16()
}

#[tokio::test]
async fn preflight_ancestor_rule_gate_precedes_leaf_extension_ref_gate() {
    // Control: only the leaf ExtensionRef gate fails.
    assert_eq!(preflight_status(18771, json!({})).await, 451);
    // Both fail: the ancestor's invalid ExtensionRef (rule gate, 500) runs
    // before the leaf's policy-blocked ExtensionRef (451), root to leaf.
    let invalid_ext_ref = json!({"filters": [{"type": "ExtensionRef",
        "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "bad/name"}}]});
    assert_eq!(preflight_status(18772, invalid_ext_ref).await, 500);
}

// ---------------------------------------------------------------------------
// WAF owner election on a matched delegated chain
// ---------------------------------------------------------------------------

/// A route-attachable runtime carrying one WAF with `precedence`.
fn waf_plugins(
    precedence: edgion_resources::resources::edgion_plugins::WafDelegationPrecedence,
) -> Arc<crate::plugins::runtime::PluginRuntime> {
    use edgion_resources::resources::edgion_plugins::{WafConfig, WafMode};
    let mut runtime = crate::plugins::runtime::PluginRuntime::new();
    runtime
        .add_waf(&WafConfig {
            rules: "SecAction \"id:1,phase:1,pass\"".to_string(),
            mode: WafMode::On,
            delegation_precedence: precedence,
            ..WafConfig::default()
        })
        .expect("test WAF compiles");
    Arc::new(runtime)
}

fn owns_waf(selection: &crate::ctx::WafSelection, runtime: &Arc<crate::plugins::runtime::PluginRuntime>) -> bool {
    Arc::ptr_eq(selection.owner(), &runtime.waf_candidate().unwrap().owner())
}

/// Match a root -> mid -> leaf HTTPRoute chain whose root and leaf each attach
/// a WAF ExtensionRef (the mid level attaches an ExtensionRef without a WAF),
/// install every level's ExtensionRefs through the production installers, and
/// run the request filter's WAF election.
async fn elect_http_chain_waf(
    port: u16,
    root_precedence: edgion_resources::resources::edgion_plugins::WafDelegationPrecedence,
) -> (bool, bool) {
    use edgion_resources::resources::edgion_plugins::WafDelegationPrecedence;
    let routes = chain_routes(
        port,
        &[
            Level {
                ns: "default",
                name: "waf-root",
                prefix: "/api",
                rule: ext_ref("waf"),
            },
            Level {
                ns: "ns-mid",
                name: "waf-mid",
                prefix: "/api/v1",
                rule: ext_ref("order"),
            },
            Level {
                ns: "ns-leaf",
                name: "waf-leaf",
                prefix: "/api/v1/x",
                rule: ext_ref("waf"),
            },
        ],
    );
    let (_session, _client, mut ctx) = matched(port, &routes, "/api/v1/x/1").await;
    assert_eq!(ctx.delegated.as_ref().map(|d| d.ancestors.len()), Some(2));
    let root_waf = waf_plugins(root_precedence);
    let leaf_waf = waf_plugins(WafDelegationPrecedence::Child);
    let registry = HashMap::from([
        ("default/waf".to_string(), Arc::clone(&root_waf)),
        ("ns-mid/order".to_string(), order_plugins("ns-mid", "mid")),
        ("ns-leaf/waf".to_string(), Arc::clone(&leaf_waf)),
    ]);
    install_all_ext_refs(&mut ctx, &registry);
    let leaf_refs = ctx.resolved_route_ext_refs();
    let selected = elect_request_waf(&ctx, &[], &[], leaf_refs.as_deref().unwrap_or_default())
        .expect("a route-level WAF is on the chain");
    (owns_waf(&selected, &root_waf), owns_waf(&selected, &leaf_waf))
}

#[tokio::test]
async fn delegated_chain_root_parent_precedence_waf_owns_request() {
    use edgion_resources::resources::edgion_plugins::WafDelegationPrecedence;
    assert_eq!(
        elect_http_chain_waf(18781, WafDelegationPrecedence::Parent).await,
        (true, false),
        "the root's `parent` WAF beats the leaf's WAF"
    );
}

#[tokio::test]
async fn delegated_chain_default_precedence_child_waf_owns_request() {
    use edgion_resources::resources::edgion_plugins::WafDelegationPrecedence;
    assert_eq!(
        elect_http_chain_waf(18782, WafDelegationPrecedence::Child).await,
        (false, true),
        "by default the deepest level's WAF owns the request"
    );
}

#[tokio::test]
async fn grpc_delegated_chain_waf_election_sees_ancestor_ext_refs() {
    use crate::routes::grpc::GlobalGrpcRouteManagers;
    use edgion_resources::resources::edgion_plugins::WafDelegationPrecedence;
    let ext = |name: &str| {
        json!([{"type": "ExtensionRef",
            "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": name}}])
    };
    let root = grpc_route(
        "gwaf-root",
        Some(18792),
        None,
        json!({"matches": [{"method": {"service": "svc.Users"}}], "filters": ext("waf-root"),
            "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute", "name": "gwaf-leaf"}]}),
    );
    let leaf = grpc_route(
        "gwaf-leaf",
        None,
        Some("gwaf-root"),
        json!({"matches": [{"method": {"method": "Get"}}], "filters": ext("waf-leaf"),
            "backendRefs": [{"name": "svc", "port": 80}]}),
    );
    let mgr = GlobalGrpcRouteManagers::new();
    mgr.full_set(&HashMap::from([
        (root.key_name(), Arc::clone(&root)),
        (leaf.key_name(), Arc::clone(&leaf)),
    ]));
    let table = Arc::clone(&mgr.get_or_create_listener_manager(&listener(18792)).load_route_table());

    for (root_precedence, root_owns) in [
        (WafDelegationPrecedence::Parent, true),
        (WafDelegationPrecedence::Child, false),
    ] {
        let (mut client, server) = tokio::io::duplex(16 * 1024);
        let mut session = Session::new_h1(Box::new(server));
        client
            .write_all(b"POST /svc.Users/Get HTTP/1.1\r\nHost: api.example.com\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        assert!(session.read_request().await.unwrap());
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.hostname = NormalizedHostname::new("api.example.com");
        ctx.request_info.normalized_current_path = "/svc.Users/Get".to_string();
        assert!(
            crate::routes::grpc::try_match_grpc_route(&table, &mut session, &mut ctx, &index())
                .await
                .unwrap()
        );
        assert_eq!(ctx.delegated.as_ref().map(|d| d.grpc_ancestors.len()), Some(1));

        let root_waf = waf_plugins(root_precedence);
        let leaf_waf = waf_plugins(WafDelegationPrecedence::Child);
        let registry = HashMap::from([
            ("default/waf-root".to_string(), Arc::clone(&root_waf)),
            ("default/waf-leaf".to_string(), Arc::clone(&leaf_waf)),
        ]);
        let keys = ctx
            .grpc_route_unit
            .as_ref()
            .unwrap()
            .plugin_runtime
            .ext_ref_keys()
            .to_vec();
        ctx.install_resolved_route_ext_refs(
            keys.iter()
                .map(|key| key.as_deref().and_then(|key| registry.get(key).cloned()))
                .collect(),
        );
        install_delegated_ext_refs(&mut ctx, |key| registry.get(key).cloned());
        assert_eq!(
            ctx.delegated.as_ref().unwrap().ext_refs.len(),
            1,
            "the gRPC ancestor's ExtensionRefs are installed"
        );

        let leaf_refs = ctx.resolved_route_ext_refs();
        let selected = elect_request_waf(&ctx, &[], &[], leaf_refs.as_deref().unwrap_or_default())
            .expect("a route-level WAF is on the chain");
        assert_eq!(
            (owns_waf(&selected, &root_waf), owns_waf(&selected, &leaf_waf)),
            (root_owns, !root_owns),
            "root precedence {root_precedence:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Mirror tuning is per level
// ---------------------------------------------------------------------------

/// A route in `default` with `edgion.io/mirror-connect-timeout-ms` set to
/// `connect_ms`; a root when `parent` is `None`, otherwise a child of `parent`.
fn mirror_tuned_route(name: &str, port: u16, parent: Option<&str>, connect_ms: u64, rules: Value) -> Arc<HTTPRoute> {
    let parent_ref = match parent {
        None => json!({"group": "gateway.networking.k8s.io", "kind": "Gateway", "namespace": "default", "name": "gw1"}),
        Some(parent) => json!({"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": parent}),
    };
    let mut route: HTTPRoute = crate::test_current_status::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"namespace": "default", "name": name, "creationTimestamp": "2026-09-20T00:00:00Z",
            "annotations": {"edgion.io/mirror-connect-timeout-ms": connect_ms.to_string()}},
        "spec": {
            "resolvedStatusController": "edgion.io/gateway-controller",
            "parentRefs": [parent_ref],
            "rules": rules,
        }
    }))
    .unwrap();
    match parent {
        None => route.spec.resolved_listeners = Some(vec![listener(port)]),
        Some(parent) => route.spec.resolved_delegation_keys = vec![key("default", parent, "default", name)],
    }
    route.pre_parse();
    Arc::new(route)
}

fn mirror_filter() -> Value {
    json!([{"type": "RequestMirror", "requestMirror": {"backendRef": {"name": "shadow", "port": 80}}}])
}

/// `(mirror filters compiled into the unit's runtime, connect timeout the
/// unit's runtime was compiled with)`.
fn mirror_view(unit: &crate::routes::http::HttpRouteRuleUnit) -> (usize, Option<u64>) {
    let mirrors = unit
        .plugin_runtime
        .request_plugins_iter()
        .filter(|plugin| plugin.name() == "RequestMirror")
        .count();
    (
        mirrors,
        unit.rule.parsed_mirror_tuning.as_ref().map(|t| t.connect_timeout_ms),
    )
}

#[tokio::test]
async fn delegated_mirror_units_use_their_own_level_tuning() {
    let port = 18793;
    let delegate = |child: &str| json!([{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": child}]);
    let svc = json!([{"name": "svc", "port": 80}]);
    // `/a`: the parent's mandatory RequestMirror, child without one.
    // `/b`: no parent mirror, the child mirrors (no mask conflict).
    let parent = mirror_tuned_route(
        "mt-parent",
        port,
        None,
        111,
        json!([
            {"matches": [{"path": {"type": "PathPrefix", "value": "/a"}}], "filters": mirror_filter(),
                "backendRefs": delegate("mt-child-a")},
            {"matches": [{"path": {"type": "PathPrefix", "value": "/b"}}], "backendRefs": delegate("mt-child-b")},
        ]),
    );
    let child_a = mirror_tuned_route(
        "mt-child-a",
        port,
        Some("mt-parent"),
        222,
        json!([{"matches": [{"path": {"type": "PathPrefix", "value": "/a"}}], "backendRefs": svc}]),
    );
    let child_b = mirror_tuned_route(
        "mt-child-b",
        port,
        Some("mt-parent"),
        333,
        json!([{"matches": [{"path": {"type": "PathPrefix", "value": "/b"}}], "filters": mirror_filter(),
            "backendRefs": svc}]),
    );
    let routes = [parent, child_a, child_b];

    let (_session, _client, ctx) = matched(port, &routes, "/a/x").await;
    let delegated = ctx.delegated.as_ref().expect("delegated chain");
    assert_eq!(delegated.ancestors.len(), 1);
    assert_eq!(
        mirror_view(&delegated.ancestors[0]),
        (1, Some(111)),
        "the parent's mandatory mirror is compiled with the parent's tuning"
    );
    let leaf = ctx.route_unit.as_ref().unwrap();
    assert_eq!(leaf.matched_info.rsc.name, "mt-child-a");
    assert_eq!(mirror_view(leaf), (0, Some(222)));

    let (_session, _client, ctx) = matched(port, &routes, "/b/x").await;
    let delegated = ctx.delegated.as_ref().expect("delegated chain");
    assert_eq!(mirror_view(&delegated.ancestors[0]), (0, Some(111)));
    let leaf = ctx.route_unit.as_ref().unwrap();
    assert_eq!(leaf.matched_info.rsc.name, "mt-child-b");
    assert_eq!(
        mirror_view(leaf),
        (1, Some(333)),
        "the child's mirror is compiled with the child's tuning"
    );
}
