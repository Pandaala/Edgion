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
