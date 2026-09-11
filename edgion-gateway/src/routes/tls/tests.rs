//! Regression tests for TLS route plugin dispatch and shared route gates.

use super::*;
use crate::plugins::stream::plugin_generation::{StreamPluginGeneration, StreamStages};
use edgion_resources::resources::edgion_stream_plugins::{EdgionStreamPlugins, StreamPluginEntry, TlsRoutePluginEntry};
use edgion_resources::resources::plugin_policy::{PluginPolicyConfig, PluginPolicyEntryConfig, PolicyActionKind};
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
