//! Regression tests for TLS route plugin dispatch and shared route gates.

use super::*;
use crate::plugins::publication::{inputs::GatewayInput, PluginPublisher};
use crate::plugins::stream::plugin_generation::{StreamPluginGeneration, StreamStages};
use crate::plugins::stream::{StreamPluginRuntime, TlsRoutePlugin, TlsRoutePluginRuntime};
use edgion_resources::resources::common::RefDenied;
use edgion_resources::resources::edgion_stream_plugins::{EdgionStreamPlugins, StreamPluginEntry, TlsRoutePluginEntry};
use edgion_resources::resources::plugin_policy::{PluginPolicyConfig, PluginPolicyEntryConfig, PolicyActionKind};
use edgion_resources::{Gateway, GatewayClass};
use parking_lot::Mutex;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;

fn selection_class(name: &str) -> Arc<GatewayClass> {
    Arc::new(
        serde_json::from_value(json!({
            "metadata": {"name": name},
            "spec": {"controllerName": "edgion.io/gateway-controller"}
        }))
        .unwrap(),
    )
}

fn selection_gateway(name: &str, class: &str, hostname: &str, port: u16) -> Gateway {
    crate::test_current_status::from_value(json!({
        "metadata": {"namespace": "default", "name": name},
        "spec": {"gatewayClassName": class, "listeners": [
            {"name": "tls", "hostname": hostname, "port": port, "protocol": "TLS",
                "tls": {"mode": "Passthrough"}}
        ]}
    }))
    .unwrap()
}

fn selection_unit(name: &str, hostname: &str, parents: &[&str], port: u16) -> Arc<TlsRouteUnit> {
    let parent_refs: Vec<_> = parents
        .iter()
        .map(|gateway| json!({"name": gateway, "sectionName": "tls"}))
        .collect();
    let attachments: Vec<_> = parents
        .iter()
        .map(|gateway| {
            json!({
                "listenerKey": format!("0.0.0.0:{port}/TLS"), "gatewayNamespace": "default",
                "gatewayName": gateway, "listenerName": "tls", "hostnames": [hostname]
            })
        })
        .collect();
    let route = crate::test_current_status::from_value(json!({
        "metadata": {"namespace": "default", "name": name},
        "spec": {"hostnames": [hostname], "resolvedStatusController": "edgion.io/gateway-controller", "parentRefs": parent_refs, "resolvedListeners": attachments}
    }))
    .unwrap();
    Arc::new(TlsRouteUnit::from_route(Arc::new(route)).unwrap())
}

