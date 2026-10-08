use super::*;

fn plugin() -> EdgionPlugins {
    serde_json::from_value(serde_json::json!({
        "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins", "metadata":{"name":"owner"},
        "spec":{"waf":{"policyRef":{"name":"policy"},"activeProfileRef":{"name":"selector","namespace":"selectors"}}}
    }))
    .unwrap()
}
fn policy() -> EdgionConfigData {
    serde_json::from_value(serde_json::json!({
        "apiVersion":"edgion.io/v1", "kind":"EdgionConfigData", "metadata":{"name":"policy"},
        "spec":{"data":{"type":"WafPolicy","config":{"defaultProfile":"base","profiles":{"base":{"bundleRefs":[{"name":"rules"}]}}}}}
    })).unwrap()
}

#[test]
fn waf_loans_use_actual_sources_and_canonical_defaults() {
    let plugin = plugin();
    let fields = WafPluginReferences::from_plugins(&plugin).unwrap();
    let field = fields.policy().unwrap();
    assert_eq!(field.key(), "default/policy");
    assert_eq!(field.query().source().group_kind().kind(), "EdgionPlugins");
    assert_eq!(fields.selector().unwrap().unwrap().key(), "selectors/selector");
    let policy = policy();
    let fields = WafPolicyReferences::from_policy(&policy).unwrap();
    let field = fields.bundle("base", 0).unwrap().unwrap();
    assert_eq!(field.key(), "default/rules");
    assert_eq!(field.query().source().group_kind().kind(), "EdgionConfigData");
    assert_eq!(field.target().group_kind().group(), "edgion.io");
    assert_eq!(field.target().group_kind().kind(), "EdgionConfigData");
}

#[test]
fn waf_owners_and_slots_validate_separately_without_aliases() {
    for namespace in ["", "bad/ns", "Bad"] {
        let mut plugin = plugin();
        plugin.metadata.namespace = Some(namespace.into());
        assert!(WafPluginReferences::from_plugins(&plugin).is_err());
        plugin.metadata.namespace = None;
        plugin.spec.waf.as_mut().unwrap().policy_ref.namespace = Some(namespace.into());
        assert!(WafPluginReferences::from_plugins(&plugin).unwrap().policy().is_err());
        let mut policy = policy();
        let ConfigEntry::WafPolicy(data) = &mut policy.spec.data else {
            unreachable!()
        };
        data.profiles.get_mut("base").unwrap().bundle_refs[0].target.namespace = Some(namespace.into());
        assert!(WafPolicyReferences::from_policy(&policy)
            .unwrap()
            .bundle("base", 0)
            .unwrap()
            .is_err());
    }
    for name in ["", "bad/name", "Bad", "bad_name"] {
        let mut plugin = plugin();
        plugin.spec.waf.as_mut().unwrap().policy_ref.name = name.into();
        assert!(WafPluginReferences::from_plugins(&plugin).unwrap().policy().is_err());
    }
    for name in ["", "bad/name", "Bad", "bad_name"] {
        let mut plugin = plugin();
        plugin
            .spec
            .waf
            .as_mut()
            .unwrap()
            .active_profile_ref
            .as_mut()
            .unwrap()
            .name = name.into();
        assert!(WafPluginReferences::from_plugins(&plugin)
            .unwrap()
            .selector()
            .unwrap()
            .is_err());
        let mut policy = policy();
        let ConfigEntry::WafPolicy(data) = &mut policy.spec.data else {
            unreachable!()
        };
        data.profiles.get_mut("base").unwrap().bundle_refs[0].target.name = name.into();
        assert!(WafPolicyReferences::from_policy(&policy)
            .unwrap()
            .bundle("base", 0)
            .unwrap()
            .is_err());
    }
    let mut policy = policy();
    policy.spec.data = ConfigEntry::Misc(serde_json::json!({}));
    assert!(WafPolicyReferences::from_policy(&policy).is_err());
}
