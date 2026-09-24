use super::*;

fn analyze_wat(source: &str) -> CompatibilityReport {
    analyze(&wat::parse_str(source).expect("test WAT must compile"))
}

#[test]
fn supported_abi_and_exact_import_are_compatible() {
    let report = analyze_wat(
        r#"(module
                (import "env" "proxy_log" (func (param i32 i32 i32) (result i32)))
                (func (export "proxy_abi_version_0_2_1"))
                (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
                    i32.const 0)
            )"#,
    );
    assert!(report.compatible, "{:?}", report.errors);
    assert_eq!(report.abi_markers, [ABI_0_2_1]);
    assert_eq!(report.imports.len(), 1);
    assert_eq!(report.hooks.len(), 1);
}

#[test]
fn missing_marker_is_only_a_warning() {
    let report = analyze_wat("(module)");
    assert!(report.compatible);
    assert_eq!(report.warnings[0].code, CompatibilityCode::MissingAbiMarker);
}

#[test]
fn unsupported_and_mixed_markers_are_rejected() {
    let unsupported = analyze_wat(r#"(module (func (export "proxy_abi_version_0_1_0")))"#);
    assert_eq!(
        unsupported.errors.last().map(|issue| issue.code),
        Some(CompatibilityCode::UnsupportedAbiMarker)
    );

    let mixed = analyze_wat(
        r#"(module
                (func (export "proxy_abi_version_0_2_0"))
                (func (export "proxy_abi_version_0_2_1"))
            )"#,
    );
    assert_eq!(
        mixed.errors.last().map(|issue| issue.code),
        Some(CompatibilityCode::MixedAbiMarkers)
    );
}

#[test]
fn supported_and_unsupported_markers_are_mixed_and_rejected() {
    let report = analyze_wat(
        r#"(module
                (func (export "proxy_abi_version_0_2_0"))
                (func (export "proxy_abi_version_9_9_9"))
            )"#,
    );
    assert_eq!(
        report.errors.last().map(|issue| issue.code),
        Some(CompatibilityCode::MixedAbiMarkers)
    );
}

#[test]
fn unknown_wrong_module_wrong_signature_and_nonfunction_imports_are_rejected() {
    let cases = [
        (
            r#"(module (import "env" "proxy_unknown" (func)))"#,
            CompatibilityCode::UnknownImport,
        ),
        (
            r#"(module (import "wasi_snapshot_preview1" "proxy_log" (func (param i32 i32 i32) (result i32))))"#,
            CompatibilityCode::WrongImportModule,
        ),
        (
            r#"(module (import "env" "proxy_log" (func (param i64 i32 i32) (result i32))))"#,
            CompatibilityCode::ImportSignatureMismatch,
        ),
        (
            r#"(module (import "env" "proxy_log" (memory 1)))"#,
            CompatibilityCode::NonFunctionImport,
        ),
    ];
    for (wat, expected) in cases {
        let report = analyze_wat(wat);
        assert!(
            report.errors.iter().any(|issue| issue.code == expected),
            "{expected:?}: {:?}",
            report.errors
        );
    }
}

#[test]
fn analysis_limits_reject_import_and_export_inventory_before_reporting() {
    let limits = AnalysisLimits {
        entries: 100,
        inventory_items: 1,
        inventory_name_bytes: 1024,
        ..DEFAULT_ANALYSIS_LIMITS
    };
    let cases = [
        r#"(module
                (import "env" "proxy_close_stream" (func (param i32) (result i32)))
                (import "env" "proxy_get_log_level" (func (param i32) (result i32)))
            )"#,
        r#"(module
                (func (export "custom_one"))
                (func (export "custom_two"))
            )"#,
    ];
    for wat in cases {
        let bytes = wat::parse_str(wat).expect("test WAT");
        let report = analyze_with_limits(&bytes, limits);
        assert!(!report.compatible);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].code, CompatibilityCode::AnalysisLimitExceeded);
        assert!(report.imports.is_empty());
        assert!(report.exports.is_empty());
        assert!(report.warnings.is_empty());
    }
}

