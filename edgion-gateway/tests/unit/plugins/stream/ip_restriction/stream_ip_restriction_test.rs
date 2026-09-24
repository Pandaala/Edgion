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
