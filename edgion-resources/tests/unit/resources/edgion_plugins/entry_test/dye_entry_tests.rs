use super::*;
use crate::resources::edgion_plugins::dye::{DyeOutcome, DyeRule, DyeSuite};
use crate::resources::edgion_plugins::EdgionPlugin;
use crate::resources::plugin_configs::CorsConfig;

fn dye_response(name: &str, on: &[DyeOutcome]) -> DyeSuite {
    DyeSuite {
        request: vec![],
        response: vec![DyeRule {
            name: name.to_string(),
            on: on.to_vec(),
            value: None,
            conditions: None,
        }],
    }
}

#[test]
fn entry_dye_serde_roundtrip() {
    let yaml = "type: Cors\nconfig: {}\ndye:\n  response:\n    - name: X-O\n      on: [success]\n";
    let entry: RequestFilterEntry = serde_yaml::from_str(yaml).unwrap();
    assert!(entry.dye.is_some());
    let json = serde_json::to_value(&entry).unwrap();
    assert_eq!(json["dye"]["response"][0]["name"], "X-O");
    // absent stays absent
    let e2 = RequestFilterEntry::new(EdgionPlugin::Cors(CorsConfig::default()));
    assert!(serde_json::to_value(&e2).unwrap().get("dye").is_none());
}

#[test]
fn validate_dye_uses_plugin_capabilities() {
    let mut entry = RequestFilterEntry::new(EdgionPlugin::Cors(CorsConfig::default()));
    entry.dye = Some(dye_response("X-O", &[DyeOutcome::FailOpen]));
    let err = entry.validate_dye().expect("cors records nothing");
    assert!(err.contains("cannot render in this stage and configuration"), "{err}");
    assert!(RequestFilterEntry::new(EdgionPlugin::Cors(CorsConfig::default()))
        .validate_dye()
        .is_none());
}

// `CanaryConfig` has no `Default` impl (its fields are all operator-required),
// so build a minimal valid instance directly rather than relying on `::default()`.
fn canary_config() -> crate::resources::plugin_configs::CanaryConfig {
    use crate::resources::plugin_configs::{CanaryConfig, DynUpstreamTarget, UpstreamFallback};
    use std::collections::HashMap;
    CanaryConfig {
        enable: true,
        active_profile: "stable".to_string(),
        profiles: HashMap::from([(
            "stable".to_string(),
            DynUpstreamTarget {
                name: "app-v1".to_string(),
                namespace: None,
            },
        )]),
        active_profile_ref: None,
        on_invalid: UpstreamFallback::Reject,
    }
}

#[test]
fn set_clobber_collision_across_entries_rejected() {
    // entry A: success dye on X-O; entry B: failure dye on x-o (case-insensitive hit)
    let mut a = RequestFilterEntry::new(EdgionPlugin::Canary(canary_config()));
    a.dye = Some(dye_response("X-O", &[DyeOutcome::Success]));
    let mut b = RequestFilterEntry::new(EdgionPlugin::Canary(canary_config()));
    b.dye = Some(dye_response("x-o", &[DyeOutcome::Fallback]));
    let err = dye_set_clobber_error([&a, &b].iter().map(|e| e.dye.as_ref())).expect("clobber must be rejected");
    assert!(err.contains("x-o") || err.contains("X-O"), "{err}");
}
