use super::*;
use crate::seclang::{
    analyze_one_semantic_draft, parse_syntax, scan_source, validate_semantic_draft, PhaseDefaults, SemanticLimits,
    SemanticValidationContext, SourceLimits, SyntaxLimits, ValidationLimits,
};

fn compile(text: &str, limits: CompilerLimits) -> Result<CompiledGeneration, CompilerError> {
    let source = scan_source("json-plan-test", text.as_bytes(), SourceLimits::default()).unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let draft = analyze_one_semantic_draft(syntax, SemanticLimits::default()).unwrap();
    let phases = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &phases).unwrap();
    let validated = validate_semantic_draft(draft, context, ValidationLimits::default()).unwrap();
    compile_validated(validated, None, limits)
}

#[test]
fn capture_program_sharing_keeps_complete_json_group_identity() {
    let generation = compile(
        concat!(
            "SecRule ARGS \"@rx (?P<word>a)\" \"id:1,phase:2,pass,capture,t:none\"\n",
            "SecRule ARGS \"@rx (?P<word>a)\" \"id:2,phase:2,pass,capture,t:none\"\n",
            "SecRule ARGS \"!@rx (?P<word>a)\" \"id:3,phase:2,pass,capture,t:none\"\n",
            "SecRule ARGS_NAMES \"@rx (?P<word>a)\" \"id:4,phase:2,pass,capture,t:none\"\n",
            "SecRule ARGS \"@rx (?P<word>a)\" \"id:5,phase:2,pass,capture,t:none,t:lowercase\"\n",
            "SecRule ARGS \"@rx (?P<word>a)\" \"id:6,phase:2,pass,capture,t:none,multiMatch\"\n",
            "SecRule ARGS \"@rx (?P<other>a)\" \"id:7,phase:2,pass,capture,t:none\"\n",
        ),
        CompilerLimits::default(),
    )
    .unwrap();
    assert_eq!(generation.capture_regexes.len(), 2);
    let plan = &generation.json_inspection;
    assert_eq!(plan.groups.len(), 6);
    assert_eq!(plan.members[0].value, plan.members[1].value);
    let first = plan.members[0].value;
    assert!(plan.members[2..].iter().all(|member| member.value != first));
    assert_eq!(plan.matcher_scan_capacity, 0);
}

#[test]
fn matcher_scan_capacity_counts_unique_matcher_ids_across_profiles_and_ranges() {
    let generation = compile(
        concat!(
            "SecRule ARGS \"@pm first\" \"id:1,phase:2,pass,t:none\"\n",
            "SecRule ARGS \"@pm second\" \"id:2,phase:2,pass,t:none\"\n",
            "SecRule ARGS \"@pm third\" \"id:3,phase:2,pass,t:none,t:lowercase,multiMatch\"\n",
            "SecRule ARGS \"@contains fourth\" \"id:4,phase:2,pass,t:none\"\n",
            "SecRule ARGS \"@rx (a)\" \"id:5,phase:2,pass,capture,t:none\"\n",
        ),
        CompilerLimits::default(),
    )
    .unwrap();
    let plan = &generation.json_inspection;
    let mut ids = plan
        .groups
        .iter()
        .filter_map(|group| match generation.operator_plans[group.member].unwrap().operand {
            CompiledOperatorOperand::Matcher { group, .. } => Some(group),
            _ => None,
        })
        .collect::<Vec<_>>();
    let matcher_groups = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(plan.matcher_scan_capacity, ids.len());
    assert_eq!(ids.len(), 2);
    assert!(matcher_groups > ids.len());
    assert!(plan.groups.len() > matcher_groups);
    assert!(plan.profiles.len() > 1);
}

#[test]
fn dispatch_reuses_compatible_programs_and_looks_up_literal_bytes() {
    let generation = compile(
        r#"
SecRule ARGS|ARGS:json.z|ARGS:json.a|ARGS:json.a "@validateUtf8Encoding" "id:1,phase:2,t:none,pass"
SecRule ARGS:json.a "@validateUtf8Encoding" "id:2,phase:2,t:none,pass"
SecRule ARGS_NAMES "@validateUtf8Encoding" "id:3,phase:2,t:none,pass"
SecRule ARGS:/^json[.]a/ "@validateUtf8Encoding" "id:4,phase:2,t:none,pass"
"#,
        CompilerLimits::default(),
    )
    .unwrap();
    let plan = &generation.json_inspection;
    assert_eq!(plan.groups.len(), 2);
    assert_eq!(plan.broad_values.len(), 1);
    assert_eq!(plan.broad_names.len(), 1);
    assert_eq!(plan.literal_matches(&generation, b"json.a").unwrap().len(), 1);
    assert_eq!(plan.literal_matches(&generation, b"json.z").unwrap().len(), 1);
    assert!(plan.literal_matches(&generation, b"json.missing").unwrap().is_empty());
    assert_eq!(plan.regexes.len(), 1);
    assert_eq!(plan.literal_lookup_work(6).unwrap(), 28);
    assert!(plan.unsupported.is_empty());
}

