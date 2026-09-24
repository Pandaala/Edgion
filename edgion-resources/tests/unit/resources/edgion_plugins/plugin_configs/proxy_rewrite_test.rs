use super::*;

#[test]
fn test_proxy_rewrite_config_default() {
    let config = ProxyRewriteConfig::default();
    assert!(config.uri.is_none());
    assert!(config.regex_uri.is_none());
    assert!(config.host.is_none());
    assert!(config.method.is_none());
    assert!(config.headers.is_none());
    assert!(config.check_fields().is_none());
    assert!(!config.has_any_config());
}

#[test]
fn test_proxy_rewrite_config_precompile_valid() {
    let mut config = ProxyRewriteConfig {
        regex_uri: Some(RegexUri {
            pattern: r"^/api/v1/users/(\d+)".to_string(),
            replacement: "/users/$1".to_string(),
        }),
        ..Default::default()
    };

    assert!(config.precompile());
    assert!(config.compiled_regex.is_some());
}

#[test]
fn test_proxy_rewrite_config_precompile_invalid() {
    let mut config = ProxyRewriteConfig {
        regex_uri: Some(RegexUri {
            pattern: r"[invalid".to_string(), // Invalid regex
            replacement: "/users/$1".to_string(),
        }),
        ..Default::default()
    };

    assert!(!config.precompile());
    assert!(config.check_fields().is_some());
}

#[test]
fn test_host_conflict_detection() {
    let mut config = ProxyRewriteConfig {
        host: Some("backend.svc".to_string()),
        headers: Some(HeaderActions {
            add: None,
            set: Some(vec![HeaderEntry {
                name: "Host".to_string(),
                value: "other.svc".to_string(),
            }]),
            remove: None,
        }),
        ..Default::default()
    };

    assert!(!config.precompile());
    assert!(config.check_fields().unwrap().contains("Conflict"));
}

#[test]
fn test_http_method_as_str() {
    assert_eq!(HttpMethod::Get.as_str(), "GET");
    assert_eq!(HttpMethod::Post.as_str(), "POST");
    assert_eq!(HttpMethod::Put.as_str(), "PUT");
    assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
    assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
}

#[test]
fn test_proxy_rewrite_config_serde() {
    let json = r#"{
            "uri": "/new/path",
            "host": "backend.svc",
            "method": "POST",
            "headers": {
                "set": [
                    {"name": "X-Api-Version", "value": "v2"}
                ],
                "remove": ["X-Debug"]
            }
        }"#;

    let config: ProxyRewriteConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.uri, Some("/new/path".to_string()));
    assert_eq!(config.host, Some("backend.svc".to_string()));
    assert!(matches!(config.method, Some(HttpMethod::Post)));

    let headers = config.headers.as_ref().unwrap();
    let set_headers = headers.set.as_ref().unwrap();
    assert_eq!(set_headers.len(), 1);
    assert_eq!(set_headers[0].name, "X-Api-Version");
    assert_eq!(set_headers[0].value, "v2");
    assert!(headers.remove.as_ref().unwrap().contains(&"X-Debug".to_string()));
}

#[test]
fn test_regex_uri_serde() {
    let json = r#"{
            "regexUri": {
                "pattern": "^/api/v1/(.*)",
                "replacement": "/internal/$1"
            }
        }"#;

    let config: ProxyRewriteConfig = serde_json::from_str(json).unwrap();
    let regex_uri = config.regex_uri.as_ref().unwrap();
    assert_eq!(regex_uri.pattern, "^/api/v1/(.*)");
    assert_eq!(regex_uri.replacement, "/internal/$1");
}

#[test]
fn test_header_order_preserved() {
    let json = r#"{
            "headers": {
                "set": [
                    {"name": "X-First", "value": "1"},
                    {"name": "X-Second", "value": "2"},
                    {"name": "X-Third", "value": "3"}
                ]
            }
        }"#;

    let config: ProxyRewriteConfig = serde_json::from_str(json).unwrap();
    let set_headers = config.headers.as_ref().unwrap().set.as_ref().unwrap();
    assert_eq!(set_headers[0].name, "X-First");
    assert_eq!(set_headers[1].name, "X-Second");
    assert_eq!(set_headers[2].name, "X-Third");
}

#[test]
fn json_patch_all_operations_deserialize_in_order() {
    let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {
            "operations": [
                {"op": "add", "path": "/items/-", "value": {"enabled": true}},
                {"op": "remove", "path": "/secret"},
                {"op": "replace", "path": "/name", "valueTemplate": "${header:X-Name}"},
                {"op": "move", "from": "/old", "path": "/new"},
                {"op": "copy", "from": "/source", "path": "/copy"},
                {"op": "test", "path": "/version", "value": 1}
            ]
        }
    }))
    .unwrap();

    let operations = &config.json_body.as_ref().unwrap().operations;
    assert_eq!(operations.len(), 6);
    assert!(matches!(operations[0], JsonPatchOperation::Add { .. }));
    assert!(matches!(operations[1], JsonPatchOperation::Remove { .. }));
    assert!(matches!(operations[2], JsonPatchOperation::Replace { .. }));
    assert!(matches!(operations[3], JsonPatchOperation::Move { .. }));
    assert!(matches!(operations[4], JsonPatchOperation::Copy { .. }));
    assert!(matches!(operations[5], JsonPatchOperation::Test { .. }));
    assert!(config.check_fields().is_none());
}

