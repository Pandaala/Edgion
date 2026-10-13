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
      config: {defaultAction: allow, denyRefs: [{name: unreferenced-budget-fixture}]}
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
    let tls_view = generation.compile(&CompiledPluginPolicy::default(), tls);
    let rsc = crate::Rsc::new("TLSRoute", "default", "budget-fixture");
    let tls_ctx = crate::plugins::stream::tls_route::TlsRouteContext {
        direct_peer_ip: "127.0.0.1".parse().unwrap(),
        remote_ip: "127.0.0.1".parse().unwrap(),
        listener_port: 443,
        sni: "budget.example.com",
        tls_id: None,
        rsc: &rsc,
        is_mtls: false,
    };
    assert!(
        matches!(
            tls_view.tls_route_plugin_runtime.run(&tls_ctx).await,
            StreamPluginResult::Allow
        ),
        "unavailable optional deny reference preserves this source's default-Allow forwarding"
    );
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
    // Static-invalid Geo input is rejected before construction and cache
    // reuse; the published runtime still denies instead of retaining an Err.
    assert!(generation.connection[0].is_none());
    assert_eq!(generation.cached_entry_count(), 0);
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

#[tokio::test]
async fn standalone_generation_default_target_live_updates_and_disabled_owner_policy() {
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    let seed = |name: &str, cidr: &str| {
        let data: edgion_resources::EdgionConfigData = serde_json::from_value(serde_json::json!({"metadata":{"name":name,"namespace":"default"},"spec":{"data":{"type":"IpList","config":{"items":[{"name":"nets","cidrs":[cidr]}]}}}})).unwrap();
        get_global_edgion_config_data_store().insert_raw_for_test(format!("default/{name}"), Arc::new(data));
    };
    for kind in ["IpRestriction", "GlobalConnectionIpRestriction"] {
        let name = if kind == "IpRestriction" {
            "r5c2-stream-l4-live"
        } else {
            "r5c2-stream-gcir-live"
        };
        let rules = serde_json::json!({"allowRefs":[{"name":name}]});
        let config = if kind == "IpRestriction" {
            rules
        } else {
            serde_json::json!({"enable":true,"activeProfile":"main","profiles":{"main":rules}})
        };
        let owner: EdgionStreamPlugins = serde_json::from_value(serde_json::json!({"metadata":{"name":"bare-generation"},"spec":{
            "plugins":[{"type":kind,"config":config}],"tlsRoutePlugins":[{"type":"IpRestriction","config":{"allowRefs":[{"name":name}]}}]
        }})).unwrap();
        let mut generation = StreamPluginGeneration::new(Arc::new(owner.clone()));
        let stages = StreamStages {
            connection: true,
            tls_route: true,
        };
        let allow = CompiledPluginPolicy::default();
        let pinned = generation.compile(&allow, stages);
        let again = generation.compile(&allow, stages);
        assert_eq!(generation.cached_entry_count(), 2);
        let context = StreamContext::new("198.51.100.8".parse().unwrap(), 18497, StreamLayer::Stage1);
        let rsc = crate::Rsc::new("TLSRoute", "default", "r5c2-live");
        let tls = crate::plugins::stream::TlsRouteContext {
            direct_peer_ip: context.direct_peer_ip,
            remote_ip: context.direct_peer_ip,
            listener_port: 443,
            sni: "test.example.com",
            tls_id: None,
            rsc: &rsc,
            is_mtls: false,
        };
        assert!(matches!(
            pinned.stream_plugin_runtime.run(&context).await,
            StreamPluginResult::Deny(_)
        ));
        assert!(matches!(
            pinned.tls_route_plugin_runtime.run(&tls).await,
            StreamPluginResult::Deny(_)
        ));
        for (cidr, expected) in [("198.51.100.8/32", true), ("203.0.113.8/32", false)] {
            seed(name, cidr);
            for view in [&pinned, &again] {
                assert_eq!(
                    matches!(
                        view.stream_plugin_runtime.run(&context).await,
                        StreamPluginResult::Allow
                    ),
                    expected
                );
                assert_eq!(
                    matches!(view.tls_route_plugin_runtime.run(&tls).await, StreamPluginResult::Allow),
                    expected
                );
            }
        }
        let mut disabled = owner;
        disabled.metadata.namespace = Some(String::new());
        disabled.spec.plugins.as_mut().unwrap()[0].enable = false;
        disabled.spec.tls_route_plugins.as_mut().unwrap()[0].enable = false;
        let disabled = StreamPluginGeneration::new(Arc::new(disabled)).compile(&allow, stages);
        assert!(disabled.stream_plugin_runtime.is_empty());
        assert!(disabled.tls_route_plugin_runtime.is_empty());
    }
}

