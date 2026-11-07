//! Digest-bound static selection and explicit language omission acceptance.
#[allow(dead_code)]
#[path = "../../edgion-waf/tests/common/structured_corpus.rs"]
mod corpus;
#[allow(dead_code)]
#[path = "common/supported_crs.rs"]
mod profile;

use bytes::Bytes;
use edgion_resources::resources::edgion_config_data::WafRuleSelection;
use edgion_waf::seclang::{
    BodyProcessorSelection, CompiledGeneration, EvaluationMode, RequestBody, RequestHead, RequestHeader,
    RuleCheckPolicy, RuleEngineMode, RuntimeLimits, Transaction,
};
use serde::Deserialize;
use std::collections::BTreeSet;

const JAVA: &str =
    include_str!("../../config/waf/crs-4.25.1-supported-v1/language-selection-examples/java-without-php.yaml");
const PHP: &str =
    include_str!("../../config/waf/crs-4.25.1-supported-v1/language-selection-examples/php-without-java.yaml");
const PHP_OMISSIONS: &[u64] = &[
    933100, 933120, 933130, 933131, 933135, 933140, 933150, 933151, 933152, 933153, 933160, 933161, 933170, 933180,
    933190, 933210, 933211,
];
const JAVA_OMISSIONS: &[u64] = &[
    944100, 944110, 944120, 944130, 944200, 944210, 944240, 944250, 944260, 944300,
];
const CASES: &str = include_str!("../../edgion-waf/tests/fixtures/crs-structured-v4.25.1/cases.json");
#[derive(Deserialize)]
struct Cases {
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    rule_id: u64,
    media: String,
    kind: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    body_bytes: Vec<u8>,
    hit: bool,
    referer: Option<String>,
}
fn observe_case(generation: &CompiledGeneration, case: &Case) -> corpus::Observation {
    let body = if case.body_bytes.is_empty() {
        case.body.as_bytes()
    } else {
        &case.body_bytes
    };
    let result = corpus::observe(
        generation,
        corpus::Input {
            body,
            xml: case.media == "xml",
            referer: case.referer.as_deref(),
        },
        RuleEngineMode::DetectionOnly,
        EvaluationMode::Accelerated,
        false,
        &[],
    )
    .unwrap();
    assert!(result.body_error.is_none(), "{}", case.name);
    result
}
fn has(generation: &CompiledGeneration, case: &Case) -> bool {
    observe_case(generation, case)
        .outcome
        .findings()
        .iter()
        .any(|finding| finding.rule_id() == case.rule_id)
}
fn score(result: &corpus::Observation, key: &str) -> i64 {
    result
        .tx
        .get(key)
        .map(|bytes| std::str::from_utf8(bytes).unwrap().parse().unwrap())
        .unwrap_or(0)
}
fn omission_level(id: u64) -> usize {
    match id {
        933151 | 933152 | 933153 | 944200 | 944210 | 944240 | 944250 | 944260 => 2,
        933131 | 933161 | 933190 | 933211 | 944300 => 3,
        _ => 1,
    }
}
fn raw_magic(generation: &CompiledGeneration) -> bool {
    let bytes = Bytes::from_static(b"\xac\xed\x00\x05");
    let headers = [
        RequestHeader::new(b"content-type", b"application/octet-stream"),
        RequestHeader::new(b"host", b"selection.test"),
        RequestHeader::new(b"content-length", b"4"),
    ];
    let head = RequestHead::new(
        b"POST / HTTP/1.1",
        b"POST",
        b"/",
        b"/",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"selection",
        &headers,
    );
    let mut tx = Transaction::new_with_policy(
        generation,
        head,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::DetectionOnly,
        RuleCheckPolicy::default(),
    )
    .unwrap();
    tx.execute_request_headers().unwrap();
    tx.publish_request_body(RequestBody::new(bytes, BodyProcessorSelection::Raw))
        .unwrap();
    tx.execute_request_body()
        .unwrap()
        .findings()
        .iter()
        .any(|finding| finding.rule_id() == 944200)
}