#[test]
fn analysis_limits_reject_inventory_name_budget_before_reporting() {
    let bytes = wat::parse_str(
        r#"(module
                (func (export "custom_name_over_budget"))
            )"#,
    )
    .expect("test WAT");
    let report = analyze_with_limits(
        &bytes,
        AnalysisLimits {
            entries: 100,
            inventory_items: 10,
            inventory_name_bytes: 4,
            ..DEFAULT_ANALYSIS_LIMITS
        },
    );
    assert!(!report.compatible);
    assert_eq!(report.errors[0].code, CompatibilityCode::AnalysisLimitExceeded);
    assert!(report.exports.is_empty());
}

#[test]
fn report_construction_defensively_rechecks_analysis_limits() {
    let bytes = wat::parse_str(
        r#"(module
                (func (export "custom_one"))
                (func (export "custom_two"))
            )"#,
    )
    .expect("test WAT");
    Validator::new().validate_all(&bytes).expect("valid test module");
    let mut report = CompatibilityReport::new(&bytes);
    analyze_valid_module(
        &bytes,
        &mut report,
        AnalysisLimits {
            entries: 100,
            inventory_items: 1,
            inventory_name_bytes: 1024,
            ..DEFAULT_ANALYSIS_LIMITS
        },
    );
    assert!(!report.compatible);
    assert!(report
        .errors
        .iter()
        .any(|issue| issue.code == CompatibilityCode::AnalysisLimitExceeded));
    assert!(report.exports.is_empty());
}

#[test]
fn analysis_limits_count_validator_index_sections() {
    let cases = [
        ("(module (global i32 (i32.const 0)))", 1),
        ("(module (table 1 funcref))", 1),
        ("(module (memory 1))", 1),
        ("(module (memory 1) (data \"x\"))", 2),
        ("(module (type (func)) (tag (type 0)))", 2),
        ("(module (func $f) (elem func $f))", 3),
    ];
    for (wat, expected_entries) in cases {
        let bytes = wat::parse_str(wat).expect("test WAT");
        let complexity = preflight_complexity(&bytes, DEFAULT_ANALYSIS_LIMITS).expect("preflight");
        assert_eq!(complexity.entries, expected_entries, "{wat}");
        let report = analyze_with_limits(
            &bytes,
            AnalysisLimits {
                entries: expected_entries - 1,
                ..DEFAULT_ANALYSIS_LIMITS
            },
        );
        assert_eq!(report.errors[0].code, CompatibilityCode::AnalysisLimitExceeded);
    }
}

#[test]
fn analysis_limits_bound_per_function_and_total_expanded_locals() {
    let per_function = wat::parse_str("(module (func (local i32 i32 i32)))").expect("test WAT");
    let report = analyze_with_limits(
        &per_function,
        AnalysisLimits {
            function_locals: 2,
            ..DEFAULT_ANALYSIS_LIMITS
        },
    );
    assert_eq!(report.errors[0].code, CompatibilityCode::AnalysisLimitExceeded);

    let total = wat::parse_str("(module (func (local i32 i32)) (func (local i32 i32)))").expect("test WAT");
    let report = analyze_with_limits(
        &total,
        AnalysisLimits {
            function_locals: 3,
            total_locals: 3,
            ..DEFAULT_ANALYSIS_LIMITS
        },
    );
    assert_eq!(report.errors[0].code, CompatibilityCode::AnalysisLimitExceeded);
}

#[test]
fn analysis_limits_bound_encoded_function_body_size() {
    let bytes = wat::parse_str("(module (func nop nop nop nop nop nop nop nop))").expect("test WAT");
    let complexity = preflight_complexity(&bytes, DEFAULT_ANALYSIS_LIMITS).expect("preflight");
    assert!(complexity.max_function_body_bytes > 1);
    let report = analyze_with_limits(
        &bytes,
        AnalysisLimits {
            function_body_bytes: complexity.max_function_body_bytes - 1,
            ..DEFAULT_ANALYSIS_LIMITS
        },
    );
    assert_eq!(report.errors[0].code, CompatibilityCode::AnalysisLimitExceeded);
}

