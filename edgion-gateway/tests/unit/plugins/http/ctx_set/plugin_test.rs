use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;

fn parse_config(yaml: &str) -> CtxSetConfig {
    serde_yaml::from_str(yaml).expect("config should deserialize")
}

fn parse_transform(yaml: &str) -> TransformConfig {
    serde_yaml::from_str(yaml).expect("transform should deserialize")
}

#[test]
fn name_is_static_across_instances() {
    let first = CtxSet::create(&parse_config(
        r#"
vars:
  - name: first
    value: one
"#,
    ));
    let second = CtxSet::create(&parse_config(
        r#"
vars:
  - name: second
    value: two
"#,
    ));

    assert_eq!(first.name(), "CtxSet");
    assert_eq!(second.name(), "CtxSet");
}

#[test]
fn source_value_wins_and_missing_source_uses_default() {
    let plugin = CtxSet {
        config: parse_config(
            r#"
vars:
  - name: result
    from:
      type: header
      name: X-Source
    default: fallback
"#,
        ),
    };
    let rule = &plugin.config.vars[0];

    let mut source_session = MockPluginSession::new();
    set_source_header(&mut source_session, Some("source"));
    let mut source_log = PluginLog::new("CtxSet");
    assert_eq!(
        plugin.process_rule(rule, 0, &source_session, &mut source_log),
        Some("source".to_string())
    );

    let mut missing_session = MockPluginSession::new();
    set_source_header(&mut missing_session, None);
    let mut missing_log = PluginLog::new("CtxSet");
    assert_eq!(
        plugin.process_rule(rule, 0, &missing_session, &mut missing_log),
        Some("fallback".to_string())
    );
}

#[test]
fn malformed_multi_operation_transform_keeps_priority() {
    let plugin = CtxSet {
        config: CtxSetConfig::default(),
    };
    let mut plugin_log = PluginLog::new("CtxSet");

    let replace_first = parse_transform(
        r#"
replace:
  pattern: a
  with: x
substring: [1, 2]
case: upper
prefix: pre-
suffix: -post
trim: true
"#,
    );
    let replace_regex = Regex::new("a").unwrap();
    assert_eq!(
        plugin.apply_transform("abc", &replace_first, Some(&replace_regex), &mut plugin_log),
        Some("xbc".to_string())
    );

    let substring_first = parse_transform(
        r#"
substring: [1, 3]
case: upper
prefix: pre-
suffix: -post
trim: true
"#,
    );
    assert_eq!(
        plugin.apply_transform("abcd", &substring_first, None, &mut plugin_log),
        Some("bc".to_string())
    );

    let case_first = parse_transform(
        r#"
case: upper
prefix: pre-
suffix: -post
trim: true
"#,
    );
    assert_eq!(
        plugin.apply_transform("ab", &case_first, None, &mut plugin_log),
        Some("AB".to_string())
    );

    let prefix_first = parse_transform(
        r#"
prefix: pre-
suffix: -post
trim: true
"#,
    );
    assert_eq!(
        plugin.apply_transform("ab", &prefix_first, None, &mut plugin_log),
        Some("pre-ab".to_string())
    );

    let suffix_first = parse_transform(
        r#"
suffix: -post
trim: true
"#,
    );
    assert_eq!(
        plugin.apply_transform("ab", &suffix_first, None, &mut plugin_log),
        Some("ab-post".to_string())
    );
}

#[test]
fn substring_uses_unicode_scalar_indices_and_clamps_ranges() {
    let plugin = CtxSet {
        config: CtxSetConfig::default(),
    };
    let mut plugin_log = PluginLog::new("CtxSet");

    for (range, expected) in [
        ((1, 3), "é€"),
        ((2, 99), "€z"),
        ((99, 100), ""),
        ((2, 2), ""),
        ((3, 1), ""),
    ] {
        let transform = parse_transform(&format!("substring: [{}, {}]", range.0, range.1));
        assert_eq!(
            plugin.apply_transform("aé€z", &transform, None, &mut plugin_log),
            Some(expected.to_string())
        );
    }
}