#[test]
fn language_examples_have_exact_digest_bound_omissions_and_retained_attack_coverage() {
    assert_eq!(
        corpus::sha256(CASES.as_bytes()),
        "4b94ff2fc6ab2305200894f5bb996b989a3181999183bb5c91d8ac755a0cc12d"
    );
    let cases = serde_json::from_str::<Cases>(CASES).unwrap().cases;
    let package = profile::load().unwrap();
    assert_eq!(
        package.core.compute_digest(),
        "eaccaa84c1126f3cc0a12c4fcdad31b98a4a592b2716f11038774f73279242b7"
    );
    for profile_name in ["standard", "strict"] {
        let config = profile::config(&package, profile_name).unwrap();
        let baseline = config.compile_generation().unwrap();
        let baseline_ids: Vec<_> = baseline.rule_units().map(|u| u.id()).collect();
        assert_eq!(baseline_ids.len(), 567);
        for (document, omissions) in [(JAVA, PHP_OMISSIONS), (PHP, JAVA_OMISSIONS)] {
            let mut configured = config.clone();
            let selected = yaml_serde::from_str::<WafRuleSelection>(document).unwrap();
            configured
                .resolved_policy
                .as_mut()
                .unwrap()
                .profiles
                .get_mut(profile_name)
                .unwrap()
                .rule_selection = selected;
            let generation = configured.compile_generation().unwrap();
            let expected = baseline_ids
                .iter()
                .copied()
                .filter(|id| !omissions.contains(id))
                .collect::<Vec<_>>();
            assert_eq!(generation.rule_units().map(|u| u.id()).collect::<Vec<_>>(), expected);
            assert_eq!(
                config.resolved_bundles.as_ref().unwrap()[1].digest,
                configured.resolved_bundles.as_ref().unwrap()[1].digest
            );
            let mut positive_witnesses = BTreeSet::new();
            for case in cases
                .iter()
                .filter(|case| omissions.contains(&case.rule_id) && case.kind != "malformed")
            {
                let before = observe_case(&baseline, case);
                let after = observe_case(&generation, case);
                let baseline_hit = before
                    .outcome
                    .findings()
                    .iter()
                    .any(|finding| finding.rule_id() == case.rule_id);
                assert!(
                    !after
                        .outcome
                        .findings()
                        .iter()
                        .any(|finding| finding.rule_id() == case.rule_id),
                    "excluded {} remained executable",
                    case.name
                );
                let removed: Vec<_> = before
                    .outcome
                    .findings()
                    .iter()
                    .filter(|finding| omissions.contains(&finding.rule_id()))
                    .collect();
                for level in 1..=4 {
                    let key = format!("inbound_anomaly_score_pl{level}");
                    let delta = 5 * removed
                        .iter()
                        .filter(|finding| omission_level(finding.rule_id()) == level)
                        .count() as i64;
                    assert_eq!(
                        score(&before, &key) - score(&after, &key),
                        delta,
                        "score delta {} {key}",
                        case.name
                    );
                }
                let key = if omissions == PHP_OMISSIONS {
                    "php_injection_score"
                } else {
                    "rce_score"
                };
                assert_eq!(
                    score(&before, key) - score(&after, key),
                    5 * removed.len() as i64,
                    "attack-family score delta {}",
                    case.name
                );
                if profile_name == "strict" {
                    assert_eq!(baseline_hit, case.hit, "baseline witness {}", case.name);
                    if baseline_hit {
                        positive_witnesses.insert(case.rule_id);
                    }
                }
            }
            if omissions.contains(&944200) {
                let baseline_hit = raw_magic(&baseline);
                assert!(!raw_magic(&generation));
                if profile_name == "strict" {
                    assert!(baseline_hit);
                    positive_witnesses.insert(944200);
                }
            }
            if profile_name == "strict" {
                assert_eq!(positive_witnesses, omissions.iter().copied().collect());
            }
            for case in cases.iter().filter(|case| {
                [933200, 934170, 944150, 941100, 942100, 932105, 921110].contains(&case.rule_id)
                    && case.kind != "malformed"
            }) {
                assert_eq!(
                    has(&generation, case),
                    has(&baseline, case),
                    "retained protection {}",
                    case.name
                );
            }
            for (body, xml) in [
                (br#"{"probe":"safe"}"#.as_slice(), false),
                (b"<r a='safe'>safe</r>".as_slice(), true),
            ] {
                let input = corpus::Input {
                    body,
                    xml,
                    referer: None,
                };
                let before = corpus::observe(
                    &baseline,
                    input,
                    RuleEngineMode::DetectionOnly,
                    EvaluationMode::Accelerated,
                    false,
                    &[],
                )
                .unwrap();
                let after = corpus::observe(
                    &generation,
                    input,
                    RuleEngineMode::DetectionOnly,
                    EvaluationMode::Accelerated,
                    true,
                    &[],
                )
                .unwrap();
                assert_eq!(before.outcome.findings(), after.outcome.findings());
                assert_eq!(before.outcome.intervention(), after.outcome.intervention());
                assert_eq!(before.tx, after.tx);
                assert_eq!(before.body_error, after.body_error);
                assert!(after.outcome.evaluated_units() <= before.outcome.evaluated_units());
            }
        }
    }
}
