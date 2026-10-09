use super::*;
use crate::seclang::{
    compile_rule_bundles_with_selection, FrontendLimits, Phase, PhaseDefaults, RuleBundleMember, RuleBundleSource,
    SemanticValidationContext, WafSelectionCompileError,
};

fn selection(value: &str) -> WafRuleSelection {
    serde_json::from_str(value).unwrap()
}
fn context() -> SemanticValidationContext {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap()
}
fn compile(source: &str, selected: &str) -> (Vec<u64>, WafSelectionSummary, usize) {
    let rules = [RuleBundleMember {
        path: "main.conf",
        bytes: source.as_bytes(),
    }];
    let bundles = [RuleBundleSource {
        identity: "test",
        entrypoints: &["main.conf"],
        rules: &rules,
        phrase_assets: &[],
    }];
    let program = selection(selected).prepare().unwrap();
    let mut admissions = 0;
    let (compiled, summary) = compile_rule_bundles_with_selection(
        &bundles,
        "",
        context(),
        FrontendLimits::default(),
        &program,
        0,
        |_| {
            admissions += 1;
            Ok::<_, ()>(())
        },
        None,
    )
    .unwrap();
    (
        compiled.generation().rule_units().map(|unit| unit.id()).collect(),
        summary,
        admissions,
    )
}