#[test]
fn selected_owner_policy_is_terminal_and_uses_the_captured_publication() {
    let publisher = PluginPublisher::new();
    let fallback_class = selection_class("tls-fallback");
    publisher.replace_gateway_classes(HashMap::from([
        ("tls-selection".into(), selection_class("tls-selection")),
        ("tls-fallback".into(), Arc::clone(&fallback_class)),
    ]));
    let first = selection_gateway("tls-selection", "tls-selection", "api.example.com", 19443);
    let second = selection_gateway("tls-fallback", "tls-fallback", "api.example.com", 19443);
    publisher.replace_gateways(HashMap::from([
        ("default/tls-selection".into(), GatewayInput::from_gateway(&first)),
        ("default/tls-fallback".into(), GatewayInput::from_gateway(&second)),
    ]));
    let key = ListenerKey::new("0.0.0.0:19443".parse().unwrap(), "TLS");
    // This published serving-set fixture deliberately exposes two eligible
    // owners. It tests policy/selection ordering, not Controller conflict status.
    let unit = selection_unit("selected", "api.example.com", &["tls-selection", "tls-fallback"], 19443);
    let runner_up = selection_unit("z-runner-up", "api.example.com", &["tls-fallback"], 19443);
    let manager = TlsRouteManager::new(key.clone());
    manager.rebuild(&HashMap::from([
        (unit.key_name(), Arc::clone(&unit)),
        (runner_up.key_name(), Arc::clone(&runner_up)),
    ]));
    let old = publisher.snapshot();
    let selected = select_tls_route(&manager, &old, &key, "api.example.com").unwrap();
    assert!(Arc::ptr_eq(&selected.unit, &unit));
    assert!(Arc::ptr_eq(
        selected.scope.as_ref().unwrap(),
        &old.gateway("default/tls-selection").unwrap().scope
    ));

    // Only the first owner's policy becomes unavailable. Both the second
    // owner and a same-hostname runner-up still have a usable scope.
    publisher.replace_gateway_classes(HashMap::from([("tls-fallback".into(), fallback_class)]));
    let current = publisher.snapshot();
    assert!(!current.gateway("default/tls-selection").unwrap().scope.available);
    assert!(current.gateway("default/tls-fallback").unwrap().scope.available);
    let control = TlsRouteManager::new(key.clone());
    control.rebuild(&HashMap::from([(runner_up.key_name(), Arc::clone(&runner_up))]));
    let alternate = select_tls_route(&control, &current, &key, "api.example.com").unwrap();
    assert!(Arc::ptr_eq(&alternate.unit, &runner_up));
    assert!(alternate.scope.is_some());

    // Neither the Route's second owner nor the runner-up can repair a policy
    // failure after the first eligible owner and Route have already won.
    let unavailable = select_tls_route(&manager, &current, &key, "api.example.com").unwrap();
    assert!(Arc::ptr_eq(&unavailable.unit, &unit));
    assert!(unavailable.scope.is_none());
    assert!(select_tls_route(&manager, &old, &key, "api.example.com")
        .unwrap()
        .scope
        .is_some());

    // Removing an owner is an eligibility change, unlike an unavailable scope.
    publisher.update_gateways(HashMap::new(), &HashSet::from(["default/tls-selection".into()]));
    let removed_owner = publisher.snapshot();
    let rebound = select_tls_route(&manager, &removed_owner, &key, "api.example.com").unwrap();
    assert!(Arc::ptr_eq(&rebound.unit, &unit));
    assert!(Arc::ptr_eq(
        rebound.scope.as_ref().unwrap(),
        &removed_owner.gateway("default/tls-fallback").unwrap().scope
    ));
    assert!(select_tls_route(&manager, &current, &key, "api.example.com")
        .unwrap()
        .scope
        .is_none());
    publisher.update_gateways(HashMap::new(), &HashSet::from(["default/tls-fallback".into()]));
    assert!(select_tls_route(&manager, &publisher.snapshot(), &key, "api.example.com").is_none());
    assert!(selected.scope.unwrap().available);
}

#[test]
fn gateway_only_empty_exact_listener_updates_block_and_restore_wildcard_route() {
    let publisher = PluginPublisher::new();
    publisher.replace_gateway_classes(HashMap::from([(
        "tls-live-owner".into(),
        selection_class("tls-live-owner"),
    )]));
    let broad = selection_gateway("wildcard-owner", "tls-live-owner", "*.example.com", 19444);
    publisher.replace_gateways(HashMap::from([(
        "default/wildcard-owner".into(),
        GatewayInput::from_gateway(&broad),
    )]));
    let key = ListenerKey::new("0.0.0.0:19444".parse().unwrap(), "TLS");
    let unit = selection_unit("wildcard-route", "*.example.com", &["wildcard-owner"], 19444);
    let manager = TlsRouteManager::new(key.clone());
    manager.rebuild(&HashMap::from([(unit.key_name(), Arc::clone(&unit))]));
    let route_table = Arc::clone(&manager.load_route_table());
    let before = publisher.snapshot();
    assert!(Arc::ptr_eq(
        &select_tls_route(&manager, &before, &key, "api.example.com")
            .unwrap()
            .unit,
        &unit
    ));

    // Another physical key must not constrain this key, even with the same SNI.
    let elsewhere = selection_gateway("empty-exact", "tls-live-owner", "api.example.com", 19445);
    publisher.update_gateways(
        HashMap::from([("default/empty-exact".into(), GatewayInput::from_gateway(&elsewhere))]),
        &HashSet::new(),
    );
    assert!(select_tls_route(&manager, &publisher.snapshot(), &key, "api.example.com").is_some());

    // Only Gateway facts change: the Route, its attachment and table stay fixed.
    let exact = selection_gateway("empty-exact", "tls-live-owner", "api.example.com", 19444);
    publisher.update_gateways(
        HashMap::from([("default/empty-exact".into(), GatewayInput::from_gateway(&exact))]),
        &HashSet::new(),
    );
    let blocked = publisher.snapshot();
    assert!(blocked
        .listener_index(&key)
        .unwrap()
        .get("default/empty-exact", "tls")
        .is_some());
    assert!(select_tls_route(&manager, &blocked, &key, "api.example.com").is_none());
    assert!(select_tls_route(&manager, &blocked, &key, "other.example.com").is_some());
    assert!(select_tls_route(&manager, &before, &key, "api.example.com").is_some());
    assert!(Arc::ptr_eq(&route_table, &manager.load_route_table()));

    publisher.update_gateways(HashMap::new(), &HashSet::from(["default/empty-exact".into()]));
    let restored = publisher.snapshot();
    assert!(restored
        .listener_index(&key)
        .unwrap()
        .get("default/empty-exact", "tls")
        .is_none());
    assert!(Arc::ptr_eq(
        &select_tls_route(&manager, &restored, &key, "api.example.com")
            .unwrap()
            .unit,
        &unit
    ));
    assert!(select_tls_route(&manager, &blocked, &key, "api.example.com").is_none());
    assert!(Arc::ptr_eq(&route_table, &manager.load_route_table()));
}
fn stage1_entry() -> StreamPluginEntry {
    serde_yaml::from_str("type: IpRestriction\nconfig:\n  deny:\n    - name: blocked\n      cidrs: [127.0.0.0/8]\n")
        .unwrap()
}

