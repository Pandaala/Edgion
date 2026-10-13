use super::*;

fn spec_with_explicit_header(name: String, custom: bool) -> OutboundRequestSpec {
    let mut headers = ListFields::default();
    if custom {
        headers.custom.push(CustomField {
            name,
            source: CustomSource::Value("value".to_string()),
        });
    } else {
        headers.origin.push(OriginField {
            name,
            presence: PresencePolicy::SkipIfMissing,
        });
    }
    OutboundRequestSpec {
        headers,
        ..Default::default()
    }
}

#[test]
fn explicit_header_names_reject_invalid_tokens_without_echoing_values() {
    let invalid_names = [
        "x bad".to_string(),
        String::new(),
        "x\rbad".to_string(),
        "x-é".to_string(),
        "x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize + 1),
    ];

    for custom in [false, true] {
        let category = if custom { "custom" } else { "origin" };
        for name in &invalid_names {
            let err = spec_with_explicit_header(name.clone(), custom)
                .validation_error()
                .expect("invalid header name must be rejected");
            assert_eq!(
                err,
                format!("request.headers.{category}[0].name must be a valid HTTP header name")
            );
            if !name.is_empty() {
                assert!(!err.contains(name), "validation error must not echo the header name");
            }
        }
    }
}

#[test]
fn explicit_header_names_accept_token_punctuation_and_exact_maximum() {
    let valid_names = [
        "X!#$%&'*+-.^_`|~09AZaz".to_string(),
        "x".repeat(crate::gwapi_types::HEADER_NAME_MAX_LEN as usize),
    ];

    for custom in [false, true] {
        for name in &valid_names {
            assert!(
                spec_with_explicit_header(name.clone(), custom)
                    .validation_error()
                    .is_none(),
                "valid header name was rejected"
            );
        }
    }
}

#[test]
fn list_fields_forward_all_defaults_false() {
    let lf: ListFields = serde_yaml::from_str("origin: []\ncustom: []").unwrap();
    assert!(!lf.forward_all);
}

#[test]
fn list_fields_parses_forward_all_true() {
    let lf: ListFields = serde_yaml::from_str("forwardAll: true").unwrap();
    assert!(lf.forward_all);
    assert!(lf.origin.is_empty());
}