#[test]
fn missing_compiled_replace_logs_error_and_uses_rule_default() {
    let plugin = CtxSet {
        config: parse_config(
            r#"
vars:
  - name: result
    from:
      type: header
      name: X-Source
    transform:
      replace:
        pattern: source
        with: transformed
    default: fallback
"#,
        ),
    };
    let rule = &plugin.config.vars[0];
    let mut session = MockPluginSession::new();
    set_source_header(&mut session, Some("source"));
    let mut plugin_log = PluginLog::new("CtxSet");

    assert_eq!(
        plugin.process_rule(rule, 0, &session, &mut plugin_log),
        Some("fallback".to_string())
    );
    assert!(plugin_log.contains("err: transform regex missing"));
}

#[tokio::test]
async fn test_template_injection_prevention() {
    let yaml = r#"
vars:
  - name: injected
    template: "${header:X-Inject}"
"#;
    let mut config: CtxSetConfig = serde_yaml::from_str(yaml).unwrap();
    config.validate().expect("validate should succeed");
    let plugin = CtxSet::create(&config);

    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("CtxSet");

    // Attacker sends header value containing template syntax
    mock_session
        .expect_request_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::HeaderName::from_bytes(b"X-Inject").unwrap(),
            http::HeaderValue::from_static("${header:Authorization}"),
        )]));
    // The value should be the literal string, NOT the resolved Authorization header
    mock_session
        .expect_set_ctx_var()
        .withf(|key, value| key == "injected" && value == "${header:Authorization}")
        .returning(|_, _| Ok(()));

    let result = plugin.run_request(&mut mock_session, &mut plugin_log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

#[test]
fn compile_plugins_rejects_multiple_transform_operations() {
    use crate::plugins::http::compile_plugins;
    use edgion_resources::resources::edgion_plugins::{EdgionPlugin, EdgionPlugins, RequestFilterEntry};

    let config = parse_config(
        r#"
vars:
  - name: user
    value: ALICE
    transform:
      case: lower
      prefix: user_
"#,
    );
    let mut resource = EdgionPlugins::new("ctx-set-multi-transform", Default::default());
    resource.metadata.namespace = Some("default".to_string());
    resource.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::CtxSet(config))]);

    let schema_errors = resource.validate_plugins();
    let compiled = compile_plugins(&resource);
    assert_eq!(compiled.preparse_errors, schema_errors);
    assert_eq!(
        compiled.preparse_errors,
        vec!["requestPlugins[0] (CtxSet): vars[0] 'user': transform allows at most one operation".to_string()]
    );
    assert!(!compiled.is_activatable);
}

#[test]
fn compile_plugins_accepts_chained_single_transform_rules() {
    use crate::plugins::http::compile_plugins;
    use edgion_resources::resources::edgion_plugins::{EdgionPlugin, EdgionPlugins, RequestFilterEntry};

    let config = parse_config(
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
      prefix: user_
"#,
    );
    let mut resource = EdgionPlugins::new("ctx-set-chained-transform", Default::default());
    resource.metadata.namespace = Some("default".to_string());
    resource.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::CtxSet(config))]);

    let compiled = compile_plugins(&resource);
    assert!(compiled.preparse_errors.is_empty());
    assert!(resource.validate_plugins().is_empty());
    assert!(compiled.is_activatable);
}

fn set_source_header(session: &mut MockPluginSession, value: Option<&str>) {
    let mut headers = http::HeaderMap::new();
    if let Some(value) = value {
        headers.insert("x-source", http::HeaderValue::from_str(value).unwrap());
    }
    session.expect_request_header_map().return_const(headers);
    session.expect_key_get().times(0);
}