fn stage2_entry(deny: bool) -> TlsRoutePluginEntry {
    let cidr = if deny { "127.0.0.0/8" } else { "192.0.2.0/24" };
    serde_yaml::from_str(&format!(
        "type: IpRestriction\nconfig:\n  ipSource: DirectPeerIp\n  deny:\n    - name: blocked\n      cidrs: [{cidr}]\n"
    ))
    .unwrap()
}

async fn assert_dispatch(resource: Option<&CompiledStreamPlugins>, allowed: bool, error: Option<&str>) {
    for mode in [TlsRouteMode::Terminated, TlsRouteMode::Passthrough] {
        let mut ctx = TlsContext::new(443, "127.0.0.1".into(), 50000, "203.0.113.7".into(), 50000);
        let actual = run_tls_route_plugins(
            Some("tls-stage-contract/policy"),
            resource,
            &mut ctx,
            "example.com",
            443,
            mode,
        )
        .await;
        assert_eq!(actual, allowed, "unexpected result: {:?}", ctx.err_log);
        if let Some(error) = error {
            assert!(ctx.err_log.as_deref().unwrap_or_default().contains(error));
        } else {
            assert!(ctx.err_log.is_none());
        }
    }
}

fn compile_stage_view(
    resource: &EdgionStreamPlugins,
    family: &str,
    action: Option<PolicyActionKind>,
) -> Arc<CompiledStreamPlugins> {
    // Exercise the conf-sync representation, including an explicit Some([]).
    let wire = serde_json::to_vec(resource).unwrap();
    let received = Arc::new(serde_json::from_slice(&wire).unwrap());
    let mut policy = PluginPolicyConfig::default();
    if let Some(action) = action {
        policy.deny.push(PluginPolicyEntryConfig {
            name: format!("{family}/IpRestriction"),
            action: Some(action),
            block_status: None,
        });
    }
    StreamPluginGeneration::new(received).compile(
        &policy.compile().unwrap(),
        StreamStages {
            connection: false,
            tls_route: true,
        },
    )
}

