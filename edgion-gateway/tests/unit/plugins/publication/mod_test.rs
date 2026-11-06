use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

use base64::Engine as _;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::{
    EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins, Gateway, GatewayClass, HTTPRoute, ListenerKey,
};
use serde_json::{json, Value};

use super::inputs::{GatewayInput, RouteInput, RouteKind};
use super::{ConnectionView, PluginPublisher, PluginScope};
use crate::plugins::http::CompiledPlugins;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use crate::plugins::runtime::{PluginLog, PluginRuntime, RequestFilter};

impl PluginScope {
    pub(crate) fn http(&self, key: &str) -> Option<Arc<CompiledPlugins>> {
        self.http.get(key).cloned()
    }
}

fn plugin(program: &str, valid: bool) -> Arc<EdgionPlugins> {
    let mut spec = json!({"requestPlugins": [
        {"type": "RateLimitLocal", "config": {"rate": 100}},
        {"type": "RequestHeaderModifier", "config": {"set": [{"name": "X-Generation", "value": if valid { "good" } else { "bad" }}]}},
    ]});
    if program == "dsl" {
        spec["requestPlugins"].as_array_mut().unwrap().push(json!({
            "type": "Dsl", "config": {"name": "program", "source": if valid {"let a = 1"} else {"invalid @@@"}}
        }));
    }
    let mut resource: EdgionPlugins = serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"namespace": "apps", "name": "plugins"}, "spec": spec,
    }))
    .unwrap();
    if program == "waf" {
        resource.spec.waf = Some(crate::plugins::http::waf::fixture::config_in_namespace(
            if valid {
                "SecAction \"id:1,phase:1,pass\"\n"
            } else {
                "SecRule invalid-("
            },
            "apps",
        ));
    }
    Arc::new(resource)
}

fn jwe_plugin(program: &str, valid_program: bool, key: &[u8]) -> Arc<EdgionPlugins> {
    let mut spec = json!({"requestPlugins": [{
        "type": "JweDecrypt",
        "config": {
            "secretRef": {"name": "jwe-key"},
            "stripPrefix": "Bearer ",
            "resolvedCredential": {
                "secret": base64::engine::general_purpose::STANDARD.encode(key)
            }
        }
    }]});
    if program == "dsl" {
        spec["requestPlugins"].as_array_mut().unwrap().push(json!({
            "type": "Dsl",
            "config": {"name": "program", "source": if valid_program {"let a = 1"} else {"invalid @@@"}}
        }));
    }
    let mut resource: EdgionPlugins = serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
        "metadata": {"namespace": "apps", "name": "plugins"}, "spec": spec,
    }))
    .unwrap();
    if program == "waf" {
        resource.spec.waf = Some(crate::plugins::http::waf::fixture::config_in_namespace(
            if valid_program {
                "SecAction \"id:1,phase:1,pass\"\n"
            } else {
                "SecRule invalid-("
            },
            "apps",
        ));
    }
    Arc::new(resource)
}

fn jwe_plugin_with_skip(program: &str, valid_program: bool, key: &[u8]) -> Arc<EdgionPlugins> {
    let mut source = (*jwe_plugin(program, valid_program, key)).clone();
    source.spec.request_plugins.as_mut().unwrap()[0].conditions = Some(
        serde_json::from_value(json!({
            "skip": {
                "allOf": [{
                    "type": "keyExist",
                    "key": {"type": "header", "name": "x-bypass"}
                }]
            }
        }))
        .unwrap(),
    );
    Arc::new(source)
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
    GatewayInput::from_gateway(&crate::test_current_status::from_value::<Gateway>(value).unwrap())
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

async fn malformed_jwe_result(view: &CompiledPlugins) -> (u16, String) {
    let filter = filter(view, "JweDecrypt");
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("authorization", "Bearer malformed")]);
    let status = Arc::new(AtomicU16::new(0));
    let written = Arc::clone(&status);
    session
        .expect_write_response_header()
        .times(1)
        .returning(move |header, _| {
            written.store(header.status.as_u16(), Ordering::Relaxed);
            Ok(())
        });
    session.expect_write_response_body().times(1).returning(|_, _| Ok(()));
    session.expect_shutdown().times(1).returning(|| ());
    let mut log = PluginLog::new("JweDecrypt");
    assert_eq!(
        filter.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
    (status.load(Ordering::Relaxed), log.render_for_test())
}

#[tokio::test]
async fn changed_jwe_credentials_block_mixed_program_retention_for_delta_and_full_updates() {
    let first_key = b"0123456789abcdef0123456789abcdef";
    let recovered_key = b"abcdef0123456789abcdef0123456789";
    for program in ["dsl", "waf"] {
        for full in [false, true] {
            let publisher = publisher();
            update_source(&publisher, jwe_plugin(program, true, first_key), full);
            let initial = view(&publisher);
            assert!(
                initial.is_activatable,
                "{program} {full:?}: {:?}",
                initial.preparse_errors
            );
            assert_eq!(
                malformed_jwe_result(&initial).await,
                (400, "invalid format".to_string())
            );

            update_source(&publisher, jwe_plugin(program, false, b"too-short"), full);
            let mixed_invalid = view(&publisher);
            assert!(!mixed_invalid.is_activatable, "{program} {full:?}");
            assert_eq!(
                mixed_invalid
                    .plugin_runtime
                    .request_plugins_iter()
                    .next()
                    .unwrap()
                    .name(),
                "RejectInvalidConfig"
            );

            update_source(&publisher, jwe_plugin(program, true, b"too-short"), full);
            let unavailable = view(&publisher);
            assert!(
                unavailable.is_activatable,
                "{program} {full:?}: {:?}",
                unavailable.preparse_errors
            );
            assert_eq!(
                malformed_jwe_result(&unavailable).await,
                (401, "err: key len err".to_string())
            );

            update_source(&publisher, jwe_plugin(program, true, recovered_key), full);
            let recovered = view(&publisher);
            assert!(
                recovered.is_activatable,
                "{program} {full:?}: {:?}",
                recovered.preparse_errors
            );
            assert_eq!(
                malformed_jwe_result(&recovered).await,
                (400, "invalid format".to_string())
            );

            // In-flight snapshots may finish, but current publication never
            // selects the obsolete credential-bearing generation again.
            assert!(initial.is_activatable);
        }
    }
}

#[test]
fn jwe_entry_condition_change_blocks_mixed_program_retention() {
    let key = b"0123456789abcdef0123456789abcdef";
    for program in ["dsl", "waf"] {
        for full in [false, true] {
            let publisher = publisher();
            update_source(&publisher, jwe_plugin_with_skip(program, true, key), full);
            assert!(view(&publisher).is_activatable, "{program} {full:?}");

            // The incoming source removes an authentication bypass while its
            // program is invalid. Retaining the old source would preserve the
            // obsolete skip wrapper even though the JWE key itself is equal.
            update_source(&publisher, jwe_plugin(program, false, key), full);
            let current = view(&publisher);
            assert!(!current.is_activatable, "{program} {full:?}");
            assert_eq!(
                current.plugin_runtime.request_plugins_iter().next().unwrap().name(),
                "RejectInvalidConfig"
            );
        }
    }
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
fn scoped_waf_views_share_executable_without_retaining_bundle_sources() {
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

    update_source(&publisher, plugin("waf", true), false);
    let first = view(&publisher);
    let second = scope(&publisher, "apps/b").http("apps/plugins").unwrap();
    assert!(first.is_activatable);
    assert!(second.is_activatable);
    assert!(Arc::ptr_eq(&filter(&first, "Waf"), &filter(&second, "Waf")));
    for scoped in [&first, &second] {
        assert!(scoped.spec.spec.waf.as_ref().unwrap().resolved_bundles.is_none());
    }
    assert!(publisher.snapshot().http_sources()["apps/plugins"]
        .spec
        .waf
        .as_ref()
        .unwrap()
        .resolved_bundles
        .is_some());
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
        crate::test_current_status::from_value(json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
            "metadata": {"namespace": "apps", "name": "route"},
            "spec": {"resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": [{"name": "a"}], "resolvedListeners": ["0.0.0.0:18080/HTTP"],
                "rules": [{"backendRefs": [{"name": "service", "port": 80}]}]},
        }))
        .unwrap(),
    )
}

#[test]
fn owner_and_status_updates_revoke_and_restore_route_plugin_contributions() {
    let publisher = publisher();
    // Route-local plugin demand must disappear when the route loses acceptance.
    publisher.replace_gateway_configs(HashMap::from([(
        "policy-a".into(),
        Arc::new(
            serde_json::from_value(json!({"apiVersion": "edgion.io/v1", "kind": "EdgionGatewayConfig",
            "metadata": {"name": "policy-a"}, "spec": {}}))
            .unwrap(),
        ),
    )]));
    publisher.replace_http_plugins(HashMap::from([("apps/plugins".into(), plugin("dsl", true))]));
    let mut source = (*route()).clone();
    source.spec.rules.as_mut().unwrap()[0].filters = Some(
        serde_json::from_value(json!([{
            "type": "ExtensionRef", "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "plugins"}
        }]))
        .unwrap(),
    );
    source.status = Some(
        serde_json::from_value(json!({"parents": [{
            "parentRef": {"name": "a"}, "controllerName": "example.net/owner",
            "conditions": [{"type": "Accepted", "status": "False", "reason": "Rejected", "message": "",
                "lastTransitionTime": "2026-09-20T00:00:00Z"}]
        }]}))
        .unwrap(),
    );
    let inputs =
        |source: &HTTPRoute| HashMap::from([("apps/route".into(), RouteInput::from_http(Arc::new(source.clone())))]);
    publisher.replace_routes(RouteKind::Http, inputs(&source));
    assert!(scope(&publisher, "apps/a").http("apps/plugins").is_some());
    source.spec.resolved_status_controller = Some("example.net/owner".into());
    publisher.update_routes(RouteKind::Http, inputs(&source), &HashSet::new());
    assert!(scope(&publisher, "apps/a").http("apps/plugins").is_none());
    source.spec.current_status.as_mut().unwrap().parents[0].controller_name = "example.net/owner".into();
    source.spec.current_status.as_mut().unwrap().parents[0].conditions[0].status = "True".into();
    publisher.update_routes(RouteKind::Http, inputs(&source), &HashSet::new());
    assert!(scope(&publisher, "apps/a").http("apps/plugins").is_some());
    source.spec.resolved_status_controller = None;
    publisher.replace_routes(RouteKind::Http, inputs(&source));
    assert!(scope(&publisher, "apps/a").http("apps/plugins").is_none());
    publisher.update_routes(RouteKind::Http, HashMap::new(), &HashSet::from(["apps/route".into()]));
    assert_eq!(publisher.snapshot().stats().ai_route_tokens, 0);
}

