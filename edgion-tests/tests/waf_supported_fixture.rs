//! Product bundle identity, strict Full admission and local/Kubernetes fixture parity.
use edgion_resources::resources::edgion_config_data::{WafPolicyData, WafRuleBundleData};
use edgion_resources::resources::edgion_plugins::plugin_configs::{ResolvedWafBundle, WafConfig};
#[test]
fn supported_wire_keeps_product_digests_and_only_adds_declared_guard() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let package = root.join("config/waf/crs-4.25.1-supported-v1/generated");
    let product: WafRuleBundleData = serde_json::from_value(
        serde_yaml::from_str::<serde_json::Value>(
            &std::fs::read_to_string(package.join("EdgionConfigData_crs-supported-core.yaml")).unwrap(),
        )
        .unwrap()["spec"]["data"]["config"]
            .clone(),
    )
    .unwrap();
    let local = root.join("edgion-tests/integration/conf/EdgionPlugins/Waf/DynamicTest");
    let k8s = root.join("edgion-tests/k8s/conf/EdgionPlugins/Waf/DynamicTest");
    let mut outputs = std::collections::BTreeMap::new();
    for dir in [
        "updates/to_supported_on",
        "updates/to_supported_detection",
        "updates/to_supported_standard",
        "updates/to_supported_invalid",
    ] {
        for entry in std::fs::read_dir(local.join(dir)).unwrap() {
            let entry = entry.unwrap();
            let bytes = std::fs::read(entry.path()).unwrap();
            assert_eq!(bytes, std::fs::read(k8s.join(dir).join(entry.file_name())).unwrap());
            outputs.insert(
                format!("{dir}/{}", entry.file_name().to_str().unwrap()),
                String::from_utf8(bytes).unwrap(),
            );
        }
    }
    assert_eq!(
        std::fs::read(local.join("delete/supported_resources.txt")).unwrap(),
        std::fs::read(k8s.join("delete/supported_resources.txt")).unwrap()
    );
    let policy: WafPolicyData = serde_json::from_value(
        serde_yaml::from_str::<serde_json::Value>(&outputs["updates/to_supported_on/EdgionConfigData_waf-policy.yaml"])
            .unwrap()["spec"]["data"]["config"]
            .clone(),
    )
    .unwrap();
    for (phase, p) in [
        ("to_supported_on", "strict"),
        ("to_supported_detection", "strict"),
        ("to_supported_standard", "standard"),
    ] {
        let value: serde_json::Value =
            serde_yaml::from_str(&outputs[&format!("updates/{phase}/EdgionPlugins_waf-dynamic.yaml")]).unwrap();
        let mut config: WafConfig = serde_json::from_value(value["spec"]["waf"].clone()).unwrap();
        config.owner_namespace = Some("edgion-default".into());
        config.selected_profile = Some(p.into());
        config.resolved_ref_indices = vec![0, 1, 2];
        config.resolved_policy = Some(policy.clone());
        let mut bundles = Vec::new();
        for reference in &policy.profiles[p].bundle_refs {
            let name = &reference.target.name;
            let value: serde_json::Value =
                serde_yaml::from_str(&outputs[&format!("updates/to_supported_on/EdgionConfigData_{name}.yaml")])
                    .unwrap();
            let data: WafRuleBundleData = serde_json::from_value(value["spec"]["data"]["config"].clone()).unwrap();
            assert!(!reference.optional);
            assert_eq!(
                reference.expected_digest.as_deref(),
                Some(data.compute_digest().as_str())
            );
            if name == "crs-supported-core" {
                assert_eq!(data.compute_digest(), product.compute_digest());
            }
            if name == &format!("crs-supported-{p}") {
                assert_eq!(
                    data.compute_digest(),
                    serde_json::from_value::<WafRuleBundleData>(
                        serde_yaml::from_str::<serde_json::Value>(
                            &std::fs::read_to_string(package.join(format!("EdgionConfigData_crs-supported-{p}.yaml")))
                                .unwrap()
                        )
                        .unwrap()["spec"]["data"]["config"]
                            .clone()
                    )
                    .unwrap()
                    .compute_digest()
                );
            }
            bundles.push(ResolvedWafBundle {
                identity: format!("edgion-default/{name}"),
                digest: data.compute_digest(),
                data,
            });
        }
        config.resolved_bundles = Some(bundles);
        let generation = config.compile_generation().unwrap();
        assert_eq!(generation.rule_units().count(), 568);
        assert_eq!(generation.rule_units().next().unwrap().id(), 9900302);
        assert!(generation.json_inspection_unsupported().is_empty());
        assert!(generation.xml_inspection_unsupported().is_empty());
    }
}