#[tokio::test]
async fn tls_route_stage_contract_survives_generations_and_scope_policy() {
    let mut resource: EdgionStreamPlugins = serde_yaml::from_str(
        "apiVersion: edgion.io/v1\nkind: EdgionStreamPlugins\nmetadata: {name: policy, namespace: tls-stage-contract}\nspec: {}\n",
    ).unwrap();
    assert_dispatch(Some(&compile_stage_view(&resource, "stream", None)), true, None).await;
    resource.spec.plugins = Some(vec![]);
    assert_dispatch(Some(&compile_stage_view(&resource, "stream", None)), true, None).await;

    for (enabled, action) in [
        (true, None),
        (false, None),
        (true, Some(PolicyActionKind::Bypass)),
        (true, Some(PolicyActionKind::Block)),
    ] {
        let mut stage1 = stage1_entry();
        stage1.enable = enabled;
        resource.spec.plugins = Some(vec![stage1]);
        assert_dispatch(
            Some(&compile_stage_view(&resource, "stream", action)),
            false,
            Some("requires tlsRoutePlugins"),
        )
        .await;
    }

    // A denying Stage 1 must never run after a deliberate Stage-2 no-op.
    resource.spec.plugins = Some(vec![stage1_entry()]);
    resource.spec.tls_route_plugins = Some(vec![]);
    assert_dispatch(Some(&compile_stage_view(&resource, "tls-route", None)), true, None).await;
    for (enabled, action) in [(false, None), (true, Some(PolicyActionKind::Bypass))] {
        let mut stage2 = stage2_entry(true);
        stage2.enable = enabled;
        resource.spec.tls_route_plugins = Some(vec![stage2]);
        assert_dispatch(Some(&compile_stage_view(&resource, "tls-route", action)), true, None).await;
    }
    resource.spec.tls_route_plugins = Some(vec![stage2_entry(false)]);
    assert_dispatch(Some(&compile_stage_view(&resource, "tls-route", None)), true, None).await;
    resource.spec.tls_route_plugins = Some(vec![stage2_entry(true)]);
    assert_dispatch(
        Some(&compile_stage_view(&resource, "tls-route", None)),
        false,
        Some("tls route plugin denied: ip:deny:"),
    )
    .await;
    resource.spec.tls_route_plugins = Some(vec![stage2_entry(false), stage2_entry(false)]);
    assert_dispatch(
        Some(&compile_stage_view(
            &resource,
            "tls-route",
            Some(PolicyActionKind::Block),
        )),
        false,
        Some("plugin disabled by policy"),
    )
    .await;

    resource.spec.tls_route_plugins = None;
    assert_dispatch(
        Some(&compile_stage_view(&resource, "tls-route", None)),
        false,
        Some("requires tlsRoutePlugins"),
    )
    .await;
    assert_dispatch(None, false, Some("not found")).await;
    resource.spec.plugins = None;
    resource.spec.tls_route_plugins = Some(vec![stage2_entry(false)]);
    assert_dispatch(Some(&compile_stage_view(&resource, "tls-route", None)), true, None).await;
}

#[derive(Debug, PartialEq, Eq)]
struct ObservedTlsRouteContext {
    direct_peer_ip: IpAddr,
    remote_ip: IpAddr,
    listener_port: u16,
    sni: String,
    tls_id: Option<String>,
    rsc_ptr: usize,
    rsc_kind: String,
    rsc_namespace: String,
    rsc_name: String,
    is_mtls: bool,
}

struct SpyDenyTlsRoutePlugin {
    observed: Arc<Mutex<Option<ObservedTlsRouteContext>>>,
}

#[async_trait::async_trait]
impl TlsRoutePlugin for SpyDenyTlsRoutePlugin {
    async fn on_tls_route(&self, ctx: &TlsRouteContext<'_>) -> StreamPluginResult {
        *self.observed.lock() = Some(ObservedTlsRouteContext {
            direct_peer_ip: ctx.direct_peer_ip,
            remote_ip: ctx.remote_ip,
            listener_port: ctx.listener_port,
            sni: ctx.sni.to_string(),
            tls_id: ctx.tls_id.map(str::to_string),
            rsc_ptr: std::ptr::from_ref(ctx.rsc) as usize,
            rsc_kind: ctx.rsc.kind.clone(),
            rsc_namespace: ctx.rsc.ns.clone(),
            rsc_name: ctx.rsc.name.clone(),
            is_mtls: ctx.is_mtls,
        });
        StreamPluginResult::Deny("test denial".to_string())
    }
}

fn compiled_spy_tls_route_resource(observed: Arc<Mutex<Option<ObservedTlsRouteContext>>>) -> CompiledStreamPlugins {
    let mut spec: EdgionStreamPlugins = serde_yaml::from_str(
        "apiVersion: edgion.io/v1\nkind: EdgionStreamPlugins\nmetadata: {name: identity-spy, namespace: default}\nspec: {}\n",
    )
    .unwrap();
    // The raw Stage-2 declaration is explicit; only its executable runtime is
    // replaced with the local spy so this test reaches the dispatcher boundary.
    spec.spec.tls_route_plugins = Some(Vec::new());
    CompiledStreamPlugins {
        spec: Arc::new(spec),
        stream_plugin_runtime: Arc::new(StreamPluginRuntime::new()),
        tls_route_plugin_runtime: Arc::new(TlsRoutePluginRuntime::from_test_plugins(vec![Arc::new(
            SpyDenyTlsRoutePlugin { observed },
        )])),
    }
}

