use super::*;

#[test]
fn test_basic_from_header_config() {
    let yaml = r#"
vars:
  - name: user_id
    from:
      type: header
      name: X-User-Id
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    config.validate().expect("validate should succeed");
    assert_eq!(config.vars.len(), 1);
    assert_eq!(config.vars[0].name, "user_id");
}

#[test]
fn test_static_value_config() {
    let yaml = r#"
vars:
  - name: env
    value: production
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    config.validate().expect("validate should succeed");
    assert_eq!(config.vars[0].value, Some("production".to_string()));
}

#[test]
fn test_template_config() {
    let yaml = r#"
vars:
  - name: rate_key
    template: "${header:X-Tenant}_${remoteIp}"
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    config.validate().expect("validate should succeed");
    assert_eq!(
        config.vars[0].template,
        Some("${header:X-Tenant}_${remoteIp}".to_string())
    );
}

#[test]
fn test_extract_config() {
    let yaml = r#"
vars:
  - name: jwt_user
    from:
      type: header
      name: Authorization
    extract:
      regex: "Bearer .*\\.(.+)\\."
      group: 1
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    config.validate().expect("validate should succeed");
    assert!(config.get_compiled_regex(0).is_some());
}

#[test]
fn test_transform_replace_config() {
    let yaml = r#"
vars:
  - name: clean_path
    from:
      type: path
    transform:
      replace:
        pattern: "^/api/v[0-9]+/"
        with: "/"
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    config.validate().expect("validate should succeed");
}

#[test]
fn test_transform_case_config() {
    let yaml = r#"
vars:
  - name: method_lower
    from:
      type: method
    transform:
      case: lower
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    config.validate().expect("validate should succeed");
    let transform = config.vars[0].transform.as_ref().unwrap();
    assert!(matches!(transform.case, Some(CaseType::Lower)));
}

#[test]
fn test_transform_substring_config() {
    let yaml = r#"
vars:
  - name: short_trace
    from:
      type: header
      name: X-Trace-Id
    transform:
      substring: [0, 8]
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    config.validate().expect("validate should succeed");
    let transform = config.vars[0].transform.as_ref().unwrap();
    assert_eq!(transform.substring, Some((0, 8)));
}

#[test]
fn test_transform_prefix_suffix_config() {
    let yaml = r#"
vars:
  - name: prefixed_id
    from:
      type: header
      name: X-Id
    transform:
      prefix: "user_"
  - name: suffixed_id
    from:
      type: header
      name: X-Id
    transform:
      suffix: "_v2"
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    config.validate().expect("validate should succeed");
    let t0 = config.vars[0].transform.as_ref().unwrap();
    let t1 = config.vars[1].transform.as_ref().unwrap();
    assert!(t0.prefix.is_some());
    assert!(t1.suffix.is_some());
}

#[test]
fn test_mapping_config() {
    let yaml = r#"
vars:
  - name: rate_tier
    from:
      type: header
      name: X-Plan
    mapping:
      values:
        premium: tier_1
        enterprise: tier_1
        basic: tier_2
      default: tier_3
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    config.validate().expect("validate should succeed");
    let mapping = config.vars[0].mapping.as_ref().unwrap();
    assert_eq!(mapping.values.get("premium"), Some(&"tier_1".to_string()));
    assert_eq!(mapping.default, Some("tier_3".to_string()));
}

#[test]
fn test_full_pipeline_config() {
    let yaml = r#"
vars:
  - name: tenant_id
    from:
      type: header
      name: Authorization
    extract:
      regex: '"tenant":"([^"]+)"'
      group: 1
    transform:
      case: lower
    mapping:
      values:
        acme: tenant_acme
        globex: tenant_globex
      default: tenant_default
    default: unknown_tenant
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    config.validate().expect("validate should succeed");
}

#[test]
fn test_validation_empty_vars() {
    let yaml = r#"
vars: []
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(err.contains("vars list is empty"));
}

#[test]
fn test_validation_empty_name() {
    let yaml = r#"
vars:
  - name: ""
    value: test
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(err.contains("name is empty"));
}

#[test]
fn test_validation_no_value_source() {
    let yaml = r#"
vars:
  - name: test_var
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(err.contains("must specify"));
}

#[test]
fn test_validation_invalid_extract_regex() {
    let yaml = r#"
vars:
  - name: test
    from:
      type: header
      name: X-Test
    extract:
      regex: "([invalid"
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(err.contains("invalid extract regex"));
}

