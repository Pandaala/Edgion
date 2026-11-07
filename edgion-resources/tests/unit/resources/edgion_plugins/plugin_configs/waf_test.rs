use super::*;
use std::collections::BTreeMap;

fn test_bundle(name: &str, id: u64) -> (WafBundleRef, ResolvedWafBundle) {
    use crate::resources::edgion_config_data::WafBundleMember;

    let mut data = WafRuleBundleData {
        version: "1".into(),
        profile: "test".into(),
        provenance: "test/revision".into(),
        digest: None,
        roots: vec!["main.conf".into()],
        rules: vec![WafBundleMember {
            name: "main.conf".into(),
            content: format!("SecAction \"id:{id},phase:1,pass\"\n"),
        }],
        phrase_assets: vec![],
    };
    data.digest = Some(data.compute_digest());
    (
        WafBundleRef {
            target: EdgionConfigDataRef {
                name: name.into(),
                namespace: None,
                ref_denied: None,
            },
            expected_digest: data.digest.clone(),
            optional: false,
        },
        ResolvedWafBundle {
            identity: format!("default/{name}"),
            digest: data.compute_digest(),
            data,
        },
    )
}

fn selection_config(source: &str, expression: &str) -> WafConfig {
    let (mut reference, mut bundle) = test_bundle("selected", 100);
    bundle.data.rules[0].content = source.into();
    bundle.data.digest = Some(bundle.data.compute_digest());
    bundle.digest = bundle.data.compute_digest();
    reference.expected_digest = Some(bundle.digest.clone());
    WafConfig {
        policy_ref: EdgionConfigDataRef {
            name: "policy".into(),
            ..Default::default()
        },
        resolved_policy: Some(WafPolicyData {
            default_profile: "default".into(),
            profiles: BTreeMap::from([(
                "default".into(),
                WafProfile {
                    bundle_refs: vec![reference],
                    rule_selection: yaml_serde::from_str(expression).unwrap(),
                },
            )]),
        }),
        selected_profile: Some("default".into()),
        owner_namespace: Some("default".into()),
        resolved_ref_indices: vec![0],
        resolved_bundles: Some(vec![bundle]),
        ..Default::default()
    }
}

#[test]
fn static_selection_recompiles_from_identical_original_sources_and_transport() {
    let before = selection_config(
        concat!(
            "SecRule REQUEST_URI \"@contains /probe\" \"id:100,phase:1,deny,status:403,tag:company/a\"\n",
            "SecRule REQUEST_URI \"@contains /probe\" \"id:101,phase:1,deny,status:409,tag:company/b\"\n"
        ),
        "include: {tag: company/a}",
    );
    let mut after = before.clone();
    after
        .resolved_policy
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("default")
        .unwrap()
        .rule_selection = yaml_serde::from_str("include: {tag: company/b}").unwrap();
    let ids = |config: &WafConfig| {
        config
            .compile_generation()
            .unwrap()
            .rule_units()
            .map(|unit| unit.id())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&before), [100]);
    assert_eq!(ids(&after), [101]);
    assert_eq!(ids(&before), [100]);
    assert_eq!(
        before.resolved_bundles.as_ref().unwrap()[0].digest,
        after.resolved_bundles.as_ref().unwrap()[0].digest
    );
    let wire = serde_json::to_string(&after).unwrap();
    assert_eq!(ids(&serde_json::from_str(&wire).unwrap()), [101]);
    assert!(!wire.contains("controllerSelectionSummary"));
    assert!(wire.contains("ruleSelection"));
}

#[test]
fn selected_profile_admits_the_full_baseline_before_body_and_regex_filtering() {
    let invalid = selection_config(
        "SecRule REQUEST_URI \"@rx (\" \"id:100,phase:1,pass,tag:php\"\n",
        "exclude: true",
    );
    assert!(invalid.compile_generation().is_err());
    let mut invalid_body = selection_config(
        "SecRule REQUEST_BODY_LENGTH \"@gt 0\" \"id:100,phase:2,pass,tag:php\"\n",
        "exclude: true",
    );
    invalid_body.request_body.inspection = WafRequestBodyInspection::Prefix;
    assert!(invalid_body.compile_generation().is_err());
}

#[test]
fn bundle_refs_require_complete_pinned_sources_and_redact_content() {
    let (first_ref, first) = test_bundle("first", 10);
    let (second_ref, second) = test_bundle("second", 11);
    let config = WafConfig {
        policy_ref: EdgionConfigDataRef {
            name: "policy".into(),
            ..Default::default()
        },
        resolved_policy: Some(WafPolicyData {
            profiles: BTreeMap::from([(
                "default".into(),
                WafProfile {
                    rule_selection: Default::default(),
                    bundle_refs: vec![first_ref, second_ref],
                },
            )]),
            default_profile: "default".into(),
        }),
        selected_profile: Some("default".into()),
        resolved_ref_indices: vec![0, 1],
        resolved_bundles: Some(vec![first, second]),
        owner_namespace: Some("default".into()),
        ..WafConfig::default()
    };
    assert_eq!(config.compile_generation().unwrap().rule_unit_count(), 2);

    let json = serde_json::to_value(&config).unwrap();
    let parsed: WafConfig = serde_json::from_value(json).unwrap();
    assert_eq!(parsed.compile_generation().unwrap().rule_unit_count(), 2);

    let mut unpinned = config.clone();
    unpinned
        .resolved_policy
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("default")
        .unwrap()
        .bundle_refs[0]
        .expected_digest = None;
    unpinned.resolved_bundles.as_mut().unwrap()[0].data.digest = None;
    let wire = serde_json::to_value(&unpinned).unwrap();
    let mut unpinned: WafConfig = serde_json::from_value(wire).unwrap();
    assert_eq!(unpinned.compile_generation().unwrap().rule_unit_count(), 2);
    unpinned.resolved_bundles.as_mut().unwrap()[0].data.rules[0]
        .content
        .push_str("# changed");
    assert!(unpinned.compile_generation().is_err());

    let mut tampered = config.clone();
    tampered.resolved_bundles.as_mut().unwrap()[0].data.rules[0]
        .content
        .push_str("# changed");
    assert!(tampered.compile_generation().is_err());

    let mut mismatch = config.clone();
    mismatch
        .resolved_policy
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("default")
        .unwrap()
        .bundle_refs[0]
        .expected_digest = Some("0".repeat(64));
    assert!(mismatch.compile_generation().is_err());

    let mut missing = config.clone();
    missing.resolved_bundles = None;
    assert!(missing.compile_generation().is_err());

    let _guard = crate::redact::AdminRedactGuard::new();
    let admin = serde_json::to_string(&config).unwrap();
    assert!(!admin.contains("SecAction"));
    assert!(!format!("{config:?}").contains("SecAction"));
}

