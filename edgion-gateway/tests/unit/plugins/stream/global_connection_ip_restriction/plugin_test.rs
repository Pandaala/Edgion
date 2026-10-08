use super::*;
use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
use crate::plugins::stream::stream_plugin_trait::StreamLayer;
use edgion_resources::resources::edgion_config_data::{
    ConfigEntry, EdgionConfigData, EdgionConfigDataRef, EdgionConfigDataSpec, IpGroup, IpListData, Selector,
};
use edgion_resources::resources::edgion_plugins::DefaultAction;
use edgion_resources::resources::edgion_stream_plugins::{GlobalConnectionIpRestrictionConfig, ProfileRules};
use std::collections::HashMap;
use std::sync::Arc;

fn ip_group(name: &str, cidr: &str) -> IpGroup {
    IpGroup {
        name: name.to_string(),
        description: None,
        cidrs: vec![cidr.to_string()],
    }
}

/// One profile that allows only `allow_cidr` (default deny), or with a deny entry.
fn profile(default_action: DefaultAction, allow_cidr: Option<&str>, deny_cidr: Option<&str>) -> ProfileRules {
    ProfileRules {
        allow: allow_cidr.map(|c| vec![ip_group("test-allow", c)]),
        deny: deny_cidr.map(|c| vec![ip_group("test-deny", c)]),
        default_action,
        allow_refs: None,
        deny_refs: None,
        allow_matcher: None,
        deny_matcher: None,
    }
}

/// Build a single-profile config (no selector ref).
fn make_config(
    active_profile: &str,
    default_action: DefaultAction,
    allow_cidr: Option<&str>,
    deny_cidr: Option<&str>,
) -> GlobalConnectionIpRestrictionConfig {
    GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: active_profile.to_string(),
        profiles: HashMap::from([(
            active_profile.to_string(),
            profile(default_action, allow_cidr, deny_cidr),
        )]),
        description: None,
        active_profile_ref: None,
    }
}

fn plugin_deny_nonloopback() -> GlobalConnectionIpRestrictionPlugin {
    let config = make_config("main", DefaultAction::Deny, Some("127.0.0.0/8"), None);
    GlobalConnectionIpRestrictionPlugin::new(&config, "default").expect("valid config")
}

/// Insert a raw EdgionConfigData into the store under "ns/name".
fn insert_ed(ns: &str, name: &str, enable: bool, data: ConfigEntry) -> String {
    let key = format!("{ns}/{name}");
    let mut ed = EdgionConfigData::new(
        name,
        EdgionConfigDataSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            enable,
            active: None,
            visibility: Default::default(),
            data,
        },
    );
    ed.metadata.namespace = Some(ns.to_string());
    get_global_edgion_config_data_store().insert_raw_for_test(&key, Arc::new(ed));
    key
}

#[tokio::test]
async fn no_selector_base_allow_loopback() {
    let plugin = plugin_deny_nonloopback();
    let ctx = StreamContext::new("127.0.0.1".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow));
}

#[tokio::test]
async fn no_selector_base_deny_non_loopback() {
    let plugin = plugin_deny_nonloopback();
    let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)));
}

#[tokio::test]
async fn absent_selector_falls_back_to_base() {
    let mut config = make_config("main", DefaultAction::Deny, Some("127.0.0.0/8"), None);
    config.active_profile_ref = Some(EdgionConfigDataRef {
        name: "gcir-unit-absent-selector-do-not-register".to_string(),
        namespace: Some("default".to_string()),
        ..Default::default()
    });
    let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, "default").expect("valid config");

    let ctx = StreamContext::new("127.0.0.1".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow));
    let ctx2 = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx2).await, StreamPluginResult::Deny(_)));
}

#[tokio::test]
async fn enable_false_allows_all_connections() {
    let mut config = make_config("main", DefaultAction::Deny, Some("127.0.0.0/8"), None);
    config.enable = false;
    let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, "default").expect("valid config");
    let ctx = StreamContext::new("1.2.3.4".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow));
}