#[tokio::test]
async fn tls_route_modes_isolate_negotiated_identity() {
    for (mode, expected_tls_id, expected_is_mtls) in [
        (TlsRouteMode::Terminated, Some("negotiated-tls-id"), true),
        (TlsRouteMode::Passthrough, None, false),
    ] {
        let observed = Arc::new(Mutex::new(None));
        let resource = compiled_spy_tls_route_resource(Arc::clone(&observed));
        let mut ctx = TlsContext::new(9443, "192.0.2.10".into(), 41000, "198.51.100.20".into(), 42000);
        ctx.tls_id = Some("negotiated-tls-id".into());
        ctx.sni = Some("route.example.com".into());
        ctx.is_mtls = true;
        ctx.matched = Some(crate::MatchedInfo {
            rsc: crate::Rsc::new("TLSRoute", "default", "identity-route"),
            section: Some("tls".into()),
            sv: 42,
        });
        let expected_rsc_ptr = std::ptr::from_ref(&ctx.matched.as_ref().unwrap().rsc) as usize;
        let sni = ctx.sni.clone().unwrap();

        let allowed = run_tls_route_plugins(
            Some("default/identity-spy"),
            Some(&resource),
            &mut ctx,
            &sni,
            9443,
            mode,
        )
        .await;

        assert!(!allowed, "the deterministic spy denial must be terminal");
        assert_eq!(ctx.err_log.as_deref(), Some("tls route plugin denied: test denial"));
        assert_eq!(
            observed.lock().take(),
            Some(ObservedTlsRouteContext {
                direct_peer_ip: "192.0.2.10".parse().unwrap(),
                remote_ip: "198.51.100.20".parse().unwrap(),
                listener_port: 9443,
                sni: "route.example.com".into(),
                tls_id: expected_tls_id.map(str::to_string),
                rsc_ptr: expected_rsc_ptr,
                rsc_kind: "TLSRoute".into(),
                rsc_namespace: "default".into(),
                rsc_name: "identity-route".into(),
                is_mtls: expected_is_mtls,
            })
        );

        assert_eq!(ctx.tls_id.as_deref(), Some("negotiated-tls-id"));
        assert!(ctx.is_mtls);
        assert_eq!(ctx.listener_port, 9443);
        assert_eq!(ctx.direct_peer_ip, "192.0.2.10");
        assert_eq!(ctx.direct_peer_port, 41000);
        assert_eq!(ctx.remote_ip, "198.51.100.20");
        assert_eq!(ctx.remote_port, 42000);
        assert_eq!(ctx.sni.as_deref(), Some("route.example.com"));
        let matched = ctx.matched.as_ref().unwrap();
        assert_eq!(std::ptr::from_ref(&matched.rsc) as usize, expected_rsc_ptr);
        assert_eq!(matched.rsc.kind, "TLSRoute");
        assert_eq!(matched.rsc.ns, "default");
        assert_eq!(matched.rsc.name, "identity-route");
        assert_eq!(matched.section.as_deref(), Some("tls"));
        assert_eq!(matched.sv, 42);
    }
}

#[tokio::test]
async fn both_tls_modes_reject_invalid_canonical_addresses() {
    for mode in [TlsRouteMode::Terminated, TlsRouteMode::Passthrough] {
        for (direct, remote, error) in [
            ("unknown", "127.0.0.1", "direct peer IP"),
            ("127.0.0.1", "unknown", "remote IP"),
        ] {
            let mut ctx = TlsContext::new(443, direct.into(), 50000, remote.into(), 50000);
            assert!(!run_tls_route_plugins(Some("default/anything"), None, &mut ctx, "example.com", 443, mode).await);
            assert!(ctx.err_log.as_deref().unwrap().contains(error));
            assert!(ctx.err_log.as_deref().unwrap().contains("fail-close"));
        }
    }
}

