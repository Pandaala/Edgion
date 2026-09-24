use super::*;
use crate::resources::edgion_plugins::plugin_configs::{CanaryConfig, DynUpstreamTarget, UpstreamFallback};
use std::collections::HashMap;

fn target(name: &str) -> DynUpstreamTarget {
    DynUpstreamTarget {
        name: name.to_string(),
        namespace: None,
    }
}

#[test]
fn type_name_and_tag_roundtrip() {
    let json = serde_json::json!({
        "type": "Canary",
        "config": {
            "enable": true,
            "activeProfile": "stable",
            "profiles": { "stable": { "name": "app-v1" } }
        }
    });
    let p: EdgionPlugin = serde_json::from_value(json).unwrap();
    assert_eq!(p.type_name(), "Canary");
    assert!(matches!(p, EdgionPlugin::Canary(_)));
    assert!(p.validation_error().is_none());
}

#[test]
fn validation_error_surfaces_bad_active_profile() {
    let cfg = CanaryConfig {
        enable: true,
        active_profile: "ghost".to_string(),
        profiles: HashMap::from([("stable".to_string(), target("app-v1"))]),
        active_profile_ref: None,
        on_invalid: UpstreamFallback::Reject,
    };
    let p = EdgionPlugin::Canary(cfg);
    assert!(p.validation_error().unwrap().contains("activeProfile"));
}
