use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use edgion_resources::filters::PluginRunningResult;
use edgion_resources::{
    EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins, Gateway, GatewayClass, HTTPRoute, ListenerKey,
};
use serde_json::{json, Value};

use super::inputs::{GatewayInput, RouteInput, RouteKind};
use super::{ConnectionView, PluginPublisher, PluginScope};
use crate::plugins::http::CompiledPlugins;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::{PluginLog, RequestFilter};

fn plugin(program: &str, valid: bool) -> Arc<EdgionPlugins> {
    let mut spec = json!({"requestPlugins": [
        {"type": "RateLimitLocal", "config": {"rate": 100}},
        {"type": "RequestHeaderModifier", "config": {"set": [{"name": "X-Generation", "value": if valid { "good" } else { "bad" }}]}},
    ]});
    if program == "dsl" {
        spec["requestPlugins"].as_array_mut().unwrap().push(json!({
            "type": "Dsl", "config": {"name": "program", "source": if valid {"let a = 1"} else {"invalid @@@"}}
        }));
    } else {
        spec["waf"] = json!({"rules": if valid {"SecAction \"id:1,phase:1,pass\"\n"} else {"SecRule invalid-("}});
    }
    Arc::new(
        serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
            "metadata": {"namespace": "apps", "name": "plugins"}, "spec": spec,
        }))
        .unwrap(),
    )
}

fn config(name: &str, policy: Value) -> Arc<EdgionGatewayConfig> {
    Arc::new(
        serde_json::from_value(json!({
            "apiVersion": "edgion.io/v1", "kind": "EdgionGatewayConfig", "metadata": {"name": name},
            "spec": {"pluginPolicy": policy, "globalPluginsRef": [{"namespace": "apps", "name": "plugins"}]},
        }))
        .unwrap(),
    )
}

fn class(name: &str, config: Option<&str>) -> Arc<GatewayClass> {
    let mut spec = json!({"controllerName": "edgion.io/gateway-controller"});
    if let Some(config) = config {
        spec["parametersRef"] = json!({"group": "edgion.io", "kind": "EdgionGatewayConfig", "name": config});
    }
    Arc::new(serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GatewayClass", "metadata": {"name": name}, "spec": spec,
    })).unwrap())
}

fn gateway(name: &str, class: &str, port: u16, connection: Option<&str>) -> GatewayInput {
    let mut value = json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
        "metadata": {"name": name, "namespace": "apps"},
        "spec": {"gatewayClassName": class, "listeners": [{"name": "web", "port": port, "protocol": "HTTP"}]},
    });
    if let Some(connection) = connection {
        value["metadata"]["annotations"] = json!({"edgion.io/edgion-stream-plugins": connection});
    }
    GatewayInput::from_gateway(&serde_json::from_value::<Gateway>(value).unwrap())
}

fn publisher() -> PluginPublisher {
    let publisher = PluginPublisher::new();
    publisher.replace_gateway_classes(HashMap::from([("a".into(), class("a", Some("policy-a")))]));
    publisher.replace_gateway_configs(HashMap::from([("policy-a".into(), config("policy-a", json!({})))]));
    publisher.replace_gateways(HashMap::from([("apps/a".into(), gateway("a", "a", 18080, None))]));
    publisher
}

fn scope(publisher: &PluginPublisher, name: &str) -> Arc<PluginScope> {
    publisher.snapshot().gateway(name).unwrap().scope.clone()
}

fn view(publisher: &PluginPublisher) -> Arc<CompiledPlugins> {
    scope(publisher, "apps/a").http("apps/plugins").unwrap()
}

fn filter(view: &CompiledPlugins, name: &str) -> Arc<dyn RequestFilter> {
    view.plugin_runtime
        .request_plugins_iter()
        .find(|filter| filter.name() == name)
        .unwrap()
        .clone()
}

fn update_source(publisher: &PluginPublisher, source: Arc<EdgionPlugins>, full: bool) {
    let incoming = HashMap::from([("apps/plugins".into(), source)]);
    if full {
        publisher.replace_http_plugins(incoming);
    } else {
        publisher.update_http_plugins(HashMap::new(), incoming, &HashSet::new());
    }
}

fn update_policy(publisher: &PluginPublisher, policy: Value, full: bool) {
    let incoming = HashMap::from([("policy-a".into(), config("policy-a", policy))]);
    if full {
        publisher.replace_gateway_configs(incoming);
    } else {
        publisher.update_gateway_configs(incoming, &HashSet::new());
    }
}

