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