/// Build a config with two profiles: "deny" (allow loopback only) and "open" (allow all),
/// active "deny" by default, referencing a Selector under `ref_key`.
fn two_profile_config(ns: &str, ref_name: &str) -> GlobalConnectionIpRestrictionConfig {
    GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "deny".to_string(),
        profiles: HashMap::from([
            (
                "deny".to_string(),
                profile(DefaultAction::Deny, Some("127.0.0.0/8"), None),
            ),
            (
                "open".to_string(),
                profile(DefaultAction::Deny, Some("0.0.0.0/0"), None),
            ),
        ]),
        description: None,
        active_profile_ref: Some(EdgionConfigDataRef {
            name: ref_name.to_string(),
            namespace: Some(ns.to_string()),
            ..Default::default()
        }),
    }
}

#[tokio::test]
async fn selector_switches_active_profile() {
    let ns = "unit-sel-switch";
    insert_ed(
        ns,
        "sel-open",
        true,
        ConfigEntry::Selector(Selector {
            active: Some("open".to_string()),
            description: None,
        }),
    );
    let config = two_profile_config(ns, "sel-open");
    let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid config");
    // Selector switches active profile to "open" → 8.8.8.8 is allowed.
    let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow));
}

#[tokio::test]
async fn selector_active_null_is_noop_uses_base() {
    let ns = "unit-sel-null";
    insert_ed(
        ns,
        "sel-null",
        true,
        ConfigEntry::Selector(Selector {
            active: None,
            description: Some("intentional no-op".to_string()),
        }),
    );
    let config = two_profile_config(ns, "sel-null");
    let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid config");
    // active: null → no override → base "deny" profile → 8.8.8.8 denied.
    let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)));
}

#[tokio::test]
async fn selector_unknown_profile_falls_back_to_base() {
    let ns = "unit-sel-unknown";
    insert_ed(
        ns,
        "sel-bad",
        true,
        ConfigEntry::Selector(Selector {
            active: Some("does-not-exist".to_string()),
            description: None,
        }),
    );
    let config = two_profile_config(ns, "sel-bad");
    let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid config");
    // Unknown profile → fail-to-base → base "deny" → 8.8.8.8 denied.
    let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)));
}

#[tokio::test]
async fn type_mismatch_falls_back_to_base() {
    let ns = "unit-sel-mismatch";
    // A non-Selector EdgionConfigData (IpList) at the referenced key.
    insert_ed(
        ns,
        "not-a-selector",
        true,
        ConfigEntry::IpList(IpListData { items: vec![] }),
    );
    let config = two_profile_config(ns, "not-a-selector");
    let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid config");
    // Wrong variant → fail-to-base → base "deny" → 8.8.8.8 denied.
    let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)));
}

/// Envelope kill-switch: a Selector that WOULD switch to "open" is ignored because the
/// EdgionConfigData envelope `spec.enable=false`. Base ("deny") is used.
#[tokio::test]
async fn envelope_disabled_falls_back_to_base() {
    let ns = "unit-sel-envelope";
    insert_ed(
        ns,
        "sel-envelope-off",
        false, // envelope kill-switch
        ConfigEntry::Selector(Selector {
            active: Some("open".to_string()),
            description: None,
        }),
    );
    let config = two_profile_config(ns, "sel-envelope-off");
    let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid config");
    // Envelope disabled → selector ignored → base "deny" → 8.8.8.8 denied.
    let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)));
    // Loopback still allowed by base.
    let ctx_lo = StreamContext::new("127.0.0.1".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx_lo).await, StreamPluginResult::Allow));
}

#[test]
fn invalid_base_returns_error() {
    // active_profile not present in profiles → construction error.
    let config = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "nonexistent".to_string(),
        profiles: HashMap::from([("p1".to_string(), profile(DefaultAction::Deny, Some("10.0.0.0/8"), None))]),
        description: None,
        active_profile_ref: None,
    };
    assert!(GlobalConnectionIpRestrictionPlugin::new(&config, "default").is_err());
}