#[test]
fn real_crs_count_length_shapes_need_no_retained_values() {
    let generation = compile(
        r#"
SecRule &TX:MAX_NUM_ARGS "@eq 1" "id:920380,phase:2,t:none,chain,pass"
SecRule &ARGS "@gt %{tx.max_num_args}" "t:none"
SecRule &TX:ARG_NAME_LENGTH "@eq 1" "id:920360,phase:2,t:none,chain,pass"
SecRule ARGS_NAMES "@gt %{tx.arg_name_length}" "t:none,t:length"
SecRule &TX:ARG_LENGTH "@eq 1" "id:920370,phase:2,t:none,chain,pass"
SecRule ARGS "@gt %{tx.arg_length}" "t:none,t:length"
SecRule &TX:TOTAL_ARG_LENGTH "@eq 1" "id:920390,phase:2,t:none,chain,pass"
SecRule ARGS_COMBINED_SIZE "@gt %{tx.total_arg_length}" "t:none"
"#,
        CompilerLimits::default(),
    )
    .unwrap();
    let plan = &generation.json_inspection;
    assert!(plan.unsupported.is_empty());
    assert!(plan.named_demands.is_empty());
    assert!(plan.needs_argument_count);
    assert!(plan.needs_combined_size);
    assert_eq!(plan.members[3].name, JsonMemberMode::NameDynamic);
    assert_eq!(plan.members[5].value, JsonMemberMode::SemanticLength);
    assert_eq!(plan.members[1].value, JsonMemberMode::None);
}

#[test]
fn dynamic_names_and_static_chain_parents_remain_supported() {
    let generation = compile(
        r#"
SecRule ARGS_NAMES "@streq %{tx.expected}" "id:1,phase:2,t:none,pass"
SecRule ARGS "@rx (alice)" "id:2,phase:2,t:none,capture,chain,pass"
SecRule TX:1 "@streq %{tx.expected}" "t:none"
"#,
        CompilerLimits::default(),
    )
    .unwrap();
    let plan = &generation.json_inspection;
    assert_eq!(plan.members[0].name, JsonMemberMode::NameDynamic);
    assert!(matches!(plan.members[1].value, JsonMemberMode::StaticFact(_)));
    assert_eq!(plan.members[2], JsonMemberPlan::default());
    assert!(plan.unsupported.is_empty());
    assert!(plan.named_demands.is_empty());
}

#[test]
fn dynamic_named_and_length_metadata_have_explicit_dispatch_without_value_retention() {
    let generation = compile(
        r#"
SecRule ARGS_NAMES:json.name "@streq %{tx.expected}" "id:1,phase:2,pass"
SecRule ARGS:json.value "@gt %{tx.length}" "id:2,phase:2,t:none,t:length,pass"
SecRule ARGS_NAMES:/^json[.]prefix/ "@streq %{tx.expected}" "id:3,phase:2,pass"
"#,
        CompilerLimits::default(),
    )
    .unwrap();
    let plan = &generation.json_inspection;
    assert!(!plan.needs_all_candidate_names);
    assert!(plan.named_demands.is_empty());
    let names: Vec<_> = plan
        .candidate_literals
        .iter()
        .map(|name| name.bytes(&generation).unwrap())
        .collect();
    assert_eq!(names, [b"json.name".as_slice(), b"json.value"]);
    assert_eq!(plan.candidate_regexes.len(), 1);
    assert!(plan.groups.is_empty());
}

#[test]
fn named_demands_deduplicate_selector_and_macro_arenas_across_phases() {
    let generation = compile(
        r#"
SecRule ARGS:json.secret "@streq %{tx.expected}" "id:1,phase:2,t:none,pass"
SecAction "id:2,phase:3,pass,setvar:'tx.saved=%{ARGS.json.secret}',msg:'%{ARGS.json.other}'"
SecAction "id:3,phase:5,pass,setvar:'tx.%{ARGS.json.key}=%{ARGS.json.secret}'"
"#,
        CompilerLimits::default(),
    )
    .unwrap();
    let plan = &generation.json_inspection;
    let names: Vec<_> = plan
        .named_demands
        .iter()
        .map(|name| name.bytes(&generation).unwrap())
        .collect();
    assert_eq!(names, [b"json.key".as_slice(), b"json.other", b"json.secret"]);
    assert_eq!(plan.members[0].value, JsonMemberMode::NamedDeferred);
    assert!(plan.unsupported.is_empty());
}

