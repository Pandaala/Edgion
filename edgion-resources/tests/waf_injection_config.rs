use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigData};
use edgion_resources::resources::edgion_plugins::plugin_configs::WafConfig;
use edgion_resources::resources::edgion_plugins::ResolvedWafBundle;

const FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../edgion-tests/integration/conf/EdgionPlugins/Waf/DynamicTest/updates/to_injection"
);

fn config_data(file: &str) -> EdgionConfigData {
    let path = format!("{FIXTURE_DIR}/{file}");
    serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// Mirror the Controller's policyRef resolution for the fixture's single profile.
fn resolve(config: &mut WafConfig) {
    let ConfigEntry::WafPolicy(policy) = config_data("EdgionConfigData_waf-policy.yaml").spec.data else {
        panic!("policy fixture must be a WafPolicy");
    };
    let bundle = config_data("EdgionConfigData_waf-rules-injection.yaml");
    let ConfigEntry::WafRuleBundle(data) = bundle.spec.data else {
        panic!("bundle fixture must be a WafRuleBundle");
    };
    let profile = policy.default_profile.clone();
    assert_eq!(policy.profiles[&profile].bundle_refs.len(), 1);
    config.owner_namespace = bundle.metadata.namespace.clone();
    config.selected_profile = Some(profile);
    config.resolved_ref_indices = vec![0];
    config.resolved_bundles = Some(vec![ResolvedWafBundle {
        identity: format!(
            "{}/{}",
            bundle.metadata.namespace.unwrap(),
            bundle.metadata.name.unwrap()
        ),
        digest: data.compute_digest(),
        data,
    }]);
    config.resolved_policy = Some(policy);
}

#[test]
fn gateway_injection_hot_reload_config_strictly_compiles() {
    let document: serde_yaml::Value = serde_yaml::from_str(
        &std::fs::read_to_string(format!("{FIXTURE_DIR}/EdgionPlugins_waf-dynamic.yaml")).unwrap(),
    )
    .unwrap();
    let mut config: WafConfig = serde_yaml::from_value(document["spec"]["waf"].clone()).unwrap();
    resolve(&mut config);
    // Support inspection covers inline rules only; inspect the resolved bundle text.
    let bundle = &config.resolved_bundles.as_ref().unwrap()[0].data;
    assert_eq!(bundle.rules.len(), 1);
    let inline = WafConfig {
        rules: bundle.rules[0].content.clone(),
        request_body: config.request_body.clone(),
        ..WafConfig::default()
    };
    let report = inline.inspect_support();
    assert!(
        report.analysis_complete && report.strict_validation_passed,
        "{report:?}"
    );
    assert_eq!(report.unavailable_rule_count, Some(0));
    assert_eq!(
        config
            .compile_generation()
            .unwrap()
            .rule_units()
            .map(|unit| unit.id())
            .collect::<Vec<_>>(),
        [3501, 3502]
    );
}
