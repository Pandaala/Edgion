use std::collections::BTreeMap;

use edgion_resources::resources::edgion_config_data::{
    EdgionConfigDataRef, WafBundleMember, WafPolicyData, WafRuleBundleData,
};
use edgion_resources::resources::edgion_plugins::{ResolvedWafBundle, WafBundleRef, WafConfig, WafProfile};

/// Build one digest-pinned, Controller-resolved profile for Gateway unit tests.
pub(crate) fn config(rules: &str) -> WafConfig {
    config_in_namespace(rules, "default")
}

pub(crate) fn config_in_namespace(rules: &str, namespace: &str) -> WafConfig {
    let mut data = WafRuleBundleData {
        version: "test-v1".into(),
        profile: "test".into(),
        provenance: "test/fixture".into(),
        digest: None,
        roots: vec!["main.conf".into()],
        rules: vec![WafBundleMember {
            name: "main.conf".into(),
            content: rules.into(),
        }],
        phrase_assets: vec![],
    };
    data.digest = Some(data.compute_digest());
    WafConfig {
        policy_ref: EdgionConfigDataRef {
            name: "test-policy".into(),
            ..Default::default()
        },
        resolved_policy: Some(WafPolicyData {
            profiles: BTreeMap::from([(
                "default".into(),
                WafProfile {
                    bundle_refs: vec![WafBundleRef {
                        target: EdgionConfigDataRef {
                            name: "test-bundle".into(),
                            namespace: None,
                            ref_denied: None,
                        },
                        expected_digest: data.digest.clone(),
                        optional: false,
                    }],
                },
            )]),
            default_profile: "default".into(),
        }),
        selected_profile: Some("default".into()),
        resolved_ref_indices: vec![0],
        resolved_bundles: Some(vec![ResolvedWafBundle {
            identity: format!("{namespace}/test-bundle"),
            digest: data.compute_digest(),
            data,
        }]),
        owner_namespace: Some(namespace.into()),
        ..WafConfig::default()
    }
}