/// C1: args.forwardAll must be rejected at the schema-validation layer.
#[test]
fn args_forward_all_is_rejected() {
    let spec = OutboundRequestSpec {
        args: ListFields {
            forward_all: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let err = spec
        .validation_error()
        .expect("args.forwardAll must produce a validation error");
    assert!(
        err.contains("forwardAll"),
        "Error message should mention forwardAll; got: {err}"
    );
}

/// C1: cookies.forwardAll must be rejected at the schema-validation layer.
#[test]
fn cookies_forward_all_is_rejected() {
    let spec = OutboundRequestSpec {
        cookies: ListFields {
            forward_all: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let err = spec
        .validation_error()
        .expect("cookies.forwardAll must produce a validation error");
    assert!(
        err.contains("forwardAll"),
        "Error message should mention forwardAll; got: {err}"
    );
}

/// Sanity: headers.forwardAll passes validation (it is the supported case).
#[test]
fn headers_forward_all_passes_validation() {
    let spec = OutboundRequestSpec {
        headers: ListFields {
            forward_all: true,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(
        spec.validation_error().is_none(),
        "headers.forwardAll should pass validation"
    );
}

#[test]
fn body_spec_parses_json_type() {
    let spec: OutboundRequestSpec =
        serde_yaml::from_str("body:\n  type: json\n  template: '{\"userId\": \"${ctx:userId}\"}'").unwrap();
    match spec.body {
        Some(BodySpec::Json { ref template }) => {
            assert!(template.contains("${ctx:userId}"));
        }
        other => panic!("expected Json body, got {other:?}"),
    }
}

#[test]
fn body_spec_parses_form_fields() {
    let yaml = r#"
body:
  type: form
  fields:
    - name: userId
      template: "${ctx:userId}"
    - name: source
      value: edgion
"#;
    let spec: OutboundRequestSpec = serde_yaml::from_str(yaml).unwrap();
    match spec.body {
        Some(BodySpec::Form { ref fields }) => {
            assert_eq!(fields.len(), 2);
            assert!(matches!(fields[1].source, CustomSource::Value(_)));
        }
        other => panic!("expected Form body, got {other:?}"),
    }
}

#[test]
fn body_spec_parses_raw_type() {
    let spec: OutboundRequestSpec = serde_yaml::from_str("body:\n  type: raw\n  template: 'plain ${ctx:x}'").unwrap();
    assert!(matches!(spec.body, Some(BodySpec::Raw { .. })));
}

#[test]
fn body_spec_rejects_unknown_type() {
    let r: Result<OutboundRequestSpec, _> = serde_yaml::from_str("body:\n  type: xml\n  template: '<a/>'");
    assert!(r.is_err(), "unknown body type must be rejected");
}

#[test]
fn body_spec_rejects_unknown_field() {
    let r: Result<OutboundRequestSpec, _> =
        serde_yaml::from_str("body:\n  type: json\n  template: '{}'\n  extra: nope");
    assert!(r.is_err(), "unknown field on body must be rejected");
}

#[test]
fn body_defaults_to_none() {
    let spec: OutboundRequestSpec = serde_yaml::from_str("args: {}").unwrap();
    assert!(spec.body.is_none());
}

fn body_json(template: &str) -> Option<BodySpec> {
    Some(BodySpec::Json {
        template: template.to_string(),
    })
}

#[test]
fn json_body_with_explicit_content_type_custom_rejected() {
    let spec = OutboundRequestSpec {
        body: body_json(r#"{"a": "${ctx:x}"}"#),
        headers: ListFields {
            custom: vec![CustomField {
                name: "content-type".to_string(),
                source: CustomSource::Value("text/plain".to_string()),
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    let err = spec.validation_error().expect("must reject CT conflict");
    assert!(err.contains("Content-Type"), "got: {err}");
}

#[test]
fn form_body_with_explicit_content_type_origin_rejected() {
    let spec = OutboundRequestSpec {
        body: Some(BodySpec::Form { fields: vec![] }),
        headers: ListFields {
            origin: vec![OriginField {
                name: "Content-Type".to_string(),
                presence: PresencePolicy::SkipIfMissing,
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(spec.validation_error().is_some(), "origin CT must also conflict");
}

#[test]
fn raw_body_with_explicit_content_type_allowed() {
    let spec = OutboundRequestSpec {
        body: Some(BodySpec::Raw {
            template: "hello".to_string(),
        }),
        headers: ListFields {
            custom: vec![CustomField {
                name: "Content-Type".to_string(),
                source: CustomSource::Value("text/csv".to_string()),
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(
        spec.validation_error().is_none(),
        "raw + explicit CT is the supported combo"
    );
}

#[test]
fn explicit_content_length_header_is_rejected() {
    let spec = OutboundRequestSpec {
        headers: ListFields {
            custom: vec![CustomField {
                name: "Content-Length".to_string(),
                source: CustomSource::Value("9".to_string()),
            }],
            ..Default::default()
        },
        ..Default::default()
    };

    let err = spec.validation_error().expect("Content-Length must be proxy-managed");
    assert!(err.contains("Content-Length"), "got: {err}");
}

#[test]
fn explicit_transfer_encoding_origin_is_rejected() {
    let spec = OutboundRequestSpec {
        headers: ListFields {
            origin: vec![OriginField {
                name: "transfer-encoding".to_string(),
                presence: PresencePolicy::SkipIfMissing,
            }],
            ..Default::default()
        },
        ..Default::default()
    };

    let err = spec
        .validation_error()
        .expect("Transfer-Encoding must be proxy-managed");
    assert!(err.contains("transfer-encoding"), "got: {err}");
}

#[test]
fn json_body_canary_lint_accepts_quoted_placeholders() {
    let spec = OutboundRequestSpec {
        body: body_json(r#"{"userId": "${ctx:userId}", "caller": "edgion-gateway"}"#),
        ..Default::default()
    };
    assert!(spec.validation_error().is_none());
}

#[test]
fn json_body_canary_lint_rejects_unquoted_placeholder() {
    let spec = OutboundRequestSpec {
        body: body_json(r#"{"count": ${ctx:n}}"#),
        ..Default::default()
    };
    let err = spec.validation_error().expect("unquoted placeholder must be rejected");
    assert!(err.contains("valid JSON"), "got: {err}");
}

#[test]
fn json_body_canary_lint_rejects_broken_skeleton() {
    let spec = OutboundRequestSpec {
        body: body_json(r#"{"a": "${ctx:x}""#), // missing closing brace
        ..Default::default()
    };
    assert!(spec.validation_error().is_some());
}

#[test]
fn json_body_compile_error_rejected() {
    let spec = OutboundRequestSpec {
        body: body_json(r#"{"a": "${ctx:x"}"#), // unclosed ${
        ..Default::default()
    };
    assert!(spec.validation_error().is_some());
}

#[test]
fn form_body_field_template_compile_error_rejected() {
    let spec = OutboundRequestSpec {
        body: Some(BodySpec::Form {
            fields: vec![CustomField {
                name: "u".to_string(),
                source: CustomSource::Template("${ctx:x".to_string()),
            }],
        }),
        ..Default::default()
    };
    assert!(spec.validation_error().is_some());
}

#[test]
fn custom_field_requires_exactly_one_visible_string_source() {
    for input in [
        r#"{"name":"X-Test"}"#,
        r#"{"name":"X-Test","value":"literal","template":"${ctx:x}"}"#,
        r#"{"name":"X-Test","template":"${ctx:x}","value":"literal"}"#,
        r#"{"name":"X-Test","value":"literal","template":"${ctx:broken"}"#,
        r#"{"name":"X-Test","value":"a","value":"b"}"#,
        r#"{"name":"X-Test","template":"a","template":"b"}"#,
        r#"{"name":"X-Test","name":"X-Other","value":"a"}"#,
        r#"{"value":"a"}"#,
    ] {
        assert!(serde_json::from_str::<CustomField>(input).is_err(), "{input}");
        assert!(serde_yaml::from_str::<CustomField>(input).is_err(), "{input}");
    }
    for source in ["value", "template"] {
        for operand in ["null", "true", "42", "1.5", "[]", "{}"] {
            let input = format!(r#"{{"name":"X-Test","{source}":{operand}}}"#);
            assert!(serde_json::from_str::<CustomField>(&input).is_err(), "{input}");
            let yaml = format!("name: X-Test\n{source}: {operand}\n");
            assert!(serde_yaml::from_str::<CustomField>(&yaml).is_err(), "{yaml}");
        }
        let other = if source == "value" { "template" } else { "value" };
        let input = format!(r#"{{"name":"X-Test","{source}":null,"{other}":"valid"}}"#);
        assert!(serde_json::from_str::<CustomField>(&input).is_err(), "{input}");
        assert!(serde_yaml::from_str::<CustomField>(&input).is_err(), "{input}");
        let yaml = format!("name: X-Test\n{source}: a\n{source}: b\n");
        assert!(serde_yaml::from_str::<CustomField>(&yaml).is_err(), "{yaml}");
    }
}

#[test]
fn custom_field_valid_sources_preserve_wire_shape_and_yaml_string_types() {
    for source in ["value", "template"] {
        for value in ["", "literal", "${secretRef:ns/token:key}"] {
            let wire = serde_json::json!({"name": "X-Test", source: value});
            let field: CustomField = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(&field).unwrap(), wire);
            let yaml = serde_yaml::to_string(&field).unwrap();
            let decoded: CustomField = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
        }
        for (yaml_value, expected) in [
            ("'42'", "42"),
            ("!!str 42", "42"),
            ("'true'", "true"),
            ("!!str null", "null"),
        ] {
            let yaml = format!("name: X-Test\n{source}: {yaml_value}\n");
            let field: CustomField = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(serde_json::to_value(field).unwrap()[source], expected);
        }
    }
    // Required-name decoding keeps its pre-existing YAML String policy.
    let field: CustomField = serde_yaml::from_str("name: 42\nvalue: 'literal'\n").unwrap();
    assert_eq!(field.name, "42");
}

#[test]
fn custom_source_arity_is_enforced_in_all_outbound_placements() {
    for placement in ["args", "headers", "cookies", "body"] {
        for fields in [
            r#""value":"literal","template":"${ctx:x}""#,
            r#""template":"${ctx:x}","value":"literal""#,
        ] {
            let field = format!(r#"{{"name":"X-Test",{fields}}}"#);
            let request = if placement == "body" {
                format!(r#"{{"body":{{"type":"form","fields":[{field}]}}}}"#)
            } else {
                format!(r#"{{"{placement}":{{"custom":[{field}]}}}}"#)
            };
            assert!(serde_json::from_str::<OutboundRequestSpec>(&request).is_err());
            assert!(serde_yaml::from_str::<OutboundRequestSpec>(&request).is_err());
            let request: serde_json::Value = serde_json::from_str(&request).unwrap();
            let resource = serde_json::json!({
                "type": "webhook", "config": {
                    "target": {"name": "backend", "port": 80}, "request": request
                }
            });
            assert!(serde_json::from_value::<crate::resources::link_sys::LinkSysSpec>(resource).is_err());
        }
    }
}

#[test]
fn custom_sources_preserve_advisory_unknown_fields_and_secret_collection() {
    let schema = schemars::schema_for!(OutboundRequestSpec);
    for typo in [false, true] {
        let mut field = serde_json::json!({"name": "X-Test", "template": "${secretRef:ns/token:key}"});
        if typo {
            field["templat"] = serde_json::json!("ignored");
        }
        let wire = serde_json::json!({"headers": {"custom": [field]}});
        let findings = crate::schema_check::unknown_key_paths(&wire, &schema);
        assert_eq!(
            findings,
            if typo {
                vec!["headers.custom[0].templat"]
            } else {
                vec![]
            }
        );
        let request: OutboundRequestSpec = serde_json::from_value(wire).unwrap();
        assert!(request.validation_error().is_none());
        assert_eq!(request.collect_secret_ref_names(), vec!["ns/token:key"]);
    }
}

#[test]
fn custom_fields_reject_sequence_wire_shapes() {
    for input in [r#"["X-Test","literal"]"#, r#"["X-Test","literal",null]"#] {
        assert!(serde_json::from_str::<CustomField>(input).is_err(), "{input}");
        assert!(serde_yaml::from_str::<CustomField>(input).is_err(), "{input}");
        let request = format!(r#"{{"headers":{{"custom":[{input}]}}}}"#);
        assert!(serde_json::from_str::<OutboundRequestSpec>(&request).is_err());
        assert!(serde_yaml::from_str::<OutboundRequestSpec>(&request).is_err());
        let request: serde_json::Value = serde_json::from_str(&request).unwrap();
        assert!(serde_json::from_value::<OutboundRequestSpec>(request.clone()).is_err());
        let resource = serde_json::json!({
            "type": "webhook", "config": {
                "target": {"name": "backend", "port": 80}, "request": request
            }
        });
        assert!(serde_json::from_value::<crate::resources::link_sys::LinkSysSpec>(resource).is_err());
    }
}