#[test]
fn unsupported_reasons_are_per_member_and_do_not_disable_other_groups() {
    let generation = compile(
        r#"
SecRule ARGS "@streq %{tx.expected}" "id:11,phase:2,t:none,pass"
SecRule ARGS:/^json[.]secret/ "@streq %{tx.expected}" "id:12,phase:2,t:none,pass"
SecRule ARGS "@validateUtf8Encoding" "id:13,phase:2,t:none,pass"
SecRule ARGS_NAMES "@streq %{tx.expected}" "id:14,phase:2,t:none,pass"
"#,
        CompilerLimits::default(),
    )
    .unwrap();
    assert_eq!(
        generation.json_inspection_unsupported(),
        [
            JsonInspectionUnsupported {
                rule_id: 11,
                member: 0,
                reason: JsonInspectionUnsupportedReason::DynamicBroadValueOperand
            },
            JsonInspectionUnsupported {
                rule_id: 12,
                member: 1,
                reason: JsonInspectionUnsupportedReason::DynamicRegexValueOperand
            },
        ]
    );
    assert_eq!(generation.json_inspection.groups.len(), 1);
}

#[test]
fn summary_only_counts_do_not_request_candidate_names_but_exclusions_do() {
    let simple = compile(
        r#"SecRule &ARGS "@gt 20" "id:1,phase:2,pass""#,
        CompilerLimits::default(),
    )
    .unwrap();
    assert!(!simple.json_inspection.needs_candidate_names);
    assert!(simple.json_inspection.groups.is_empty());
    let excluded = compile(
        r#"SecRule &ARGS|!ARGS:json.x "@gt 20" "id:1,phase:2,pass""#,
        CompilerLimits::default(),
    )
    .unwrap();
    assert!(excluded.json_inspection.needs_candidate_names);
    assert!(excluded.json_inspection.needs_all_candidate_names);
}

#[test]
fn multimatch_length_does_not_lose_initial_semantic_input() {
    let generation = compile(
        r#"SecRule ARGS "@gt %{tx.length}" "id:1,phase:2,t:none,t:length,multiMatch,pass""#,
        CompilerLimits::default(),
    )
    .unwrap();
    assert_eq!(generation.json_inspection.members[0].value, JsonMemberMode::Unsupported);
    assert_eq!(
        generation.json_inspection_unsupported()[0].reason,
        JsonInspectionUnsupportedReason::DynamicBroadValueOperand
    );
}

#[test]
fn inspection_records_and_comparison_work_use_existing_compiler_budgets() {
    for text in [
        r#"SecRule ARGS:json.a "@validateUtf8Encoding" "id:1,phase:2,t:none,pass""#,
        r#"SecRule ARGS|ARGS_NAMES "@validateUtf8Encoding" "id:1,phase:2,t:none,pass""#,
    ] {
        let generation = compile(text, CompilerLimits::default()).unwrap();
        let exact = generation.compiler_owned_bytes();
        assert!(compile(
            text,
            CompilerLimits {
                max_compiler_owned_non_matcher_bytes: exact,
                ..CompilerLimits::default()
            }
        )
        .is_ok());
        assert!(matches!(
            compile(
                text,
                CompilerLimits {
                    max_compiler_owned_non_matcher_bytes: exact - 1,
                    ..CompilerLimits::default()
                }
            ),
            Err(CompilerError::OwnedByteLimit { .. })
        ));
    }
    let generation = compile(
        r#"SecRule ARGS "@validateUtf8Encoding" "id:1,phase:2,t:none,pass""#,
        CompilerLimits::default(),
    )
    .unwrap();
    let mut work = CanonicalWork::default();
    let mut memory = MatcherCompileWork {
        limits: CompilerLimits {
            max_group_key_record_comparisons: 0,
            ..CompilerLimits::default()
        },
        validated_bytes: 0,
        working_bytes: 0,
        compiler_owned_bytes: 0,
        matcher_charged_bytes: 0,
        peak_bytes: 0,
        requirement_cursors: [0; 8],
    };
    assert!(matches!(
        CompiledJsonPlan::compile(&generation, &mut memory, &mut work),
        Err(CompilerError::GroupKeyRecordComparisonLimit { .. })
    ));
}