#[test]
fn support_report_separates_inventory_from_strict_validation_and_redacts_operands() {
    let config = WafConfig {
        rules: concat!(
            "SecRule REQUEST_URI \"@contains private-token\" \"id:1,phase:1,pass\"\n",
            "SecRule RESPONSE_BODY \"@rx secret-pattern\" \"id:2,phase:4,pass\"\n",
        )
        .to_string(),
        ..WafConfig::default()
    };
    let report = config.inspect_support();
    assert!(report.analysis_complete, "{report:?}");
    assert!(!report.strict_validation_passed);
    assert_eq!(report.rule_count, Some(2));
    assert_eq!(report.unavailable_rule_count, Some(1));
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].rule_id, Some(2));
    assert_eq!(report.findings[0].capability, "full_response_body");
    let rendered = serde_json::to_string(&report).unwrap();
    assert!(!rendered.contains("private-token"));
    assert!(!rendered.contains("secret-pattern"));
    assert!(!format!("{report:?}").contains("secret-pattern"));
}

#[test]
fn support_report_classifies_incomplete_syntax_and_missing_assets() {
    let syntax = WafConfig {
        rules: "SecRule REQUEST_URI \"@contains secret".to_string(),
        ..WafConfig::default()
    }
    .inspect_support();
    assert!(!syntax.analysis_complete);
    assert!(!syntax.strict_validation_passed);
    assert_eq!(syntax.failure.unwrap().category, "syntax");
    assert_eq!(syntax.failure.unwrap().physical_line, Some(1));

    let asset = WafConfig {
        rules: "SecRule ARGS \"@pmFromFile private.data\" \"id:1,phase:2,pass\"\n".to_string(),
        ..WafConfig::default()
    }
    .inspect_support();
    assert!(asset.analysis_complete);
    assert!(!asset.strict_validation_passed);
    assert_eq!(asset.failure.unwrap().category, "missing_asset");
    assert_eq!(asset.failure.unwrap().directive_index, Some(0));
    assert!(!serde_json::to_string(&asset).unwrap().contains("private.data"));
}

#[test]
fn support_report_bounds_findings_and_marks_omissions() {
    let rules = (1..=130)
        .map(|id| format!("SecRule RESPONSE_BODY \"@contains secret\" \"id:{id},phase:4,pass\"\n"))
        .collect::<String>();
    let report = WafConfig {
        rules,
        ..WafConfig::default()
    }
    .inspect_support();
    assert!(report.analysis_complete);
    assert_eq!(report.findings.len(), 128);
    assert_eq!(report.omitted_findings, 2);
    assert_eq!(report.omitted_rule_summaries, 2);
    assert!(!report.strict_validation_passed);
}

#[test]
fn support_report_locates_missing_rule_id_without_fabricating_one() {
    let report = WafConfig {
        rules: "SecRule RESPONSE_BODY \"@contains secret\" \"phase:4,pass\"\n".to_string(),
        ..WafConfig::default()
    }
    .inspect_support();
    assert!(!report.analysis_complete);
    assert!(report.findings.is_empty());
    assert_eq!(report.failure.unwrap().physical_line, Some(1));
    assert_eq!(report.failure.unwrap().reason, "Add an ID to the rule");
    assert!(!report.strict_validation_passed);
}

