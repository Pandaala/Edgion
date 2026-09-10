//! Regression tests for TLS route plugin dispatch and shared route gates.

use super::*;
use crate::plugins::stream::create_stream_plugin_handler;
use edgion_resources::resources::edgion_stream_plugins::{EdgionStreamPlugins, StreamPluginEntry, TlsRoutePluginEntry};
use edgion_resources::resources::plugin_policy::PluginPolicyAction;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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

async fn assert_dispatch(key: &str, allowed: bool, error: Option<&str>) {
    for mode in [TlsRouteMode::Terminated, TlsRouteMode::Passthrough] {
        let mut ctx = TlsContext::new(443, "127.0.0.1".into(), 50000, "203.0.113.7".into(), 50000);
        let actual = run_tls_route_plugins(Some(key), &mut ctx, "example.com", 443, mode).await;
        assert_eq!(actual, allowed, "unexpected result: {:?}", ctx.err_log);
        if let Some(error) = error {
            assert!(ctx.err_log.as_deref().unwrap_or_default().contains(error));
        } else {
            assert!(ctx.err_log.is_none());
        }
    }
}

// Keep all global-store mutations in one serial test: the production handler
// relies on conf-sync's single writer, so distinct keys alone are not isolation.
#[tokio::test]
async fn tls_route_stage_contract_survives_hot_updates_and_delete() {
    let key = "tls-stage-contract/policy";
    let handler = create_stream_plugin_handler();
    let mut resource: EdgionStreamPlugins = serde_yaml::from_str(
        "apiVersion: edgion.io/v1\nkind: EdgionStreamPlugins\nmetadata:\n  name: policy\n  namespace: tls-stage-contract\nspec: {}\n",
    )
    .unwrap();
    let publish = |resource: &EdgionStreamPlugins| {
        // Exercise the actual conf-sync serialization shape, including Some([]).
        let wire = serde_json::to_vec(resource).unwrap();
        let received = serde_json::from_slice(&wire).unwrap();
        handler.partial_update(
            HashMap::new(),
            HashMap::from([(key.to_string(), Arc::new(received))]),
            HashSet::new(),
        );
    };
    publish(&resource);
    assert_dispatch(key, true, None).await;
    resource.spec.plugins = Some(vec![]);
    publish(&resource);
    assert_dispatch(key, true, None).await;

    for (enabled, action) in [
        (true, None),
        (false, None),
        (true, Some(PluginPolicyAction::Bypass)),
        (true, Some(PluginPolicyAction::Block { status: 503 })),
    ] {
        let mut stage1 = stage1_entry();
        stage1.enable = enabled;
        stage1.policy_action = action;
        resource.spec.plugins = Some(vec![stage1]);
        publish(&resource);
        assert_dispatch(key, false, Some("requires tlsRoutePlugins")).await;
    }

    // A denying Stage 1 must never execute after a deliberate Stage-2 no-op.
    resource.spec.plugins = Some(vec![stage1_entry()]);
    resource.spec.tls_route_plugins = Some(vec![]);
    publish(&resource);
    assert_dispatch(key, true, None).await;
    for (enabled, action) in [(false, None), (true, Some(PluginPolicyAction::Bypass))] {
        let mut stage2 = stage2_entry(true);
        stage2.enable = enabled;
        stage2.policy_action = action;
        resource.spec.tls_route_plugins = Some(vec![stage2]);
        publish(&resource);
        assert_dispatch(key, true, None).await;
    }

    resource.spec.tls_route_plugins = Some(vec![stage2_entry(false)]);
    publish(&resource);
    assert_dispatch(key, true, None).await;
    resource.spec.tls_route_plugins = Some(vec![stage2_entry(true)]);
    publish(&resource);
    assert_dispatch(key, false, Some("tls route plugin denied: ip:deny:")).await;
    let mut blocked = stage2_entry(false);
    blocked.policy_action = Some(PluginPolicyAction::Block { status: 503 });
    resource.spec.tls_route_plugins = Some(vec![stage2_entry(false), blocked]);
    publish(&resource);
    assert_dispatch(key, false, Some("plugin disabled by policy")).await;

    resource.spec.tls_route_plugins = None;
    publish(&resource);
    assert_dispatch(key, false, Some("requires tlsRoutePlugins")).await;
    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from([key.to_string()]));
    assert_dispatch(key, false, Some("not found")).await;
    resource.spec.plugins = None;
    resource.spec.tls_route_plugins = Some(vec![stage2_entry(false)]);
    publish(&resource);
    assert_dispatch(key, true, None).await;
    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from([key.to_string()]));
    assert!(get_global_stream_plugin_store().get(key).is_none());
}

#[tokio::test]
async fn both_tls_modes_reject_invalid_canonical_addresses() {
    for mode in [TlsRouteMode::Terminated, TlsRouteMode::Passthrough] {
        for (direct, remote, error) in [
            ("unknown", "127.0.0.1", "direct peer IP"),
            ("127.0.0.1", "unknown", "remote IP"),
        ] {
            let mut ctx = TlsContext::new(443, direct.into(), 50000, remote.into(), 50000);
            assert!(!run_tls_route_plugins(Some("default/anything"), &mut ctx, "example.com", 443, mode).await);
            assert!(ctx.err_log.as_deref().unwrap().contains(error));
            assert!(ctx.err_log.as_deref().unwrap().contains("fail-close"));
        }
    }
}

#[tokio::test]
async fn fail_close_when_store_key_not_in_store() {
    // Stage-2 fail-close: rule references "default/missing" but store is
    // empty. Must reject and surface an err_log entry.
    let store = get_global_stream_plugin_store();
    // Avoid touching other tests' state: make sure this key is not present.
    // (The global store is shared; we only assert the missing-key path,
    // which never inserts anything.)
    let key = "default/__fail_close_missing__".to_string();
    assert!(store.get(&key).is_none(), "test precondition: key must not be in store");

    let mut ctx = TlsContext::new(443, "127.0.0.1".to_string(), 50000, "127.0.0.1".to_string(), 50000);

    let allowed = run_tls_route_plugins(
        Some(key.as_str()),
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
    let allowed = run_tls_route_plugins(None, &mut ctx, "example.com", 443, TlsRouteMode::Passthrough).await;
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
    assert!(apply_tls_l4_post_select(
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
    assert!(apply_tls_l4_post_select(
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
    assert!(!apply_tls_l4_post_select(&mut ctx, None, None, None));
    assert!(ctx.status.is_none());
    assert!(ctx.err_log.is_none());
    let json = serde_json::to_value(&ctx).expect("TlsContext serializes");
    assert!(json.get("status").is_none());
}