#[test]
fn analysis_limits_bound_type_and_repeated_report_signature_cells() {
    let type_only = wat::parse_str("(module (type (func (param i32 i32 i32 i32))))").expect("test WAT");
    let report = analyze_with_limits(
        &type_only,
        AnalysisLimits {
            type_signature_cells: 3,
            ..DEFAULT_ANALYSIS_LIMITS
        },
    );
    assert_eq!(report.errors[0].code, CompatibilityCode::AnalysisLimitExceeded);

    let repeated = wat::parse_str(
        r#"(module
                (type $long (func (param i32 i32 i32 i32)))
                (import "env" "custom_one" (func (type $long)))
                (import "env" "custom_two" (func (type $long)))
            )"#,
    )
    .expect("test WAT");
    let report = analyze_with_limits(
        &repeated,
        AnalysisLimits {
            report_signature_cells: 7,
            ..DEFAULT_ANALYSIS_LIMITS
        },
    );
    assert_eq!(report.errors[0].code, CompatibilityCode::AnalysisLimitExceeded);
    assert!(report.imports.is_empty());
    assert!(report.warnings.is_empty());
}

#[test]
fn reference_sdk_guest_is_far_below_analysis_limits() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/wasm/artifacts/header_demo.wasm");
    let bytes = std::fs::read(path).expect("committed reference guest");
    let complexity = preflight_complexity(&bytes, DEFAULT_ANALYSIS_LIMITS).expect("reference guest preflight");
    assert_eq!(complexity.entries, 487);
    assert_eq!(complexity.inventory_items, 63);
    assert_eq!(complexity.inventory_name_bytes, 1_451);
    assert_eq!(complexity.type_signature_cells, 188);
    assert_eq!(complexity.report_signature_cells, 304);
    assert_eq!(complexity.total_locals, 891);
    assert_eq!(complexity.max_function_body_bytes, 4_973);
    let report = analyze(&bytes);
    assert!(report.compatible, "{:#?}", report.errors);
}

#[test]
fn function_export_kinds_share_signature_classification() {
    assert!(is_function_export_kind(ExternalKind::Func));
    assert!(is_function_export_kind(ExternalKind::FuncExact));
    assert!(!is_function_export_kind(ExternalKind::Global));
    assert_eq!(export_kind(ExternalKind::FuncExact), "function");
}

#[test]
fn single_type_rec_group_remains_compatible() {
    let bytes = wat::parse_str("(module (rec (type (func))))").expect("rec-group WAT");
    let report = analyze(&bytes);
    assert!(report.compatible, "{:#?}", report.errors);
}

#[test]
fn sdk_declared_unimplemented_import_is_load_compatible_but_not_supported() {
    let report = analyze_wat(
        r#"(module
                (import "env" "proxy_close_stream" (func (param i32) (result i32)))
                (func (export "proxy_abi_version_0_2_0"))
            )"#,
    );
    assert!(report.compatible, "{:?}", report.errors);
    assert_eq!(report.imports[0].support, ImportSupport::KnownUnsupportedTrap);
    assert!(report
        .warnings
        .iter()
        .any(|issue| issue.code == CompatibilityCode::KnownUnsupportedImport));
}

#[test]
fn sdk_metric_import_preserves_i64_signature_and_trap_classification() {
    let report = analyze_wat(
        r#"(module
                (import "env" "proxy_record_metric" (func (param i32 i64) (result i32)))
                (func (export "proxy_abi_version_0_2_0"))
            )"#,
    );
    assert!(report.compatible, "{:?}", report.errors);
    assert_eq!(report.imports[0].support, ImportSupport::KnownUnsupportedTrap);
    assert_eq!(
        report.imports[0].signature.as_ref().expect("function signature").params,
        ["i32", "i64"]
    );
}

