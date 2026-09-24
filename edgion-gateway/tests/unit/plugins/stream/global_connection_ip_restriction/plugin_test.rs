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
