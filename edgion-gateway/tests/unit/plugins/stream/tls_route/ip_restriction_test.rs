use super::super::tls_route_plugin_trait::TlsRouteContext;
use super::*;
use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
use edgion_resources::resources::edgion_config_data::{
    ConfigEntry, EdgionConfigData, EdgionConfigDataRef, EdgionConfigDataSpec, IpGroup, IpListData,
};
use edgion_resources::resources::edgion_plugins::IpRestrictionConfig;
use edgion_resources::resources::edgion_plugins::{DefaultAction, IpSource};
use std::net::IpAddr;
use std::sync::Arc;

fn ctx<'a>(ip: &str, rsc: &'a crate::Rsc) -> TlsRouteContext<'a> {
    ctx_with_ips(ip, ip, rsc)
}

fn ctx_with_ips<'a>(direct_peer_ip: &str, remote_ip: &str, rsc: &'a crate::Rsc) -> TlsRouteContext<'a> {
    TlsRouteContext {
        direct_peer_ip: direct_peer_ip.parse::<IpAddr>().unwrap(),
        remote_ip: remote_ip.parse::<IpAddr>().unwrap(),
        listener_port: 443,
        sni: "test.example.com",
        tls_id: Some("t"),
        rsc,
        is_mtls: false,
    }
}

#[tokio::test]
async fn source_selector_distinguishes_direct_peer_and_remote_ip() {
    let ns = "tls-direct-source-unit";
    let data: EdgionConfigData=serde_json::from_value(serde_json::json!({"metadata":{"name":"blocked","namespace":ns},"spec":{"data":{"type":"IpList","config":{"items":[{"name":"blocked","cidrs":["10.0.0.7/32"]}]}}}})).unwrap();
    get_global_edgion_config_data_store().insert_raw_for_test(format!("{ns}/blocked"), Arc::new(data));
    let base: IpRestrictionConfig =
        serde_json::from_value(serde_json::json!({"denyRefs":[{"name":"blocked"}],"ipSource":"DirectPeerIp"})).unwrap();
    let rsc = crate::Rsc::new("TLSRoute", "default", "r");
    let addresses = ctx_with_ips("10.0.0.7", "203.0.113.9", &rsc);
    let direct = from_owner(&owner(&base, ns));
    assert!(matches!(
        direct.on_tls_route(&addresses).await,
        StreamPluginResult::Deny(_)
    ));

    let mut remote_config = base;
    remote_config.ip_source = IpSource::RemoteIp;
    let remote = from_owner(&owner(&remote_config, ns));
    assert!(matches!(
        remote.on_tls_route(&addresses).await,
        StreamPluginResult::Allow
    ));
}

#[tokio::test]
async fn deny_refs_block_referenced_ip() {
    let ns = "tls-ipr-denyref-unit";
    let spec = EdgionConfigDataSpec {
        current_status: None,
        enable: true,
        active: None,
        visibility: Default::default(),
        unknown_fields: Vec::new(),
        data: ConfigEntry::IpList(IpListData {
            items: vec![IpGroup {
                name: "blk".into(),
                description: None,
                cidrs: vec!["9.9.9.9/32".into()],
            }],
        }),
    };
    let mut ed = EdgionConfigData::new("blk", spec);
    ed.metadata.namespace = Some(ns.to_string());
    get_global_edgion_config_data_store().insert_raw_for_test(format!("{ns}/blk"), Arc::new(ed));

    let config = IpRestrictionConfig {
        refs: edgion_resources::resources::edgion_plugins::ip_acl_refs::IpAclReferenceSet {
            allow_refs: None,
            deny_refs: Some(vec![EdgionConfigDataRef {
                name: "blk".into(),
                namespace: Some(ns.into()),
                ..Default::default()
            }]),
            ..Default::default()
        },
        default_action: DefaultAction::Allow,
        ip_source: IpSource::RemoteIp,
        message: None,
        status: 403,
    };
    let plugin = from_owner(&owner(&config, ns));
    let rsc = crate::Rsc::new("TLSRoute", "default", "r");
    assert!(matches!(
        plugin.on_tls_route(&ctx("9.9.9.9", &rsc)).await,
        StreamPluginResult::Deny(_)
    ));
    assert!(matches!(
        plugin.on_tls_route(&ctx("8.8.8.8", &rsc)).await,
        StreamPluginResult::Allow
    ));
}

#[tokio::test]
async fn tls_route_denied_allow_ref_contributes_no_key() {
    use edgion_resources::resources::edgion_config_data::EdgionConfigDataRef;
    let ns = "tls-ipr-ref-denied-unit";
    let config = IpRestrictionConfig {
        refs: edgion_resources::resources::edgion_plugins::ip_acl_refs::IpAclReferenceSet {
            allow_refs: Some(vec![EdgionConfigDataRef {
                name: "corp".into(),
                namespace: Some(ns.into()),
                ref_denied: Some(edgion_resources::resources::common::RefDenied {
                    target_namespace: ns.into(),
                    target_name: "corp".into(),
                    reason: None,
                }),
            }]),
            deny_refs: None,
            ..Default::default()
        },
        default_action: DefaultAction::Allow,
        ip_source: IpSource::RemoteIp,
        message: None,
        status: 403,
    };
    let plugin = from_owner(&owner(&config, ns));
    assert!(
        matches!(
            plugin.acl.evaluate_in(None, Some("8.8.8.8".parse().unwrap())),
            edgion_resources::AccessDecision::DenyByWhitelistMiss
        ),
        "denied allow_ref must yield no key in TLS route"
    );
    // Evaluate path: whitelist mode + zero allow matchers → deny-all (fail-closed).
    // Empty key lists keep the snapshot absent, so on_tls_route reaches
    // check_ip_access_with_refs with empty ref slices — a whitelist miss → Deny.
    let rsc = crate::Rsc::new("TLSRoute", "default", "r");
    assert!(
        matches!(
            plugin.on_tls_route(&ctx("8.8.8.8", &rsc)).await,
            StreamPluginResult::Deny(_)
        ),
        "denied allow_ref → empty allow matchers → non-matching IP must be DENIED (deny-all fail-closed)",
    );
}

