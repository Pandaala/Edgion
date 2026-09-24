use edgion_resources::resources::edgion_plugins::plugin_configs::WafConfig;

#[test]
fn gateway_injection_hot_reload_config_strictly_compiles() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../edgion-tests/integration/conf/EdgionPlugins/Waf/DynamicTest/updates/to_injection/EdgionPlugins_waf-dynamic.yaml"
    ));
    let document: serde_yaml::Value = serde_yaml::from_str(source).unwrap();
    let config: WafConfig = serde_yaml::from_value(document["spec"]["waf"].clone()).unwrap();
    let report = config.inspect_support();
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