async fn first_result(view: &CompiledPlugins) -> PluginRunningResult {
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("gate");
    view.plugin_runtime
        .request_plugins_iter()
        .next()
        .unwrap()
        .run_request(&mut session, &mut log)
        .await
}

#[tokio::test]
async fn current_policy_wins_over_invalid_dsl_and_waf_for_delta_full_and_event_order() {
    for program in ["dsl", "waf"] {
        for full in [false, true] {
            for policy_first in [false, true] {
                let publisher = publisher();
                update_source(&publisher, plugin(program, true), full);
                let first = view(&publisher);
                assert!(first.is_activatable, "{:?}", first.preparse_errors);
                let rate = filter(&first, "RateLimitLocal");
                let block = json!({"deny": [{"name": "http/RequestHeaderModifier", "blockStatus": 451}]});
                if policy_first {
                    update_policy(&publisher, block.clone(), full);
                }
                update_source(&publisher, plugin(program, false), full);
                if !policy_first {
                    update_policy(&publisher, block, full);
                }
                let restricted = view(&publisher);
                assert!(restricted.is_activatable, "retained valid program must stay usable");
                assert!(matches!(
                    first_result(&restricted).await,
                    PluginRunningResult::ErrResponse { status: 451, .. }
                ));
                assert!(Arc::ptr_eq(&rate, &filter(&restricted, "RateLimitLocal")));
                assert!(!restricted
                    .plugin_runtime
                    .request_plugins_iter()
                    .any(|filter| filter.name() == "RequestHeaderModifier"));
            }
        }
    }
}

#[test]
fn revoked_candidate_slots_do_not_resurrect_on_relaxation() {
    let publisher = publisher();
    update_source(&publisher, plugin("dsl", true), false);
    let first = view(&publisher);
    let rate = filter(&first, "RateLimitLocal");
    let weak = Arc::downgrade(&rate);
    update_source(&publisher, plugin("dsl", false), false);
    update_policy(
        &publisher,
        json!({"deny": [{"name": "http/RateLimitLocal", "action": "bypass"}]}),
        false,
    );
    let restricted = view(&publisher);
    assert!(restricted.is_activatable);
    drop(first);
    drop(rate);
    assert!(
        weak.upgrade().is_none(),
        "no candidate cache may retain the revoked old instance"
    );
    update_policy(&publisher, json!({}), false);
    let relaxed = view(&publisher);
    assert!(
        !relaxed.is_activatable,
        "relaxation must use the current invalid gate, not resurrect old slots"
    );
    assert_eq!(
        relaxed.plugin_runtime.request_plugins_iter().next().unwrap().name(),
        "RejectInvalidConfig"
    );
    assert!(!Arc::ptr_eq(&restricted, &relaxed));
}

#[tokio::test]
async fn latest_new_block_and_retained_deleted_entry_both_participate() {
    let publisher = publisher();
    update_source(&publisher, plugin("dsl", true), false);
    let first = view(&publisher);
    let original = filter(&first, "RateLimitLocal");
    let mut latest = (*plugin("dsl", false)).clone();
    latest
        .spec
        .request_plugins
        .as_mut()
        .unwrap()
        .retain(|entry| entry.plugin.type_name() != "RequestHeaderModifier");
    latest
        .spec
        .request_plugins
        .as_mut()
        .unwrap()
        .push(serde_json::from_value(json!({"type": "RequestId", "config": {}})).unwrap());
    update_source(&publisher, Arc::new(latest), false);
    update_policy(
        &publisher,
        json!({"deny": [
            {"name": "http/RequestId", "blockStatus": 451},
            {"name": "http/RequestHeaderModifier", "blockStatus": 403},
        ]}),
        false,
    );
    let current = view(&publisher);
    assert!(current.is_activatable);
    assert!(matches!(
        first_result(&current).await,
        PluginRunningResult::ErrResponse { status: 451, .. }
    ));
    assert!(!current
        .plugin_runtime
        .request_plugins_iter()
        .any(|filter| filter.name() == "RequestHeaderModifier"));
    assert!(
        Arc::ptr_eq(&original, &filter(&current, "RateLimitLocal")),
        "the selected candidate keeps its permitted state"
    );
}