#[test]
fn borrowed_inputs_preserve_extract_transform_and_mapping_defaults() {
    for static_source in [false, true] {
        for (input, expected) in [
            ("id=alice", "selected"),
            ("id=bob", "mapping-default"),
            ("invalid", "rule-default"),
        ] {
            let mut rule = serde_json::json!({
                "name": "result", "extract": {"regex": "^id=(.+)$", "group": 1},
                "transform": {"case": "upper"},
                "mapping": {"values": {"ALICE": "selected"}, "default": "mapping-default"},
                "default": "rule-default"
            });
            if static_source {
                rule["value"] = input.into();
            } else {
                rule["from"] = serde_json::json!({"type": "header", "name": "X-Source"});
            }
            let mut config: CtxSetConfig = serde_json::from_value(serde_json::json!({"vars": [rule]})).unwrap();
            config.validate().unwrap();
            let plugin = CtxSet { config };
            let mut session = MockPluginSession::new();
            set_source_header(&mut session, Some(input));
            let mut log = PluginLog::new("CtxSet");
            assert_eq!(
                plugin
                    .process_rule(&plugin.config.vars[0], 0, &session, &mut log)
                    .as_deref(),
                Some(expected)
            );
            // Without mapping default, a mapping miss returns the rule default.
            if input == "id=bob" {
                let mut rule = plugin.config.vars[0].clone();
                rule.mapping.as_mut().unwrap().default = None;
                assert_eq!(
                    plugin.process_rule(&rule, 0, &session, &mut log).as_deref(),
                    Some("rule-default")
                );
            }
        }
    }
}

#[test]
fn borrowed_plain_source_preserves_empty_and_whitespace() {
    let mut config =
        parse_config("vars:\n  - name: result\n    from: {type: header, name: X-Source}\n    default: fallback\n");
    config.validate().unwrap();
    let plugin = CtxSet { config };
    for input in ["", "  source  "] {
        let mut session = MockPluginSession::new();
        set_source_header(&mut session, Some(input));
        assert_eq!(
            plugin
                .process_rule(&plugin.config.vars[0], 0, &session, &mut PluginLog::new("CtxSet"))
                .as_deref(),
            Some(input)
        );
    }
}

#[test]
fn borrowed_source_mapping_allocates_only_final_result() {
    let mut config = parse_config("vars:\n  - name: result\n    from: {type: header, name: X-Source}\n    mapping:\n      values: {source: mapped}\n");
    config.validate().unwrap();
    let plugin = CtxSet { config };
    let mut session = MockPluginSession::new();
    set_source_header(&mut session, Some("source"));
    let _ = session.request_header_map();
    let mut log = PluginLog::new("CtxSet");
    let (value, calls) = crate::test_allocation_counter::count_allocations(|| {
        plugin.process_rule(&plugin.config.vars[0], 0, &session, &mut log)
    });
    assert_eq!(value.as_deref(), Some("mapped"));
    assert_eq!(calls, 1);
}

#[tokio::test]
async fn borrowed_context_read_follows_previous_rule_write() {
    let plugin = CtxSet::create(&parse_config("vars:\n  - name: user\n    value: ALICE\n    transform: {case: lower}\n  - name: user\n    from: {type: ctx, name: user}\n    transform: {prefix: user_}\n"));
    let mut session = MockPluginSession::new();
    session.expect_key_get().times(0);
    let mut sequence = mockall::Sequence::new();
    session
        .expect_set_ctx_var()
        .withf(|key, value| key == "user" && value == "alice")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    session
        .expect_get_ctx_var_ref()
        .withf(|key| key == "user")
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(Some("alice"));
    session
        .expect_set_ctx_var()
        .withf(|key, value| key == "user" && value == "user_alice")
        .times(1)
        .in_sequence(&mut sequence)
        .returning(|_, _| Ok(()));
    assert_eq!(
        plugin.run_request(&mut session, &mut PluginLog::new("CtxSet")).await,
        PluginRunningResult::GoodNext
    );
}