#[test]
fn structured_wire_is_strict_and_canonical_bytes_are_exact() {
    for value in [
        "{}",
        r#"{"include":true,"exclude":false}"#,
        r#"{"include":{"allOf":[{"tag":"é/\"\\"},{"phase":2},{"not":{"ruleIdRange":{"start":1,"end":18446744073709551615}}}]}}"#,
    ] {
        let selected = selection(value);
        assert_eq!(
            selected.validate().unwrap().encoded_bytes,
            serde_json::to_vec(&selected).unwrap().len()
        );
        assert_eq!(
            selected,
            serde_json::from_slice(&serde_json::to_vec(&selected).unwrap()).unwrap()
        );
    }
    for value in [
        "null",
        "[]",
        r#"{"include":null}"#,
        r#"{"include":"tag:php"}"#,
        r#"{"include":{},"exclude":false}"#,
        r#"{"include":{"allOf":[]}}"#,
        r#"{"include":{"tag":" "}}"#,
        r#"{"include":{"tag":"a\nb"}}"#,
        r#"{"include":{"tag":"php","phase":2}}"#,
        r#"{"include":{"ruleId":0}}"#,
        r#"{"include":{"ruleId":-1}}"#,
        r#"{"include":{"ruleId":"1"}}"#,
        r#"{"include":{"ruleId":18446744073709551616}}"#,
        r#"{"include":{"phase":6}}"#,
        r#"{"include":{"phase":1.0}}"#,
        r#"{"include":{"ruleIdRange":{"start":2,"end":1}}}"#,
        r#"{"include":{"ruleIdRange":{"start":1,"end":2,"other":3}}}"#,
        r#"{"include":true,"include":false}"#,
        r#"{"include":{"tag":"php","tag":"java"}}"#,
        r#"{"excludeTags":[]}"#,
    ] {
        assert!(
            serde_json::from_str::<WafRuleSelection>(value).is_err(),
            "accepted {value}"
        );
    }
    assert!(serde_yaml::from_str::<WafRuleSelection>(
        "include:\n  allOf:\n    - tag: language-php\n    - not: {phase: 1}\n"
    )
    .is_ok());
    assert!(serde_yaml::from_str::<WafRuleSelection>("include: true\ninclude: false\n").is_err());
    // ConfigData's preexisting normalized DOM has already collapsed duplicate JSON keys.
    let normalized: serde_json::Value = serde_json::from_str(r#"{"include":true,"include":false}"#).unwrap();
    assert_eq!(
        serde_json::from_value::<WafRuleSelection>(normalized).unwrap(),
        selection(r#"{"include":false}"#)
    );
}

#[test]
fn shared_tree_limits_apply_before_descent_even_to_unreachable_nodes() {
    let wrap = |depth: usize| format!("{{\"include\":{}true{}}}", "{\"not\":".repeat(depth), "}".repeat(depth));
    assert!(serde_json::from_str::<WafRuleSelection>(&wrap(15)).is_ok());
    assert!(serde_json::from_str::<WafRuleSelection>(&wrap(16)).is_err());
    let atoms = |count: usize| (0..count).map(|_| r#"{"ruleId":1}"#).collect::<Vec<_>>().join(",");
    let value = format!(
        "{{\"include\":{{\"anyOf\":[{}]}},\"exclude\":{{\"allOf\":[{}]}}}}",
        atoms(32),
        atoms(32)
    );
    assert_eq!(selection(&value).validate().unwrap().metadata_leaves, 64);
    let value = format!(
        "{{\"include\":{{\"anyOf\":[{}]}},\"exclude\":{{\"allOf\":[{}]}}}}",
        atoms(32),
        atoms(33)
    );
    assert!(serde_json::from_str::<WafRuleSelection>(&value).is_err());
    let constants = (0..256).map(|_| "true").collect::<Vec<_>>().join(",");
    assert!(serde_json::from_str::<WafRuleSelection>(&format!("{{\"include\":{{\"anyOf\":[{constants}]}}}}")).is_err());
    assert!(
        serde_json::from_str::<WafRuleSelection>(&format!("{{\"include\":{{\"tag\":\"{}\"}}}}", "a".repeat(257)))
            .is_err()
    );
}

#[test]
fn schema_and_wire_share_boolean_or_exactly_one_operator_shape() {
    let schema = serde_json::to_value(schemars::schema_for!(WafRuleSelection)).unwrap();
    assert!(schema.get("required").is_none());
    assert_eq!(schema["additionalProperties"], false);
    let expression = &schema["$defs"]["WafRuleSelectionExpr"];
    assert!(expression["anyOf"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["type"] == "boolean"));
    assert!(!serde_json::to_string(&schema).unwrap().contains("null"));
}

#[test]
fn whole_chain_union_exclusion_precedence_and_source_removals() {
    let source = "SecRule REQUEST_URI \"@contains x\" \"id:1,phase:1,pass,tag:php,tag:java,chain\"\n\
        SecRule REQUEST_METHOD \"@streq GET\" \"t:none\"\n\
        SecAction \"id:2,phase:1,pass,tag:php\"\n\
        SecAction \"id:3,phase:1,pass,tag:Java\"\nSecRuleRemoveById 3\n";
    let (ids, summary, passes) = compile(
        source,
        r#"{"include":{"anyOf":[{"allOf":[{"tag":"php"},{"tag":"java"}]},{"ruleId":3}]},"exclude":{"ruleId":2}}"#,
    );
    assert_eq!(ids, [1]);
    assert_eq!(
        (summary.raw_units, summary.source_active_units, summary.selected_units),
        (3, 2, 1)
    );
    assert_eq!(passes, 2);
    let (ids, summary, passes) = compile(source, r#"{"include":{"not":{"tag":"absent"}}}"#);
    assert_eq!(ids, [1, 2]);
    assert_eq!(summary.unmatched_atoms, 1);
    assert_eq!(passes, 1);
    let (ids, summary, _) = compile(source, r#"{"include":{"tag":"absent"}}"#);
    assert!(ids.is_empty());
    assert!(summary.empty_selection);
    let (ids, _, _) = compile(source, r#"{"include":true,"exclude":true}"#);
    assert!(ids.is_empty());
}

#[test]
fn strict_baseline_rejects_excluded_bad_regex_asset_and_body_admission() {
    let program = selection(r#"{"exclude":true}"#).prepare().unwrap();
    for source in [
        "SecRule REQUEST_URI \"@rx [\" \"id:1,phase:1,pass\"\n",
        "SecRule REQUEST_URI \"@pmFromFile missing.data\" \"id:1,phase:1,pass\"\n",
        "SecRule ARGS:/[/ \"@contains x\" \"id:1,phase:2,pass\"\n",
    ] {
        let rules = [RuleBundleMember {
            path: "main.conf",
            bytes: source.as_bytes(),
        }];
        let bundles = [RuleBundleSource {
            identity: "test",
            entrypoints: &["main.conf"],
            rules: &rules,
            phrase_assets: &[],
        }];
        let mut admissions = 0;
        assert!(matches!(
            compile_rule_bundles_with_selection(
                &bundles,
                "",
                context(),
                FrontendLimits::default(),
                &program,
                0,
                |_| {
                    admissions += 1;
                    Ok::<_, ()>(())
                },
                None
            ),
            Err(WafSelectionCompileError::Frontend(_))
        ));
        assert_eq!(admissions, 0);
    }
    let rules = [RuleBundleMember {
        path: "main.conf",
        bytes: b"SecAction \"id:1,phase:1,pass\"\n",
    }];
    let bundles = [RuleBundleSource {
        identity: "test",
        entrypoints: &["main.conf"],
        rules: &rules,
        phrase_assets: &[],
    }];
    assert!(matches!(
        compile_rule_bundles_with_selection(
            &bundles,
            "",
            context(),
            FrontendLimits::default(),
            &program,
            0,
            |_| Err("body admission"),
            None
        ),
        Err(WafSelectionCompileError::Admission("body admission"))
    ));
}

#[test]
fn constant_truth_table_and_independent_atom_diagnostics() {
    let source = "SecAction \"id:1,phase:2,pass,tag:php\"\n";
    for include in [false, true] {
        for exclude in [false, true] {
            let (_, summary, _) = compile(source, &format!("{{\"include\":{include},\"exclude\":{exclude}}}"));
            assert_eq!(summary.selected_units, usize::from(include && !exclude));
        }
    }
    let (_, summary, _) = compile(source, r#"{"include":{"anyOf":[true,{"tag":"unknown"}]}}"#);
    assert_eq!(summary.selected_units, 1);
    assert_eq!(summary.unmatched_atoms, 1);
}

#[test]
fn excluded_illegal_child_tag_still_fails_the_original_frontend() {
    let source = "SecRule REQUEST_URI \"@contains x\" \"id:1,phase:1,pass,chain\"\nSecRule REQUEST_METHOD \"@streq GET\" \"tag:child\"\n";
    let rules = [RuleBundleMember {
        path: "main.conf",
        bytes: source.as_bytes(),
    }];
    let bundles = [RuleBundleSource {
        identity: "test",
        entrypoints: &["main.conf"],
        rules: &rules,
        phrase_assets: &[],
    }];
    let program = selection("{\"exclude\":true}").prepare().unwrap();
    assert!(matches!(
        compile_rule_bundles_with_selection(
            &bundles,
            "",
            context(),
            FrontendLimits::default(),
            &program,
            0,
            |_| Ok::<_, ()>(()),
            None
        ),
        Err(WafSelectionCompileError::Frontend(
            crate::seclang::FrontendError::Semantic(crate::seclang::SemanticError::ChildAction {
                kind: ActionKind::Tag,
                ..
            })
        ))
    ));
}

#[test]
fn cumulative_budget_can_reject_after_the_baseline_was_admitted() {
    let rules = [RuleBundleMember {
        path: "main.conf",
        bytes: b"SecAction \"id:1,phase:1,pass,tag:keep\"\nSecAction \"id:2,phase:1,pass,tag:drop\"\n",
    }];
    let bundles = [RuleBundleSource {
        identity: "test",
        entrypoints: &["main.conf"],
        rules: &rules,
        phrase_assets: &[],
    }];
    let program = selection("{\"exclude\":{\"tag\":\"drop\"}}").prepare().unwrap();
    let mut witnessed = false;
    for cap in 1..512 {
        let mut limits = FrontendLimits::default();
        limits.compiler.max_group_key_record_comparisons = cap;
        let mut admitted = 0;
        let result = compile_rule_bundles_with_selection(
            &bundles,
            "",
            context(),
            limits,
            &program,
            0,
            |_| {
                admitted += 1;
                Ok::<_, ()>(())
            },
            None,
        );
        if admitted == 1
            && matches!(
                result,
                Err(WafSelectionCompileError::Frontend(
                    crate::seclang::FrontendError::Compiler(CompilerError::GroupKeyRecordComparisonLimit { .. })
                ))
            )
        {
            witnessed = true;
            break;
        }
    }
    assert!(
        witnessed,
        "both passes and selection must share one cumulative work budget"
    );
    let mut limits = FrontendLimits::default();
    limits.compiler.max_compilation_peak_bytes = program.owned_bytes() - 1;
    assert!(matches!(
        compile_rule_bundles_with_selection(&bundles, "", context(), limits, &program, 0, |_| Ok::<_, ()>(()), None),
        Err(WafSelectionCompileError::Frontend(
            crate::seclang::FrontendError::Compiler(CompilerError::PeakByteLimit { .. })
        ))
    ));
}

#[test]
fn yaml_tag_scalars_preserve_types_without_coercion() {
    for value in ["123", "true", "false", "null", "~", "1.5", "[]", "{}"] {
        assert!(
            serde_yaml::from_str::<WafRuleSelection>(&format!("include: {{tag: {value}}}")).is_err(),
            "{value}"
        );
    }
    for value in ["'123'", "'true'", "'null'", "language-php"] {
        assert!(
            serde_yaml::from_str::<WafRuleSelection>(&format!("include: {{tag: {value}}}")).is_ok(),
            "{value}"
        );
    }
    for value in ["'1'", "true", "null", "1.0"] {
        for field in ["ruleId", "phase"] {
            assert!(
                serde_yaml::from_str::<WafRuleSelection>(&format!("include: {{{field}: {value}}}")).is_err(),
                "{field}: {value}"
            );
        }
        assert!(serde_yaml::from_str::<WafRuleSelection>(&format!(
            "include: {{ruleIdRange: {{start: {value}, end: 2}}}}"
        ))
        .is_err());
    }
}