use edgion_resources::refs::ip_list_fields::PreparedIpListReferences;
fn owner(config: &IpRestrictionConfig, namespace: &str) -> edgion_resources::EdgionStreamPlugins {
    serde_json::from_value(serde_json::json!({"metadata":{"name":"actual","namespace":namespace},
        "spec":{"tlsRoutePlugins":[{"type":"IpRestriction","config":config}]}}))
    .unwrap()
}
fn from_owner(owner: &edgion_resources::EdgionStreamPlugins) -> TlsRouteIpRestriction {
    let prepared = PreparedIpListReferences::from_stream_plugins(owner);
    let binding = prepared.bind_stream_plugins(owner).unwrap();
    TlsRouteIpRestriction::from_bound(binding.tls_entry(0).unwrap()).unwrap()
}

#[tokio::test]
async fn owning_tls_invalid_slots_keep_allow_intent_and_deny_set_drop() {
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    use edgion_resources::EdgionConfigData;
    let namespace = "r5c2-tls";
    for key in [
        format!("{namespace}/bad/name"),
        "/r5c2-tls-nets".into(),
        format!("{namespace}/good"),
    ] {
        let data: EdgionConfigData = serde_json::from_value(serde_json::json!({
            "metadata":{"name":key.rsplit('/').next().unwrap(),"namespace":namespace},"spec":{"data":{"type":"IpList","config":{"items":[{"name":"set","cidrs":["198.51.100.8/32"]}]}}}
        })).unwrap();
        get_global_edgion_config_data_store().insert_raw_for_test(key, std::sync::Arc::new(data));
    }
    for bad in [
        serde_json::json!({"name":"bad/name"}),
        serde_json::json!({"name":"r5c2-tls-nets","namespace":""}),
    ] {
        for (case, allow) in [
            ("allow", false),
            ("deny", true),
            ("inline", true),
            ("allow-sibling", true),
            ("deny-sibling", false),
        ] {
            let mut config = serde_json::json!({"defaultAction":"allow"});
            let side = if case.starts_with("deny") {
                "denyRefs"
            } else {
                "allowRefs"
            };
            config[side] = serde_json::json!([bad]);
            if case.ends_with("sibling") {
                config[side]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"name":"good"}));
            }
            if case == "inline" {
                config["allow"] = serde_json::json!([{"name":"inline","cidrs":["198.51.100.8/32"]}]);
            }
            let owner: edgion_resources::EdgionStreamPlugins = serde_json::from_value(serde_json::json!({
                "metadata":{"name":"actual","namespace":namespace},"spec":{"tlsRoutePlugins":[{"type":"IpRestriction","config":config}]}
            })).unwrap();
            let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
            let bound = prepared.bind_stream_plugins(&owner).unwrap();
            let result = TlsRouteIpRestriction::from_bound(bound.tls_entry(0).unwrap());
            if case == "inline" {
                assert!(result.is_err());
                continue;
            }
            let plugin = result.unwrap();
            let rsc = crate::Rsc::new("TLSRoute", "default", "r5c2");
            let context = ctx("198.51.100.8", &rsc);
            let result = plugin.on_tls_route(&context).await;
            assert_eq!(matches!(result, StreamPluginResult::Allow), allow, "{case} / {bad}");
        }
    }
}
#[test]
fn owning_tls_invalid_owner_fails_separately_from_invalid_target() {
    let config = serde_json::json!({"denyRefs":[{"name":"bad/name"}]});
    let mut owner: edgion_resources::EdgionStreamPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"actual","namespace":""},"spec":{"tlsRoutePlugins":[{"type":"IpRestriction","config":config}]}
    })).unwrap();
    let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
    let bound = prepared.bind_stream_plugins(&owner).unwrap();
    assert!(TlsRouteIpRestriction::from_bound(bound.tls_entry(0).unwrap()).is_err());
    owner.metadata.namespace = None;
    let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
    let bound = prepared.bind_stream_plugins(&owner).unwrap();
    assert!(TlsRouteIpRestriction::from_bound(bound.tls_entry(0).unwrap()).is_ok());
    owner.metadata.uid = Some("changed".into());
    assert!(prepared.bind_stream_plugins(&owner).is_err());
}

#[tokio::test]
async fn blank_reference_name_preserves_structural_constructor_failure() {
    for side in ["allowRefs", "denyRefs"] {
        let mut config = serde_json::json!({"defaultAction":"allow"});
        config[side] = serde_json::json!([{"name":""}]);
        let owner:edgion_resources::EdgionStreamPlugins=serde_json::from_value(serde_json::json!({
            "metadata":{"name":"actual","namespace":"default"},"spec":{"tlsRoutePlugins":[{"type":"IpRestriction","config":config}]}
        })).unwrap();
        let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
        let binding = prepared.bind_stream_plugins(&owner).unwrap();
        let result = TlsRouteIpRestriction::from_bound(binding.tls_entry(0).unwrap());
        assert!(result.is_err());
    }
}
