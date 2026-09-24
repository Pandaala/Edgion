use super::*;
use crate::resources::edgion_plugins::plugin_configs::RequestAccessPolicyConfig;

#[test]
fn type_name_and_tag_roundtrip() {
    let json = serde_json::json!({
        "type": "RequestAccessPolicy",
        "config": {
            "defaultProfile": "normal",
            "profiles": {
                "normal": {
                    "requiredRuleGroups": [{
                        "name": "peer",
                        "anyOfRules": [{
                            "name": "internal",
                            "type": "directPeerIp",
                            "config": { "ips": ["10.0.0.0/8"] }
                        }]
                    }]
                }
            }
        }
    });
    let p: EdgionPlugin = serde_json::from_value(json).unwrap();
    assert_eq!(p.type_name(), "RequestAccessPolicy");
    assert!(matches!(p, EdgionPlugin::RequestAccessPolicy(_)));
    // validation_error delegates to the config.
    assert!(p.validation_error().is_none());
}

#[test]
fn validation_error_surfaces_bad_default_profile() {
    let cfg = RequestAccessPolicyConfig {
        default_profile: "ghost".into(),
        profiles: std::collections::HashMap::from([(
            "normal".to_string(),
            crate::resources::edgion_plugins::plugin_configs::RequestAccessProfile {
                required_rule_groups: vec![crate::resources::edgion_plugins::plugin_configs::RequiredRuleGroup {
                    name: "peer".to_string(),
                    any_of_rules: vec![crate::resources::edgion_plugins::plugin_configs::RequestAccessRule {
                        name: "internal".to_string(),
                        rule: crate::resources::edgion_plugins::plugin_configs::RequestAccessRuleType::DirectPeerIp(
                            crate::resources::edgion_plugins::plugin_configs::IpAccessRuleConfig {
                                ip: crate::resources::edgion_plugins::plugin_configs::IpAccessMatch {
                                    ips: vec!["10.0.0.0/8".to_string()],
                                    ..Default::default()
                                },
                                conditions: None,
                            },
                        ),
                    }],
                }],
            },
        )]),
        description: None,
        active_profile_ref: None,
        status: 403,
        message: None,
        resolved_profiles: None,
        resolution_warnings: vec![],
    };
    let p = EdgionPlugin::RequestAccessPolicy(cfg);
    assert!(p.validation_error().unwrap().contains("defaultProfile"));
}
