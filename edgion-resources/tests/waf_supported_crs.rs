//! Strict acceptance of the explicit product profile, including excluded-action rejection.
#[allow(dead_code)]
#[path = "../../edgion-waf/tests/common/structured_corpus.rs"]
mod corpus;
#[allow(dead_code)]
#[path = "common/supported_crs.rs"]
mod profile;
use edgion_waf::seclang::{EvaluationMode, RuleEngineMode};

#[test]
fn supported_package_is_reproducible_and_keeps_exact_original_order() {
    profile::generate(true).unwrap();
    profile::generate_wire(true).unwrap();
    let package = profile::load().unwrap();
    for name in ["standard", "strict"] {
        let config = profile::config(&package, name).unwrap();
        let generation = config.compile_generation().unwrap();
        let expected = [profile::TUNING_ID, 900990]
            .into_iter()
            .chain(
                package
                    .manifest
                    .units
                    .iter()
                    .filter(|unit| unit.excluded_capabilities.is_empty())
                    .map(|unit| unit.id),
            )
            .collect::<Vec<_>>();
        assert_eq!(
            generation.rule_units().map(|unit| unit.id()).collect::<Vec<_>>(),
            expected
        );
        assert!(generation.json_inspection_unsupported().is_empty());
        assert!(generation.xml_inspection_unsupported().is_empty());
        for (body, xml, attack) in [
            (br#"{"probe":"safe"}"#.as_slice(), false, false),
            (br#"{"probe":"phar://payload"}"#.as_slice(), false, true),
            (b"<r a='safe'>safe</r>".as_slice(), true, false),
            (b"<r a='phar://payload'/>".as_slice(), true, true),
        ] {
            for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
                let result = corpus::observe(
                    &generation,
                    corpus::Input {
                        body,
                        xml,
                        referer: None,
                    },
                    mode,
                    EvaluationMode::Accelerated,
                    true,
                    &[],
                )
                .unwrap();
                assert!(result.body_error.is_none());
                assert_eq!(result.outcome.findings().iter().any(|f| f.rule_id() == 933200), attack);
                assert_eq!(result.outcome.findings().iter().any(|f| f.rule_id() == 949110), attack);
                assert_eq!(
                    result.outcome.intervention().is_some(),
                    attack && mode == RuleEngineMode::On
                );
            }
        }
    }
}

#[test]
fn excluded_units_still_fail_strict_admission_and_pins_are_not_optional() {
    let package = profile::load().unwrap();
    for id in [901320, 905100, 905110] {
        let mut config = profile::config(&package, "strict").unwrap();
        let bundle = &mut config.resolved_bundles.as_mut().unwrap()[1];
        let unit = package.manifest.units.iter().find(|u| u.id == id).unwrap();
        let member = package.original.iter().find(|m| m.name == unit.source).unwrap();
        bundle.data.rules[0]
            .content
            .push_str(&member.content[unit.start..unit.end]);
        bundle.data.digest = Some(bundle.data.compute_digest());
        bundle.digest = bundle.data.compute_digest();
        config
            .resolved_policy
            .as_mut()
            .unwrap()
            .profiles
            .get_mut("strict")
            .unwrap()
            .bundle_refs[1]
            .expected_digest = Some(bundle.digest.clone());
        let error = config
            .compile_generation()
            .expect_err("unsupported restored unit must reject");
        assert!(error.contains("unavailable"), "{id}: {error}");
    }
    let mut config = profile::config(&package, "strict").unwrap();
    config.resolved_bundles.as_mut().unwrap()[1].data.rules[0]
        .content
        .push('\n');
    assert!(config.compile_generation().is_err());
    let mut config = profile::config(&package, "strict").unwrap();
    let bundle = &mut config.resolved_bundles.as_mut().unwrap()[1];
    bundle.data.rules = package.original.clone();
    bundle.data.roots = bundle.data.rules.iter().map(|m| m.name.clone()).collect();
    bundle.data.digest = Some(bundle.data.compute_digest());
    bundle.digest = bundle.data.compute_digest();
    config
        .resolved_policy
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("strict")
        .unwrap()
        .bundle_refs[1]
        .expected_digest = Some(bundle.digest.clone());
    assert!(
        config.compile_generation().is_err(),
        "full original corpus must remain rejected"
    );
}

#[test]
fn required_assets_markers_and_supported_mode_cannot_be_weakened() {
    let package = profile::load().unwrap();
    for variant in ["asset", "marker", "response", "pin", "prefix"] {
        let mut config = profile::config(&package, "strict").unwrap();
        if variant == "prefix" {
            config.request_body.inspection =
                edgion_resources::resources::edgion_plugins::plugin_configs::WafRequestBodyInspection::Prefix;
        } else if variant == "pin" {
            config
                .resolved_policy
                .as_mut()
                .unwrap()
                .profiles
                .get_mut("strict")
                .unwrap()
                .bundle_refs[1]
                .expected_digest = Some("0".repeat(64));
        } else {
            let bundle = &mut config.resolved_bundles.as_mut().unwrap()[1];
            match variant {
                "asset" => bundle.data.phrase_assets.retain(|m| m.name != "java-classes.data"),
                "marker" => {
                    let m = bundle
                        .data
                        .rules
                        .iter_mut()
                        .find(|m| m.name == "RESPONSE-959-BLOCKING-EVALUATION.conf")
                        .unwrap();
                    m.content = m
                        .content
                        .replace("SecMarker \"END-RESPONSE-959-BLOCKING-EVALUATION\"", "");
                }
                "response" => {
                    let unit = package
                        .manifest
                        .units
                        .iter()
                        .find(|u| u.excluded_capabilities == ["FullResponseBody"])
                        .unwrap();
                    let source = package.original.iter().find(|m| m.name == unit.source).unwrap();
                    bundle.data.rules[0]
                        .content
                        .push_str(&source.content[unit.start..unit.end]);
                }
                _ => panic!("unexpected test variant"),
            }
            bundle.data.digest = Some(bundle.data.compute_digest());
            bundle.digest = bundle.data.compute_digest();
            config
                .resolved_policy
                .as_mut()
                .unwrap()
                .profiles
                .get_mut("strict")
                .unwrap()
                .bundle_refs[1]
                .expected_digest = Some(bundle.digest.clone());
        }
        assert!(
            config.compile_generation().is_err(),
            "broken {variant} must strictly reject even with recomputed digest"
        );
    }
}
