//! Hermetic integrity and production admission of the original CRS wire fixture.

use std::path::Path;

use edgion_resources::resources::edgion_config_data::{WafPolicyData, WafRuleBundleData};
use edgion_resources::resources::edgion_plugins::plugin_configs::{ResolvedWafBundle, WafConfig};
use serde_json::Value;
use sha2::{Digest, Sha256};

const DIGEST: &str = "1eddcdfa0cbaa2299e1e2ddbdb44d2eb6a1c0c6f388226ecd4b6c078167923af";
const UNITS: &str = include_str!("../../edgion-waf/tests/fixtures/crs-structured-v4.25.1/units.json");

fn yaml(path: &Path) -> Value {
    serde_yaml::from_str(&std::fs::read_to_string(path).expect("fixture exists")).expect("typed YAML value")
}

#[test]
fn original_wire_fixture_preserves_original_bytes_and_strict_pinned_admission() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let relative = "conf/EdgionPlugins/Waf/DynamicTest";
    let local = root.join("integration").join(relative);
    let k8s = root.join("k8s").join(relative);
    for phase in ["to_original_on", "to_original_detection"] {
        for entry in std::fs::read_dir(local.join("updates").join(phase)).expect("phase fixtures") {
            let entry = entry.expect("fixture entry");
            assert_eq!(
                std::fs::read(entry.path()).unwrap(),
                std::fs::read(k8s.join("updates").join(phase).join(entry.file_name())).unwrap()
            );
        }
    }
    for list in ["original_bundles.txt", "resources.txt"] {
        let local_bytes = std::fs::read(local.join("delete").join(list)).unwrap();
        let k8s_bytes = std::fs::read(k8s.join("delete").join(list)).unwrap();
        if list == "original_bundles.txt" {
            assert_eq!(local_bytes, k8s_bytes);
        }
        for bytes in [local_bytes, k8s_bytes] {
            assert_eq!(
                String::from_utf8(bytes)
                    .unwrap()
                    .contains("EdgionConfigData/edgion-default/waf-rules-original-crs\n"),
                list == "original_bundles.txt",
                "scoped cleanup must not be repeated by the strict final list"
            );
        }
    }
    let on = local.join("updates/to_original_on");
    let data: WafRuleBundleData = serde_json::from_value(
        yaml(&on.join("EdgionConfigData_waf-rules-original-crs.yaml"))["spec"]["data"]["config"].clone(),
    )
    .unwrap();
    assert_eq!(data.digest.as_deref(), Some(DIGEST));
    assert_eq!(data.compute_digest(), DIGEST);
    data.validate().expect("original fixture is a valid pinned bundle");
    let source = &data.rules[0].content;
    let originals: Value = serde_json::from_str(UNITS).unwrap();
    for id in [933200, 944260, 920370] {
        let unit = originals["units"]
            .as_array()
            .unwrap()
            .iter()
            .find(|unit| unit["id"] == id)
            .unwrap();
        assert!(
            source.contains(unit["raw"].as_str().unwrap()),
            "original {id} bytes changed"
        );
    }
    let starts = [
        source.find("SecRule TX:BLOCKING_PARANOIA_LEVEL \"@ge 1\"").unwrap(),
        source.find("SecRule TX:BLOCKING_PARANOIA_LEVEL \"@ge 2\"").unwrap(),
        source.find("SecRule TX:BLOCKING_INBOUND_ANOMALY_SCORE").unwrap(),
        source.len(),
    ];
    for (span, expected) in starts.windows(2).zip([
        "17e65c546f5656cac1742152e90c75ef85d6caf40700e2fd19bf05794f58492d",
        "84329ec85ad4ad55c24f2d0501ac091b62e969cb528f856cd9535ff679a37ba9",
        "47a48caaabeea30d4a7e40a893e986042b4e512fefa6d44e1c51d0b32f0533f6",
    ]) {
        assert_eq!(
            hex::encode(Sha256::digest(&source.as_bytes()[span[0]..span[1]])),
            expected
        );
    }
    let policy: WafPolicyData =
        serde_json::from_value(yaml(&on.join("EdgionConfigData_waf-policy.yaml"))["spec"]["data"]["config"].clone())
            .unwrap();
    assert_eq!(
        policy.profiles["original"].bundle_refs[0].expected_digest.as_deref(),
        Some(DIGEST)
    );
    for (phase, mode) in [("to_original_on", "on"), ("to_original_detection", "detectionOnly")] {
        let value = yaml(&local.join("updates").join(phase).join("EdgionPlugins_waf-dynamic.yaml"));
        assert_eq!(value["spec"]["waf"]["mode"], mode);
        let mut config: WafConfig = serde_json::from_value(value["spec"]["waf"].clone()).unwrap();
        config.owner_namespace = Some("edgion-default".into());
        config.selected_profile = Some("original".into());
        config.resolved_ref_indices = vec![0];
        config.resolved_policy = Some(policy.clone());
        config.resolved_bundles = Some(vec![ResolvedWafBundle {
            identity: "edgion-default/waf-rules-original-crs".into(),
            digest: DIGEST.into(),
            data: data.clone(),
        }]);
        let generation = config
            .compile_generation()
            .expect("original full-body fixture strictly admitted");
        assert_eq!(
            generation.rule_units().map(|unit| unit.id()).collect::<Vec<_>>(),
            [9900201, 9900202, 9900203, 933200, 944260, 920370, 949060, 949061, 949110]
        );
        assert!(generation.json_inspection_unsupported().is_empty());
        assert!(generation.xml_inspection_unsupported().is_empty());
    }
}
