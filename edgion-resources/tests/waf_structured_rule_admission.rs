//! Strict resource admission of the same hermetic original units used by leaf coverage.
#[allow(dead_code)]
#[path = "../../edgion-waf/tests/common/structured_corpus.rs"]
mod corpus;

use std::collections::BTreeMap;

use edgion_resources::resources::edgion_config_data::{
    EdgionConfigDataRef, WafBundleMember, WafBundleRef, WafPolicyData, WafProfile, WafRuleBundleData,
};
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    ResolvedWafBundle, WafConfig, WafRequestBodyInspection,
};
use edgion_waf::seclang::{EvaluationMode, RuleEngineMode};

fn config() -> WafConfig {
    let original = corpus::corpus();
    let source = format!(
        "{}{}",
        corpus::SETUP,
        original.units.iter().map(|unit| unit.raw.as_str()).collect::<String>()
    );
    let data = WafRuleBundleData {
        version: "4.25.1".into(),
        profile: "structured-original".into(),
        provenance: "Unchanged original CRS units; declared supplemental setup and source-visibility witnesses".into(),
        digest: None,
        roots: vec!["structured.conf".into()],
        rules: vec![WafBundleMember {
            name: "structured.conf".into(),
            content: source,
        }],
        phrase_assets: corpus::ASSETS
            .iter()
            .map(|(name, content)| WafBundleMember {
                name: (*name).into(),
                content: (*content).into(),
            })
            .collect(),
    };
    let digest = data.compute_digest();
    let bundle_ref = WafBundleRef {
        target: EdgionConfigDataRef {
            name: "crs-structured".into(),
            ..Default::default()
        },
        expected_digest: Some(digest.clone()),
        optional: false,
    };
    let mut config = WafConfig {
        policy_ref: EdgionConfigDataRef {
            name: "structured-policy".into(),
            ..Default::default()
        },
        owner_namespace: Some("structured".into()),
        selected_profile: Some("original".into()),
        resolved_ref_indices: vec![0],
        resolved_policy: Some(WafPolicyData {
            default_profile: "original".into(),
            profiles: BTreeMap::from([(
                "original".into(),
                WafProfile {
                    bundle_refs: vec![bundle_ref],
                },
            )]),
        }),
        resolved_bundles: Some(vec![ResolvedWafBundle {
            identity: "structured/crs-structured".into(),
            digest,
            data,
        }]),
        ..Default::default()
    };
    config.request_body.inspection = WafRequestBodyInspection::Full;
    config
}

#[test]
fn original_structured_profile_passes_full_resource_admission_with_exact_ids() {
    let config = config();
    let generation = config
        .compile_generation()
        .expect("original structured profile strictly admitted");
    let expected = corpus::corpus().units.iter().map(|unit| unit.id).collect::<Vec<_>>();
    let actual = generation
        .rule_units()
        .filter(|unit| unit.id() < 9900001)
        .map(|unit| unit.id())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert_eq!(generation.rule_units().count(), 199);
    assert!(generation.json_inspection_unsupported().is_empty());
    assert!(generation.xml_inspection_unsupported().is_empty());
    for (body, xml) in [
        (br#"{"probe":"phar://phpinfo.zip/phpinfo.txt"}"#.as_slice(), false),
        (b"<r>phar://phpinfo.zip/phpinfo.txt</r>".as_slice(), true),
    ] {
        let observation = corpus::observe(
            &generation,
            corpus::Input {
                body,
                xml,
                referer: None,
            },
            RuleEngineMode::On,
            EvaluationMode::Accelerated,
            true,
            &[],
        )
        .expect("production pass-default structured execution");
        assert!(observation.body_error.is_none());
        assert_eq!(
            observation.tx.get("supplement_raw_absent").map(Vec::as_slice),
            Some(b"1".as_slice())
        );
        assert!(observation
            .outcome
            .findings()
            .iter()
            .any(|finding| finding.rule_id() == 933200));
        // WafConfig's existing pass defaults resolve original `block` as pass in this profile.
        assert_eq!(observation.outcome.intervention(), None);
    }
}

#[test]
fn original_structured_profile_cannot_bypass_bundle_digest_pins() {
    let mut config = config();
    config.resolved_bundles.as_mut().expect("resolved fixture bundle")[0]
        .data
        .rules[0]
        .content
        .push('\n');
    assert!(
        config.compile_generation().is_err(),
        "changed original bundle bytes require an updated reviewed pin"
    );
}