#[test]
fn xml_broad_vectors_reuse_profile_order_under_exact_shared_compile_work() {
    let generation = compile(
        concat!(
            "SecRule XML|XML:/*|XML://@* \"@contains first\" \"id:1,phase:2,pass,t:none\"\n",
            "SecRule XML \"@contains second\" \"id:2,phase:2,pass,t:lowercase\"\n",
            "SecRule XML \"@contains third\" \"id:3,phase:2,pass,t:none\"\n",
            "SecRule XML \"@contains fourth\" \"id:4,phase:2,pass,t:lowercase,multiMatch\"\n"
        ),
        CompilerLimits::default(),
    )
    .unwrap();
    let compile_plan = |limit| {
        let mut work = CanonicalWork::default();
        let mut memory = MatcherCompileWork {
            limits: CompilerLimits {
                max_group_key_record_comparisons: limit,
                ..CompilerLimits::default()
            },
            validated_bytes: 0,
            working_bytes: 0,
            compiler_owned_bytes: 0,
            matcher_charged_bytes: 0,
            peak_bytes: 0,
            requirement_cursors: [0; 8],
        };
        let result =
            CompiledJsonPlan::compile_domain(&generation, &mut memory, &mut work, super::super::InspectionDomain::Xml);
        (result, work.record_comparisons, memory.compiler_owned_bytes)
    };
    let (result, exact, owned) = compile_plan(usize::MAX);
    let plan = result.unwrap();
    assert_eq!(plan.groups.len(), 4);
    assert_eq!(plan.profiles.len(), 3);
    assert!(plan.broad_only_order.is_empty());
    for order in [&plan.xml_text, &plan.xml_attributes] {
        assert_eq!(order.len(), 4);
        assert!(order
            .windows(2)
            .all(|pair| (plan.groups[pair[0]].profile, pair[0]) < (plan.groups[pair[1]].profile, pair[1])));
    }
    assert!(compile_plan(exact).0.is_ok());
    assert!(matches!(
        compile_plan(exact - 1).0,
        Err(CompilerError::GroupKeyRecordComparisonLimit { .. })
    ));
    assert_eq!(compile_plan(exact).2, owned);
}

#[test]
fn dense_profiles_share_recipes_but_partition_view_and_multi_match() {
    let generation = compile(
        r#"
SecRule ARGS "@contains first" "id:1,phase:2,t:lowercase,pass"
SecRule ARGS "@contains second" "id:2,phase:2,t:lowercase,pass"
SecRule ARGS_NAMES "@contains third" "id:3,phase:2,t:lowercase,pass"
SecRule ARGS "@contains fourth" "id:4,phase:2,t:lowercase,multiMatch,pass"
SecRule REQUEST_HEADERS "@contains other" "id:5,phase:1,t:lowercase,pass"
SecRule RESPONSE_HEADERS "@contains other" "id:6,phase:3,t:length,pass"
"#,
        CompilerLimits::default(),
    )
    .unwrap();
    let plan = &generation.json_inspection;
    assert_eq!(plan.groups.len(), 4);
    assert_eq!(plan.profiles.len(), 3);
    assert_eq!(plan.groups[0].profile, plan.groups[1].profile);
    assert_ne!(plan.groups[0].profile, plan.groups[2].profile);
    assert_ne!(plan.groups[0].profile, plan.groups[3].profile);
}

#[test]
fn broad_only_order_is_precompiled_by_profile_then_group() {
    let generation = compile(
        concat!(
            "SecRule ARGS \"@contains a\" \"id:1,phase:2,pass,t:none,t:lowercase\"\n",
            "SecRule ARGS_NAMES \"@contains b\" \"id:2,phase:2,pass,t:none\"\n",
            "SecRule ARGS \"@contains c\" \"id:3,phase:2,pass,t:none,t:lowercase\"\n",
            "SecRule ARGS \"@contains d\" \"id:4,phase:2,pass,t:none\"\n",
        ),
        CompilerLimits::default(),
    )
    .unwrap();
    let plan = &generation.json_inspection;
    assert_eq!(plan.broad_only_order.len(), 4);
    let mut expected = plan
        .broad_names
        .iter()
        .chain(&plan.broad_values)
        .copied()
        .collect::<Vec<_>>();
    expected.sort_by_key(|&id| (plan.groups[id].profile, id));
    assert_eq!(plan.broad_only_order, expected);
    for target in ["ARGS:json.x", "ARGS:/^json/"] {
        let source = format!("SecRule ARGS|{target} \"@contains a\" \"id:1,phase:2,pass\"\n");
        let mixed = compile(&source, CompilerLimits::default()).unwrap();
        assert!(mixed.json_inspection.broad_only_order.is_empty());
    }
    let empty = compile(
        "SecRule &ARGS \"@gt 2\" \"id:1,phase:2,pass\"",
        CompilerLimits::default(),
    )
    .unwrap();
    assert!(empty.json_inspection.broad_only_order.is_empty());
}