#[test]
fn every_route_plugin_input_uses_its_same_generation_owner() {
    use edgion_resources::{GRPCRoute, TCPRoute, TLSRoute, UDPRoute};
    for kind in ["HTTPRoute", "GRPCRoute", "TCPRoute", "TLSRoute", "UDPRoute"] {
        let publisher = publisher();
        let protocol = match kind {
            "TCPRoute" => "TCP",
            "TLSRoute" => "TLS",
            "UDPRoute" => "UDP",
            _ => "HTTP",
        };
        let listener = ListenerKey::new("0.0.0.0:18080".parse().unwrap(), protocol);
        let mut gateway = gateway("a", "a", 18080, None);
        gateway.listeners[0].key = listener.clone();
        publisher.replace_gateways(HashMap::from([("apps/a".into(), gateway.clone())]));
        publisher.replace_gateway_configs(HashMap::from([(
            "policy-a".into(),
            Arc::new(
                serde_json::from_value(json!({"apiVersion": "edgion.io/v1", "kind": "EdgionGatewayConfig",
                "metadata": {"name": "policy-a"}, "spec": {}}))
                .unwrap(),
            ),
        )]));
        publisher.replace_http_plugins(HashMap::from([("apps/plugins".into(), plugin("dsl", true))]));
        publisher.replace_stream_plugins(HashMap::from([("apps/stream".into(), stream_source())]));
        let mut value = json!({"apiVersion": "gateway.networking.k8s.io/v1", "kind": kind,
            "metadata": {"name": "route", "namespace": "apps"},
            "spec": {"parentRefs": [{"name": "a"}], "resolvedStatusController": "example.net/owner",
                "resolvedListeners": [listener.to_string()]},
            "status": {"parents": [{"parentRef": {"name": "a"}, "controllerName": "example.net/owner",
                "conditions": [{"type": "Accepted", "status": "False", "reason": "Rejected", "message": "",
                    "lastTransitionTime": "2026-09-20T00:00:00Z"}]}]}});
        if kind == "TLSRoute" {
            value["spec"]["resolvedListeners"] = json!([{"listenerKey": listener.to_string(),
                "gatewayNamespace": "apps", "gatewayName": "a", "listenerName": "web", "hostnames": ["example.com"]}]);
        }
        let route_kind = match kind {
            "HTTPRoute" => RouteKind::Http,
            "GRPCRoute" => RouteKind::Grpc,
            "TCPRoute" => RouteKind::Tcp,
            "TLSRoute" => RouteKind::Tls,
            _ => RouteKind::Udp,
        };
        let http = matches!(route_kind, RouteKind::Http | RouteKind::Grpc);
        if http {
            value["spec"]["rules"] = json!([{"filters": [{"type": "ExtensionRef",
                "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "plugins"}}]}]);
        } else {
            value["metadata"]["annotations"] = json!({"edgion.io/edgion-stream-plugins": "stream"});
        }
        let input = |value: serde_json::Value| match kind {
            "HTTPRoute" => RouteInput::from_http(Arc::new(serde_json::from_value::<HTTPRoute>(value).unwrap())),
            "GRPCRoute" => RouteInput::from_grpc(&serde_json::from_value::<GRPCRoute>(value).unwrap()),
            "TCPRoute" => RouteInput::from_tcp(&serde_json::from_value::<TCPRoute>(value).unwrap()),
            "TLSRoute" => RouteInput::from_tls(&serde_json::from_value::<TLSRoute>(value).unwrap()),
            _ => RouteInput::from_udp(&serde_json::from_value::<UDPRoute>(value).unwrap()),
        };
        value["spec"]["currentStatus"] = value["status"].clone();
        value["status"]["parents"][0]["conditions"][0]["status"] = json!("True");
        assert!(!input(value.clone()).attaches_to("apps/a", &gateway), "{kind}");
        let has_contribution = || {
            let scope = scope(&publisher, "apps/a");
            if http {
                scope.http("apps/plugins").is_some()
            } else {
                scope.stream("apps/stream").is_some()
            }
        };
        let inputs = |value: Value| HashMap::from([("apps/route".into(), input(value))]);
        publisher.replace_routes(route_kind, inputs(value.clone()));
        assert!(!has_contribution(), "{kind}");
        value["spec"]["resolvedStatusController"] = json!("example.net/other");
        assert!(!input(value.clone()).attaches_to("apps/a", &gateway), "{kind}");
        publisher.update_routes(route_kind, inputs(value.clone()), &HashSet::new());
        assert!(!has_contribution(), "{kind}");
        value["spec"]["resolvedStatusController"] = json!("example.net/owner");
        publisher.update_routes(route_kind, inputs(value.clone()), &HashSet::new());
        assert!(!has_contribution(), "{kind}");
        value["status"]["parents"][0]["conditions"][0]["status"] = json!("False");
        value["spec"]["currentStatus"]["parents"][0]["conditions"][0]["status"] = json!("True");
        publisher.update_routes(route_kind, inputs(value.clone()), &HashSet::new());
        assert!(has_contribution(), "{kind}");
        value["status"]["parents"][0]["conditions"][0]["status"] = json!("True");
        value["spec"]["currentStatus"] = Value::Null;
        publisher.update_routes(route_kind, inputs(value.clone()), &HashSet::new());
        assert!(
            !has_contribution(),
            "missing current status must not use full status: {kind}"
        );
        value["spec"]["resolvedStatusController"] = serde_json::Value::Null;
        assert!(!input(value.clone()).attaches_to("apps/a", &gateway), "{kind}");
        publisher.replace_routes(route_kind, inputs(value));
        assert!(!has_contribution(), "{kind}");
        publisher.update_routes(route_kind, HashMap::new(), &HashSet::from(["apps/route".into()]));
        assert!(!has_contribution(), "{kind}");
    }
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

#[test]
fn ordinary_invalid_backend_slots_do_not_seal_the_published_rule() {
    use crate::routes::http::match_unit::HttpRouteRuleUnit;

    let publisher = publisher();
    let mut mixed = (*route()).clone();
    mixed.spec.rules = Some(vec![serde_json::from_value(json!({
        "backendRefs": [
            {"name": "service", "port": 80, "weight": 1},
            {"group": "core", "name": "service", "port": 80, "weight": 1}
        ],
        "resolvedAiAdmission": {"backends": [{"kind": "service"}, {"kind": "missing"}]}
    }))
    .unwrap()]);
    let mixed = Arc::new(mixed);
    publisher.replace_routes(
        RouteKind::Http,
        HashMap::from([("apps/route".into(), RouteInput::from_http(mixed.clone()))]),
    );
    let scope = scope(&publisher, "apps/a");
    assert!(
        scope.ai_rule(&mixed, 0),
        "the ordinary request admission gate must leave selection reachable"
    );
    assert!(scope.graphql_rule(&mixed, 0));
    let runtime = HttpRouteRuleUnit::compile_http_rule(&mixed, 0).unwrap();
    assert_eq!(runtime.backends.len(), 2);
    assert!(runtime.backends[0].permitted_service().is_some());
    assert!(runtime.backends[1].permitted_service().is_none());
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
    assert!(publisher.snapshot().http_listener(&listener).unique_gateway().is_none());
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
    source.spec.request_plugins.as_mut().unwrap().push(
        serde_json::from_value(
            json!({"type": "AiProxy", "config": {"api":"openaiChat","usageObservation":"provider",}}),
        )
        .unwrap(),
    );
    update_source(&publisher, Arc::new(source), false);
    let mut ai_route = (*route()).clone();
    ai_route.spec.rules = Some(vec![serde_json::from_value(json!({
        "filters": [{"type": "ExtensionRef", "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "plugins"}}],
        "backendRefs": [{"group": "edgion.io", "kind": "EdgionBackend", "name": "ai"}],
        "resolvedAiAdmission": {"backends": [{"kind": "edgionBackend", "provider":"OpenAICompatible", "hasCredentialTpm":false, "specValid": true, "canonicalModels": []}]},
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

#[tokio::test]
async fn unsupported_plugin_update_replaces_previous_runtime_and_correction_recovers() {
    for full in [true, false] {
        let publisher = publisher();
        update_source(&publisher, plugin("none", true), full);
        assert!(view(&publisher).is_activatable);
        let invalid: EdgionPlugins = serde_json::from_value(json!({
            "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
            "metadata":{"namespace":"apps", "name":"plugins"},
            "spec":{"requestPlugins":[{"type":"FuturePlugin", "config":{}}]}
        }))
        .unwrap();
        update_source(&publisher, Arc::new(invalid), full);
        let rejected = view(&publisher);
        assert!(!rejected.is_activatable);
        assert!(matches!(
            first_result(&rejected).await,
            PluginRunningResult::ErrResponse { status: 500, .. }
        ));
        update_source(&publisher, plugin("none", true), full);
        assert!(view(&publisher).is_activatable);
    }
}

#[tokio::test]
async fn owning_extension_lifecycle_pins_old_scope_and_rebuilds_new_requests() {
    let publisher = publisher();
    // Remove global demand: this test exercises the actual route's local reference.
    let mut policy = (*config("policy-a", json!({}))).clone();
    policy.spec.global_plugins_ref = None;
    publisher.replace_gateway_configs(HashMap::from([("policy-a".into(), Arc::new(policy))]));
    let mut source = (*route()).clone();
    source.spec.rules.as_mut().unwrap()[0].filters = Some(
        serde_json::from_value(json!([{
            "type":"ExtensionRef", "extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"plugins"}
        }]))
        .unwrap(),
    );
    let source = Arc::new(source);
    publisher.replace_routes(
        RouteKind::Http,
        HashMap::from([("apps/route".into(), RouteInput::from_http(source.clone()))]),
    );
    let mut missing = crate::EdgionHttpContext::new();
    assert!(missing.capture_plugin_scope(publisher.snapshot().gateway("apps/a").unwrap()));
    let missing_scope = missing.plugin_scope().unwrap().scope.clone();
    assert!(missing.resolve_pinned_backend_ext_ref("apps/plugins").is_none());
    update_source(&publisher, plugin("dsl", true), false);
    let created = view(&publisher);
    let mut captured = crate::EdgionHttpContext::new();
    assert!(captured.capture_plugin_scope(publisher.snapshot().gateway("apps/a").unwrap()));
    let captured_scope = captured.plugin_scope().unwrap().scope.clone();
    let original = captured.resolve_pinned_backend_ext_ref("apps/plugins").unwrap();
    assert!(Arc::ptr_eq(&original, &created.plugin_runtime));
    let mut updated = (*plugin("dsl", true)).clone();
    updated.metadata.uid = Some("replacement".into());
    updated.metadata.generation = Some(2);
    update_source(&publisher, Arc::new(updated), false);
    assert!(!Arc::ptr_eq(&created, &view(&publisher)));
    publisher.update_http_plugins(HashMap::new(), HashMap::new(), &HashSet::from(["apps/plugins".into()]));
    assert!(scope(&publisher, "apps/a").http("apps/plugins").is_none());
    update_source(&publisher, plugin("dsl", true), false);
    assert!(scope(&publisher, "apps/a").http("apps/plugins").is_some());
    update_policy(
        &publisher,
        json!({"deny":[{"name":"http/RateLimitLocal","action":"block"}]}),
        false,
    );
    assert!(matches!(
        first_result(&view(&publisher)).await,
        PluginRunningResult::ErrResponse { .. }
    ));
    assert!(captured.capture_plugin_scope(publisher.snapshot().gateway("apps/a").unwrap()));
    assert!(Arc::ptr_eq(&captured.plugin_scope().unwrap().scope, &captured_scope));
    for _ in 0..3 {
        assert!(Arc::ptr_eq(
            &captured.resolve_pinned_backend_ext_ref("apps/plugins").unwrap(),
            &original
        ));
        assert!(missing.resolve_pinned_backend_ext_ref("apps/plugins").is_none());
    }
    assert!(captured_scope.ai_rule(&source, 0));
    assert!(missing_scope.http("apps/plugins").is_none());
}

#[test]
fn descriptor_publication_rejects_constructor_material_but_retains_opaque_source_and_old_arcs() {
    let publisher = publisher();
    publisher.replace_gateway_configs(HashMap::from([(
        "policy-a".into(),
        Arc::new(serde_json::from_value(json!({"metadata":{"name":"policy-a"},"spec":{}})).unwrap()),
    )]));
    let listener = ListenerKey::new("0.0.0.0:18080".parse().unwrap(), "HTTP");
    let route: HTTPRoute = crate::test_current_status::from_value(json!({
        "metadata":{"name":"descriptor-route","namespace":"apps"},
        "spec":{"parentRefs":[{"name":"a"}],"resolvedStatusController":"example.net/owner","resolvedListeners":[listener.to_string()],
            "rules":[{"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"plugins"}}]}]},
        "status":{"parents":[{"parentRef":{"name":"a"},"controllerName":"example.net/owner","conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"","lastTransitionTime":"2026-10-08T00:00:00Z"}]}]}
    })).unwrap();
    publisher.replace_routes(
        RouteKind::Http,
        HashMap::from([("apps/descriptor-route".into(), RouteInput::from_http(Arc::new(route)))]),
    );
    let bytes = include_bytes!("../../../../../edgion-tests/proto/grpc_json_transcoding/test.pb");
    let source: EdgionPlugins = serde_json::from_value(json!({"metadata":{"name":"plugins","namespace":"apps","uid":"first","generation":1},"spec":{"requestPlugins":[
        {"type":"RequestHeaderModifier","config":{"set":[{"name":"X-Sibling","value":"kept"}]}},
        {"type":"GrpcJsonTranscoder","config":{"descriptor":{"type":"configMapRef","name":"schema","key":"test.pb"},"services":["test.TestService"],"resolvedDescriptor":bytes.to_vec()}}
    ]}})).unwrap();
    update_source(&publisher, Arc::new(source.clone()), false);
    let first = view(&publisher);
    assert!(first.is_activatable);
    let old = filter(&first, "GrpcJsonTranscoder");
    for full in [false, true] {
        let mut invalid = source.clone();
        invalid.metadata.uid = Some(format!("invalid-{full}"));
        let edgion_resources::EdgionPlugin::GrpcJsonTranscoder(config) =
            &mut invalid.spec.request_plugins.as_mut().unwrap()[1].plugin
        else {
            panic!()
        };
        config.descriptor = edgion_resources::grpc_transcoding::GrpcDescriptorSource::ConfigMapRef {
            name: "bad/name".into(),
            key: "test.pb".into(),
        };
        update_source(&publisher, Arc::new(invalid), full);
        let rejected = view(&publisher);
        assert!(!rejected.is_activatable);
        let edgion_resources::EdgionPlugin::GrpcJsonTranscoder(config) =
            &rejected.spec.spec.request_plugins.as_ref().unwrap()[1].plugin
        else {
            panic!()
        };
        assert!(config.resolved_descriptor.is_none());
        let snapshot = publisher.snapshot();
        let raw = &snapshot.http_sources()["apps/plugins"];
        let edgion_resources::EdgionPlugin::GrpcJsonTranscoder(config) =
            &raw.spec.request_plugins.as_ref().unwrap()[1].plugin
        else {
            panic!()
        };
        assert_eq!(config.resolved_descriptor.as_deref(), Some(bytes.as_slice()));
        assert!(Arc::ptr_eq(&old, &filter(&first, "GrpcJsonTranscoder")));
    }
    let mut recreated = source;
    recreated.metadata.uid = Some("recreated".into());
    recreated.metadata.generation = Some(2);
    publisher.update_http_plugins(
        HashMap::from([("apps/plugins".into(), Arc::new(recreated))]),
        HashMap::new(),
        &HashSet::new(),
    );
    let recovered = view(&publisher);
    assert!(recovered.is_activatable);
    assert!(!Arc::ptr_eq(&old, &filter(&recovered, "GrpcJsonTranscoder")));
    assert_eq!(old.name(), "GrpcJsonTranscoder");
}

fn default_target_publisher(policy: Value) -> PluginPublisher {
    let publisher = publisher();
    let mut cfg = (*config("policy-a", policy)).clone();
    cfg.spec.global_plugins_ref.as_mut().unwrap()[0].namespace = None;
    publisher.replace_gateway_configs(HashMap::from([("policy-a".into(), Arc::new(cfg))]));
    publisher
}

fn default_source(bare: bool, valid: bool, uid: &str) -> Arc<EdgionPlugins> {
    let mut source = (*plugin("dsl", valid)).clone();
    source.metadata.namespace = (!bare).then(|| "default".into());
    source.metadata.uid = Some(uid.into());
    Arc::new(source)
}

#[test]
fn semantic_plugin_collision_clears_lkg_and_recovers_only_current_survivor() {
    for full in [false, true] {
        for bare_first in [false, true] {
            let publisher = default_target_publisher(json!({}));
            let first = if bare_first { "plugins" } else { "default/plugins" };
            let second = if bare_first { "default/plugins" } else { "plugins" };
            let good = default_source(bare_first, true, "old");
            publisher.replace_http_plugins(HashMap::from([(first.into(), good)]));
            let pinned = scope(&publisher, "apps/a");
            let old = pinned.http("default/plugins").unwrap();
            assert!(old.is_activatable);
            assert_eq!(old.spec.metadata.namespace.is_none(), bare_first);
            let invalid = default_source(bare_first, false, "new-uid");
            let sources = HashMap::from([(first.into(), invalid.clone())]);
            if full {
                publisher.replace_http_plugins(sources);
            } else {
                publisher.update_http_plugins(HashMap::new(), sources, &HashSet::new());
            }
            let retained = scope(&publisher, "apps/a").http("default/plugins").unwrap();
            assert!(
                retained.is_activatable,
                "same raw owner UID replacement retains eligible LKG"
            );
            assert_eq!(retained.spec.metadata.uid.as_deref(), Some("old"));
            let sibling = default_source(!bare_first, true, "new-uid");
            if full {
                publisher.replace_http_plugins(HashMap::from([
                    (first.into(), invalid.clone()),
                    (second.into(), sibling),
                ]));
            } else {
                publisher.update_http_plugins(
                    HashMap::from([(second.into(), sibling)]),
                    HashMap::new(),
                    &HashSet::new(),
                );
            }
            let denied = scope(&publisher, "apps/a").http("default/plugins").unwrap();
            assert!(!denied.is_activatable);
            assert!(!denied.retain_previous_on_invalid_update);
            assert!(denied.plugin_runtime.rejects_invalid_config());
            assert!(denied.spec.spec.request_plugins.is_none());
            assert!(denied.spec.spec.access_log_extern.is_none());
            assert_eq!(publisher.snapshot().http_sources().len(), 2);
            assert!(pinned.http("default/plugins").unwrap().is_activatable);
            if full {
                publisher.replace_http_plugins(HashMap::from([(first.into(), invalid)]));
            } else {
                publisher.update_http_plugins(HashMap::new(), HashMap::new(), &HashSet::from([second.into()]));
            }
            assert!(
                !scope(&publisher, "apps/a")
                    .http("default/plugins")
                    .unwrap()
                    .is_activatable,
                "invalid survivor cannot revive pre-collision LKG"
            );
            publisher.update_http_plugins(
                HashMap::new(),
                HashMap::from([(first.into(), default_source(bare_first, true, "fixed"))]),
                &HashSet::new(),
            );
            assert!(
                scope(&publisher, "apps/a")
                    .http("default/plugins")
                    .unwrap()
                    .is_activatable
            );
            publisher.replace_http_plugins(HashMap::new());
            assert!(scope(&publisher, "apps/a").http("default/plugins").is_none());
        }
    }
}

#[test]
fn collision_denial_survives_policy_bypass_and_raw_owner_replacement() {
    for action in ["bypass", "block"] {
        let publisher = default_target_publisher(json!({"deny":[{"name":"http/Dsl","action":action}]}));
        // Two identical specs still have distinct raw owners.
        let mut raw = (*default_source(true, true, "same")).clone();
        raw.spec.request_plugins = None;
        let bare = Arc::new(raw.clone());
        raw.metadata.namespace = Some("default".into());
        publisher.replace_http_plugins(HashMap::from([
            ("plugins".into(), bare),
            ("default/plugins".into(), Arc::new(raw)),
        ]));
        let denied = scope(&publisher, "apps/a").http("default/plugins").unwrap();
        assert!(denied.plugin_runtime.rejects_invalid_config());
        assert_eq!(
            denied.plugin_runtime.request_plugins_iter().next().unwrap().name(),
            "RejectInvalidConfig"
        );
    }
    let publisher = default_target_publisher(json!({}));
    publisher.replace_http_plugins(HashMap::from([("plugins".into(), default_source(true, true, "a"))]));
    publisher.update_http_plugins(
        HashMap::from([("default/plugins".into(), default_source(false, false, "b"))]),
        HashMap::new(),
        &HashSet::from(["plugins".into()]),
    );
    assert!(
        !scope(&publisher, "apps/a")
            .http("default/plugins")
            .unwrap()
            .is_activatable
    );
    assert_eq!(publisher.snapshot().http_sources().len(), 1);
    assert!(!publisher.snapshot().http_sources().contains_key("plugins"));
}

#[test]
fn default_namespace_rule_and_backend_collision_use_owner_namespace_and_pinned_scope() {
    let publisher = publisher();
    // The route lives in `default`; the listener admits routes from all namespaces.
    let cross_ns_gateway: Gateway = crate::test_current_status::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
        "metadata": {"name": "a", "namespace": "apps"},
        "spec": {"gatewayClassName": "a", "listeners": [{"name": "web", "port": 18080, "protocol": "HTTP",
            "allowedRoutes": {"namespaces": {"from": "All"}}}]},
    }))
    .unwrap();
    publisher.replace_gateways(HashMap::from([(
        "apps/a".into(),
        GatewayInput::from_gateway(&cross_ns_gateway),
    )]));
    let mut cfg = (*config("policy-a", json!({}))).clone();
    cfg.spec.global_plugins_ref = None;
    publisher.replace_gateway_configs(HashMap::from([("policy-a".into(), Arc::new(cfg))]));
    // ExtensionRefs resolve in the declaring route's namespace.
    let source: HTTPRoute = crate::test_current_status::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"namespace": "default", "name": "route"},
        "spec": {"resolvedStatusController": "edgion.io/gateway-controller",
            "parentRefs": [{"name": "a", "namespace": "apps"}], "resolvedListeners": ["0.0.0.0:18080/HTTP"],
            "rules": [{
                "filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"plugins"}}],
                "backendRefs":[{"name":"service","port":80,"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"plugins"}}]}]
            }]},
    }))
    .unwrap();
    let input = RouteInput::from_http(Arc::new(source));
    assert!(input.http_keys.contains("default/plugins"));
    assert!(!input.http_keys.contains("apps/plugins"));
    publisher.replace_routes(RouteKind::Http, HashMap::from([("default/route".into(), input)]));
    publisher.replace_http_plugins(HashMap::from([("plugins".into(), default_source(true, true, "old"))]));
    let mut old_request = crate::EdgionHttpContext::new();
    assert!(old_request.capture_plugin_scope(publisher.snapshot().gateway("apps/a").unwrap()));
    let old_scope = old_request.plugin_scope().unwrap().scope.clone();
    let old = old_request
        .pinned_backend_ext_ref("default/plugins", || {
            old_scope
                .http("default/plugins")
                .map(|view| view.plugin_runtime.clone())
        })
        .unwrap();
    assert!(!old.rejects_invalid_config());
    publisher.update_http_plugins(
        HashMap::from([("default/plugins".into(), default_source(false, true, "new"))]),
        HashMap::new(),
        &HashSet::new(),
    );
    let mut new_request = crate::EdgionHttpContext::new();
    assert!(new_request.capture_plugin_scope(publisher.snapshot().gateway("apps/a").unwrap()));
    let new_scope = new_request.plugin_scope().unwrap().scope.clone();
    let denied = new_request
        .pinned_backend_ext_ref("default/plugins", || {
            new_scope
                .http("default/plugins")
                .map(|view| view.plugin_runtime.clone())
        })
        .unwrap();
    assert!(denied.rejects_invalid_config());
    assert!(Arc::ptr_eq(
        &old,
        &old_request
            .pinned_backend_ext_ref("default/plugins", || panic!("old backend stays pinned"))
            .unwrap()
    ));
    publisher.update_http_plugins(
        HashMap::new(),
        HashMap::new(),
        &HashSet::from(["default/plugins".into()]),
    );
    assert!(
        scope(&publisher, "apps/a")
            .http("default/plugins")
            .unwrap()
            .is_activatable
    );
    assert!(Arc::ptr_eq(
        &denied,
        &new_request
            .pinned_backend_ext_ref("default/plugins", || panic!("denied backend stays pinned"))
            .unwrap()
    ));
}

#[test]
fn class_parameters_contract_is_shared_across_full_and_delta_repair() {
    for full in [false, true] {
        let publisher = publisher();
        let original = scope(&publisher, "apps/a");
        assert!(original.available);
        for (field, value) in [
            ("group", "wrong.io"),
            ("kind", "Secret"),
            ("name", "bad/name"),
            ("name", ""),
            ("namespace", ""),
            ("namespace", "default"),
        ] {
            let mut raw = serde_json::to_value(&*class("a", Some("policy-a"))).unwrap();
            raw["spec"]["parametersRef"][field] = json!(value);
            let invalid = Arc::new(serde_json::from_value(raw).unwrap());
            let classes = HashMap::from([("a".into(), invalid)]);
            if full {
                publisher.replace_gateway_classes(classes);
            } else {
                publisher.update_gateway_classes(classes, &HashSet::new());
            }
            assert!(!scope(&publisher, "apps/a").available, "{field}={value}");
            assert!(original.available, "captured scope remains immutable");
            let repaired = HashMap::from([("a".into(), class("a", Some("policy-a")))]);
            if full {
                publisher.replace_gateway_classes(repaired);
            } else {
                publisher.update_gateway_classes(repaired, &HashSet::new());
            }
            assert!(scope(&publisher, "apps/a").available);
        }
        if full {
            publisher.replace_gateway_configs(HashMap::new());
        } else {
            publisher.update_gateway_configs(HashMap::new(), &HashSet::from(["policy-a".into()]));
        }
        assert!(!scope(&publisher, "apps/a").available);
        let configs = HashMap::from([("policy-a".into(), config("policy-a", json!({})))]);
        if full {
            publisher.replace_gateway_configs(configs);
        } else {
            publisher.update_gateway_configs(configs, &HashSet::new());
        }
        assert!(scope(&publisher, "apps/a").available);
        publisher.replace_gateway_classes(HashMap::from([("a".into(), class("a", None))]));
        assert!(scope(&publisher, "apps/a").available);
    }
}

#[test]
fn invalid_waf_policy_identity_retains_prior_generation_and_pinned_view() {
    let publisher = publisher();
    update_source(&publisher, plugin("waf", true), false);
    let pinned = view(&publisher);
    let old = filter(&pinned, "Waf");
    let mut invalid = plugin("waf", true);
    Arc::make_mut(&mut invalid)
        .spec
        .waf
        .as_mut()
        .unwrap()
        .policy_ref
        .namespace = Some("".into());
    update_source(&publisher, invalid, false);
    assert!(Arc::ptr_eq(&old, &filter(&view(&publisher), "Waf")));
    assert!(Arc::ptr_eq(&old, &filter(&pinned, "Waf")));
    update_source(&publisher, plugin("waf", true), false);
    assert!(!Arc::ptr_eq(&old, &filter(&view(&publisher), "Waf")));
    assert!(Arc::ptr_eq(&old, &filter(&pinned, "Waf")));
}

fn graphql_router_source(second: &str, program: Option<&str>) -> Arc<EdgionPlugins> {
    let mut entries = json!([{"type":"GraphqlOperationRouter","config":{"rules":[
        {"operationType":"query","operationName":"GetUser","backendRef":{"name":"users"}},
        {"operationType":"mutation","operationName":"CreateOrder","backendRef":{"name":second}}
    ]}}]);
    if let Some(source) = program {
        entries
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"Dsl","config":{"name":"program","source":source}}));
    }
    Arc::new(
        serde_json::from_value(json!({"apiVersion":"edgion.io/v1","kind":"EdgionPlugins",
        "metadata":{"namespace":"apps","name":"plugins"},"spec":{"requestPlugins":entries}}))
        .unwrap(),
    )
}
fn graphql_router_route() -> Arc<HTTPRoute> {
    let mut source = (*route()).clone();
    source.spec.rules = Some(
        serde_json::from_value(json!([{"filters":[{"type":"ExtensionRef",
        "extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"plugins"}}],
        "backendRefs":[{"name":"users","port":80},{"name":"orders","port":81}]}]))
        .unwrap(),
    );
    Arc::new(source)
}
fn graphql_router_policy(publisher: &PluginPublisher, policy: Value) {
    let mut source = (*config("policy-a", policy)).clone();
    source.spec.global_plugins_ref = None;
    publisher.replace_gateway_configs(HashMap::from([("policy-a".into(), Arc::new(source))]));
}

#[test]
fn graphql_router_admission_revalidates_unselected_targets_and_pins_exact_route_generation() {
    let publisher = publisher();
    graphql_router_policy(&publisher, json!({}));
    let route = graphql_router_route();
    publisher.replace_routes(
        RouteKind::Http,
        HashMap::from([("apps/route".into(), RouteInput::from_http(route.clone()))]),
    );
    update_source(&publisher, graphql_router_source("orders", None), false);
    let old = scope(&publisher, "apps/a");
    assert!(old.graphql_rule(&route, 0));
    update_source(&publisher, graphql_router_source("undeclared", None), false);
    assert!(!scope(&publisher, "apps/a").graphql_rule(&route, 0));
    assert!(old.graphql_rule(&route, 0));
    let mut changed = (*route).clone();
    changed.spec.rules.as_mut().unwrap()[0]
        .backend_refs
        .as_mut()
        .unwrap()
        .push(serde_json::from_value(json!({"name":"undeclared","port":82})).unwrap());
    let changed = Arc::new(changed);
    publisher.update_routes(
        RouteKind::Http,
        HashMap::from([("apps/route".into(), RouteInput::from_http(changed.clone()))]),
        &HashSet::new(),
    );
    let new = scope(&publisher, "apps/a");
    assert!(new.graphql_rule(&changed, 0));
    assert!(!new.graphql_rule(&route, 0));
    assert!(!old.graphql_rule(&changed, 0));
}

#[tokio::test]
async fn graphql_router_masks_and_mixed_invalid_updates_preserve_current_policy() {
    let publisher = publisher();
    graphql_router_policy(&publisher, json!({}));
    let route = graphql_router_route();
    publisher.replace_routes(
        RouteKind::Http,
        HashMap::from([("apps/route".into(), RouteInput::from_http(route.clone()))]),
    );
    update_source(&publisher, graphql_router_source("undeclared", None), false);
    assert!(!scope(&publisher, "apps/a").graphql_rule(&route, 0));
    graphql_router_policy(
        &publisher,
        json!({"deny":[{"name":"http/GraphqlOperationRouter","action":"bypass"}]}),
    );
    let bypass = view(&publisher);
    assert!(!bypass.plugin_runtime.has_graphql_operation_router());
    assert!(!scope(&publisher, "apps/a").graphql_rule(&route, 0));
    update_source(&publisher, graphql_router_source("orders", None), false);
    assert!(scope(&publisher, "apps/a").graphql_rule(&route, 0));
    graphql_router_policy(
        &publisher,
        json!({"deny":[{"name":"http/GraphqlOperationRouter","blockStatus":451}]}),
    );
    assert!(matches!(
        first_result(&view(&publisher)).await,
        PluginRunningResult::ErrResponse { status: 451, .. }
    ));
    graphql_router_policy(&publisher, json!({}));
    update_source(&publisher, graphql_router_source("orders", Some("let a = 1")), false);
    let valid = view(&publisher);
    assert!(valid.is_activatable);
    update_source(
        &publisher,
        graphql_router_source("undeclared", Some("invalid @@@")),
        false,
    );
    let invalid = view(&publisher);
    assert!(!invalid.is_activatable);
    assert!(!Arc::ptr_eq(&valid, &invalid));
    assert!(!scope(&publisher, "apps/a").graphql_rule(&route, 0));
}

#[test]
fn graphql_router_binding_allows_bypassed_conflicting_publishers_but_not_undeclared_defaults() {
    let publisher = publisher();
    graphql_router_policy(&publisher, json!({}));
    let route = graphql_router_route();
    publisher.replace_routes(
        RouteKind::Http,
        HashMap::from([("apps/route".into(), RouteInput::from_http(route.clone()))]),
    );
    let mut source = (*graphql_router_source("orders", None)).clone();
    source.spec.request_plugins.as_mut().unwrap().push(
        serde_json::from_value(json!({"type":"DirectEndpoint","config":{"from":{"type":"header","name":"X-Target"}}}))
            .unwrap(),
    );
    update_source(&publisher, Arc::new(source), false);
    assert!(!scope(&publisher, "apps/a").graphql_rule(&route, 0));
    graphql_router_policy(
        &publisher,
        json!({"deny":[{"name":"http/DirectEndpoint","action":"bypass"}]}),
    );
    assert!(scope(&publisher, "apps/a").graphql_rule(&route, 0));
    let mut source = (*graphql_router_source("orders", None)).clone();
    if let edgion_resources::EdgionPlugin::GraphqlOperationRouter(config) =
        &mut source.spec.request_plugins.as_mut().unwrap()[0].plugin
    {
        config.default_backend_ref = Some(edgion_resources::plugin_configs::DynUpstreamTarget {
            name: "undeclared".into(),
            namespace: None,
        });
    }
    graphql_router_policy(
        &publisher,
        json!({"deny":[{"name":"http/GraphqlOperationRouter","action":"bypass"}]}),
    );
    update_source(&publisher, Arc::new(source), false);
    assert!(!scope(&publisher, "apps/a").graphql_rule(&route, 0));
}

#[test]
fn merged_graphql_bare_source_admission_collision_and_pins() {
    let publisher = publisher();
    graphql_router_policy(&publisher, json!({}));
    let mut allowed_gateway = gateway("a", "a", 18080, None);
    allowed_gateway.listeners[0].allowed_routes =
        Some(serde_json::from_value(json!({"namespaces":{"from":"All"}})).unwrap());
    publisher.replace_gateways(HashMap::from([("apps/a".into(), allowed_gateway)]));
    let route: HTTPRoute = crate::test_current_status::from_value(json!({"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute","metadata":{"namespace":"default","name":"route"},"spec":{"resolvedStatusController":"edgion.io/gateway-controller","parentRefs":[{"name":"a","namespace":"apps"}],"resolvedListeners":["0.0.0.0:18080/HTTP"],"rules":[{"filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":"plugins"}}],"backendRefs":[{"name":"users","port":80},{"name":"orders","port":81}]}]}})).unwrap();
    let route = Arc::new(route);
    publisher.replace_routes(
        RouteKind::Http,
        HashMap::from([("default/route".into(), RouteInput::from_http(route.clone()))]),
    );
    let source = |target: &str, namespace: Option<&str>| {
        let mut source = (*graphql_router_source(target, None)).clone();
        source.metadata.namespace = namespace.map(str::to_owned);
        Arc::new(source)
    };
    publisher.replace_http_plugins(HashMap::from([("plugins".into(), source("orders", None))]));
    let pinned = scope(&publisher, "apps/a");
    assert!(pinned.graphql_rule(&route, 0));
    publisher.replace_http_plugins(HashMap::from([("plugins".into(), source("undeclared", None))]));
    assert!(!scope(&publisher, "apps/a").graphql_rule(&route, 0));
    assert!(pinned.graphql_rule(&route, 0));
    publisher.replace_http_plugins(HashMap::from([
        ("plugins".into(), source("undeclared", None)),
        ("default/plugins".into(), source("orders", Some("default"))),
    ]));
    let collision = scope(&publisher, "apps/a").http("default/plugins").unwrap();
    assert!(!collision.is_activatable);
    publisher.replace_http_plugins(HashMap::from([("plugins".into(), source("undeclared", None))]));
    assert!(!scope(&publisher, "apps/a").graphql_rule(&route, 0));
    assert!(pinned.graphql_rule(&route, 0));
}

#[test]
fn actual_global_fields_preserve_siblings_missing_drop_and_pinned_scope() {
    let publisher = publisher();
    publisher.replace_http_plugins(HashMap::from([
        ("apps/plugins".into(), plugin("dsl", true)),
        ("/decoy".into(), plugin("dsl", true)),
        ("apps/bad/name".into(), plugin("dsl", true)),
    ]));
    let mut cfg = (*config("policy-a", json!({}))).clone();
    cfg.spec.global_plugins_ref = Some(
        serde_json::from_value(json!([
            {"namespace":"apps","name":"plugins"},
            {"namespace":"","name":"decoy"},
            {"namespace":"apps","name":"bad/name"},
            {"name":"missing"},
            {"namespace":"apps","name":"plugins"}
        ]))
        .unwrap(),
    );
    publisher.replace_gateway_configs(HashMap::from([("policy-a".into(), Arc::new(cfg.clone()))]));
    let pinned = scope(&publisher, "apps/a");
    assert_eq!(pinned.global_keys, ["apps/plugins", "default/missing", "apps/plugins"]);
    assert!(pinned.http("apps/plugins").unwrap().is_activatable);
    assert!(pinned.http("default/missing").is_none());
    assert!(pinned.http("/decoy").is_none());
    assert!(pinned.http("apps/bad/name").is_none());
    cfg.metadata.namespace = Some("".into());
    publisher.replace_gateway_configs(HashMap::from([("policy-a".into(), Arc::new(cfg.clone()))]));
    let invalid = scope(&publisher, "apps/a");
    assert!(invalid.global_keys.is_empty());
    assert!(invalid.http("apps/plugins").is_none());
    assert!(pinned.http("apps/plugins").unwrap().is_activatable);
    cfg.metadata.namespace = None;
    publisher.replace_gateway_configs(HashMap::from([("policy-a".into(), Arc::new(cfg))]));
    assert!(scope(&publisher, "apps/a").http("apps/plugins").unwrap().is_activatable);
}

#[test]
fn actual_gateway_owner_cannot_borrow_same_name_plugin_from_live_inventory() {
    let publisher = publisher();
    let mut cfg = (*config("policy-a", json!({}))).clone();
    cfg.spec.global_plugins_ref = None;
    publisher.replace_gateway_configs(HashMap::from([("policy-a".into(), Arc::new(cfg))]));
    publisher.replace_http_plugins(HashMap::from([("apps/plugins".into(), plugin("dsl", true))]));
    let mut owner: Gateway = crate::test_current_status::from_value(json!({
        "apiVersion":"gateway.networking.k8s.io/v1", "kind":"Gateway",
        "metadata":{"name":"a", "namespace":"apps", "annotations":{
            "edgion.io/edgion-plugins":"plugins missing /bad"}},
        "spec":{"gatewayClassName":"a", "listeners":[{"name":"web","port":18080,"protocol":"HTTP"}]}
    }))
    .unwrap();
    publisher.replace_gateways(HashMap::from([("apps/a".into(), GatewayInput::from_gateway(&owner))]));
    let pinned = scope(&publisher, "apps/a");
    assert!(pinned.http("apps/plugins").unwrap().is_activatable);
    assert!(pinned.http("apps/missing").is_none());
    owner.metadata.name = None;
    publisher.replace_gateways(HashMap::from([("apps/a".into(), GatewayInput::from_gateway(&owner))]));
    assert!(scope(&publisher, "apps/a").http("apps/plugins").is_none());
    assert!(pinned.http("apps/plugins").unwrap().is_activatable);
    assert_eq!(publisher.snapshot().http_sources().len(), 1);
}

#[test]
fn configured_invalid_stream_gateway_never_activates_executable_empty_key_decoy() {
    let publisher = publisher();
    let listener = ListenerKey::new("0.0.0.0:18080".parse().unwrap(), "HTTP");
    let executable = stream_source();
    publisher.replace_stream_plugins(HashMap::from([
        ("".into(), executable.clone()),
        ("apps/stream".into(), executable),
    ]));
    publisher.replace_gateways(HashMap::from([(
        "apps/a".into(),
        gateway("a", "a", 18080, Some("stream")),
    )]));
    let pinned = publisher.snapshot();
    assert!(matches!(pinned.connection(&listener), ConnectionView::Ready(_)));
    assert!(scope(&publisher, "apps/a").stream("apps/stream").is_some());
    for policy in [
        json!({}),
        json!({"deny":[{"name":"stream/ConnectionRateLimit","action":"bypass"}]}),
    ] {
        publisher.replace_gateway_configs(HashMap::from([("policy-a".into(), config("policy-a", policy))]));
        for invalid_owner in [false, true] {
            let mut owner:Gateway=crate::test_current_status::from_value(json!({
                "metadata":{"name":"a","namespace":"apps","annotations":{"edgion.io/edgion-stream-plugins":if invalid_owner {"stream"} else {"apps/stream"}}},
                "spec":{"gatewayClassName":"a","listeners":[{"name":"web","port":18080,"protocol":"HTTP"}]}
            })).unwrap();
            if invalid_owner {
                owner.metadata.name = None;
            }
            publisher.replace_gateways(HashMap::from([("apps/a".into(), GatewayInput::from_gateway(&owner))]));
            assert!(matches!(
                publisher.snapshot().connection(&listener),
                ConnectionView::Unavailable
            ));
            assert!(scope(&publisher, "apps/a").stream("").is_none());
            assert_eq!(publisher.snapshot().stats().stream_cached_entries, 0);
            for invalid_first in [false, true] {
                let (invalid_key, valid_key) = if invalid_first {
                    ("apps/a", "apps/z")
                } else {
                    ("apps/z", "apps/a")
                };
                publisher.replace_gateways(HashMap::from([
                    (invalid_key.into(), GatewayInput::from_gateway(&owner)),
                    (valid_key.into(), gateway("valid", "a", 18080, Some("stream"))),
                ]));
                assert!(matches!(
                    publisher.snapshot().connection(&listener),
                    ConnectionView::Unavailable
                ));
                assert!(scope(&publisher, invalid_key).stream("").is_none());
            }

            assert!(matches!(pinned.connection(&listener), ConnectionView::Ready(_)));
        }
    }
    publisher.replace_gateway_configs(HashMap::from([("policy-a".into(), config("policy-a", json!({})))]));
    publisher.replace_gateways(HashMap::from([(
        "apps/a".into(),
        gateway("a", "a", 18080, Some("stream")),
    )]));
    assert!(matches!(
        publisher.snapshot().connection(&listener),
        ConnectionView::Ready(_)
    ));
    publisher.replace_gateways(HashMap::from([
        ("apps/a".into(), gateway("a", "a", 18080, Some("stream"))),
        ("apps/z".into(), gateway("z", "a", 18080, Some("stream"))),
    ]));
    assert!(matches!(
        publisher.snapshot().connection(&listener),
        ConnectionView::Ready(_)
    ));
}

#[test]
fn invalid_l4_owner_has_no_executable_decoy_demand_and_valid_lexical_key_recovers() {
    for kind in ["TCPRoute", "UDPRoute", "TLSRoute"] {
        let publisher = publisher();
        let mut executable = (*stream_source()).clone();
        executable.spec.tls_route_plugins=Some(serde_json::from_value(json!([{"type":"IpRestriction","config":{"ipSource":"DirectPeerIp","deny":[{"name":"blocked","cidrs":["192.0.2.0/24"]}]}}])).unwrap());
        let executable = Arc::new(executable);
        publisher.replace_stream_plugins(HashMap::from([
            ("".into(), executable.clone()),
            ("apps/stream".into(), executable),
        ]));
        let make_input = |invalid| {
            let mut raw = json!({"metadata":{"name":"r","namespace":"apps","annotations":{"edgion.io/edgion-stream-plugins":"apps/stream"}},
                "spec":{"resolvedStatusController":"edgion.io/gateway-controller","parentRefs":[{"name":"a","sectionName":"web"}],"resolvedListeners":["0.0.0.0:18080/HTTP"],"rules":[{}]}});
            if invalid {
                raw["metadata"]["name"] = serde_json::Value::Null;
            }
            match kind {
                "TCPRoute" => RouteInput::from_tcp(&crate::test_current_status::from_value(raw).unwrap()),
                "UDPRoute" => RouteInput::from_udp(&crate::test_current_status::from_value(raw).unwrap()),
                _ => {
                    raw["spec"]["hostnames"] = json!(["example.com"]);
                    raw["spec"]["resolvedListeners"] = json!([{"listenerKey":"0.0.0.0:18080/HTTP","gatewayNamespace":"apps","gatewayName":"a","listenerName":"web","hostnames":["example.com"]}]);
                    RouteInput::from_tls(&crate::test_current_status::from_value(raw).unwrap())
                }
            }
        };
        let valid = make_input(false);
        let route_kind = valid.kind;
        publisher.replace_routes(route_kind, HashMap::from([("apps/r".into(), valid)]));
        let pinned = scope(&publisher, "apps/a");
        let active = pinned.stream("apps/stream").unwrap();
        if kind == "TLSRoute" {
            assert!(!active.tls_route_plugin_runtime.is_empty());
        } else {
            assert!(!active.stream_plugin_runtime.is_empty());
        }
        let invalid = make_input(true);
        assert_eq!(invalid.stream_key.as_deref(), Some(""));
        publisher.replace_routes(route_kind, HashMap::from([("apps/r".into(), invalid)]));
        assert!(scope(&publisher, "apps/a").stream("").is_none());
        assert_eq!(publisher.snapshot().stats().stream_cached_entries, 0);
        assert!(pinned.stream("apps/stream").is_some());
        publisher.replace_routes(route_kind, HashMap::from([("apps/r".into(), make_input(false))]));
        assert!(scope(&publisher, "apps/a").stream("apps/stream").is_some());
    }
}

// Existing policy-only fixtures submit empty compiled tables. Request-lifetime
// regressions below use the actual route manager commit and matching paths.
trait PolicyRouteFixture {
    fn replace_routes(&self, kind: RouteKind, incoming: HashMap<String, RouteInput>);
    fn update_routes(&self, kind: RouteKind, incoming: HashMap<String, RouteInput>, removed: &HashSet<String>);
}

impl PolicyRouteFixture for PluginPublisher {
    fn replace_routes(&self, kind: RouteKind, incoming: HashMap<String, RouteInput>) {
        match kind {
            RouteKind::Http => self.replace_http_routes(incoming, HashMap::new()),
            RouteKind::Grpc => self.replace_grpc_routes(incoming, HashMap::new()),
            _ => self.replace_stream_routes(kind, incoming),
        }
    }
    fn update_routes(&self, kind: RouteKind, incoming: HashMap<String, RouteInput>, removed: &HashSet<String>) {
        match kind {
            RouteKind::Http => self.update_http_routes(incoming, removed, HashMap::new()),
            RouteKind::Grpc => self.update_grpc_routes(incoming, removed, HashMap::new()),
            _ => self.update_stream_routes(kind, incoming, removed),
        }
    }
}

#[test]
fn same_listener_key_updates_complete_gateway_entry_and_reuses_unchanged_view() {
    let publisher = publisher();
    let key = ListenerKey("0.0.0.0:18080/HTTP".into());
    let before = publisher.snapshot().http_listener(&key);
    let old = before.index.get("apps/a", "web").unwrap();
    assert!(old.info.metrics_test_key.is_none());
    let mut source: Gateway = crate::test_current_status::from_value(json!({
        "metadata":{"namespace":"apps","name":"a","uid":"replacement","annotations":{
            "edgion.io/metrics-test-key":"coherent-metrics",
            "edgion.io/metrics-test-type":"integration",
            "edgion.io/require-sni-host-match":"true",
            "edgion.io/dns-resolver":"127.0.0.1:53"}},
        "spec":{"gatewayClassName":"a","listeners":[{"name":"web","protocol":"HTTP","port":18080}]}
    }))
    .unwrap();
    source.spec.resolved_attachment_proof = Some(edgion_resources::GatewayAttachmentProof {
        instance: "controller-a".into(),
        generation: 7,
        epoch: "epoch-a".into(),
        attachment_digest: "a".repeat(64),
    });
    publisher.update_gateways(
        HashMap::from([("apps/a".into(), GatewayInput::from_gateway(&source))]),
        &HashSet::new(),
    );
    let after = publisher.snapshot().http_listener(&key);
    let entry = after.index.get("apps/a", "web").unwrap();
    assert!(!Arc::ptr_eq(&before, &after));
    assert_eq!(entry.info.metrics_test_key.as_deref(), Some("coherent-metrics"));
    assert_eq!(entry.info.require_sni_host_match, Some(true));
    assert!(entry.info.metrics_test_type.is_some());
    assert!(entry.info.dns_resolver_candidate.is_some());
    assert_ne!(entry.gateway_proof, old.gateway_proof);
    assert!(
        old.info.metrics_test_key.is_none(),
        "the captured index remains immutable"
    );
    publisher.update_gateways(
        HashMap::from([("apps/a".into(), GatewayInput::from_gateway(&source))]),
        &HashSet::new(),
    );
    assert!(Arc::ptr_eq(&after, &publisher.snapshot().http_listener(&key)));
    source.spec.listeners.as_mut().unwrap()[0].port = 18081;
    publisher.replace_gateways(HashMap::from([("apps/a".into(), GatewayInput::from_gateway(&source))]));
    assert!(publisher.snapshot().http_listener(&key).index.is_empty());
    let moved = publisher
        .snapshot()
        .http_listener(&ListenerKey("0.0.0.0:18081/HTTP".into()));
    assert_eq!(moved.index.get("apps/a", "web").unwrap().port, 18081);
    assert_eq!(after.index.get("apps/a", "web").unwrap().port, 18080);
}

impl super::HttpListenerPublication {
    /// A cold fixture for policy-only tests that do not enter route matching.
    pub(crate) fn fixture_for_scope(scope: Arc<super::GatewayScope>) -> Arc<Self> {
        let empty = Self::empty();
        Self::new(
            (empty.http_manager.clone(), empty.http.clone()),
            (empty.grpc_manager.clone(), empty.grpc.clone()),
            empty.index.clone(),
            HashMap::from([(scope.gateway_key.clone(), scope)]),
        )
    }
}

#[tokio::test]
async fn typed_ip_invalid_input_replaces_committed_runtime_and_clean_generation_recovers() {
    for full in [true, false] {
        for removed in ["ips", "resolvedIps", "unknownIpField"] {
            let publisher = publisher();
            let mut wire = json!({
                "metadata":{"name":"plugins","namespace":"apps"},
                "spec":{"requestPlugins":[{"type":"RequestHeaderModifier","config":{"set":[{"name":"X-Test","value":"ok"}]},
                    "conditions":{"run":{"allOf":[{"type":"remoteIpMatch","ipRefs":[{"name":"nets"}]}]}}
                }]}
            });
            let clean: EdgionPlugins = serde_json::from_value(wire.clone()).unwrap();
            update_source(&publisher, Arc::new(clean.clone()), full);
            let before = view(&publisher);
            assert!(before.is_activatable, "temporary list absence is not a cold seal");
            wire["spec"]["requestPlugins"][0]["conditions"]["run"]["allOf"][0][removed] = json!(["192.0.2.0/24"]);
            wire["spec"]["requestPlugins"][0]["conditions"]["run"]["allOf"][0]["invalidInput"] = json!(false);
            let invalid: EdgionPlugins = serde_json::from_value(wire).unwrap();
            let transport = serde_json::to_value(invalid).unwrap();
            assert!(transport["spec"]["requestPlugins"][0]["conditions"]["run"]["allOf"][0]
                .get(removed)
                .is_none());
            let receiver: EdgionPlugins = serde_json::from_value(transport).unwrap();
            update_source(&publisher, Arc::new(receiver), full);
            let rejected = view(&publisher);
            assert!(!rejected.is_activatable);
            assert!(!Arc::ptr_eq(&before, &rejected));
            assert!(matches!(
                first_result(&rejected).await,
                PluginRunningResult::ErrResponse { status: 500, .. }
            ));
            update_source(&publisher, Arc::new(clean), full);
            let recovered = view(&publisher);
            assert!(recovered.is_activatable);
            assert!(!Arc::ptr_eq(&rejected, &recovered));
        }
    }
}

#[tokio::test]
async fn primary_rap_negate_presence_replaces_runtime_instead_of_retaining_ignored_configuration() {
    for full in [true, false] {
        for negate in [json!(false), json!(true), json!(null)] {
            let publisher = publisher();
            let profile = json!({"requiredRuleGroups":[{"name":"clients","anyOfRules":[{"name":"peer","type":"directPeerIp","config":{"ipRefs":[{"name":"nets"}]}}]}]});
            let mut wire = json!({"metadata":{"name":"plugins","namespace":"apps"},"spec":{"requestPlugins":[{
                "type":"RequestAccessPolicy","config":{"defaultProfile":"main","profiles":{"main":profile},"resolvedProfiles":{"main":profile}}
            }]}});
            let clean: EdgionPlugins = serde_json::from_value(wire.clone()).unwrap();
            update_source(&publisher, Arc::new(clean.clone()), full);
            assert!(view(&publisher).is_activatable);
            wire["spec"]["requestPlugins"][0]["config"]["profiles"]["main"]["requiredRuleGroups"][0]["anyOfRules"][0]
                ["config"]["negate"] = negate.clone();
            wire["spec"]["requestPlugins"][0]["config"]["resolvedProfiles"]["main"]["requiredRuleGroups"][0]
                ["anyOfRules"][0]["config"]["negate"] = negate;
            let invalid: EdgionPlugins = serde_json::from_value(wire).unwrap();
            let receiver: EdgionPlugins = serde_json::from_value(serde_json::to_value(invalid).unwrap()).unwrap();
            update_source(&publisher, Arc::new(receiver), full);
            let rejected = view(&publisher);
            assert!(!rejected.is_activatable);
            assert!(matches!(
                first_result(&rejected).await,
                PluginRunningResult::ErrResponse { status: 500, .. }
            ));
            update_source(&publisher, Arc::new(clean), full);
            assert!(view(&publisher).is_activatable);
        }
    }
}

#[test]
fn real_ip_legacy_presence_replaces_previous_runtime_and_clean_publication_recovers() {
    for full in [false, true] {
        let publisher = publisher();
        let make = |legacy: Option<Value>| {
            let mut config = json!({"trustedIpRefs": [{"name": "proxies"}]});
            if let Some(value) = legacy {
                config["trustedIps"] = value;
            }
            let source: EdgionPlugins = serde_json::from_value(json!({
                "metadata": {"name": "plugins", "namespace": "apps"},
                "spec": {"requestPlugins": [{"type": "RealIp", "config": config}]}
            }))
            .unwrap();
            let transported = serde_json::to_value(source).unwrap();
            Arc::new(serde_json::from_value(transported).unwrap())
        };
        update_source(&publisher, make(None), full);
        let good = view(&publisher);
        assert!(good
            .plugin_runtime
            .request_plugins_iter()
            .any(|filter| filter.name() == "RealIp"));
        for legacy in [Value::Null, json!([])] {
            update_source(&publisher, make(Some(legacy)), full);
            let invalid = view(&publisher);
            assert!(!Arc::ptr_eq(&good, &invalid));
            assert!(!invalid
                .plugin_runtime
                .request_plugins_iter()
                .any(|filter| filter.name() == "RealIp"));
            assert!(invalid
                .plugin_runtime
                .request_plugins_iter()
                .any(|filter| filter.name() == "RejectInvalidConfig"));
        }
        update_source(&publisher, make(None), full);
        let recovered = view(&publisher);
        assert!(recovered
            .plugin_runtime
            .request_plugins_iter()
            .any(|filter| filter.name() == "RealIp"));
        assert!(!recovered
            .plugin_runtime
            .request_plugins_iter()
            .any(|filter| filter.name() == "RejectInvalidConfig"));
    }
}

#[tokio::test]
async fn geo_removed_and_typo_fields_replace_previous_runtime_and_clean_generation_recovers() {
    for full in [false, true] {
        let publisher = publisher();
        let make = |field: Option<(&str, Value)>| {
            let mut rule = json!({"source":"DirectPeerIp","key":"cidr","ipRefs":[{"name":"clients"}]});
            if let Some((name, value)) = field {
                rule[name] = value;
            }
            let source:EdgionPlugins=serde_json::from_value(json!({"metadata":{"name":"plugins","namespace":"apps"},"spec":{"requestPlugins":[{"type":"GeoIpLocation","config":{"failOpen":true,"allowRuleGroups":[{"rules":[rule]}]}}]}})).unwrap();
            Arc::new(serde_json::from_value(serde_json::to_value(source).unwrap()).unwrap())
        };
        update_source(&publisher, make(None), full);
        let good = view(&publisher);
        assert!(good
            .plugin_runtime
            .request_plugins_iter()
            .any(|filter| filter.name() == "GeoIpLocation"));
        for (name, value) in [
            ("values", Value::Null),
            ("values", json!([])),
            ("values", json!(["10.0.0.0/8"])),
            ("geoipRefs", Value::Null),
            ("ipRefz", json!([])),
        ] {
            update_source(&publisher, make(Some((name, value))), full);
            let invalid = view(&publisher);
            assert!(!Arc::ptr_eq(&good, &invalid));
            assert!(!invalid
                .plugin_runtime
                .request_plugins_iter()
                .any(|filter| filter.name() == "GeoIpLocation"));
            assert!(matches!(
                first_result(&invalid).await,
                PluginRunningResult::ErrResponse { status: 500, .. }
            ));
        }
        update_source(&publisher, make(None), full);
        let recovered = view(&publisher);
        assert!(recovered
            .plugin_runtime
            .request_plugins_iter()
            .any(|filter| filter.name() == "GeoIpLocation"));
        assert!(!recovered
            .plugin_runtime
            .request_plugins_iter()
            .any(|filter| filter.name() == "RejectInvalidConfig"));
        assert!(
            good.plugin_runtime
                .request_plugins_iter()
                .any(|filter| filter.name() == "GeoIpLocation"),
            "held old generation remains intact"
        );
    }
}

#[test]
fn geo_stream_typed_invalid_replaces_cached_policy_and_clean_publication_restores_forwarding() {
    use crate::plugins::stream::{StreamContext, StreamLayer, StreamPluginResult};
    use futures::FutureExt;
    let _lock = crate::geoip::GEO_TEST_LOCK.lock();
    for full in [false, true] {
        let publisher = publisher();
        publisher.replace_gateways(HashMap::from([(
            "apps/a".into(),
            gateway("a", "a", 18080, Some("stream")),
        )]));
        let listener = ListenerKey::new("0.0.0.0:18080".parse().unwrap(), "HTTP");
        let make = |invalid: bool| {
            let mut config = json!({"failOpen":true,"denyRuleGroups":[{"rules":[{"source":"DirectPeerIp","key":"country","values":["ZZ"]}]}]});
            if invalid {
                config["denyRuleGroup"] = Value::Null;
            }
            let source:EdgionStreamPlugins=serde_json::from_value(json!({"metadata":{"name":"stream","namespace":"apps"},"spec":{"plugins":[{"type":"GeoIpLocation","config":config}]}})).unwrap();
            Arc::new(serde_json::from_value(serde_json::to_value(source).unwrap()).unwrap())
        };
        for invalid in [false, true, false] {
            let incoming = HashMap::from([("apps/stream".into(), make(invalid))]);
            if full {
                publisher.replace_stream_plugins(incoming);
            } else {
                publisher.update_stream_plugins(incoming, &HashSet::new());
            }
            let ConnectionView::Ready(runtime) = publisher.snapshot().connection(&listener) else {
                panic!("existing listener must remain bound");
            };
            let ctx = StreamContext::new("192.0.2.1".parse().unwrap(), 18080, StreamLayer::Stage1);
            let result = runtime
                .stream_plugin_runtime
                .run(&ctx)
                .now_or_never()
                .expect("synchronous Geo connection decision");
            assert_eq!(matches!(result, StreamPluginResult::Deny(_)), invalid);
        }
    }
}

#[test]
fn acl_actual_publication_replaces_legacy_generation_and_clean_recovers() {
    let publisher = publisher();
    let value = json!({"metadata":{"name":"plugins","namespace":"apps"},"spec":{"requestPlugins":[{
        "type":"IpRestriction","config":{"allowRefs":[{"name":"networks"}]}
    }]}});
    let clean: Arc<EdgionPlugins> = Arc::new(serde_json::from_value(value.clone()).unwrap());
    update_source(&publisher, clean.clone(), true);
    let held = view(&publisher);
    assert!(held.is_activatable);
    assert_eq!(
        held.plugin_runtime.request_plugins_iter().next().unwrap().name(),
        "IpRestriction"
    );
    for (field, payload) in [("allow", Value::Null), ("deny", json!([]))] {
        let mut invalid = value.clone();
        invalid["spec"]["requestPlugins"][0]["config"][field] = payload;
        let typed: EdgionPlugins = serde_json::from_value(invalid).unwrap();
        let received = Arc::new(serde_json::from_str(&serde_json::to_string(&typed).unwrap()).unwrap());
        update_source(&publisher, received, false);
        let rejected = view(&publisher);
        assert!(!Arc::ptr_eq(&held, &rejected));
        assert_eq!(
            rejected.plugin_runtime.request_plugins_iter().next().unwrap().name(),
            "RejectInvalidConfig"
        );
        // An in-flight retained view remains immutable while current callers
        // receive the rejected generation; a clean new publication recovers.
        assert_eq!(
            held.plugin_runtime.request_plugins_iter().next().unwrap().name(),
            "IpRestriction"
        );
        update_source(&publisher, clean.clone(), false);
        let recovered = view(&publisher);
        assert!(recovered.is_activatable);
        assert_eq!(
            recovered.plugin_runtime.request_plugins_iter().next().unwrap().name(),
            "IpRestriction"
        );
    }
}

#[tokio::test]
async fn acl_stream_actual_publication_replaces_invalid_generation_and_clean_recovers() {
    use crate::plugins::stream::{StreamContext, StreamLayer, StreamPluginResult};
    for kind in ["IpRestriction", "GlobalConnectionIpRestriction"] {
        let publisher = publisher();
        publisher.replace_gateways(HashMap::from([(
            "apps/a".into(),
            gateway("a", "a", 18080, Some("stream")),
        )]));
        let mut cfg = json!({"defaultAction":"allow","denyRefs":[{"name":"unavailable-publication-deny"}]});
        if kind == "GlobalConnectionIpRestriction" {
            cfg = json!({"enable":true,"activeProfile":"base","profiles":{"base":cfg}});
        }
        let authored =
            json!({"metadata":{"name":"stream","namespace":"apps"},"spec":{"plugins":[{"type":kind,"config":cfg}]}});
        let clean: Arc<EdgionStreamPlugins> = Arc::new(serde_json::from_value(authored.clone()).unwrap());
        publisher.replace_stream_plugins(HashMap::from([("apps/stream".into(), clean.clone())]));
        let held = scope(&publisher, "apps/a").stream("apps/stream").unwrap();
        let context = StreamContext::new("192.0.2.1".parse().unwrap(), 18080, StreamLayer::Stage1);
        assert!(matches!(
            held.stream_plugin_runtime.run(&context).await,
            StreamPluginResult::Allow
        ));
        for payload in [Value::Null, json!([])] {
            let mut bad = authored.clone();
            let config = &mut bad["spec"]["plugins"][0]["config"];
            if kind == "IpRestriction" {
                config["deny"] = payload;
            } else {
                config["profiles"]["base"]["deny"] = payload;
            }
            let typed: EdgionStreamPlugins = serde_json::from_value(bad).unwrap();
            let wire = serde_json::to_string(&typed).unwrap();
            publisher.replace_stream_plugins(HashMap::from([(
                "apps/stream".into(),
                Arc::new(serde_json::from_str(&wire).unwrap()),
            )]));
            let rejected = scope(&publisher, "apps/a").stream("apps/stream").unwrap();
            assert!(!Arc::ptr_eq(&held, &rejected));
            assert!(matches!(
                rejected.stream_plugin_runtime.run(&context).await,
                StreamPluginResult::Deny(_)
            ));
            assert!(matches!(
                held.stream_plugin_runtime.run(&context).await,
                StreamPluginResult::Allow
            ));
            assert_eq!(publisher.snapshot().stats().stream_cached_entries, 0);
            publisher.replace_stream_plugins(HashMap::from([("apps/stream".into(), clean.clone())]));
            let recovered = scope(&publisher, "apps/a").stream("apps/stream").unwrap();
            assert!(matches!(
                recovered.stream_plugin_runtime.run(&context).await,
                StreamPluginResult::Allow
            ));
        }
    }
    // The accepted B4 static-before-cache gate likewise publishes a denial,
    // never retains a constructor Err, and permits a clean source to recover.
    let publisher = publisher();
    publisher.replace_gateways(HashMap::from([(
        "apps/a".into(),
        gateway("a", "a", 18080, Some("stream")),
    )]));
    publisher.replace_stream_plugins(HashMap::from([("apps/stream".into(),Arc::new(serde_json::from_value(json!({"metadata":{"name":"stream","namespace":"apps"},"spec":{"plugins":[{"type":"GeoIpLocation","config":{}}]}})).unwrap()))]));
    let invalid = scope(&publisher, "apps/a").stream("apps/stream").unwrap();
    let context = StreamContext::new("192.0.2.1".parse().unwrap(), 18080, StreamLayer::Stage1);
    assert!(matches!(
        invalid.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Deny(_)
    ));
    assert_eq!(publisher.snapshot().stats().stream_cached_entries, 0);
    publisher.replace_stream_plugins(HashMap::from([("apps/stream".into(),Arc::new(serde_json::from_value(json!({"metadata":{"name":"stream","namespace":"apps"},"spec":{"plugins":[{"type":"IpRestriction","config":{"defaultAction":"allow","denyRefs":[{"name":"unavailable-publication-deny"}]}}]}})).unwrap()))]));
    assert!(matches!(
        scope(&publisher, "apps/a")
            .stream("apps/stream")
            .unwrap()
            .stream_plugin_runtime
            .run(&context)
            .await,
        StreamPluginResult::Allow
    ));
}

// ---------------------------------------------------------------------------
// Route delegation: child routes contribute to every Gateway scope (spec §6.4)
// ---------------------------------------------------------------------------

/// Publisher with two Gateway scopes: class `a` (policy-a, empty) and class
/// `b` (policy-b, `policy_b`).
fn two_scope_publisher(policy_b: Value) -> PluginPublisher {
    let publisher = publisher();
    let mut cfg = (*config("policy-a", json!({}))).clone();
    cfg.spec.global_plugins_ref = None;
    publisher.replace_gateway_configs(HashMap::from([
        ("policy-a".into(), Arc::new(cfg)),
        ("policy-b".into(), {
            let mut cfg = (*config("policy-b", policy_b)).clone();
            cfg.spec.global_plugins_ref = None;
            Arc::new(cfg)
        }),
    ]));
    publisher.update_gateway_classes(
        HashMap::from([("b".into(), class("b", Some("policy-b")))]),
        &HashSet::new(),
    );
    publisher.update_gateways(
        HashMap::from([("apps/b".into(), gateway("b", "b", 18081, None))]),
        &HashSet::new(),
    );
    publisher
}

fn plugins_extension_ref() -> Value {
    json!({"type": "ExtensionRef", "extensionRef": {"group": "edgion.io", "kind": "EdgionPlugins", "name": "plugins"}})
}

/// A delegation child of `apps/parent`: route-kind parentRef only, no
/// listeners, carrying the Controller-confirmed delegation key.
fn delegation_child(rules: Value) -> Arc<HTTPRoute> {
    use edgion_resources::resources::route_delegation::{
        key_for, DelegationTarget, DelegationTargetName, RouteKindName,
    };
    let mut route: HTTPRoute = crate::test_current_status::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"namespace": "apps", "name": "child"},
        "spec": {"resolvedStatusController": "edgion.io/gateway-controller",
            "parentRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "parent"}],
            "rules": rules},
    }))
    .unwrap();
    route.spec.resolved_delegation_keys = vec![key_for(
        RouteKindName::HTTPRoute,
        "apps",
        "parent",
        &DelegationTarget {
            ns: "apps".into(),
            name: DelegationTargetName::Named("child".into()),
        },
    )];
    Arc::new(route)
}

fn publish_http(publisher: &PluginPublisher, routes: &[&Arc<HTTPRoute>]) {
    publisher.replace_routes(
        RouteKind::Http,
        routes
            .iter()
            .map(|route| {
                (
                    format!("apps/{}", route.metadata.name.as_deref().unwrap()),
                    RouteInput::from_http(Arc::clone(route)),
                )
            })
            .collect(),
    );
}

#[test]
fn child_plugin_refs_compiled_in_every_gateway_scope() {
    let publisher = two_scope_publisher(json!({}));
    publisher.replace_http_plugins(HashMap::from([("apps/plugins".into(), plugin("dsl", true))]));
    let child = delegation_child(json!([{
        "filters": [plugins_extension_ref()],
        "backendRefs": [{"name": "service", "port": 80}],
    }]));
    publish_http(&publisher, &[&child]);
    let a = scope(&publisher, "apps/a")
        .http("apps/plugins")
        .expect("child ref in scope a");
    let b = scope(&publisher, "apps/b")
        .http("apps/plugins")
        .expect("child ref in scope b");
    assert!(a.is_activatable && b.is_activatable);
    // Each scope admits the child's rule (no AI / GraphQL owner on it).
    assert!(scope(&publisher, "apps/a").graphql_rule(&child, 0));
    assert!(scope(&publisher, "apps/b").graphql_rule(&child, 0));

    // A child whose delegation key is withdrawn is no longer published anywhere.
    let mut orphan = (*child).clone();
    orphan.spec.resolved_delegation_keys.clear();
    let orphan = Arc::new(orphan);
    publish_http(&publisher, &[&orphan]);
    assert!(scope(&publisher, "apps/a").http("apps/plugins").is_none());
    assert!(scope(&publisher, "apps/b").http("apps/plugins").is_none());
}

#[tokio::test]
async fn child_plugin_denied_by_gateway_policy_in_that_scope_only() {
    let publisher = two_scope_publisher(json!({"deny": [{"name": "http/RateLimitLocal", "blockStatus": 451}]}));
    publisher.replace_http_plugins(HashMap::from([("apps/plugins".into(), plugin("dsl", true))]));
    let child = delegation_child(json!([{
        "filters": [plugins_extension_ref()],
        "backendRefs": [{"name": "service", "port": 80}],
    }]));
    publish_http(&publisher, &[&child]);
    let a = scope(&publisher, "apps/a").http("apps/plugins").unwrap();
    let b = scope(&publisher, "apps/b").http("apps/plugins").unwrap();
    assert!(a
        .plugin_runtime
        .request_plugins_iter()
        .any(|filter| filter.name() == "RateLimitLocal"));
    assert!(a.is_activatable, "scope a allows the child's plugins");
    assert!(matches!(
        first_result(&b).await,
        PluginRunningResult::ErrResponse { status: 451, .. }
    ));
}

#[test]
fn child_ai_admission_per_scope() {
    let publisher = two_scope_publisher(json!({"deny": [{"name": "http/AiProxy", "action": "bypass"}]}));
    let mut source = (*plugin("dsl", true)).clone();
    source.spec.request_plugins.as_mut().unwrap().push(
        serde_json::from_value(
            json!({"type": "AiProxy", "config": {"api": "openaiChat", "usageObservation": "provider"}}),
        )
        .unwrap(),
    );
    publisher.replace_http_plugins(HashMap::from([("apps/plugins".into(), Arc::new(source))]));
    let child = delegation_child(json!([{
        "filters": [plugins_extension_ref()],
        "backendRefs": [{"group": "edgion.io", "kind": "EdgionBackend", "name": "ai"}],
        "resolvedAiAdmission": {"backends": [{"kind": "edgionBackend", "provider": "OpenAICompatible",
            "hasCredentialTpm": false, "specValid": true, "canonicalModels": []}]},
    }]));
    publish_http(&publisher, &[&child]);
    assert!(
        scope(&publisher, "apps/a").ai_rule(&child, 0),
        "scope a admits the child AI rule"
    );
    assert!(
        !scope(&publisher, "apps/b").ai_rule(&child, 0),
        "scope b bypasses AiProxy, so the child AI rule is not admitted there"
    );
}

#[test]
fn ordinary_route_scope_membership_unchanged() {
    let publisher = two_scope_publisher(json!({}));
    publisher.replace_http_plugins(HashMap::from([("apps/plugins".into(), plugin("dsl", true))]));
    let mut source = (*route()).clone();
    source.spec.rules.as_mut().unwrap()[0].filters =
        Some(serde_json::from_value(json!([plugins_extension_ref()])).unwrap());
    let ordinary = Arc::new(source);
    let input = RouteInput::from_http(Arc::clone(&ordinary));
    assert!(!input.delegation_child);
    publish_http(&publisher, &[&ordinary]);
    // Attached to Gateway `a` only: scope `b` gains neither the ref nor an admission.
    assert!(scope(&publisher, "apps/a").http("apps/plugins").is_some());
    assert!(scope(&publisher, "apps/b").http("apps/plugins").is_none());
    assert!(scope(&publisher, "apps/a").graphql_rule(&ordinary, 0));
    assert!(!scope(&publisher, "apps/b").graphql_rule(&ordinary, 0));
    assert_eq!(publisher.snapshot().stats().ai_route_tokens, 1);
}

fn named_plugin(name: &str) -> Arc<EdgionPlugins> {
    let mut source = (*plugin("dsl", true)).clone();
    source.metadata.name = Some(name.into());
    Arc::new(source)
}

fn same_runtimes(prepared: &Option<Arc<[Arc<PluginRuntime>]>>, scope: &PluginScope, keys: &[&str]) {
    let prepared = prepared.as_deref().expect("prepared slice is present");
    assert_eq!(prepared.len(), keys.len());
    for (runtime, key) in prepared.iter().zip(keys) {
        assert!(Arc::ptr_eq(runtime, &scope.http(key).unwrap().plugin_runtime), "{key}");
    }
}

fn set_global_refs(publisher: &PluginPublisher, refs: Option<Value>) {
    let mut cfg = (*config("policy-a", json!({}))).clone();
    cfg.spec.global_plugins_ref = refs.map(|refs| serde_json::from_value(refs).unwrap());
    publisher.replace_gateway_configs(HashMap::from([("policy-a".into(), Arc::new(cfg))]));
}

#[test]
fn prepared_global_slice_matches_reference_order_and_tracks_publication() {
    let publisher = publisher();
    publisher.replace_http_plugins(HashMap::from([
        ("apps/plugins".into(), plugin("dsl", true)),
        ("default/plugins".into(), default_source(false, true, "default")),
    ]));
    // Missing targets are dropped (current fail-open behavior tracked by gw-core-02);
    // a reference without a namespace resolves in `default`.
    set_global_refs(
        &publisher,
        Some(json!([
            {"namespace":"apps","name":"other"},
            {"name":"missing"},
            {"namespace":"apps","name":"plugins"},
            {"namespace":"apps","name":"plugins"},
            {"name":"plugins"}
        ])),
    );
    let first = scope(&publisher, "apps/a");
    let first_order = ["apps/plugins", "apps/plugins", "default/plugins"];
    same_runtimes(&first.resolved_globals, &first, &first_order);

    // An unchanged republish reuses the scope and therefore the same slice.
    publisher.replace_gateways(HashMap::from([("apps/a".into(), gateway("a", "a", 18080, None))]));
    let reused = scope(&publisher, "apps/a");
    assert!(Arc::ptr_eq(&first, &reused));

    // A previously missing target arrives: the scope and its slice are rebuilt.
    publisher.replace_http_plugins(HashMap::from([
        ("apps/plugins".into(), plugin("dsl", true)),
        ("default/plugins".into(), default_source(false, true, "default")),
        ("apps/other".into(), named_plugin("other")),
    ]));
    let rebuilt = scope(&publisher, "apps/a");
    assert!(!Arc::ptr_eq(&first, &rebuilt));
    same_runtimes(
        &rebuilt.resolved_globals,
        &rebuilt,
        &["apps/other", "apps/plugins", "apps/plugins", "default/plugins"],
    );
    same_runtimes(&first.resolved_globals, &first, &first_order);

    set_global_refs(&publisher, Some(json!([{"name":"missing"}])));
    assert!(scope(&publisher, "apps/a").resolved_globals.is_none());
    set_global_refs(&publisher, None);
    assert!(scope(&publisher, "apps/a").resolved_globals.is_none());
}

#[test]
fn prepared_gateway_slice_matches_keys_and_reuses_unchanged_owner() {
    let publisher = publisher();
    set_global_refs(&publisher, None);
    publisher.replace_http_plugins(HashMap::from([
        ("apps/plugins".into(), plugin("dsl", true)),
        ("apps/other".into(), named_plugin("other")),
    ]));
    let mut input = gateway("a", "a", 18080, None);
    input.http_keys = ["apps/other", "apps/missing", "apps/plugins", "apps/plugins"]
        .map(String::from)
        .to_vec();
    publisher.replace_gateways(HashMap::from([("apps/a".into(), input.clone())]));
    let first = publisher.snapshot().gateway("apps/a").unwrap();
    // Missing targets are dropped (current fail-open behavior tracked by gw-core-02).
    same_runtimes(
        &first.resolved_http,
        &first.scope,
        &["apps/other", "apps/plugins", "apps/plugins"],
    );

    publisher.replace_gateways(HashMap::from([("apps/a".into(), input.clone())]));
    assert!(Arc::ptr_eq(&first, &publisher.snapshot().gateway("apps/a").unwrap()));

    // Reordering keys keeps the class scope (same key set) but must rebuild the
    // Gateway owner and its slice.
    input.http_keys = ["apps/plugins", "apps/missing", "apps/plugins", "apps/other"]
        .map(String::from)
        .to_vec();
    publisher.replace_gateways(HashMap::from([("apps/a".into(), input.clone())]));
    let reordered = publisher.snapshot().gateway("apps/a").unwrap();
    assert!(Arc::ptr_eq(&first.scope, &reordered.scope));
    assert!(!Arc::ptr_eq(&first, &reordered));
    same_runtimes(
        &reordered.resolved_http,
        &reordered.scope,
        &["apps/plugins", "apps/plugins", "apps/other"],
    );

    input.http_keys = vec!["apps/plugins".into()];
    publisher.replace_gateways(HashMap::from([("apps/a".into(), input.clone())]));
    let changed = publisher.snapshot().gateway("apps/a").unwrap();
    same_runtimes(&changed.resolved_http, &changed.scope, &["apps/plugins"]);

    input.http_keys = vec!["apps/missing".into()];
    publisher.replace_gateways(HashMap::from([("apps/a".into(), input.clone())]));
    let missing = publisher.snapshot().gateway("apps/a").unwrap();
    assert!(missing.resolved_http.is_none());

    // Same keys, but the missing target arrives: the class scope is rebuilt, so
    // the Gateway owner and its slice are rebuilt too.
    publisher.replace_http_plugins(HashMap::from([
        ("apps/plugins".into(), plugin("dsl", true)),
        ("apps/other".into(), named_plugin("other")),
        ("apps/missing".into(), named_plugin("missing")),
    ]));
    let arrived = publisher.snapshot().gateway("apps/a").unwrap();
    assert!(!Arc::ptr_eq(&missing, &arrived));
    same_runtimes(&arrived.resolved_http, &arrived.scope, &["apps/missing"]);

    input.http_keys.clear();
    publisher.replace_gateways(HashMap::from([("apps/a".into(), input.clone())]));
    let no_keys = publisher.snapshot().gateway("apps/a").unwrap();
    assert!(no_keys.scope.available);
    assert!(no_keys.resolved_http.is_none());

    // An unavailable class scope prepares nothing; requests answer 503 first.
    input.http_keys = vec!["apps/plugins".into()];
    publisher.replace_gateways(HashMap::from([("apps/a".into(), input)]));
    publisher.replace_gateway_configs(HashMap::new());
    let unavailable = publisher.snapshot().gateway("apps/a").unwrap();
    assert!(!unavailable.scope.available);
    assert!(unavailable.resolved_http.is_none());
    assert!(unavailable.scope.resolved_globals.is_none());
}

#[test]
fn prepared_gateway_slice_keeps_ambiguous_placeholder() {
    let publisher = publisher();
    set_global_refs(&publisher, None);
    publisher.replace_http_plugins(HashMap::from([
        ("plugins".into(), default_source(true, true, "bare")),
        ("default/plugins".into(), default_source(false, true, "qualified")),
    ]));
    let mut input = gateway("a", "a", 18080, None);
    input.http_keys = vec!["default/missing".into(), "default/plugins".into()];
    publisher.replace_gateways(HashMap::from([("apps/a".into(), input)]));
    let owner = publisher.snapshot().gateway("apps/a").unwrap();
    let prepared = owner.resolved_http.as_deref().expect("collision is kept");
    assert_eq!(prepared.len(), 1, "ordinary missing drops but collision does not");
    assert!(prepared[0].rejects_invalid_config());
}
