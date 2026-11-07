//! Hermetic supplemental structured-input coverage of unchanged CRS 4.25.1 units.
#[path = "common/structured_corpus.rs"]
mod corpus;

use std::collections::{BTreeMap, BTreeSet};

use corpus::{Input, Observation};
use edgion_waf::seclang::{
    BodyError, BodyProcessorSelection, CompiledGeneration, EvaluationMode, RuleEngineMode, RuntimeError,
};
use serde::Deserialize;

const CASES: &str = include_str!("fixtures/crs-structured-v4.25.1/cases.json");
const CASES_SHA256: &str = "4b94ff2fc6ab2305200894f5bb996b989a3181999183bb5c91d8ac755a0cc12d";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: String,
    rules_revision: String,
    context: String,
    coverage: Vec<Coverage>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Coverage {
    rule_id: u64,
    media: String,
    status: String,
    case_names: Vec<String>,
    rationale: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    source: serde_json::Value,
    variants: Vec<String>,
    expect_tx: BTreeMap<String, String>,
    absent_tx: Vec<String>,
    referer: Option<String>,
}

fn manifest() -> Manifest {
    assert_eq!(
        corpus::sha256(CASES.as_bytes()),
        CASES_SHA256,
        "review fixed expectations and provenance"
    );
    let manifest: Manifest = serde_json::from_str(CASES).expect("reviewed supplemental manifest");
    assert_eq!(manifest.schema, "edgion-waf-structured-supplement/v1");
    assert_eq!(manifest.rules_revision, "3b89d5a05322f448b4b74b9cadc5fb05ac6915ad");
    assert!(manifest.context.contains("production resource pass defaults"));
    manifest
}

impl Case {
    fn bytes(&self) -> &[u8] {
        if self.body_bytes.is_empty() {
            self.body.as_bytes()
        } else {
            assert!(self.body.is_empty());
            &self.body_bytes
        }
    }

    fn input<'a>(&'a self, bytes: &'a [u8]) -> Input<'a> {
        Input {
            body: bytes,
            xml: self.media == "xml",
            referer: self.referer.as_deref(),
        }
    }

    fn keys(&self) -> Vec<String> {
        self.expect_tx.keys().chain(&self.absent_tx).cloned().collect()
    }
}

fn assert_valid(case: &Case, observation: &Observation, on: bool) {
    assert_eq!(observation.body_error, None, "{} must parse successfully", case.name);
    assert_eq!(
        observation.tx.get("supplement_raw_absent").map(Vec::as_slice),
        Some(b"1".as_slice()),
        "{} raw source must be invisible",
        case.name
    );
    let hit = observation
        .outcome
        .findings()
        .iter()
        .any(|finding| finding.rule_id() == case.rule_id);
    assert_eq!(hit, case.hit, "original rule {} case {}", case.rule_id, case.name);
    assert_eq!(observation.outcome.omitted_findings(), 0);
    let intervention = observation
        .outcome
        .intervention()
        .map(|value| (value.rule_id(), value.status()));
    let expected = (on && case.hit && case.kind == "activation").then_some((case.rule_id, 403));
    assert_eq!(
        intervention, expected,
        "{} explicit test deny-default semantics",
        case.name
    );
    for (key, value) in &case.expect_tx {
        assert_eq!(
            observation.tx.get(key).map(Vec::as_slice),
            Some(value.as_bytes()),
            "{} TX {key}",
            case.name
        );
    }
    for key in &case.absent_tx {
        assert_eq!(
            observation.tx.get(key),
            None,
            "{} rolled-back/absent TX {key}",
            case.name
        );
    }
}

