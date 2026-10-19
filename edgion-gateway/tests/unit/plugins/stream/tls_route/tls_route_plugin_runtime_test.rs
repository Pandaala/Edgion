use super::*;
use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;
use edgion_resources::resources::plugin_policy::{PluginPolicyConfig, PolicyActionKind};
use std::net::IpAddr;

fn policy_runtime(entries: &[TlsRoutePluginEntry], action: PolicyActionKind) -> TlsRoutePluginRuntime {
    let mut policy = PluginPolicyConfig::default();
    policy
        .deny
        .push(edgion_resources::resources::plugin_policy::PluginPolicyEntryConfig {
            name: "tls-route/IpRestriction".to_string(),
            action: Some(action),
            block_status: None,
        });
    let policy = policy.compile().unwrap();
    TlsRoutePluginRuntime::from_cached_entries(entries, "default", Some(&policy), &mut vec![None; entries.len()])
}

fn make_ctx(rsc: &crate::Rsc) -> TlsRouteContext<'_> {
    TlsRouteContext {
        direct_peer_ip: "127.0.0.1".parse::<IpAddr>().unwrap(),
        remote_ip: "127.0.0.1".parse::<IpAddr>().unwrap(),
        listener_port: 443,
        sni: "test.example.com",
        tls_id: Some("abcd1234"),
        rsc,
        is_mtls: false,
    }
}

#[tokio::test]
async fn test_empty_runtime_allows() {
    let rt = TlsRoutePluginRuntime::new();
    assert!(rt.is_empty());
    let rsc = crate::Rsc::new("TLSRoute", "default", "test-route");
    let ctx = make_ctx(&rsc);
    let result = rt.run(&ctx).await;
    assert!(matches!(result, StreamPluginResult::Allow));
}

#[tokio::test]
async fn test_from_empty_entries() {
    let rt = TlsRoutePluginRuntime::from_entries(&[], "default");
    assert!(rt.is_empty());
    assert_eq!(rt.plugin_count(), 0);
}

/// A structurally valid IpRestriction config (validate_and_init requires at
/// least one of allow/deny/allowRefs/denyRefs) that denies an unrelated
/// range and default-allows everything else — so `make_ctx()`'s
/// 127.0.0.1 client passes through when the plugin actually runs.
fn ip_restriction_entry() -> TlsRoutePluginEntry {
    let config: IpRestrictionConfig = serde_json::from_value(serde_json::json!({
        "denyRefs":[{"name":"unrelated-blocked-range"}],"defaultAction":"allow"
    }))
    .unwrap();
    TlsRoutePluginEntry {
        enable: true,
        plugin: TlsRouteStreamPlugin::IpRestriction(Box::new(config)),
    }
}

#[tokio::test]
async fn block_policy_denies_connection_bypass_policy_skips() {
    let blocked = ip_restriction_entry();
    let runtime = policy_runtime(&[blocked], PolicyActionKind::Block);
    let rsc = crate::Rsc::new("TLSRoute", "default", "test-route");
    let ctx = make_ctx(&rsc);
    match runtime.run(&ctx).await {
        StreamPluginResult::Deny(Cow::Owned(reason)) => {
            assert_eq!(reason, "plugin disabled by policy: tls-route/IpRestriction")
        }
        other => panic!("expected Deny, got {other:?}"),
    }

    let bypassed = ip_restriction_entry();
    let runtime = policy_runtime(&[bypassed], PolicyActionKind::Bypass);
    assert!(runtime.is_empty(), "bypassed plugin must not be instantiated");
}

#[tokio::test]
async fn block_policy_collapses_multiple_restricted_entries() {
    let normal = ip_restriction_entry();
    let blocked = ip_restriction_entry();
    let runtime = policy_runtime(&[normal, blocked], PolicyActionKind::Block);
    let rsc = crate::Rsc::new("TLSRoute", "default", "test-route");
    let ctx = make_ctx(&rsc);
    match runtime.run(&ctx).await {
        StreamPluginResult::Deny(reason) => {
            assert!(reason.contains("plugin disabled by policy: tls-route/IpRestriction"))
        }
        other => panic!("expected Deny, got {other:?}"),
    }
}

#[tokio::test]
async fn standalone_entries_are_allowed() {
    let entry = ip_restriction_entry();
    let runtime = TlsRoutePluginRuntime::from_entries(&[entry], "default");
    assert!(!runtime.is_empty());
    assert_eq!(runtime.plugin_count(), 1);
    let rsc = crate::Rsc::new("TLSRoute", "default", "test-route");
    let ctx = make_ctx(&rsc);
    assert!(matches!(runtime.run(&ctx).await, StreamPluginResult::Allow));
}