#[test]
fn test_validation_invalid_transform_regex() {
    let yaml = r#"
vars:
  - name: test
    from:
      type: path
    transform:
      replace:
        pattern: "([invalid"
        with: ""
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(err.contains("invalid transform.replace pattern"));
}

#[test]
fn test_validation_invalid_substring_bounds() {
    let yaml = r#"
vars:
  - name: test
    from:
      type: header
      name: X-Test
    transform:
      substring: [10, 5]
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(err.contains("substring start"));
}

fn parse_ctx_set(yaml: &str) -> CtxSetConfig {
    yaml_serde::from_str(yaml).expect("ctx set yaml should deserialize")
}

fn single_transform_yaml(transform: &str) -> String {
    format!("vars:\n  - name: user\n    value: ALICE\n    transform:\n{transform}")
}

#[test]
fn transform_accepts_zero_or_one_operation() {
    for yaml in [
        single_transform_yaml("      {}"),
        single_transform_yaml("      trim: false"),
        single_transform_yaml("      replace:\n        pattern: A\n        with: x"),
        single_transform_yaml("      substring: [0, 1]"),
        single_transform_yaml("      case: lower"),
        single_transform_yaml("      prefix: user_"),
        single_transform_yaml("      suffix: _id"),
        single_transform_yaml("      trim: true"),
        single_transform_yaml("      prefix: user_\n      trim: false"),
    ] {
        let mut config = parse_ctx_set(&yaml);
        config
            .validate()
            .unwrap_or_else(|err| panic!("expected valid transform, got {err}: {yaml}"));
        assert_eq!(config.validation_error(), None, "{yaml}");
    }
}

#[test]
fn transform_rejects_multiple_operations() {
    let expected = "vars[0] 'user': transform allows at most one operation";
    for yaml in [
        single_transform_yaml("      case: lower\n      prefix: user_"),
        single_transform_yaml("      prefix: user_\n      trim: true"),
        single_transform_yaml("      substring: [0, 8]\n      suffix: _id"),
        single_transform_yaml("      replace:\n        pattern: A\n        with: x\n      case: lower"),
    ] {
        let mut config = parse_ctx_set(&yaml);
        assert_eq!(config.validate().unwrap_err(), expected, "{yaml}");
        assert_eq!(config.validation_error().as_deref(), Some(expected), "{yaml}");
    }
}

#[test]
fn chained_ctx_rules_may_apply_one_transform_each() {
    let mut config = parse_ctx_set(
        r#"
vars:
  - name: user
    value: ALICE
    transform:
      case: lower
  - name: user
    from:
      type: ctx
      name: user
    transform:
      prefix: "user_"
"#,
    );
    config.validate().expect("chained single-operation rules remain valid");
    assert_eq!(config.validation_error(), None);
}

#[test]
fn test_template_compilation() {
    let compiled = CompiledTemplate::compile("hello ${header:X-User} from ${remoteIp}").unwrap();
    let segs = compiled.segments();
    assert_eq!(segs.len(), 4);
    assert!(matches!(&segs[0], TemplateSegment::Literal(s) if s == "hello "));
    assert!(
        matches!(&segs[1], TemplateSegment::Variable { var_type, var_name } if matches!(var_type, TemplateVarType::Header) && var_name.as_deref() == Some("X-User"))
    );
    assert!(matches!(&segs[2], TemplateSegment::Literal(s) if s == " from "));
    assert!(
        matches!(&segs[3], TemplateSegment::Variable { var_type, var_name } if matches!(var_type, TemplateVarType::RemoteIp) && var_name.is_none())
    );
    assert!(compiled.hint_len() > 0);
}

#[test]
fn test_template_compilation_unclosed() {
    let result = CompiledTemplate::compile("hello ${header:X-User");
    assert!(result.is_err());
}

#[test]
fn test_template_compilation_unknown_var_type() {
    let result = CompiledTemplate::compile("${unknown:foo}");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unknown variable type"));
}

#[test]
fn old_client_ip_template_variable_is_rejected() {
    let result = CompiledTemplate::compile("${clientIp}");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unknown variable type"));
}

#[test]
fn test_template_compilation_no_variables() {
    let compiled = CompiledTemplate::compile("just a literal string").unwrap();
    assert_eq!(compiled.segments().len(), 1);
    assert!(matches!(&compiled.segments()[0], TemplateSegment::Literal(s) if s == "just a literal string"));
}