#[tokio::test]
async fn fail_close_when_store_key_not_in_store() {
    // Stage-2 fail-close: rule references "default/missing" but store is
    // empty. Must reject and surface an err_log entry.
    let key = "default/__fail_close_missing__".to_string();

    let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000, "127.0.0.1".to_string(), 50000);

    let allowed = run_tls_route_plugins(
        Some(key.as_str()),
        None,
        &mut ctx,
        "example.com",
        443,
        TlsRouteMode::Passthrough,
    )
    .await;
    assert!(!allowed, "missing ESP must fail-close");
    let err = ctx.err_log.as_deref().unwrap_or("");
    assert!(
        err.contains("fail-close"),
        "err_log should mention fail-close, got: {err}"
    );
    assert!(
        err.contains(&key),
        "err_log should include the missing store key, got: {err}"
    );
}

#[tokio::test]
async fn admit_when_store_key_is_none() {
    // No ESP attached to the rule — intentional pass-through, must allow.
    let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000, "127.0.0.1".to_string(), 50000);
    let allowed = run_tls_route_plugins(None, None, &mut ctx, "example.com", 443, TlsRouteMode::Passthrough).await;
    assert!(allowed, "rule with no store_key must pass through");
    assert!(ctx.err_log.is_none());
}

#[tokio::test]
async fn reject_when_direct_peer_ip_unparseable() {
    // Rule references an ESP but DirectPeerIp is not a valid IP (e.g. the
    // literal "unknown" set when Pingora cannot extract the peer addr).
    // Without a parseable client IP we cannot evaluate an IP-allowlist
    // policy, so the connection must be rejected (fail-close).
    let mut ctx = TlsContext::new(443, "unknown".to_string(), 0, "unknown".to_string(), 0);
    let allowed = run_tls_route_plugins(
        Some("default/anything"),
        None,
        &mut ctx,
        "example.com",
        443,
        TlsRouteMode::Passthrough,
    )
    .await;
    assert!(!allowed, "unparseable DirectPeerIp must fail-close");
    let err = ctx.err_log.as_deref().unwrap_or("");
    assert!(
        err.contains("fail-close"),
        "err_log should mention fail-close, got: {err}"
    );
    assert!(
        err.contains("direct peer IP"),
        "err_log should mention DirectPeerIp, got: {err}"
    );
}

#[test]
fn apply_tls_l4_post_select_denied_sets_status_and_is_terminal() {
    let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000, "127.0.0.1".to_string(), 50000);
    let denied = RefDenied {
        target_namespace: "other".to_string(),
        target_name: "svc".to_string(),
        reason: Some("NoMatchingReferenceGrant".to_string()),
    };
    assert!(apply_fixture_post_select(
        &mut ctx,
        Some(&denied),
        Some("apps"),
        Some("Deployment")
    ));
    assert_eq!(ctx.status, Some(crate::routes::common::L4_BACKEND_REF_DENIED_STATUS));
    assert_eq!(
        ctx.err_log.as_deref(),
        Some(crate::routes::common::L4_BACKEND_REF_DENIED_LOG)
    );
    let json = serde_json::to_value(&ctx).expect("TlsContext serializes");
    assert_eq!(json["status"], "RefDenied");
    assert_eq!(json["err_log"], "backend ref denied");
}

#[test]
fn apply_tls_l4_post_select_unsupported_kind_sets_status_and_is_terminal() {
    let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000, "127.0.0.1".to_string(), 50000);
    assert!(apply_fixture_post_select(
        &mut ctx,
        None,
        Some("apps"),
        Some("Deployment")
    ));
    assert_eq!(
        ctx.status,
        Some(crate::routes::common::L4_BACKEND_UNSUPPORTED_KIND_STATUS)
    );
    assert_eq!(
        ctx.err_log.as_deref(),
        Some(crate::routes::common::L4_BACKEND_UNSUPPORTED_KIND_LOG)
    );
    let json = serde_json::to_value(&ctx).expect("TlsContext serializes");
    assert_eq!(json["status"], "UnsupportedBackend");
    assert_eq!(json["err_log"], "unsupported backend kind");
}

#[test]
fn apply_tls_l4_post_select_core_service_is_not_terminal() {
    let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000, "127.0.0.1".to_string(), 50000);
    assert!(!apply_fixture_post_select(&mut ctx, None, None, None));
    assert!(ctx.status.is_none());
    assert!(ctx.err_log.is_none());
    let json = serde_json::to_value(&ctx).expect("TlsContext serializes");
    assert!(json.get("status").is_none());
}

