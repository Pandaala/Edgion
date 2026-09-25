use super::*;
use crate::plugins::stream::{StreamContext, StreamLayer, StreamPluginResult};
use edgion_resources::resources::plugin_policy::PluginPolicyConfig;

const CONNECTION: StreamStages = StreamStages {
    connection: true,
    tls_route: false,
};

fn source() -> Arc<EdgionStreamPlugins> {
    Arc::new(
        serde_yaml::from_str(
            r#"
apiVersion: edgion.io/v1alpha1
kind: EdgionStreamPlugins
metadata: {name: shared-generation, namespace: default}
spec:
  plugins:
    - type: ConnectionRateLimit
      config:
        perListener: {rate: 2, interval: 1h}
  tlsRoutePlugins:
    - type: IpRestriction
      config: {defaultAction: allow}
"#,
        )
        .unwrap(),
    )
}

fn policy(action: &str) -> CompiledPluginPolicy {
    serde_yaml::from_str::<PluginPolicyConfig>(&format!(
        "deny:\n  - name: stream/ConnectionRateLimit\n    action: {action}"
    ))
    .unwrap()
    .compile()
    .unwrap()
}

#[tokio::test]
async fn scopes_share_the_real_rate_limit_budget_and_revoke_releases_owner() {
    let mut generation = StreamPluginGeneration::new(source());
    let allow = CompiledPluginPolicy::default();
    let first = generation.compile(&allow, CONNECTION);
    let cached = generation.connection[0].as_ref().unwrap().as_ref().unwrap();
    let owner = Arc::downgrade(cached);
    let second = generation.compile(&allow, CONNECTION);
    assert_eq!(owner.strong_count(), 3, "one cache and two views share the same filter");
    let context = StreamContext::new("127.0.0.1".parse().unwrap(), 18491, StreamLayer::Stage1);
    assert!(matches!(
        first.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Allow
    ));
    assert!(matches!(
        second.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Allow
    ));
    assert!(matches!(
        first.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Deny(_)
    ));
    let bypass = generation.compile(&policy("bypass"), CONNECTION);
    assert!(bypass.stream_plugin_runtime.is_empty());
    generation.release_unneeded_entries(&StreamEntryDemand::default());
    assert_eq!(generation.cached_entry_count(), 0);
    assert!(owner.upgrade().is_some(), "admitted views keep their instance");
    drop(first);
    drop(second);
    assert!(owner.upgrade().is_none(), "a bypass view holds no executable reference");
    let restored = generation.compile(&allow, CONNECTION);
    assert!(matches!(
        restored.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Deny(_)
    ));
}

#[tokio::test]
async fn denied_and_unreferenced_stages_never_construct_entries() {
    let mut generation = StreamPluginGeneration::new(source());
    let context = StreamContext::new("127.0.0.1".parse().unwrap(), 18492, StreamLayer::Stage1);
    let blocked = generation.compile(&policy("block"), CONNECTION);
    assert!(matches!(
        blocked.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Deny(_)
    ));
    assert_eq!(generation.cached_entry_count(), 0);
    assert!(blocked.tls_route_plugin_runtime.is_empty());
    let bypass = generation.compile(&policy("bypass"), CONNECTION);
    assert!(bypass.stream_plugin_runtime.is_empty());
    assert_eq!(generation.cached_entry_count(), 0);
    let tls = StreamStages {
        connection: false,
        tls_route: true,
    };
    let _tls_view = generation.compile(&CompiledPluginPolicy::default(), tls);
    assert_eq!(generation.cached_entry_count(), 1);
    let mut demand = generation.demand(&CompiledPluginPolicy::default(), tls);
    demand.extend(&generation.demand(&policy("block"), CONNECTION));
    generation.release_unneeded_entries(&demand);
    assert_eq!(generation.cached_entry_count(), 1);
    assert!(generation.connection[0].is_none());
}

#[tokio::test]
async fn failed_entries_stay_fail_closed_and_do_not_pollute_bypassed_scope() {
    let mut source = (*source()).clone();
    source.spec.plugins = Some(
        serde_yaml::from_str(
            r#"
- type: GeoIpLocation
  config: {}
"#,
        )
        .unwrap(),
    );
    let mut generation = StreamPluginGeneration::new(Arc::new(source));
    let allow = CompiledPluginPolicy::default();
    let invalid = generation.compile(&allow, CONNECTION);
    assert!(generation.connection[0].as_ref().unwrap().is_err());
    let context = StreamContext::new("127.0.0.1".parse().unwrap(), 18493, StreamLayer::Stage1);
    assert!(matches!(
        invalid.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Deny(_)
    ));
    let bypass =
        serde_yaml::from_str::<PluginPolicyConfig>("deny:\n  - name: stream/GeoIpLocation\n    action: bypass")
            .unwrap()
            .compile()
            .unwrap();
    let scoped = generation.compile(&bypass, CONNECTION);
    assert!(scoped.stream_plugin_runtime.is_empty());
    generation.release_unneeded_entries(&generation.demand(&bypass, CONNECTION));
    assert_eq!(generation.cached_entry_count(), 0);
}

#[tokio::test]
async fn unsupported_stream_source_denies_both_stages_despite_disable_or_policy_bypass() {
    use crate::plugins::stream::tls_route::TlsRouteContext;
    use edgion_resources::resources::plugin_policy::PolicyActionKind;
    let bypass = PluginPolicyConfig {
        allow: Some(Vec::new()),
        denied_action: PolicyActionKind::Bypass,
        ..Default::default()
    }
    .compile()
    .unwrap();
    let context = StreamContext::new("127.0.0.1".parse().unwrap(), 443, StreamLayer::Stage1);
    let rsc = crate::Rsc::new("TLSRoute", "default", "test");
    let tls_context = TlsRouteContext {
        direct_peer_ip: "127.0.0.1".parse().unwrap(),
        remote_ip: "127.0.0.1".parse().unwrap(),
        listener_port: 443,
        sni: "test.example.com",
        tls_id: None,
        rsc: &rsc,
        is_mtls: false,
    };
    for stage in ["plugins", "tlsRoutePlugins"] {
        for enabled in [true, false] {
            let source: EdgionStreamPlugins = serde_json::from_value(serde_json::json!({
                "apiVersion":"edgion.io/v1", "kind":"EdgionStreamPlugins",
                "metadata":{"name":"unknown", "namespace":"default"},
                "spec":{stage:[{"type":"FuturePlugin", "enable":enabled, "config":{}}]}
            }))
            .unwrap();
            let mut generation = StreamPluginGeneration::new(Arc::new(source));
            let view = generation.compile(
                &bypass,
                StreamStages {
                    connection: true,
                    tls_route: true,
                },
            );
            assert!(matches!(
                view.stream_plugin_runtime.run(&context).await,
                StreamPluginResult::Deny(_)
            ));
            assert!(matches!(
                view.tls_route_plugin_runtime.run(&tls_context).await,
                StreamPluginResult::Deny(_)
            ));
            assert_eq!(generation.cached_entry_count(), 0);
        }
    }
}