/// Regression test: `validation_error()` via `PluginValidation` must work
/// without calling `validate()` first, and must NOT touch the cached field.
#[test]
fn validation_error_without_validate_call() {
    let config = CtxSetConfig::default(); // vars is empty
    assert_eq!(config.validation_error(), Some("vars list is empty".to_string()));
}

#[test]
fn test_template_validation_catches_unclosed() {
    let yaml = r#"
vars:
  - name: bad_template
    template: "hello ${header:oops"
"#;
    let mut config: CtxSetConfig = yaml_serde::from_str(yaml).unwrap();
    let err = config.validate().unwrap_err();
    assert!(err.contains("invalid template"));
}

#[test]
fn secret_ref_variant_parses() {
    let compiled = CompiledTemplate::compile("${secretRef:edgion-system/tok:key}").expect("compile");
    let names = secret_ref_var_names(&compiled);
    assert_eq!(names, vec!["edgion-system/tok:key".to_string()]);
}

#[test]
fn parse_secret_ref_valid() {
    let p = parse_secret_ref("edgion-system/tok:api-key").expect("valid");
    assert_eq!(p.namespace, "edgion-system");
    assert_eq!(p.name, "tok");
    assert_eq!(p.key, "api-key");
}

#[test]
fn parse_secret_ref_rejects_missing_key() {
    assert!(parse_secret_ref("edgion-system/tok").is_err());
}

#[test]
fn parse_secret_ref_rejects_missing_namespace() {
    assert!(parse_secret_ref("tok:key").is_err());
}

#[test]
fn ctx_set_rejects_secret_ref_template() {
    let cfg: CtxSetConfig = serde_json::from_value(serde_json::json!({
        "vars": [ { "name": "x", "template": "${secretRef:ns/n:k}" } ]
    }))
    .expect("deserialize");
    let err = cfg.check_fields().expect("must reject");
    assert!(err.contains("secretRef"), "got: {err}");
}

#[test]
fn ctx_set_rejects_bare_secret_ref() {
    // A bare `${secretRef}` (no ns/name:key) must also be rejected by check_fields.
    let cfg: CtxSetConfig = serde_json::from_value(serde_json::json!({
        "vars": [ { "name": "x", "template": "${secretRef}" } ]
    }))
    .expect("deserialize");
    let err = cfg.check_fields().expect("must reject bare secretRef");
    assert!(err.contains("secretRef"), "got: {err}");
}

#[test]
fn test_matched_route_tokens_accepted() {
    assert!(CompiledTemplate::compile("${matchedHttpRoute:name}").is_ok());
    assert!(CompiledTemplate::compile("${matchedGrpcRoute:annotation.owner}").is_ok());
    assert!(CompiledTemplate::compile("${matchedFooRoute:name}").is_err());
}

#[test]
fn transform_validation_preserves_large_operands_and_refreshes_only_owned_caches() {
    let large = "x".repeat(4096);
    for transform in [
        serde_json::json!({"prefix": large}),
        serde_json::json!({"suffix": large}),
        serde_json::json!({"replace": {"pattern": "(ALICE)", "with": large}}),
    ] {
        let mut config: CtxSetConfig = serde_json::from_value(serde_json::json!({
            "vars": [{"name": "user", "value": "ALICE", "transform": transform}]
        }))
        .unwrap();
        let authored = serde_json::to_value(&config.vars).unwrap();
        assert_eq!(config.validation_error(), None);
        assert!(config.compiled_transform_patterns.is_empty());
        config.validate().unwrap();
        let has_replace = transform.get("replace").is_some();
        assert_eq!(config.get_compiled_transform_regex(0).is_some(), has_replace);
        if let Some(regex) = config.get_compiled_transform_regex(0) {
            assert!(regex.is_match("ALICE"));
        }
        assert_eq!(serde_json::to_value(&config.vars).unwrap(), authored);
        config.vars[0].transform = Some(serde_json::from_value(serde_json::json!({"prefix": large})).unwrap());
        assert_eq!(config.validation_error(), None);
        assert_eq!(config.get_compiled_transform_regex(0).is_some(), has_replace);
        config.validate().unwrap();
        assert!(config.compiled_transform_patterns.is_empty());
    }
}