#[test]
fn active_classes_share_instances_and_release_after_last_allowing_attachment() {
    let publisher = publisher();
    publisher.update_gateway_configs(
        HashMap::from([("policy-b".into(), config("policy-b", json!({})))]),
        &HashSet::new(),
    );
    publisher.update_gateway_classes(
        HashMap::from([("b".into(), class("b", Some("policy-b")))]),
        &HashSet::new(),
    );
    publisher.update_gateways(
        HashMap::from([("apps/b".into(), gateway("b", "b", 18081, None))]),
        &HashSet::new(),
    );
    update_source(&publisher, plugin("dsl", true), false);
    let first = view(&publisher);
    let second_scope = scope(&publisher, "apps/b");
    let second = second_scope.http("apps/plugins").unwrap();
    let rate = filter(&first, "RateLimitLocal");
    let weak = Arc::downgrade(&rate);
    assert!(Arc::ptr_eq(&rate, &filter(&second, "RateLimitLocal")));
    update_policy(&publisher, json!({"allow": []}), false);
    assert!(
        Arc::ptr_eq(&second_scope, &scope(&publisher, "apps/b")),
        "an unrelated class keeps its scope Arc"
    );
    publisher.update_gateways(HashMap::new(), &HashSet::from(["apps/b".into()]));
    assert_eq!(publisher.snapshot().stats().http_cached_entries, 0);
    drop(first);
    drop(second);
    drop(second_scope);
    drop(rate);
    assert!(weak.upgrade().is_none());
}

#[test]
fn unreferenced_sources_and_policies_never_construct_and_full_empty_cleans_up() {
    let publisher = PluginPublisher::new();
    publisher.replace_http_plugins(HashMap::from([("apps/plugins".into(), plugin("waf", true))]));
    publisher.replace_gateway_configs(HashMap::from([("unused".into(), config("unused", json!({})))]));
    assert_eq!(publisher.snapshot().stats().http_cached_entries, 0);
    publisher.replace_gateway_classes(HashMap::from([("a".into(), class("a", Some("unused")))]));
    publisher.replace_gateways(HashMap::from([("apps/a".into(), gateway("a", "a", 18080, None))]));
    let active = view(&publisher);
    assert_eq!(publisher.snapshot().inventory().http_generations.len(), 1);
    assert!(!publisher.snapshot().inventory().http_entries.is_empty());
    let owner = active.plugin_runtime.waf_candidate().unwrap().owner();
    let weak = Arc::downgrade(&owner);
    publisher.replace_gateways(HashMap::new());
    assert_eq!(publisher.snapshot().stats().http_cached_entries, 0);
    assert_eq!(publisher.snapshot().stats().scopes, 0);
    assert!(publisher.snapshot().inventory().scopes.is_empty());
    assert!(publisher.snapshot().inventory().http_entries.is_empty());
    drop(owner);
    drop(active);
    assert!(weak.upgrade().is_none());
    publisher.replace_http_plugins(HashMap::new());
    assert_eq!(publisher.snapshot().stats().http_generations, 0);
    assert!(publisher.snapshot().http_sources().is_empty());
    assert!(publisher.snapshot().inventory().http_generations.is_empty());
}

#[test]
fn missing_deleted_invalid_and_rebound_policies_never_become_implicit_allow() {
    let publisher = publisher();
    update_source(&publisher, plugin("dsl", true), false);
    publisher.replace_gateway_configs(HashMap::new());
    assert!(!scope(&publisher, "apps/a").available);
    assert!(scope(&publisher, "apps/a").http("apps/plugins").is_none());
    assert_eq!(publisher.snapshot().stats().http_cached_entries, 0);
    update_policy(&publisher, json!({"deny": [{"name": "http/Unknown"}]}), false);
    assert!(!scope(&publisher, "apps/a").available);
    update_policy(&publisher, json!({}), false);
    assert!(scope(&publisher, "apps/a").available);
    publisher.replace_gateway_classes(HashMap::new());
    assert!(!scope(&publisher, "apps/a").available);
    publisher.replace_gateway_classes(HashMap::from([("a".into(), class("a", None))]));
    assert!(
        scope(&publisher, "apps/a").available,
        "explicit no-ref uses built-in defaults"
    );
    publisher.replace_gateway_classes(HashMap::from([("a".into(), class("a", Some("missing")))]));
    assert!(!scope(&publisher, "apps/a").available);
}