#[test]
fn support_report_preserves_available_locations_and_resource_categories() {
    let position = SourcePosition {
        line: 7,
        column: 1,
        byte_offset: 0,
    };
    let chain_limit = frontend_failure(&FrontendError::Semantic(SemanticError::ChainLengthLimit {
        limit: 2,
        position,
    }));
    assert_eq!(chain_limit.category, "resource_limit");
    assert_eq!(chain_limit.physical_line, Some(7));
    let work_limit = frontend_failure(&FrontendError::Validation(SemanticValidationError::WorkingByteLimit {
        limit: 1,
    }));
    assert_eq!(work_limit.category, "resource_limit");
    let ctl_target = frontend_failure(&FrontendError::Semantic(SemanticError::CtlTarget(
        edgion_waf::seclang::TargetSyntaxError::AtomLimit { limit: 1, position },
    )));
    assert_eq!(ctl_target.category, "resource_limit");
    assert_eq!(ctl_target.physical_line, Some(7));
    let identity_limit = frontend_failure(&FrontendError::Include(
        edgion_waf::seclang::IncludeError::IdentityComparisonLimit { limit: 1 },
    ));
    assert_eq!(identity_limit.category, "resource_limit");
    assert_eq!(
        identity_limit.reason,
        "Reduce the number of equal-length rule bundle member or include paths, or shorten them"
    );
    let include_limit = frontend_failure(&FrontendError::Include(edgion_waf::seclang::IncludeError::DepthLimit {
        actual: 2,
        limit: 1,
    }));
    assert_eq!(include_limit.reason, "Reduce or remove include directives");

    let actions = std::iter::repeat_n("t:none", 1_025).collect::<Vec<_>>().join(",");
    let action_limit = WafConfig {
        rules: format!("SecAction \"id:1,phase:1,pass,{actions}\""),
        ..WafConfig::default()
    }
    .inspect_support();
    assert_eq!(action_limit.failure.unwrap().category, "resource_limit");
    assert_eq!(action_limit.failure.unwrap().physical_line, Some(1));

    let unknown_operator = WafConfig {
        rules: "SecRule REQUEST_URI \"@notAnOperator secret\" \"id:1,phase:1,pass\"".to_string(),
        ..WafConfig::default()
    }
    .inspect_support();
    assert!(!unknown_operator.analysis_complete);
    assert_eq!(unknown_operator.failure.unwrap().physical_line, Some(1));

    let duplicate_id = WafConfig {
        rules: concat!(
            "SecRule REQUEST_URI \"@contains first\" \"id:1,phase:1,pass\"\n",
            "SecRule REQUEST_URI \"@contains second\" \"id:1,phase:1,pass\"\n",
        )
        .to_string(),
        ..WafConfig::default()
    }
    .inspect_support();
    assert!(duplicate_id.analysis_complete);
    let failure = duplicate_id.failure.unwrap();
    assert_eq!(failure.document_index, Some(0));
    assert_eq!(failure.directive_index, Some(1));

    let missing_fields = WafConfig {
        rules: "SecRule REQUEST_URI".to_string(),
        ..WafConfig::default()
    }
    .inspect_support();
    assert_eq!(missing_fields.failure.unwrap().physical_line, Some(1));
}