#[test]
fn standard_hooks_report_invocation_support_without_affecting_load_compatibility() {
    let report = analyze_wat(
        r#"(module
                (func (export "proxy_abi_version_0_2_0"))
                (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
                    i32.const 0)
                (func (export "proxy_on_request_trailers") (param i32 i32) (result i32)
                    i32.const 0)
            )"#,
    );
    assert!(report.compatible, "{:?}", report.errors);
    let implemented = report
        .hooks
        .iter()
        .find(|hook| hook.name == "proxy_on_request_headers")
        .expect("implemented hook is classified");
    assert_eq!(implemented.support, HookSupport::Implemented);
    let not_invoked = report
        .hooks
        .iter()
        .find(|hook| hook.name == "proxy_on_request_trailers")
        .expect("known uninvoked hook is classified");
    assert_eq!(not_invoked.support, HookSupport::KnownNotInvoked);
    assert_eq!(
        report
            .warnings
            .iter()
            .filter(|issue| issue.code == CompatibilityCode::KnownNotInvokedHook)
            .count(),
        1,
        "implemented hooks must not emit not-invoked warnings"
    );
}

#[test]
fn wrong_signature_cannot_masquerade_as_a_standard_hook() {
    let report = analyze_wat(
        r#"(module
                (func (export "proxy_abi_version_0_2_0"))
                (func (export "proxy_on_request_trailers") (param i64) (result i32)
                    i32.const 0)
            )"#,
    );
    assert!(!report.compatible);
    assert!(report.hooks.is_empty());
    assert!(report
        .errors
        .iter()
        .any(|issue| issue.code == CompatibilityCode::HookSignatureMismatch));
    assert!(!report
        .warnings
        .iter()
        .any(|issue| issue.code == CompatibilityCode::KnownNotInvokedHook));
}

#[test]
fn unknown_or_misspelled_proxy_hook_is_rejected() {
    let report = analyze_wat(
        r#"(module
                (func (export "proxy_abi_version_0_2_0"))
                (func (export "proxy_on_request_header") (param i32 i32 i32) (result i32)
                    i32.const 0)
                (func (export "custom_extension"))
            )"#,
    );
    assert!(!report.compatible);
    assert!(report
        .errors
        .iter()
        .any(|issue| issue.code == CompatibilityCode::UnknownHook));
    assert!(report.exports.iter().any(|export| export.name == "custom_extension"));
}

#[test]
fn malformed_component_and_bad_hook_are_rejected() {
    let malformed = analyze(b"not wasm");
    assert_eq!(malformed.errors[0].code, CompatibilityCode::Malformed);

    let component = analyze(&wat::parse_str("(component)").expect("component WAT"));
    assert_eq!(component.errors[0].code, CompatibilityCode::ComponentUnsupported);

    let hook = analyze_wat(
        r#"(module
                (func (export "proxy_abi_version_0_2_0"))
                (func (export "proxy_on_tick") (param i64))
            )"#,
    );
    assert!(hook
        .errors
        .iter()
        .any(|issue| issue.code == CompatibilityCode::HookSignatureMismatch));
}

#[test]
fn marker_and_hook_exports_must_be_functions_with_exact_signatures() {
    let marker = analyze_wat(
        r#"(module
                (memory (export "proxy_abi_version_0_2_0") 1)
            )"#,
    );
    assert!(marker
        .errors
        .iter()
        .any(|issue| issue.code == CompatibilityCode::InvalidAbiMarker));

    let hook = analyze_wat(
        r#"(module
                (func (export "proxy_abi_version_0_2_0"))
                (global (export "proxy_on_tick") i32 (i32.const 0))
            )"#,
    );
    assert!(hook
        .errors
        .iter()
        .any(|issue| issue.code == CompatibilityCode::NonFunctionHook));
}

#[test]
fn report_json_shape_is_stable() {
    let report = analyze_wat(r#"(module (func (export "proxy_abi_version_0_2_0")))"#);
    let value = serde_json::to_value(report).expect("report serializes");
    assert_eq!(value["reportVersion"], REPORT_VERSION);
    assert_eq!(value["compatible"], true);
    assert_eq!(value["abiMarkers"][0], ABI_0_2_0);
    assert!(value["sha256"].as_str().is_some_and(|digest| digest.len() == 64));

    let hook_report = analyze_wat(
        r#"(module
                (func (export "proxy_on_request_trailers") (param i32 i32) (result i32)
                    i32.const 0)
            )"#,
    );
    let hook_value = serde_json::to_value(hook_report).expect("hook report serializes");
    assert_eq!(hook_value["hooks"][0]["support"], "known_not_invoked");
}