#[test]
fn same_key_add_and_delete_recreate_do_not_retain_old_programs() {
    for remove_first in [false, true] {
        let publisher = publisher();
        update_source(&publisher, plugin("dsl", true), false);
        if remove_first {
            publisher.update_http_plugins(HashMap::new(), HashMap::new(), &HashSet::from(["apps/plugins".into()]));
        }
        publisher.update_http_plugins(
            HashMap::from([("apps/plugins".into(), plugin("dsl", false))]),
            HashMap::new(),
            &HashSet::new(),
        );
        assert!(!view(&publisher).is_activatable);
        assert_eq!(publisher.snapshot().stats().http_generations, 1);
    }
}

fn route() -> Arc<HTTPRoute> {
    Arc::new(
        serde_json::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
            "metadata": {"namespace": "apps", "name": "route"},
            "spec": {"parentRefs": [{"name": "a"}], "resolvedListeners": ["0.0.0.0:18080/HTTP"],
                "rules": [{"backendRefs": [{"name": "service", "port": 80}]}]},
        }))
        .unwrap(),
    )
}

#[test]
fn admission_tokens_reject_cross_generation_pairs_and_do_not_pin_routes() {
    let publisher = publisher();
    let old_route = route();
    publisher.replace_routes(
        RouteKind::Http,
        HashMap::from([("apps/route".into(), RouteInput::from_http(old_route.clone()))]),
    );
    let old_scope = scope(&publisher, "apps/a");
    assert!(old_scope.ai_rule(&old_route, 0));
    assert!(!old_scope.ai_rule(&old_route, 1));
    let new_route = route();
    assert!(!old_scope.ai_rule(&new_route, 0));
    publisher.update_routes(
        RouteKind::Http,
        HashMap::from([("apps/route".into(), RouteInput::from_http(new_route.clone()))]),
        &HashSet::new(),
    );
    let new_scope = scope(&publisher, "apps/a");
    assert!(new_scope.ai_rule(&new_route, 0));
    assert!(!new_scope.ai_rule(&old_route, 0));
    assert!(
        old_scope.ai_rule(&old_route, 0),
        "coherent captured old source remains valid"
    );
    let weak = Arc::downgrade(&old_route);
    drop(old_route);
    assert!(weak.upgrade().is_none(), "old scope admission tokens are Weak");
    publisher.replace_routes(RouteKind::Http, HashMap::new());
    assert!(!scope(&publisher, "apps/a").ai_rule(&new_route, 0));
    assert_eq!(publisher.snapshot().stats().ai_route_tokens, 0);
}

fn stream_source() -> Arc<EdgionStreamPlugins> {
    Arc::new(serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionStreamPlugins",
        "metadata": {"namespace": "apps", "name": "stream"},
        "spec": {"plugins": [{"type": "ConnectionRateLimit", "config": {"perListener": {"rate": 2, "interval": "1h"}}}]},
    })).unwrap())
}

#[test]
fn shared_listener_requires_equivalent_connection_views_but_not_equal_http_policy() {
    let publisher = publisher();
    publisher.update_gateway_configs(
        HashMap::from([("policy-b".into(), config("policy-b", json!({"allow": []})))]),
        &HashSet::new(),
    );
    publisher.update_gateway_classes(
        HashMap::from([("b".into(), class("b", Some("policy-b")))]),
        &HashSet::new(),
    );
    publisher.update_gateways(
        HashMap::from([("apps/b".into(), gateway("b", "b", 18080, None))]),
        &HashSet::new(),
    );
    let listener = ListenerKey::new("0.0.0.0:18080".parse().unwrap(), "HTTP");
    assert!(publisher.snapshot().unique_gateway(&listener).is_none());
    assert!(matches!(
        publisher.snapshot().connection(&listener),
        ConnectionView::Empty
    ));
    publisher.replace_stream_plugins(HashMap::from([("apps/stream".into(), stream_source())]));
    assert_eq!(publisher.snapshot().stats().stream_cached_entries, 0);
    publisher.replace_gateways(HashMap::from([
        ("apps/a".into(), gateway("a", "a", 18080, Some("stream"))),
        ("apps/b".into(), gateway("b", "b", 18080, Some("stream"))),
    ]));
    assert!(matches!(
        publisher.snapshot().connection(&listener),
        ConnectionView::Unavailable
    ));
    publisher.update_gateway_configs(
        HashMap::from([(
            "policy-b".into(),
            config("policy-b", json!({"deny": [{"name": "http/RateLimitLocal"}]})),
        )]),
        &HashSet::new(),
    );
    assert!(matches!(
        publisher.snapshot().connection(&listener),
        ConnectionView::Ready(_)
    ));
    assert_eq!(publisher.snapshot().stats().stream_cached_entries, 1);
    publisher.replace_stream_plugins(HashMap::new());
    assert!(matches!(
        publisher.snapshot().connection(&listener),
        ConnectionView::Unavailable
    ));
    publisher.replace_gateways(HashMap::new());
    assert!(publisher.snapshot().listener_index(&listener).is_none());
}