fn assert_variants(case: &Case, generation: &CompiledGeneration) {
    for variant in &case.variants {
        let mut value: serde_json::Value = serde_json::from_slice(case.bytes()).expect("valid variant JSON");
        let body = if variant == "escaped" {
            fn render(value: &serde_json::Value) -> String {
                match value {
                    serde_json::Value::String(text) => format!(
                        "\"{}\"",
                        text.chars()
                            .map(|ch| format!("\\u{:04x}", ch as u32))
                            .collect::<String>()
                    ),
                    serde_json::Value::Object(fields) => format!(
                        "{{{}}}",
                        fields
                            .iter()
                            .map(|(key, value)| format!(
                                "{}:{}",
                                render(&serde_json::Value::String(key.clone())),
                                render(value)
                            ))
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    _ => panic!("reviewed variants contain only objects and strings"),
                }
            }
            render(&value)
        } else {
            assert_eq!(variant, "nested");
            value = serde_json::json!({"user":value});
            serde_json::to_string(&value).expect("valid nested JSON")
        };
        let reference = corpus::observe(
            generation,
            case.input(body.as_bytes()),
            RuleEngineMode::DetectionOnly,
            EvaluationMode::Reference,
            false,
            &case.keys(),
        )
        .expect("variant reference publication");
        assert_valid(case, &reference, false);
        let streamed = corpus::observe(
            generation,
            case.input(body.as_bytes()),
            RuleEngineMode::DetectionOnly,
            EvaluationMode::Accelerated,
            true,
            &case.keys(),
        )
        .expect("variant streamed publication");
        assert_valid(case, &streamed, false);
        assert_eq!(streamed, reference, "{} {variant}", case.name);
    }
}

#[test]
fn original_ids_hashes_and_supplemental_statuses_are_frozen() {
    let corpus = corpus::corpus();
    let manifest = manifest();
    assert_eq!(manifest.cases.len(), 420);
    assert_eq!(manifest.coverage.len(), 198);
    let original_ids = corpus.units.iter().map(|unit| unit.id).collect::<BTreeSet<_>>();
    assert_eq!(original_ids.len(), 195);
    let json_ids = manifest
        .coverage
        .iter()
        .filter(|entry| entry.media == "json")
        .map(|entry| entry.rule_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(json_ids, original_ids);
    let xml_ids = manifest
        .coverage
        .iter()
        .filter(|entry| entry.media == "xml")
        .map(|entry| entry.rule_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(xml_ids, BTreeSet::from([933200, 944200, 944260]));
    let names = manifest
        .cases
        .iter()
        .map(|case| case.name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(names.len(), manifest.cases.len());
    for entry in &manifest.coverage {
        assert!(!entry.rationale.is_empty());
        assert!(matches!(
            entry.status.as_str(),
            "positive-and-clean-covered"
                | "structural-effect-covered"
                | "format-constrained-rejection-covered"
                | "selector-not-applicable-covered"
        ));
        for name in &entry.case_names {
            let case = manifest
                .cases
                .iter()
                .find(|case| &case.name == name)
                .expect("all coverage cases exist");
            assert_eq!(case.rule_id, entry.rule_id);
            assert_eq!(case.media, entry.media);
        }
        if entry.status == "positive-and-clean-covered" {
            assert!(entry.case_names.iter().any(|name| manifest
                .cases
                .iter()
                .any(|case| &case.name == name && case.kind == "activation" && case.hit)));
            assert!(entry.case_names.iter().any(|name| manifest
                .cases
                .iter()
                .any(|case| &case.name == name && case.kind == "clean" && !case.hit)));
        }
    }
    for case in &manifest.cases {
        assert!(original_ids.contains(&case.rule_id));
        assert!(!case.bytes().is_empty());
        assert!(matches!(case.media.as_str(), "json" | "xml"));
        if case.source["origin"] == "derived-upstream-stage" {
            assert!(case.source["original_expect_ids"]
                .as_array()
                .expect("original expectation list")
                .iter()
                .any(|id| id.as_u64() == Some(case.rule_id)));
            assert_eq!(case.source["file_sha256"].as_str().expect("source hash").len(), 64);
            assert_eq!(case.source["stage_sha256"].as_str().expect("stage hash").len(), 64);
            if case.rule_id == 933200 {
                assert_eq!(case.source["document_rule_id"], 9331200);
            }
        } else {
            assert_eq!(case.source["origin"], "authored-original-predicate");
        }
    }
}

#[test]
fn original_structured_cases_match_across_modes_and_chunk_publication() {
    let corpus = corpus::corpus();
    let manifest = manifest();
    let mut tested = 0;
    for unit in &corpus.units {
        let generation = corpus::compile(&unit.raw);
        assert_eq!(generation.rule_units().count(), 5);
        for case in manifest
            .cases
            .iter()
            .filter(|case| case.rule_id == unit.id && case.kind != "malformed")
        {
            let reference = corpus::observe(
                &generation,
                case.input(case.bytes()),
                RuleEngineMode::DetectionOnly,
                EvaluationMode::Reference,
                false,
                &case.keys(),
            )
            .unwrap_or_else(|error| panic!("{} reference: {error:?}", case.name));
            assert_valid(case, &reference, false);
            for (mode, evaluation, chunked) in [
                (RuleEngineMode::DetectionOnly, EvaluationMode::Accelerated, false),
                (RuleEngineMode::DetectionOnly, EvaluationMode::Accelerated, true),
                (RuleEngineMode::On, EvaluationMode::Accelerated, false),
                (RuleEngineMode::On, EvaluationMode::Reference, true),
            ] {
                let actual = corpus::observe(
                    &generation,
                    case.input(case.bytes()),
                    mode,
                    evaluation,
                    chunked,
                    &case.keys(),
                )
                .unwrap_or_else(|error| panic!("{} {mode:?}/{evaluation:?}/chunked={chunked}: {error:?}", case.name));
                assert_valid(case, &actual, mode == RuleEngineMode::On);
                if mode == RuleEngineMode::DetectionOnly {
                    assert_eq!(actual, reference, "{} publication/evaluation parity", case.name);
                } else {
                    assert_eq!(actual.tx, reference.tx, "{} On effects", case.name);
                }
            }
            assert_variants(case, &generation);
            tested += 1;
        }
    }
    assert_eq!(tested, 416);
    eprintln!(
        "Structured supplemental valid cases: {tested}; fixed JSON variants: 375; original IDs: 195; new XML IDs: 3"
    );
}

#[test]
fn format_constrained_rules_reject_in_on_and_keep_detection_raw_fallback_separate() {
    let corpus = corpus::corpus();
    let manifest = manifest();
    let mut tested = 0;
    for case in manifest.cases.iter().filter(|case| case.kind == "malformed") {
        let unit = corpus
            .units
            .iter()
            .find(|unit| unit.id == case.rule_id)
            .expect("original malformed-context rule");
        let generation = corpus::compile(&unit.raw);
        let expected = BodyError::Malformed {
            processor: if case.media == "xml" {
                BodyProcessorSelection::Xml
            } else {
                BodyProcessorSelection::Json
            },
        };
        for evaluation in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            for chunked in [false, true] {
                assert_eq!(
                    corpus::observe(
                        &generation,
                        case.input(case.bytes()),
                        RuleEngineMode::On,
                        evaluation,
                        chunked,
                        &[]
                    ),
                    Err(RuntimeError::Body { kind: expected }),
                    "{} malformed On",
                    case.name
                );
                let actual = corpus::observe(
                    &generation,
                    case.input(case.bytes()),
                    RuleEngineMode::DetectionOnly,
                    evaluation,
                    chunked,
                    &[],
                )
                .expect("DetectionOnly preserves bounded raw fallback");
                assert_eq!(actual.body_error, Some(expected));
                assert_eq!(actual.outcome.intervention(), None);
                assert_eq!(actual.tx.get("supplement_raw_absent"), None);
                for key in ["supplement_args_absent", "supplement_xml_absent"] {
                    assert_eq!(actual.tx.get(key).map(Vec::as_slice), Some(b"1".as_slice()));
                }
                assert_eq!(
                    actual
                        .outcome
                        .findings()
                        .iter()
                        .any(|finding| finding.rule_id() == case.rule_id),
                    case.hit,
                    "{} raw fallback activation only",
                    case.name
                );
            }
        }
        tested += 1;
    }
    assert_eq!(tested, 4);
}

#[test]
fn combined_original_profile_keeps_selected_structured_outcomes_in_order() {
    let corpus = corpus::corpus();
    let manifest = manifest();
    let raw = corpus.units.iter().map(|unit| unit.raw.as_str()).collect::<String>();
    let generation = corpus::compile(&raw);
    assert_eq!(generation.rule_units().count(), 199);
    for name in [
        "933200-positive",
        "944260-positive",
        "933200-xml-text",
        "933200-xml-attribute",
        "944260-xml-text",
        "944260-xml-attribute",
    ] {
        let case = manifest
            .cases
            .iter()
            .find(|case| case.name == name)
            .expect("fixed combined cases");
        let reference = corpus::observe(
            &generation,
            case.input(case.bytes()),
            RuleEngineMode::DetectionOnly,
            EvaluationMode::Reference,
            false,
            &[],
        )
        .expect("strict combined reference");
        let streamed = corpus::observe(
            &generation,
            case.input(case.bytes()),
            RuleEngineMode::DetectionOnly,
            EvaluationMode::Accelerated,
            true,
            &[],
        )
        .expect("strict combined streamed");
        assert_eq!(reference, streamed, "combined {name}");
        assert!(reference.body_error.is_none());
        assert_eq!(
            reference.tx.get("supplement_raw_absent").map(Vec::as_slice),
            Some(b"1".as_slice())
        );
        assert!(reference
            .outcome
            .findings()
            .iter()
            .any(|finding| finding.rule_id() == case.rule_id));
    }
}