fn apply_fixture_post_select(
    ctx: &mut TlsContext,
    denied: Option<&RefDenied>,
    group: Option<&str>,
    kind: Option<&str>,
) -> bool {
    let route = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "gate-fixture", "namespace": "default"},
        "spec": {"rules": [{"backendRefs": [{"name": "svc", "port": 80, "group": group, "kind": kind, "refDenied": denied}]}]}
    })).unwrap();
    let unit = TlsRouteUnit::from_route(Arc::new(route)).unwrap();
    super::apply_tls_l4_post_select(ctx, unit.select_backend().unwrap())
}
#[test]
fn actual_tls_attempt_distinguishes_terminal_descriptors_from_live_retry() {
    for denied in [false, true] {
        for port in [serde_json::Value::Null, json!(0), json!(65536)] {
            let route = serde_json::from_value(json!({
                "metadata": {"name": "attempt", "namespace": "apps"},
                "spec": {"rules": [{"backendRefs": [
                    {"name": "bad", "port": port, "weight": 1,
                     "refDenied": denied.then(|| json!({"targetNamespace": "other", "targetName": "bad"}))},
                    {"name": "good", "port": 80, "weight": 1}
                ]}]}
            }))
            .unwrap();
            let unit = TlsRouteUnit::from_route(Arc::new(route)).unwrap();
            let mut ctx = TlsContext::new(443, "127.0.0.1".into(), 50000, "127.0.0.1".into(), 50000);
            let reads = std::cell::Cell::new(0);
            let attempt = resolve_tls_backend_attempt(&unit, &mut ctx, |_| {
                reads.set(reads.get() + 1);
                Ok::<_, ()>(())
            });
            assert!(matches!(attempt, TlsBackendAttempt::Terminal));
            assert_eq!(reads.get(), 0);
            assert_eq!(
                ctx.status,
                Some(if denied { "RefDenied" } else { "UnsupportedBackend" })
            );
            // Inspect the next selection independently: the actual attempt consumed
            // just the bad slot. Both production loops return on its Terminal result.
            assert_eq!(unit.select_backend().unwrap().service().unwrap().name(), "good");
        }
    }
    let route = serde_json::from_value(json!({
        "metadata": {"name": "attempt"},
        "spec": {"rules": [{"backendRefs": [{"name": "good", "port": 80}]}]}
    }))
    .unwrap();
    let unit = TlsRouteUnit::from_route(Arc::new(route)).unwrap();
    let mut ctx = TlsContext::new(443, "127.0.0.1".into(), 50000, "127.0.0.1".into(), 50000);
    let reads = std::cell::Cell::new(0);
    let failed = resolve_tls_backend_attempt(&unit, &mut ctx, |target| {
        reads.set(reads.get() + 1);
        assert_eq!(target.key(), "default/good");
        Err::<(), _>("live endpoint unavailable")
    });
    assert!(matches!(failed, TlsBackendAttempt::Retryable(_)));
    let ready = resolve_tls_backend_attempt(&unit, &mut ctx, |_| {
        reads.set(reads.get() + 1);
        Ok::<_, ()>(42)
    });
    assert!(matches!(ready, TlsBackendAttempt::Ready(_, 42)));
    assert_eq!(reads.get(), 2);
    assert!(ctx.status.is_none());
    assert!(ctx.err_log.is_none());
}

#[test]
fn actual_tls_connect_retry_delay_preserves_bounds_and_exhaustion() {
    assert_eq!(tls_connect_retry_delay(0, 1), None);
    assert_eq!(tls_connect_retry_delay(0, 2), Some(Duration::from_millis(25)));
    assert_eq!(tls_connect_retry_delay(1, 2), None);
    assert_eq!(tls_connect_retry_delay(3, 5), Some(Duration::from_millis(200)));
    assert_eq!(tls_connect_retry_delay(4, 5), None);
    assert_eq!(tls_connect_retry_delay(u32::MAX, 5), None);
    assert_eq!(tls_connect_retry_backoff(31), Duration::from_millis(250));
    assert_eq!(cap_tls_connect_retries(0), 1);
    assert_eq!(cap_tls_connect_retries(100), 5);
}