#[test]
fn explicit_null_literal_is_present_and_round_trips() {
    let config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {
            "operations": [
                {"op": "add", "path": "/nullable", "value": null}
            ]
        }
    }))
    .unwrap();
    let operation = &config.json_body.as_ref().unwrap().operations[0];
    match operation {
        JsonPatchOperation::Add {
            value, value_template, ..
        } => {
            assert_eq!(value.as_ref(), Some(&serde_json::Value::Null));
            assert!(value_template.is_none());
        }
        _ => panic!("expected add operation"),
    }

    let encoded = serde_json::to_value(&config).unwrap();
    assert_eq!(
        encoded.pointer("/jsonBody/operations/0/value"),
        Some(&serde_json::Value::Null)
    );
}

#[test]
fn literal_and_template_must_be_exactly_one() {
    let both: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {
            "operations": [
                {
                    "op": "replace",
                    "path": "/name",
                    "value": "literal",
                    "valueTemplate": "${ctx:name}"
                }
            ]
        }
    }))
    .unwrap();
    assert!(both.check_fields().unwrap().contains("mutually exclusive"));

    let neither: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {
            "operations": [
                {"op": "test", "path": "/name"}
            ]
        }
    }))
    .unwrap();
    assert!(neither.check_fields().unwrap().contains("configure exactly one"));
}

#[test]
fn templates_compile_once_and_secret_refs_are_rejected() {
    let mut config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {
            "operations": [
                {"op": "add", "path": "/tenant", "valueTemplate": "${header:X-Tenant}"}
            ]
        }
    }))
    .unwrap();
    assert!(config.precompile());
    assert_eq!(config.compiled_json_body.as_ref().unwrap().operations.len(), 1);

    let secret: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {
            "operations": [
                {"op": "add", "path": "/token", "valueTemplate": "${secretRef:ns/name:key}"}
            ]
        }
    }))
    .unwrap();
    assert!(secret.check_fields().unwrap().contains("must not reference"));

    for template in ["${header}", "${query:}", "${path:ignored}", "${method:ignored}"] {
        let invalid: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
            "jsonBody": {
                "operations": [
                    {"op": "add", "path": "/value", "valueTemplate": template}
                ]
            }
        }))
        .unwrap();
        assert!(invalid.check_fields().is_some(), "{template}");
    }
}

#[test]
fn json_pointer_syntax_size_and_token_limits_are_validated() {
    let invalid: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {
            "operations": [
                {"op": "remove", "path": "/bad~2escape"}
            ]
        }
    }))
    .unwrap();
    assert!(invalid.check_fields().unwrap().contains("valid JSON Pointer"));

    let too_long_path = format!("/{}", "x".repeat(MAX_JSON_POINTER_BYTES));
    let too_long = ProxyRewriteConfig {
        json_body: Some(JsonBodyRewrite {
            operations: vec![JsonPatchOperation::Remove { path: too_long_path }],
        }),
        ..Default::default()
    };
    assert!(too_long.check_fields().unwrap().contains("encoded bytes"));

    let too_many_tokens = format!("/{}", vec!["x"; MAX_JSON_POINTER_TOKENS + 1].join("/"));
    let too_deep = ProxyRewriteConfig {
        json_body: Some(JsonBodyRewrite {
            operations: vec![JsonPatchOperation::Remove { path: too_many_tokens }],
        }),
        ..Default::default()
    };
    assert!(too_deep.check_fields().unwrap().contains("tokens"));

    let root: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "jsonBody": {
            "operations": [
                {"op": "replace", "path": "", "value": {"ok": true}}
            ]
        }
    }))
    .unwrap();
    assert!(root.check_fields().is_none());
}

#[test]
fn operation_count_and_static_literal_complexity_are_bounded() {
    let too_many = ProxyRewriteConfig {
        json_body: Some(JsonBodyRewrite {
            operations: (0..=MAX_JSON_PATCH_OPERATIONS)
                .map(|index| JsonPatchOperation::Remove {
                    path: format!("/{index}"),
                })
                .collect(),
        }),
        ..Default::default()
    };
    assert!(too_many.check_fields().unwrap().contains("at most"));

    let mut deeply_nested = serde_json::Value::Null;
    for _ in 0..65 {
        deeply_nested = serde_json::json!([deeply_nested]);
    }
    let too_deep = ProxyRewriteConfig {
        json_body: Some(JsonBodyRewrite {
            operations: vec![JsonPatchOperation::Add {
                path: "/value".to_string(),
                value: JsonLiteralValue::new(deeply_nested),
                value_template: None,
            }],
        }),
        ..Default::default()
    };
    assert!(too_deep.check_fields().unwrap().contains("nesting-depth"));

    let oversized_plan = ProxyRewriteConfig {
        json_body: Some(JsonBodyRewrite {
            operations: vec![JsonPatchOperation::Add {
                path: "/value".to_string(),
                value: JsonLiteralValue::new(serde_json::Value::String("x".repeat(MAX_JSON_PATCH_PLAN_BYTES))),
                value_template: None,
            }],
        }),
        ..Default::default()
    };
    assert!(oversized_plan.check_fields().unwrap().contains("serialized bytes"));
}

#[test]
fn compiled_runtime_fields_are_not_serialized() {
    let mut config: ProxyRewriteConfig = serde_json::from_value(serde_json::json!({
        "regexUri": {"pattern": "^/old$", "replacement": "/new"},
        "jsonBody": {
            "operations": [
                {"op": "copy", "from": "/source", "path": "/copy"}
            ]
        }
    }))
    .unwrap();
    assert!(config.precompile());
    let encoded = serde_json::to_value(config).unwrap();
    assert!(encoded.get("compiledRegex").is_none());
    assert!(encoded.get("compiledJsonBody").is_none());
}

#[test]
fn unknown_operation_fields_are_rejected() {
    let result = serde_json::from_value::<ProxyRewriteConfig>(serde_json::json!({
        "jsonBody": {
            "operations": [
                {"op": "remove", "path": "/x", "value": 1}
            ]
        }
    }));
    assert!(result.is_err());
}