/// A denied active_profile_ref must not be resolved as a store key (ref_denied is
/// set) — even if a Selector at that key would switch to the "open" profile.
/// Effect: base "deny" profile is used (8.8.8.8 is denied instead of allowed).
#[tokio::test]
async fn gcir_denied_active_profile_ref_falls_back_to_base() {
    let ns = "gcir-ref-denied-unit";
    // Seed a valid Selector that WOULD switch the active profile to "open".
    insert_ed(
        ns,
        "sel-denied",
        true,
        ConfigEntry::Selector(edgion_resources::resources::edgion_config_data::Selector {
            active: Some("open".to_string()),
            description: None,
        }),
    );
    // Build a two-profile config pointing at that selector, but with ref_denied set.
    let mut config = two_profile_config(ns, "sel-denied");
    config.active_profile_ref.as_mut().unwrap().ref_denied = Some(edgion_resources::resources::common::RefDenied {
        target_namespace: ns.to_string(),
        target_name: "sel-denied".to_string(),
        reason: None,
    });
    let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid config");
    // Denied ref → active_profile_ref_key is None → base "deny" profile used.
    // 8.8.8.8 must be denied (not allowed by "open").
    let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(
        matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)),
        "denied active_profile_ref must not switch profile; base deny must apply"
    );
}

#[tokio::test]
async fn profile_deny_refs_block_referenced_ip() {
    let ns = "gcir-denyref-unit";
    // Seed a deny IpList.
    insert_ed(
        ns,
        "blk",
        true,
        ConfigEntry::IpList(IpListData {
            items: vec![ip_group("blocked", "9.9.9.9/32")],
        }),
    );
    // One profile, default Allow, with a denyRef to the seeded IpList.
    let mut profile = profile(DefaultAction::Allow, None, None);
    profile.deny_refs = Some(vec![EdgionConfigDataRef {
        name: "blk".into(),
        namespace: Some(ns.into()),
        ..Default::default()
    }]);
    let config = GlobalConnectionIpRestrictionConfig {
        enable: true,
        active_profile: "main".to_string(),
        profiles: HashMap::from([("main".to_string(), profile)]),
        description: None,
        active_profile_ref: None,
    };
    let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid");
    let ctx_block = StreamContext::new("9.9.9.9".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(
        plugin.on_connection(&ctx_block).await,
        StreamPluginResult::Deny(_)
    ));
    let ctx_ok = StreamContext::new("8.8.8.8".parse().unwrap(), 8080, StreamLayer::Stage1);
    assert!(matches!(plugin.on_connection(&ctx_ok).await, StreamPluginResult::Allow));
}

use edgion_resources::refs::ip_list_fields::PreparedIpListReferences;
// Detached convenience exists only in this mirrored test module.
impl GlobalConnectionIpRestrictionPlugin {
    pub(crate) fn new(config: &GlobalConnectionIpRestrictionConfig, namespace: &str) -> Result<Self, String> {
        let owner: edgion_resources::EdgionStreamPlugins = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "fixture", "namespace": namespace},
            "spec": {"plugins": [{"type": "GlobalConnectionIpRestriction", "config": config}]}
        }))
        .unwrap();
        let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
        let binding = prepared.bind_stream_plugins(&owner).unwrap();
        Self::from_bound(binding.global_entry(0).unwrap())
    }
}