struct BorrowIdentityPlugin {
    sni_ptr: usize,
    tls_id_ptr: usize,
    rsc_ptr: usize,
}

#[async_trait::async_trait]
impl TlsRoutePlugin for BorrowIdentityPlugin {
    async fn on_tls_route(&self, ctx: &TlsRouteContext<'_>) -> StreamPluginResult {
        assert_eq!(ctx.sni.as_ptr() as usize, self.sni_ptr);
        assert_eq!(ctx.tls_id.unwrap().as_ptr() as usize, self.tls_id_ptr);
        assert_eq!(std::ptr::from_ref(ctx.rsc) as usize, self.rsc_ptr);
        StreamPluginResult::Allow
    }
}

#[tokio::test]
async fn async_trait_dispatch_preserves_borrowed_metadata_identity() {
    let sni = "borrowed.example.com".to_string();
    let tls_id = "tls-identity".to_string();
    let rsc = crate::Rsc::new("TLSRoute", "default", "borrowed-route");
    let runtime = TlsRoutePluginRuntime {
        plugins: vec![Arc::new(BorrowIdentityPlugin {
            sni_ptr: sni.as_ptr() as usize,
            tls_id_ptr: tls_id.as_ptr() as usize,
            rsc_ptr: std::ptr::from_ref(&rsc) as usize,
        })],
    };
    let ctx = TlsRouteContext {
        direct_peer_ip: "127.0.0.1".parse().unwrap(),
        remote_ip: "127.0.0.1".parse().unwrap(),
        listener_port: 443,
        sni: &sni,
        tls_id: Some(&tls_id),
        rsc: &rsc,
        is_mtls: true,
    };

    assert!(matches!(runtime.run(&ctx).await, StreamPluginResult::Allow));
}

#[tokio::test]
async fn unsupported_direct_tls_route_entries_deny_before_disable_and_policy_checks() {
    let policy = PluginPolicyConfig {
        allow: Some(Vec::new()),
        denied_action: PolicyActionKind::Bypass,
        ..Default::default()
    }
    .compile()
    .unwrap();
    let rsc = crate::Rsc::new("TLSRoute", "default", "test");
    let ctx = make_ctx(&rsc);
    assert!(matches!(
        TlsRoutePluginRuntime::unsupported_source().run(&ctx).await,
        StreamPluginResult::Deny(Cow::Borrowed("unsupported stream plugin type"))
    ));
    for enabled in [true, false] {
        let entry: TlsRoutePluginEntry = serde_json::from_value(serde_json::json!({
            "type":"FuturePlugin", "enable":enabled, "config":{}
        }))
        .unwrap();
        let mut cache = vec![None];
        let runtime = TlsRoutePluginRuntime::from_cached_entries(&[entry], "default", Some(&policy), &mut cache);
        assert!(matches!(
            runtime.run(&ctx).await,
            StreamPluginResult::Deny(Cow::Borrowed("unsupported TLS route plugin type"))
        ));
        assert!(cache[0].is_none());
    }
}

use edgion_resources::refs::ip_list_fields::PreparedIpListReferences;
use edgion_resources::resources::edgion_stream_plugins::TlsRoutePluginEntry;
impl TlsRoutePluginRuntime {
    pub(crate) fn from_entries(entries: &[TlsRoutePluginEntry], namespace: &str) -> Self {
        Self::from_cached_entries(entries, namespace, None, &mut vec![None; entries.len()])
    }
    pub(crate) fn from_cached_entries(
        entries: &[TlsRoutePluginEntry],
        namespace: &str,
        policy: Option<&CompiledPluginPolicy>,
        cache: &mut [Option<Arc<dyn TlsRoutePlugin>>],
    ) -> Self {
        let owner: edgion_resources::EdgionStreamPlugins = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "fixture", "namespace": namespace}, "spec": {"tlsRoutePlugins": entries}
        }))
        .unwrap();
        let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
        let binding = prepared.bind_stream_plugins(&owner).unwrap();
        Self::from_owner_cached(&binding, policy, cache)
    }
}

#[tokio::test]
async fn invalid_configuration_denies_with_borrowed_reason() {
    let entry = TlsRoutePluginEntry {
        enable: true,
        plugin: TlsRouteStreamPlugin::IpRestriction(Box::new(IpRestrictionConfig {
            status: 100,
            ..Default::default()
        })),
    };
    let mut cache = vec![None];
    let runtime = TlsRoutePluginRuntime::from_cached_entries(&[entry], "default", None, &mut cache);
    let rsc = crate::Rsc::new("TLSRoute", "default", "invalid-config");
    assert!(matches!(
        runtime.run(&make_ctx(&rsc)).await,
        StreamPluginResult::Deny(Cow::Borrowed("invalid owning IP reference"))
    ));
    assert!(cache[0].is_none());
}