#[tokio::test]
async fn gcir_selector_fresh_retarget_generation_and_pinned_view_keep_live_snapshot() {
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    let store = get_global_edgion_config_data_store();
    let seed = |name: &str, active: &str| {
        let data=serde_json::from_value(serde_json::json!({"metadata":{"name":name,"namespace":"default"},"spec":{"data":{"type":"Selector","config":{"active":active}}}})).unwrap();
        store.insert_raw_for_test(format!("default/{name}"), Arc::new(data));
    };
    for (name, cidr) in [
        ("r5e-base-networks", "203.0.113.0/24"),
        ("r5e-blocked-networks", "0.0.0.0/0"),
    ] {
        let data=serde_json::from_value(serde_json::json!({"metadata":{"name":name,"namespace":"default"},"spec":{"data":{"type":"IpList","config":{"items":[{"name":"all","cidrs":[cidr]}]}}}})).unwrap();
        store.insert_raw_for_test(format!("default/{name}"), Arc::new(data));
    }
    let source = |name: &str| -> Arc<EdgionStreamPlugins> {
        Arc::new(serde_json::from_value(serde_json::json!({"metadata":{"name":"r5e-gcir"},"spec":{"plugins":[{
        "type":"GlobalConnectionIpRestriction","config":{"enable":true,"activeProfile":"base","activeProfileRef":{"name":name},"profiles":{"base":{"defaultAction":"allow","denyRefs":[{"name":"r5e-base-networks"}]},"blocked":{"defaultAction":"deny","denyRefs":[{"name":"r5e-blocked-networks"}]}}}
    }]}})).unwrap())
    };
    let mut first = StreamPluginGeneration::new(source("r5e-selector-first"));
    let pinned = first.compile(&CompiledPluginPolicy::default(), CONNECTION);
    let context = StreamContext::new("198.51.100.9".parse().unwrap(), 18502, StreamLayer::Stage1);
    assert!(matches!(
        pinned.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Allow
    ));
    seed("r5e-selector-first", "blocked");
    assert!(matches!(
        pinned.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Deny(_)
    ));
    seed("r5e-selector-second", "base");
    let mut next = StreamPluginGeneration::new(source("r5e-selector-second"));
    let repaired = next.compile(&CompiledPluginPolicy::default(), CONNECTION);
    assert!(matches!(
        repaired.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Allow
    ));
    assert!(matches!(
        pinned.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Deny(_)
    ));
    seed("r5e-selector-first", "base");
    assert!(matches!(
        pinned.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Allow
    ));
}

#[tokio::test]
async fn active_redis_generation_checks_new_owner_without_revoking_captured_view() {
    let owner: EdgionStreamPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"redis-generation","namespace":"r6h"},"spec":{"plugins":[{
            "type":"ConnectionRateLimit","config":{"redisRef":"r6h/unregistered","perSourceIp":{"rate":100,"interval":"1s","onRedisFailure":"Allow"}}
        }]}
    })).unwrap();
    let mut first = StreamPluginGeneration::new(Arc::new(owner.clone()));
    let policy = CompiledPluginPolicy::default();
    let captured = first.compile(&policy, CONNECTION);
    let repeated = first.compile(&policy, CONNECTION);
    assert_eq!(first.cached_entry_count(), 1);
    let context = StreamContext::new("127.0.0.1".parse().unwrap(), 21983, StreamLayer::Stage1);
    assert!(matches!(
        captured.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Allow
    ));
    assert!(matches!(
        repeated.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Allow
    ));
    let mut invalid = owner;
    invalid.metadata.name = None;
    let mut next = StreamPluginGeneration::new(Arc::new(invalid));
    let closed = next.compile(&policy, CONNECTION);
    assert_eq!(next.cached_entry_count(), 0);
    assert!(
        matches!(closed.stream_plugin_runtime.run(&context).await,StreamPluginResult::Deny(reason) if reason == "invalid owning LinkSys reference")
    );
    first.release_unneeded_entries(&StreamEntryDemand::default());
    assert_eq!(first.cached_entry_count(), 0);
    assert!(matches!(
        captured.stream_plugin_runtime.run(&context).await,
        StreamPluginResult::Allow
    ));
}

#[tokio::test]
async fn tls_acl_legacy_wire_generation_replaces_valid_and_clean_recovers() {
    use crate::plugins::stream::tls_route::TlsRouteContext;
    let value = serde_json::json!({"metadata":{"name":"tls-acl-generations","namespace":"default"},"spec":{"tlsRoutePlugins":[{
        "type":"IpRestriction","config":{"ipSource":"DirectPeerIp","defaultAction":"allow","denyRefs":[{"name":"unavailable-tls-generation-deny"}]}
    }]}});
    let clean: Arc<EdgionStreamPlugins> = Arc::new(serde_json::from_value(value.clone()).unwrap());
    let stages = StreamStages {
        connection: false,
        tls_route: true,
    };
    let policy = CompiledPluginPolicy::default();
    let mut generation = StreamPluginGeneration::new(clean.clone());
    let held = generation.compile(&policy, stages);
    let rsc = crate::Rsc::new("TLSRoute", "default", "tls-acl-generations");
    let context = TlsRouteContext {
        direct_peer_ip: "192.0.2.1".parse().unwrap(),
        remote_ip: "192.0.2.1".parse().unwrap(),
        listener_port: 443,
        sni: "example.com",
        tls_id: None,
        rsc: &rsc,
        is_mtls: false,
    };
    assert!(matches!(
        held.tls_route_plugin_runtime.run(&context).await,
        StreamPluginResult::Allow
    ));
    for payload in [serde_json::Value::Null, serde_json::json!([])] {
        let mut invalid = value.clone();
        invalid["spec"]["tlsRoutePlugins"][0]["config"]["deny"] = payload;
        let typed: EdgionStreamPlugins = serde_json::from_value(invalid).unwrap();
        let received = Arc::new(serde_json::from_str(&serde_json::to_string(&typed).unwrap()).unwrap());
        let mut rejected_generation = StreamPluginGeneration::new(received);
        let rejected = rejected_generation.compile(&policy, stages);
        assert!(matches!(
            rejected.tls_route_plugin_runtime.run(&context).await,
            StreamPluginResult::Deny(_)
        ));
        assert_eq!(rejected_generation.cached_entry_count(), 0);
        assert!(matches!(
            held.tls_route_plugin_runtime.run(&context).await,
            StreamPluginResult::Allow
        ));
        let recovered = StreamPluginGeneration::new(clean.clone()).compile(&policy, stages);
        assert!(matches!(
            recovered.tls_route_plugin_runtime.run(&context).await,
            StreamPluginResult::Allow
        ));
    }
}