#[test]
fn prefix_size_is_bounded_and_round_trips_without_changing_full_defaults() {
    for (size, bytes) in [("1", 1), ("16KiB", 16384), ("100k", 102400), ("1m", 1048576)] {
        let mut config: WafConfig = serde_json::from_value(serde_json::json!({
            "policyRef": {"name": "policy"},
            "requestBody": {"inspection": "prefix", "prefixSize": size}
        }))
        .unwrap();
        config.rules = "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny\"".into();
        assert_eq!(config.request_body.prefix_size_bytes().unwrap(), bytes);
        assert_eq!(config.validation_error(), None);
        let encoded = serde_json::to_value(&config).unwrap();
        assert_eq!(encoded["requestBody"]["prefixSize"], size);
        let restored: WafConfig = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored.request_body.prefix_size_bytes().unwrap(), bytes);
    }
    for size in ["0", "1048577", "2m", "-1", "invalid"] {
        let config = WafConfig {
            rules: "SecAction \"id:1,phase:1,pass\"".to_string(),
            request_body: WafRequestBodyConfig {
                inspection: WafRequestBodyInspection::Prefix,
                prefix_size: Some(size.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(config.validation_error().is_some(), "{size}");
    }
    let full = WafRequestBodyConfig {
        inspection: WafRequestBodyInspection::Full,
        ..Default::default()
    };
    assert!(full.prefix_size_bytes().is_ok());
    assert!(WafRequestBodyConfig {
        prefix_size: Some("1m".to_string()),
        ..full
    }
    .prefix_size_bytes()
    .is_err());
}

#[test]
fn full_default_and_explicit_prefix_share_the_same_compiler() {
    let mut config = WafConfig {
        rules: "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny\"".into(),
        ..WafConfig::default()
    };
    assert_eq!(config.request_body.inspection, WafRequestBodyInspection::Full);
    assert_eq!(config.validation_error(), None);
    config.request_body.inspection = WafRequestBodyInspection::Prefix;
    assert_eq!(config.validation_error(), None);
    config.request_body.inspection = WafRequestBodyInspection::Full;
    let serialized = serde_json::to_value(&config).unwrap();
    assert_eq!(serialized["requestBody"]["inspection"], "full");
    let restored: WafConfig = serde_json::from_value(serialized).unwrap();
    assert_eq!(restored.request_body.inspection, WafRequestBodyInspection::Full);
    // Test-only inline rules are deliberately absent from serialized resources.
    assert_eq!(
        restored.validation_error().as_deref(),
        Some("policyRef requires a valid name")
    );
}

#[test]
fn full_capture_dependency_includes_later_phases_but_not_query_only_headers() {
    for (rules, requires_body) in [
        ("SecRule QUERY_STRING \"@contains query\" \"id:1,phase:1,deny\"", false),
        (
            "SecRule REQUEST_HEADERS:X-Test \"@contains test\" \"id:1,phase:2,deny\"",
            false,
        ),
        ("SecRule REQUEST_BODY \"@contains test\" \"id:1,phase:3,deny\"", true),
        ("SecRule ARGS:item \"@contains test\" \"id:1,phase:5,pass\"", true),
        ("SecRule RESPONSE_STATUS \"@eq 200\" \"id:1,phase:3,pass\"", false),
        (
            "SecRule REQUEST_HEADERS:X-Test \"@streq %{ARGS.item}\" \"id:1,phase:3,pass\"",
            true,
        ),
        ("SecAction \"id:1,phase:3,pass,setvar:tx.item=%{ARGS.item}\"", true),
        (
            "SecRule REQUEST_HEADERS:X-Test \"@streq %{REQUEST_HEADERS.X-Other}\" \"id:1,phase:3,pass\"",
            false,
        ),
    ] {
        let config = WafConfig {
            rules: rules.to_owned(),
            request_body: WafRequestBodyConfig {
                inspection: WafRequestBodyInspection::Full,
                ..Default::default()
            },
            ..WafConfig::default()
        };
        let generation = config.compile_generation().unwrap();
        assert_eq!(generation.requires_complete_request_body(), requires_body, "{rules}");
    }
}

#[test]
fn prefix_accepts_body_independent_phase_two_rules() {
    for target in [
        "REQUEST_HEADERS:X-Test",
        "REQUEST_URI",
        "ARGS_GET:item",
        "ARGS_GET_NAMES",
    ] {
        let config = WafConfig {
            rules: format!("SecRule {target} \"!@contains blocked\" \"id:1,phase:2,deny\""),
            ..Default::default()
        };
        let generation = config.compile_generation().unwrap();
        assert!(!generation.requires_complete_request_body(), "{target}");
        assert!(
            !generation.request_body_execution_summary().requires_request_body(),
            "{target}"
        );
        assert_eq!(
            generation.request_body_execution_summary().streaming_raw_rule_units(),
            1
        );
    }
}

#[test]
fn prefix_rejects_whole_body_and_control_dependencies_across_generation() {
    for rules in [
            "SecRule REQUEST_BODY \"!@contains forbidden\" \"id:1,phase:2,deny\"",
            "SecRule REQUEST_BODY \"@endsWith forbidden\" \"id:1,phase:2,deny\"",
            "SecRule ARGS:item \"@contains forbidden\" \"id:1,phase:2,deny\"",
            "SecRule ARGS_NAMES \"@contains forbidden\" \"id:1,phase:2,deny\"",
            "SecRule REQBODY_PROCESSOR \"@streq JSON\" \"id:1,phase:2,deny\"",
            "SecRule REQUEST_BODY_LENGTH \"@gt 10\" \"id:1,phase:2,deny\"",
            "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny,t:base64Decode\"",
            "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:3,deny\"",
            "SecRule ARGS:item \"@contains forbidden\" \"id:1,phase:5,pass\"",
            "SecAction \"id:1,phase:1,pass,setvar:tx.flag=1\"",
            "SecRule REQUEST_URI \"@contains private\" \"id:1,phase:1,pass,skip:1\"\nSecRule REQUEST_BODY \"@contains forbidden\" \"id:2,phase:2,deny\"",
            "SecRule REQUEST_BODY \"@contains forbidden\" \"id:1,phase:2,deny,chain\"\nSecRule REQUEST_BODY \"@contains secret\" \"t:none\"",
        ] {
            let mut config = WafConfig { rules: rules.to_owned(), request_body: WafRequestBodyConfig { inspection: WafRequestBodyInspection::Prefix, ..Default::default() }, ..WafConfig::default() };
            let error = config.validation_error().expect("prefix must reject unsupported semantics");
            assert!(error.contains("inspection prefix"), "{error}");
            config.request_body.inspection = WafRequestBodyInspection::Full;
            assert_eq!(config.validation_error(), None, "full fixture must remain executable: {rules}");
        }
}

#[test]
fn blank_local_source_requires_policy_reference() {
    let config = WafConfig {
        rules: " \n\t".to_string(),
        mode: WafMode::On,
        request_body: WafRequestBodyConfig::default(),
        priority: 0,
        ..WafConfig::default()
    };

    assert_eq!(
        config.compile_generation().err().as_deref(),
        Some("policyRef requires a valid name")
    );
    assert_eq!(
        config.validation_error().as_deref(),
        Some("policyRef requires a valid name")
    );
}

#[test]
fn validation_compiles_supported_rules_without_a_separate_prepare_call() {
    let valid = WafConfig {
        rules: "SecRule REQUEST_URI \"@contains private\" \"id:1,phase:1,deny\"\n".to_string(),
        mode: WafMode::On,
        request_body: WafRequestBodyConfig::default(),
        priority: 0,
        ..WafConfig::default()
    };
    assert_eq!(valid.validation_error(), None);

    let invalid = WafConfig {
        rules: "SecRule RESPONSE_BODY \"@contains private\" \"id:2,phase:4,deny\"\n".to_string(),
        mode: WafMode::DetectionOnly,
        request_body: WafRequestBodyConfig::default(),
        priority: 0,
        ..WafConfig::default()
    };
    assert!(invalid.validation_error().is_some());
}

#[test]
fn terminal_waf_status_matches_the_final_reply_contract() {
    for status in [100, 101, 199, 200, 599, 600] {
        let config = WafConfig {
            rules: format!("SecRule REQUEST_URI \"@contains private\" \"id:1,phase:1,deny,status:{status}\"\n"),
            mode: WafMode::On,
            request_body: WafRequestBodyConfig::default(),
            priority: 0,
            ..WafConfig::default()
        };
        assert_eq!(
            config.validation_error().is_none(),
            crate::http_status::is_valid_final_http_status(status),
            "status {status}"
        );
    }
}

#[test]
fn validation_errors_do_not_render_rule_content() {
    let config = WafConfig {
        rules: "SecRule private-(".to_string(),
        mode: WafMode::On,
        request_body: WafRequestBodyConfig::default(),
        priority: 0,
        ..WafConfig::default()
    };
    let error = config.validation_error().expect("invalid source must be rejected");
    assert!(!error.contains("private"));
}

#[test]
fn rules_round_trip_for_transport_but_redact_from_admin_and_debug_output() {
    assert!(serde_json::from_str::<WafConfig>(r#"{"rules":"SecAction \"id:1,phase:1,pass\""}"#).is_err());
    let config = WafConfig {
        rules: "SecAction \"id:1,phase:1,pass,msg:'private'\"".to_string(),
        mode: WafMode::On,
        request_body: WafRequestBodyConfig::default(),
        priority: 17,
        ..WafConfig::default()
    };

    let transport = serde_json::to_value(&config).expect("transport serialization succeeds");
    assert!(transport.get("rules").is_none());

    let admin = {
        let _guard = crate::redact::AdminRedactGuard::new();
        serde_json::to_value(&config).expect("admin serialization succeeds")
    };
    assert!(admin.get("rules").is_none());
    assert_eq!(admin["priority"], 17);
    assert!(!format!("{config:?}").contains("private"));
}

#[test]
fn priority_defaults_to_zero_and_rejects_out_of_range_values() {
    let base = r#"{"policyRef":{"name":"policy"}}"#;
    let parsed: WafConfig = serde_json::from_str(base).expect("missing priority uses its default");
    assert_eq!(parsed.priority, 0);

    let maximum: WafConfig =
        serde_json::from_str(r#"{"policyRef":{"name":"policy"},"priority":65535}"#).expect("u16 maximum is accepted");
    assert_eq!(maximum.priority, u16::MAX);
    assert!(serde_json::from_str::<WafConfig>(r#"{"policyRef":{"name":"policy"},"priority":-1}"#).is_err());
    assert!(serde_json::from_str::<WafConfig>(r#"{"policyRef":{"name":"policy"},"priority":65536}"#).is_err());
}

#[test]
fn delegation_precedence_parses_from_yaml_and_omits_the_default() {
    let parent: WafConfig = yaml_serde::from_str("policyRef:\n  name: policy\ndelegationPrecedence: parent\n")
        .expect("`parent` is a valid precedence");
    assert_eq!(parent.delegation_precedence, WafDelegationPrecedence::Parent);
    let wire = serde_json::to_value(&parent).unwrap();
    assert_eq!(wire["delegationPrecedence"], "parent");
    let restored: WafConfig = serde_json::from_value(wire).unwrap();
    assert_eq!(restored.delegation_precedence, WafDelegationPrecedence::Parent);

    let explicit_child: WafConfig =
        yaml_serde::from_str("policyRef:\n  name: policy\ndelegationPrecedence: child\n").unwrap();
    let omitted: WafConfig = yaml_serde::from_str("policyRef:\n  name: policy\n").unwrap();
    for config in [&explicit_child, &omitted] {
        assert_eq!(config.delegation_precedence, WafDelegationPrecedence::Child);
        assert!(
            serde_json::to_value(config)
                .unwrap()
                .get("delegationPrecedence")
                .is_none(),
            "the default `child` is omitted on the wire"
        );
    }

    for unknown in ["Parent", "outermost", "\"\""] {
        assert!(
            yaml_serde::from_str::<WafConfig>(&format!(
                "policyRef:\n  name: policy\ndelegationPrecedence: {unknown}\n"
            ))
            .is_err(),
            "{unknown} is rejected"
        );
    }
}

#[test]
fn dynamic_integration_generations_compile_with_the_production_profile() {
    let generations = [
        concat!(
            "SecRule REQUEST_HEADERS:X-Waf-Version \"!@streq v1\" ",
            "\"id:1001,phase:1,deny,status:403\"\n",
            "SecRule REQUEST_BODY \"@contains blocked\" ",
            "\"id:1002,phase:2,deny,status:406\"\n"
        ),
        "SecRule REQUEST_HEADERS:X-Waf-Version \"!@streq v2\" \"id:2001,phase:1,deny,status:451\"\n",
        "SecRule REQUEST_HEADERS:X-Waf-Version \"!@streq v3\" \"id:3001,phase:1,deny,status:429\"\n",
    ];

    for rules in generations {
        let config = WafConfig {
            rules: rules.to_string(),
            mode: WafMode::On,
            request_body: WafRequestBodyConfig::default(),
            priority: 0,
            ..WafConfig::default()
        };
        assert_eq!(config.validation_error(), None, "fixture must compile: {rules}");
    }
}

#[test]
fn plugin_requires_one_policy_reference_and_accepts_one_profile_selector() {
    assert!(serde_json::from_value::<WafConfig>(serde_json::json!({})).is_err());
    for field in ["profiles", "defaultProfile", "bundleRefs"] {
        let mut value = serde_json::json!({"policyRef": {"name": "policy"}});
        value[field] = serde_json::json!({});
        assert!(serde_json::from_value::<WafConfig>(value).is_err(), "{field}");
    }
    let config: WafConfig = serde_json::from_value(serde_json::json!({"policyRef": {"name": "policy"}})).unwrap();
    assert!(config.validate_shape().is_ok());
    assert!(config.compile_generation().is_err());
    let selected: WafConfig = serde_json::from_value(serde_json::json!({
        "policyRef": {"name": "policy"},
        "activeProfileRef": {"name": "business-selector"}
    }))
    .unwrap();
    assert_eq!(selected.active_profile_ref.as_ref().unwrap().name, "business-selector");
    assert!(selected.validate_shape().is_ok());
    let fixed: WafConfig = serde_json::from_value(serde_json::json!({
        "policyRef": {"name": "policy"},
        "activeProfile": "high"
    }))
    .unwrap();
    assert_eq!(fixed.active_profile.as_deref(), Some("high"));
    assert!(fixed.validate_shape().is_ok());
    let mut conflicting = fixed;
    conflicting.active_profile_ref = selected.active_profile_ref;
    assert!(conflicting.validate_shape().is_err());
    let schema = serde_json::to_value(schemars::schema_for!(WafConfig)).unwrap();
    let properties = schema["properties"].as_object().unwrap();
    assert!(properties.contains_key("policyRef"));
    assert!(properties.contains_key("activeProfileRef"));
    for field in ["resolvedPolicy", "resolvedBundles", "selectedProfile", "policyResolved"] {
        assert!(!properties.contains_key(field), "{field}");
    }
}

fn json_budget_value() -> serde_json::Value {
    serde_json::json!({
        "lexical": {"maxRawTokenBytes": 2, "maxDecodedBytes": 3,
            "maxEvents": 4, "maxDepth": 5, "maxRetainedBytes": 6, "maxWork": 7},
        "structure": {"maxRetainedBytes": 8, "maxKeyBytes": 9, "maxMembers": 10,
            "maxPathBytes": 12, "maxDepth": 13, "maxWork": 14},
        "results": {"maxFacts": 17, "maxRetainedBytes": 18, "maxWork": 19, "maxCandidates": 20, "maxDeferredEntries": 21, "maxDeferredBytes": 22}
    })
}

#[test]
fn json_budgets_require_every_numeric_field_and_reject_unknown_fields() {
    let value = json_budget_value();
    for group in ["lexical", "structure", "results"] {
        let mut missing_group = value.clone();
        missing_group.as_object_mut().unwrap().remove(group);
        assert!(serde_json::from_value::<WafJsonConfig>(missing_group).is_err());
        for field in value[group].as_object().unwrap().keys() {
            let mut missing = value.clone();
            missing[group].as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<WafJsonConfig>(missing).is_err(),
                "{group}.{field}"
            );
            let mut invalid = value.clone();
            invalid[group][field] = serde_json::json!(-1);
            assert!(
                serde_json::from_value::<WafJsonConfig>(invalid).is_err(),
                "{group}.{field}"
            );
        }
        let mut unknown = value.clone();
        unknown[group]["unknownBudget"] = serde_json::json!(0);
        assert!(serde_json::from_value::<WafJsonConfig>(unknown).is_err());
    }
    let mut unknown = value;
    unknown["unknownGroup"] = serde_json::json!({});
    assert!(serde_json::from_value::<WafJsonConfig>(unknown).is_err());
}

#[test]
fn json_budgets_map_exactly_and_preserve_restrictive_zero_limits() {
    let value = json_budget_value();
    let config: WafJsonConfig = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(config).unwrap(), value);
    let limits = config.inspection_limits(64);
    assert_eq!(
        limits.lexical,
        edgion_waf::seclang::JsonStreamLimits {
            max_source_bytes: 64,
            max_raw_token_bytes: 2,
            max_decoded_bytes: 3,
            max_events: 4,
            max_depth: 5,
            max_retained_bytes: 6,
            max_work: 7,
        }
    );
    assert_eq!(
        limits.structure,
        edgion_waf::seclang::JsonStructureLimits {
            max_retained_bytes: 8,
            max_key_bytes: 9,
            max_members: 10,
            max_path_bytes: 12,
            max_depth: 13,
            max_work: 14,
        }
    );
    assert_eq!(
        (
            limits.results.candidates,
            limits.results.deferred_entries,
            limits.results.deferred_bytes
        ),
        (20, 21, 22)
    );
    assert_eq!(
        (
            limits.results.records,
            limits.results.retained_bytes,
            limits.results.work
        ),
        (17, 18, 19)
    );
    let mut zero = value;
    for group in zero.as_object_mut().unwrap().values_mut() {
        for field in group.as_object_mut().unwrap().values_mut() {
            *field = serde_json::json!(0);
        }
    }
    let zero: WafJsonConfig = serde_json::from_value(zero).unwrap();
    let limits = zero.inspection_limits(64);
    assert_eq!(limits.lexical.max_source_bytes, 64);
    assert_eq!(limits.lexical.max_work, 0);
    assert_eq!(limits.structure.max_members, 0);
    assert_eq!(limits.results.candidates, 0);
    assert_eq!(limits.results.work, 0);
}

#[test]
fn json_full_only_validation_agrees_for_shape_inline_resolved_and_support() {
    let json: WafJsonConfig = serde_json::from_value(json_budget_value()).unwrap();
    let mut inline = WafConfig {
        rules: "SecAction \"id:1,phase:2,pass\"\n".into(),
        policy_ref: EdgionConfigDataRef {
            name: "policy".into(),
            ..Default::default()
        },
        request_body: WafRequestBodyConfig {
            inspection: WafRequestBodyInspection::Prefix,
            json: Some(json),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        inline.validate_shape().err().as_deref(),
        Some("requestBody.json is only valid with full inspection")
    );
    assert!(inline.compile_generation().is_err());
    inline.policy_ref = EdgionConfigDataRef::default();
    assert_eq!(inline.inspect_support().failure.unwrap().category, "invalid_config");
    inline.request_body.inspection = WafRequestBodyInspection::Full;
    assert!(inline.compile_generation().is_ok());
    assert!(inline.inspect_support().strict_validation_passed);

    let (bundle_ref, bundle) = test_bundle("bundle", 1);
    let mut resolved = WafConfig {
        policy_ref: EdgionConfigDataRef {
            name: "policy".into(),
            ..Default::default()
        },
        resolved_policy: Some(WafPolicyData {
            profiles: BTreeMap::from([(
                "default".into(),
                WafProfile {
                    rule_selection: Default::default(),
                    bundle_refs: vec![bundle_ref],
                },
            )]),
            default_profile: "default".into(),
        }),
        selected_profile: Some("default".into()),
        resolved_ref_indices: vec![0],
        resolved_bundles: Some(vec![bundle]),
        owner_namespace: Some("default".into()),
        request_body: WafRequestBodyConfig {
            inspection: WafRequestBodyInspection::Prefix,
            json: Some(json),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(resolved.compile_generation().is_err());
    resolved.request_body.inspection = WafRequestBodyInspection::Full;
    assert!(resolved.compile_generation().is_ok());
    let wire = serde_json::to_value(&resolved).unwrap();
    assert!(serde_json::from_value::<WafConfig>(wire)
        .unwrap()
        .compile_generation()
        .is_ok());
}

#[test]
fn full_json_admission_rejects_dynamic_broad_values_with_default_or_explicit_limits() {
    let source = "SecRule ARGS \"@contains %{TX.private_operand}\" \"id:42,phase:2,pass\"\n";
    for explicit in [false, true] {
        let mut config = WafConfig {
            rules: source.into(),
            request_body: WafRequestBodyConfig {
                inspection: WafRequestBodyInspection::Full,
                json: explicit.then(|| serde_json::from_value(json_budget_value()).unwrap()),
                ..Default::default()
            },
            ..Default::default()
        };
        let error = config.compile_generation().err().unwrap();
        assert!(error.contains("rule 42"));
        assert!(!error.contains("private_operand"));
        assert_eq!(
            config.inspect_support().failure.unwrap().category,
            "unsupported_json_dependency"
        );
        let (mut reference, mut bundle) = test_bundle("bundle", 42);
        bundle.data.rules[0].content = source.into();
        bundle.data.digest = Some(bundle.data.compute_digest());
        bundle.digest = bundle.data.compute_digest();
        reference.expected_digest = Some(bundle.digest.clone());
        config.rules.clear();
        config.policy_ref.name = "policy".into();
        config.resolved_policy = Some(WafPolicyData {
            default_profile: "default".into(),
            profiles: BTreeMap::from([(
                "default".into(),
                WafProfile {
                    rule_selection: Default::default(),
                    bundle_refs: vec![reference],
                },
            )]),
        });
        config.selected_profile = Some("default".into());
        config.resolved_ref_indices = vec![0];
        config.resolved_bundles = Some(vec![bundle]);
        config.owner_namespace = Some("default".into());
        assert!(config.compile_generation().err().unwrap().contains("rule 42"));
    }
}

#[test]
fn obsolete_json_budget_groups_are_rejected() {
    for key in ["document", "plan", "facts"] {
        let mut value = json_budget_value();
        value[key] = serde_json::json!({});
        assert!(serde_json::from_value::<WafJsonConfig>(value).is_err());
    }
}

#[test]
fn dynamic_json_fixture_profiles_keep_valid_invalid_and_zero_limit_purposes() {
    for tree in ["integration", "k8s"] {
        for phase in ["v1", "v2", "detection", "invalid", "zero_work"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                "../edgion-tests/{tree}/conf/EdgionPlugins/Waf/DynamicTest/updates/to_json_{phase}/EdgionPlugins_waf-dynamic.yaml"
            ));
            let document: yaml_serde::Value = yaml_serde::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            let config: WafConfig = yaml_serde::from_value(document["spec"]["waf"].clone()).unwrap();
            assert_eq!(config.request_body.prefix_size_bytes().is_err(), phase == "invalid");
            assert_eq!(
                config.request_body.json.unwrap().inspection_limits(64).lexical.max_work == 0,
                phase == "zero_work"
            );
        }
    }
}

#[test]
fn full_xml_admission_is_identical_for_inline_and_resolved_sources() {
    for source in [
        "SecRule XML \"@contains %{TX.private_operand}\" \"id:42,phase:2,pass\"\n",
        "SecRule XML:/root/node \"@contains blocked\" \"id:42,phase:2,pass\"\n",
    ] {
        let mut config = WafConfig {
            rules: source.into(),
            request_body: WafRequestBodyConfig {
                inspection: WafRequestBodyInspection::Full,
                ..Default::default()
            },
            ..Default::default()
        };
        let error = config.compile_generation().err().unwrap();
        assert!(error.contains("rule 42"));
        assert!(!error.contains("private_operand"));
        assert_eq!(
            config.inspect_support().failure.unwrap().category,
            "unsupported_xml_dependency"
        );
        let (mut reference, mut bundle) = test_bundle("bundle", 42);
        bundle.data.rules[0].content = source.into();
        bundle.data.digest = Some(bundle.data.compute_digest());
        bundle.digest = bundle.data.compute_digest();
        reference.expected_digest = Some(bundle.digest.clone());
        config.rules.clear();
        config.policy_ref.name = "policy".into();
        config.resolved_policy = Some(WafPolicyData {
            default_profile: "default".into(),
            profiles: BTreeMap::from([(
                "default".into(),
                WafProfile {
                    rule_selection: Default::default(),
                    bundle_refs: vec![reference],
                },
            )]),
        });
        config.selected_profile = Some("default".into());
        config.resolved_ref_indices = vec![0];
        config.resolved_bundles = Some(vec![bundle]);
        config.owner_namespace = Some("default".into());
        assert!(config.compile_generation().err().unwrap().contains("rule 42"));
    }
    for target in ["XML", "XML:/*", "XML://@*", "&XML:/*"] {
        let config = WafConfig {
            rules: format!("SecRule {target} \"@contains blocked\" \"id:42,phase:2,pass\""),
            request_body: WafRequestBodyConfig {
                inspection: WafRequestBodyInspection::Full,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(config.compile_generation().is_ok());
    }
}

#[test]
fn xml_dynamic_fixtures_pin_valid_bundles_and_strict_static_xml_demands() {
    for tree in ["integration", "k8s"] {
        for phase in ["v1", "v2", "detection"] {
            let version = if phase == "v2" { "xml-v2" } else { "xml-v1" };
            let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
                "../edgion-tests/{tree}/conf/EdgionPlugins/Waf/DynamicTest/updates/to_xml_{phase}"
            ));
            let bundle: yaml_serde::Value = yaml_serde::from_str(
                &std::fs::read_to_string(base.join(format!("EdgionConfigData_waf-rules-{version}.yaml"))).unwrap(),
            )
            .unwrap();
            let data: crate::resources::WafRuleBundleData =
                yaml_serde::from_value(bundle["spec"]["data"]["config"].clone()).unwrap();
            data.validate().unwrap();
            let policy: yaml_serde::Value =
                yaml_serde::from_str(&std::fs::read_to_string(base.join("EdgionConfigData_waf-policy.yaml")).unwrap())
                    .unwrap();
            assert_eq!(
                policy["spec"]["data"]["config"]["profiles"]["baseline"]["bundleRefs"][0]["expectedDigest"].as_str(),
                Some(data.compute_digest().as_str())
            );
            let plugin: yaml_serde::Value =
                yaml_serde::from_str(&std::fs::read_to_string(base.join("EdgionPlugins_waf-dynamic.yaml")).unwrap())
                    .unwrap();
            let mut config: WafConfig = yaml_serde::from_value(plugin["spec"]["waf"].clone()).unwrap();
            assert_eq!(config.request_body.inspection, WafRequestBodyInspection::Full);
            assert!(config.request_body.json.is_none());
            config.rules = data.rules[0].content.clone();
            config.policy_ref.name.clear();
            let generation = config.compile_generation().unwrap();
            assert!(generation.xml_inspection_unsupported().is_empty());
            for tag in [
                "attack-protocol",
                "attack-lfi",
                "attack-rfi",
                "attack-rce",
                "attack-php",
                "attack-generic",
                "attack-xss",
                "attack-sqli",
                "attack-fixation",
            ] {
                assert!(config
                    .rules
                    .contains(&format!("ctl:ruleRemoveTargetByTag={tag};XML://@*")));
            }
        }
    }
}

#[test]
fn selected_bundle_transport_identity_is_lexical_and_optional_slots_stay_optional() {
    let (reference, bundle) = test_bundle("rules", 9821);
    let mut optional = reference.clone();
    optional.optional = true;
    optional.target.namespace = Some("".into());
    let mut config = WafConfig {
        policy_ref: EdgionConfigDataRef {
            name: "policy".into(),
            ..Default::default()
        },
        resolved_policy: Some(WafPolicyData {
            profiles: BTreeMap::from([(
                "base".into(),
                WafProfile {
                    bundle_refs: vec![reference, optional],
                    ..Default::default()
                },
            )]),
            default_profile: "base".into(),
        }),
        selected_profile: Some("base".into()),
        resolved_ref_indices: vec![0],
        resolved_bundles: Some(vec![bundle]),
        owner_namespace: Some("default".into()),
        ..Default::default()
    };
    assert!(config.compile_generation().is_ok());
    for namespace in ["", "bad/ns", "Bad"] {
        config
            .resolved_policy
            .as_mut()
            .unwrap()
            .profiles
            .get_mut("base")
            .unwrap()
            .bundle_refs[0]
            .target
            .namespace = Some(namespace.into());
        config.resolved_bundles.as_mut().unwrap()[0].identity = format!("{namespace}/rules");
        assert!(
            config.compile_generation().is_err(),
            "matching transport strings cannot legitimize invalid identity"
        );
    }
}

#[test]
fn format_hard_limits_are_optional_positive_and_independent_of_inspection_mode() {
    for inspection in ["prefix", "full"] {
        let body: WafRequestBodyConfig = serde_json::from_value(serde_json::json!({
            "inspection": inspection, "maxJsonBodySize": "2MiB", "maxXmlBodySize": "1KiB"
        }))
        .unwrap();
        assert_eq!(body.format_body_limits().unwrap(), (Some(2 << 20), Some(1024)));
        assert!(body.prefix_size_bytes().is_ok());
        let encoded = serde_json::to_value(&body).unwrap();
        assert_eq!(encoded["maxJsonBodySize"], "2MiB");
        assert_eq!(encoded["maxXmlBodySize"], "1KiB");
    }
    let omitted = WafRequestBodyConfig::default();
    assert_eq!(omitted.format_body_limits().unwrap(), (None, None));
    let encoded = serde_json::to_value(&omitted).unwrap();
    assert!(encoded.get("maxJsonBodySize").is_none());
    for field in ["maxJsonBodySize", "maxXmlBodySize"] {
        for invalid in ["0", "-1", "invalid", "inf", "NaN"] {
            let body: WafRequestBodyConfig = serde_json::from_value(serde_json::json!({field: invalid})).unwrap();
            assert!(body.prefix_size_bytes().unwrap_err().contains(field));
        }
    }
    assert!(serde_json::from_value::<WafRequestBodyConfig>(serde_json::json!({"maxJsonSize": "1MiB"})).is_err());
}

#[test]
fn inspection_window_is_positive_full_only_and_default_continue() {
    let body: WafRequestBodyConfig = serde_json::from_value(serde_json::json!({
        "inspection": "full", "maxInspectSize": "2MiB"
    }))
    .unwrap();
    assert_eq!(body.inspection_limit_bytes().unwrap(), Some(2 << 20));
    assert_eq!(body.on_limit.unwrap_or_default(), WafInspectionLimitAction::Continue);
    for value in [
        serde_json::json!({"inspection":"full", "maxInspectSize":"0"}),
        serde_json::json!({"inspection":"full", "maxInspectSize":"invalid"}),
        serde_json::json!({"inspection":"prefix", "maxInspectSize":"1MiB"}),
        serde_json::json!({"inspection":"prefix", "onLimit":"continue"}),
    ] {
        let body: WafRequestBodyConfig = serde_json::from_value(value).unwrap();
        assert!(body.prefix_size_bytes().is_err());
    }
    assert!(serde_json::from_value::<WafRequestBodyConfig>(serde_json::json!({
        "inspection":"full", "onLimit":"ignore"
    }))
    .is_err());
}

#[test]
fn removed_json_source_knob_is_rejected_and_full_is_default() {
    let mut budgets = json_budget_value();
    budgets["lexical"]["maxSourceBytes"] = serde_json::json!(1024);
    assert!(serde_json::from_value::<WafJsonConfig>(budgets).is_err());
    let body: WafRequestBodyConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(body.inspection, WafRequestBodyInspection::Full);
}