#[test]
fn another_class_cache_cannot_resurrect_a_revoked_candidate_slot() {
    let publisher = publisher();
    publisher.update_gateway_configs(
        HashMap::from([("policy-b".into(), config("policy-b", json!({})))]),
        &HashSet::new(),
    );
    publisher.update_gateway_classes(
        HashMap::from([("b".into(), class("b", Some("policy-b")))]),
        &HashSet::new(),
    );
    publisher.update_gateways(
        HashMap::from([("apps/b".into(), gateway("b", "b", 18081, None))]),
        &HashSet::new(),
    );
    update_source(&publisher, plugin("dsl", true), false);
    let original = filter(&view(&publisher), "RateLimitLocal");
    update_source(&publisher, plugin("dsl", false), false);
    update_policy(
        &publisher,
        json!({"deny": [{"name": "http/RateLimitLocal", "action": "bypass"}]}),
        false,
    );
    assert!(view(&publisher).is_activatable);
    let retained_b = scope(&publisher, "apps/b");
    assert!(Arc::ptr_eq(
        &original,
        &filter(&retained_b.http("apps/plugins").unwrap(), "RateLimitLocal")
    ));
    update_policy(&publisher, json!({}), false);
    assert!(!view(&publisher).is_activatable);
    assert!(Arc::ptr_eq(&retained_b, &scope(&publisher, "apps/b")));
}

#[tokio::test]
async fn ai_admission_uses_effective_policy_and_current_route_facts() {
    let publisher = publisher();
    let mut source = (*plugin("dsl", true)).clone();
    source
        .spec
        .request_plugins
        .as_mut()
        .unwrap()
        .push(serde_json::from_value(json!({"type": "AiProxy", "config": {}})).unwrap());
    update_source(&publisher, Arc::new(source), false);
    let mut ai_route = (*route()).clone();
    ai_route.spec.rules = Some(vec![serde_json::from_value(json!({
        "filters": [{"type": "ExtensionRef", "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "plugins"}}],
        "backendRefs": [{"group": "edgion.io", "kind": "EdgionBackend", "name": "ai"}],
        "resolvedAiAdmission": {"backends": [{"kind": "edgionBackend", "specValid": true, "canonicalModels": []}]},
    })).unwrap()]);
    let ai_route = Arc::new(ai_route);
    publisher.replace_routes(
        RouteKind::Http,
        HashMap::from([("apps/route".into(), RouteInput::from_http(ai_route.clone()))]),
    );
    assert!(scope(&publisher, "apps/a").ai_rule(&ai_route, 0));
    update_policy(
        &publisher,
        json!({"deny": [{"name": "http/AiProxy", "action": "bypass"}]}),
        false,
    );
    assert!(!scope(&publisher, "apps/a").ai_rule(&ai_route, 0));
    update_policy(
        &publisher,
        json!({"deny": [{"name": "http/AiProxy", "blockStatus": 451}]}),
        false,
    );
    assert!(matches!(
        first_result(&view(&publisher)).await,
        PluginRunningResult::ErrResponse { status: 451, .. }
    ));
    update_policy(&publisher, json!({}), false);
    let mut missing_facts = (*ai_route).clone();
    missing_facts.spec.rules.as_mut().unwrap()[0].resolved_ai_admission = None;
    let missing_facts = Arc::new(missing_facts);
    publisher.replace_routes(
        RouteKind::Http,
        HashMap::from([("apps/route".into(), RouteInput::from_http(missing_facts.clone()))]),
    );
    assert!(!scope(&publisher, "apps/a").ai_rule(&missing_facts, 0));
}
