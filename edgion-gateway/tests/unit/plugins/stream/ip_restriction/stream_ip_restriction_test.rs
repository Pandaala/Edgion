use super::*;
use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
use crate::plugins::stream::stream_plugin_trait::{StreamContext, StreamLayer};
use edgion_resources::resources::edgion_config_data::{
    ConfigEntry, EdgionConfigData, EdgionConfigDataRef, EdgionConfigDataSpec, IpGroup, IpListData,
};
use std::sync::Arc;

fn seed_iplist(ns: &str, name: &str, cidr: &str) {
    let spec = EdgionConfigDataSpec {
        current_status: None,
        enable: true,
        active: None,
        visibility: Default::default(),
        unknown_fields: Vec::new(),
        data: ConfigEntry::IpList(IpListData {
            items: vec![IpGroup {
                name: "g".into(),
                description: None,
                cidrs: vec![cidr.into()],
            }],
        }),
    };
    let mut ed = EdgionConfigData::new(name, spec);
    ed.metadata.namespace = Some(ns.to_string());
    get_global_edgion_config_data_store().insert_raw_for_test(format!("{ns}/{name}"), Arc::new(ed));
}

#[tokio::test]
async fn deny_refs_block_referenced_ip() {
    let ns = "l4-ipr-denyref-unit";
    seed_iplist(ns, "blk", "9.9.9.9/32");
    let mut rules = ConnectionIpRestrictionRules::default();
    rules.deny_refs = Some(vec![EdgionConfigDataRef {
        name: "blk".into(),
        namespace: Some(ns.into()),
        ..Default::default()
    }]);
    let plugin = StreamIpRestriction::new(&rules, ns);
    let ctx_block = StreamContext::new("9.9.9.9".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(
        plugin.on_connection(&ctx_block).await,
        StreamPluginResult::Deny(_)
    ));
    let ctx_ok = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx_ok).await, StreamPluginResult::Allow));
}

#[tokio::test]
async fn l4_denied_allow_ref_contributes_no_key() {
    use edgion_resources::resources::edgion_config_data::EdgionConfigDataRef;
    let ns = "l4-ipr-ref-denied-unit";
    let mut rules = ConnectionIpRestrictionRules::default();
    rules.allow_refs = Some(vec![EdgionConfigDataRef {
        name: "corp".into(),
        namespace: Some(ns.into()),
        ref_denied: Some(edgion_resources::resources::common::RefDenied {
            target_namespace: ns.into(),
            target_name: "corp".into(),
            reason: None,
        }),
    }]);
    let plugin = StreamIpRestriction::new(&rules, ns);
    // Denied allow_ref → allow_ref_keys is empty → no matcher resolved.
    assert!(plugin.allow_ref_keys.is_empty(), "denied allow_ref must yield no key");
    // Evaluate path: whitelist mode + zero allow matchers → deny-all (fail-closed).
    // Empty key lists keep the snapshot absent, so on_connection reaches
    // check_ip_with_refs with empty ref slices — a whitelist miss → Deny.
    let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(
        matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)),
        "denied allow_ref → empty allow matchers → non-matching IP must be DENIED (deny-all fail-closed)",
    );
}

use edgion_resources::refs::ip_list_fields::PreparedIpListReferences;
// Detached convenience exists only in this mirrored test module.
impl StreamIpRestriction {
    pub(crate) fn new(config: &ConnectionIpRestrictionRules, namespace: &str) -> Self {
        let owner: edgion_resources::EdgionStreamPlugins = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "fixture", "namespace": namespace},
            "spec": {"plugins": [{"type": "IpRestriction", "config": config}]}
        }))
        .unwrap();
        let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
        let binding = prepared.bind_stream_plugins(&owner).unwrap();
        Self::from_bound(binding.connection_entry(0).unwrap()).expect("valid fixture owner")
    }
}

#[tokio::test]
async fn owning_connection_invalid_slots_keep_allow_intent_and_deny_set_drop() {
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    use edgion_resources::EdgionConfigData;
    let namespace = "r5c2-connection";
    for key in [
        format!("{namespace}/bad/name"),
        "/r5c2-connection-nets".into(),
        format!("{namespace}/good"),
    ] {
        let data: EdgionConfigData = serde_json::from_value(serde_json::json!({
            "metadata":{"name":"decoy","namespace":namespace},"spec":{"data":{"type":"IpList","config":{"items":[{"name":"set","cidrs":["198.51.100.8/32"]}]}}}
        })).unwrap();
        get_global_edgion_config_data_store().insert_raw_for_test(key, std::sync::Arc::new(data));
    }
    for bad in [
        serde_json::json!({"name":"bad/name"}),
        serde_json::json!({"name":"r5c2-connection-nets","namespace":""}),
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
                "metadata":{"name":"actual","namespace":namespace},"spec":{"plugins":[{"type":"IpRestriction","config":config}]}
            })).unwrap();
            let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
            let bound = prepared.bind_stream_plugins(&owner).unwrap();
            let plugin = StreamIpRestriction::from_bound(bound.connection_entry(0).unwrap()).unwrap();
            let context = StreamContext::new("198.51.100.8".parse().unwrap(), 18498, StreamLayer::Stage1);
            let result = plugin.on_connection(&context).await;
            assert_eq!(matches!(result, StreamPluginResult::Allow), allow, "{case} / {bad}");
        }
    }
}
#[test]
fn owning_connection_invalid_owner_fails_separately_from_invalid_target() {
    let config = serde_json::json!({"denyRefs":[{"name":"bad/name"}]});
    let mut owner: edgion_resources::EdgionStreamPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"actual","namespace":""},"spec":{"plugins":[{"type":"IpRestriction","config":config}]}
    }))
    .unwrap();
    let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
    let bound = prepared.bind_stream_plugins(&owner).unwrap();
    assert!(StreamIpRestriction::from_bound(bound.connection_entry(0).unwrap()).is_err());
    owner.metadata.namespace = None;
    let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
    let bound = prepared.bind_stream_plugins(&owner).unwrap();
    assert!(StreamIpRestriction::from_bound(bound.connection_entry(0).unwrap()).is_ok());
    owner.metadata.uid = Some("changed".into());
    assert!(prepared.bind_stream_plugins(&owner).is_err());
}

#[tokio::test]
async fn blank_reference_name_preserves_structural_constructor_failure() {
    for side in ["allowRefs", "denyRefs"] {
        let mut config = serde_json::json!({"defaultAction":"allow"});
        config[side] = serde_json::json!([{"name":""}]);
        let owner:edgion_resources::EdgionStreamPlugins=serde_json::from_value(serde_json::json!({
            "metadata":{"name":"actual","namespace":"default"},"spec":{"plugins":[{"type":"IpRestriction","config":config}]}
        })).unwrap();
        let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
        let binding = prepared.bind_stream_plugins(&owner).unwrap();
        let result = StreamIpRestriction::from_bound(binding.connection_entry(0).unwrap());
        assert!(result.unwrap().init_failed);
    }
}