#[tokio::test]
async fn owning_gcir_invalid_slots_keep_allow_intent_and_deny_set_drop() {
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    use edgion_resources::EdgionConfigData;
    let namespace = "r5c2-gcir";
    for key in [
        format!("{namespace}/bad/name"),
        "/r5c2-gcir-nets".into(),
        format!("{namespace}/good"),
    ] {
        let data: EdgionConfigData = serde_json::from_value(serde_json::json!({
            "metadata":{"name":"decoy","namespace":namespace},"spec":{"data":{"type":"IpList","config":{"items":[{"name":"set","cidrs":["198.51.100.8/32"]}]}}}
        })).unwrap();
        get_global_edgion_config_data_store().insert_raw_for_test(key, std::sync::Arc::new(data));
    }
    for bad in [
        serde_json::json!({"name":"bad/name"}),
        serde_json::json!({"name":"r5c2-gcir-nets","namespace":""}),
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
                "metadata":{"name":"actual","namespace":namespace},"spec":{"plugins":[{"type":"GlobalConnectionIpRestriction","config":serde_json::json!({"enable":true,"activeProfile":"main","profiles":{"main":config}})}]}
            })).unwrap();
            let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
            let bound = prepared.bind_stream_plugins(&owner).unwrap();
            let plugin = GlobalConnectionIpRestrictionPlugin::from_bound(bound.global_entry(0).unwrap()).unwrap();
            let context = StreamContext::new("198.51.100.8".parse().unwrap(), 18498, StreamLayer::Stage1);
            let result = plugin.on_connection(&context).await;
            assert_eq!(matches!(result, StreamPluginResult::Allow), allow, "{case} / {bad}");
        }
    }
}
#[test]
fn owning_gcir_invalid_owner_fails_separately_from_invalid_target() {
    let config = serde_json::json!({"denyRefs":[{"name":"bad/name"}]});
    let mut owner: edgion_resources::EdgionStreamPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"actual","namespace":""},"spec":{"plugins":[{"type":"GlobalConnectionIpRestriction","config":serde_json::json!({"enable":true,"activeProfile":"main","profiles":{"main":config}})}]}
    })).unwrap();
    let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
    let bound = prepared.bind_stream_plugins(&owner).unwrap();
    assert!(GlobalConnectionIpRestrictionPlugin::from_bound(bound.global_entry(0).unwrap()).is_err());
    owner.metadata.namespace = None;
    let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
    let bound = prepared.bind_stream_plugins(&owner).unwrap();
    assert!(GlobalConnectionIpRestrictionPlugin::from_bound(bound.global_entry(0).unwrap()).is_ok());
    owner.metadata.uid = Some("changed".into());
    assert!(prepared.bind_stream_plugins(&owner).is_err());
}

#[tokio::test]
async fn blank_reference_name_preserves_structural_constructor_failure() {
    for side in ["allowRefs", "denyRefs"] {
        let mut config = serde_json::json!({"defaultAction":"allow"});
        config[side] = serde_json::json!([{"name":""}]);
        let owner:edgion_resources::EdgionStreamPlugins=serde_json::from_value(serde_json::json!({
            "metadata":{"name":"actual","namespace":"default"},"spec":{"plugins":[{"type":"GlobalConnectionIpRestriction","config":serde_json::json!({"enable":true,"activeProfile":"main","profiles":{"main":config}})}]}
        })).unwrap();
        let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
        let binding = prepared.bind_stream_plugins(&owner).unwrap();
        let result = GlobalConnectionIpRestrictionPlugin::from_bound(binding.global_entry(0).unwrap());
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn owning_gcir_selector_invalid_slot_uses_base_and_invalid_owner_rejects() {
    for reference in [
        serde_json::json!({"name":"bad/name"}),
        serde_json::json!({"name":"selector","namespace":""}),
    ] {
        let owner: edgion_resources::EdgionStreamPlugins=serde_json::from_value(serde_json::json!({"metadata":{"name":"gcir"},"spec":{"plugins":[{
            "type":"GlobalConnectionIpRestriction","config":{"enable":true,"activeProfile":"base","activeProfileRef":reference,"profiles":{"base":{"defaultAction":"allow","deny":[{"name":"other-net","cidrs":["203.0.113.0/24"]}]},"other":{"defaultAction":"deny","deny":[{"name":"all","cidrs":["0.0.0.0/0"]}]}}}
        }]}})).unwrap();
        let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
        let binding = prepared.bind_stream_plugins(&owner).unwrap();
        let plugin = GlobalConnectionIpRestrictionPlugin::from_bound(binding.global_entry(0).unwrap()).unwrap();
        let context = StreamContext::new("198.51.100.9".parse().unwrap(), 18501, StreamLayer::Stage1);
        assert!(matches!(
            plugin.on_connection(&context).await,
            StreamPluginResult::Allow
        ));
        let mut owner = owner;
        owner.metadata.namespace = Some(String::new());
        let prepared = PreparedIpListReferences::from_stream_plugins(&owner);
        let binding = prepared.bind_stream_plugins(&owner).unwrap();
        assert!(GlobalConnectionIpRestrictionPlugin::from_bound(binding.global_entry(0).unwrap()).is_err());
    }
}
