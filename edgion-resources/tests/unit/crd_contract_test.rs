use super::*;
use serde_json::json;

#[test]
fn selected_contract_compiles_and_uses_crd_defaults() {
    let contract = http_route_transform_contract().unwrap();
    assert_eq!(contract.defaults().redirect_status_code, 302);
    assert_eq!(contract.defaults().path_type, "PathPrefix");
    assert_eq!(contract.defaults().path_value, "/");
    assert_eq!(
        contract.defaults().matches,
        json!([{"path": {"type": "PathPrefix", "value": "/"}}])
    );
}

#[test]
fn config_openapi_and_nested_cel_are_enforced() {
    let contract = http_route_transform_contract().unwrap();
    for invalid in [
        json!({"scheme": "ftp"}),
        json!({"hostname": ""}),
        json!({"hostname": "Invalid.example"}),
        json!({"port": 0}),
        json!({"port": 65536}),
        json!({"statusCode": 300}),
        json!({"path": {}}),
        json!({"path": {"type": "ReplaceFullPath"}}),
        json!({"path": {"type": "ReplaceFullPath", "replacePrefixMatch": "/v2"}}),
        json!({"path": {"type": "ReplacePrefixMatch", "replaceFullPath": "/v2"}}),
        json!({"path": {"type": "ReplaceFullPath", "replaceFullPath": "x".repeat(1025)}}),
    ] {
        assert!(
            !contract.validate_request_redirect(&invalid).unwrap().is_empty(),
            "invalid RequestRedirect unexpectedly accepted: {invalid}"
        );
    }
    for valid in [
        json!({"port": 1}),
        json!({"port": 65535}),
        json!({"statusCode": 301}),
        json!({"statusCode": 302}),
        json!({"statusCode": 303}),
        json!({"statusCode": 307}),
        json!({"statusCode": 308}),
        json!({"path": {"type": "ReplaceFullPath", "replaceFullPath": "x".repeat(1024)}}),
        json!({"path": {"type": "ReplacePrefixMatch", "replacePrefixMatch": ""}}),
    ] {
        assert!(
            contract.validate_request_redirect(&valid).unwrap().is_empty(),
            "valid RequestRedirect unexpectedly rejected: {valid}"
        );
    }
    assert!(contract.validate_request_redirect(&json!({})).unwrap().is_empty());
}

#[test]
fn filter_list_and_rule_cel_are_enforced() {
    let contract = http_route_transform_contract().unwrap();
    for invalid in [
        json!({"type": "RequestRedirect"}),
        json!({"type": "URLRewrite"}),
        json!({"type": "RequestHeaderModifier", "requestRedirect": {}}),
        json!({"type": "RequestHeaderModifier", "urlRewrite": {}}),
    ] {
        assert!(!contract.validate_filter(&invalid).unwrap().is_empty());
    }
    assert!(!contract
        .validate_filter_list(&json!([{"type": "RequestRedirect"}, {"type": "URLRewrite"}]))
        .unwrap()
        .is_empty());
    assert!(!contract
        .validate_rule(&json!({
            "matches": [{"path": {"type": "Exact", "value": "/x"}}],
            "filters": [{"type": "URLRewrite", "urlRewrite": {"path": {
                "type": "ReplacePrefixMatch", "replacePrefixMatch": "/v2"
            }}}]
        }))
        .unwrap()
        .is_empty());
}

#[test]
fn non_nullable_null_is_pruned_before_openapi_validation() {
    let contract = http_route_transform_contract().unwrap();
    let violations = contract.validate_url_rewrite(&json!({"path": null})).unwrap();
    assert!(violations.is_empty());
}

#[test]
fn non_nullable_null_is_pruned_before_defaulting() {
    let schema = json!({
        "type": "object",
        "properties": {"mode": {"type": "string", "default": "safe"}}
    });
    let mut instance = json!({"mode": null});

    prune_non_nullable_null_properties(&schema, &mut instance);
    apply_defaults(&schema, &mut instance);

    assert_eq!(instance, json!({"mode": "safe"}));
}

#[test]
fn null_pruning_preserves_nullable_unknown_and_root_values() {
    let schema = json!({
        "type": "object",
        "properties": {
            "nullableFlag": {"type": "string", "nullable": true},
            "unionFlag": {"type": ["string", "null"]}
        }
    });
    let mut instance = json!({
        "nullableFlag": null,
        "unionFlag": null,
        "unknown": null
    });

    prune_non_nullable_null_properties(&schema, &mut instance);

    assert_eq!(
        instance,
        json!({"nullableFlag": null, "unionFlag": null, "unknown": null})
    );

    let mut root = Value::Null;
    prune_non_nullable_null_properties(&schema, &mut root);
    assert!(root.is_null());
    assert!(!CompiledFragment::compile(schema, "/test")
        .unwrap()
        .validate(&root)
        .unwrap()
        .is_empty());
}

#[test]
fn null_pruning_recurses_through_object_arrays() {
    let schema = json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"mode": {"type": "string", "default": "safe"}}
                }
            }
        }
    });
    let mut instance = json!({"items": [{"mode": null}]});

    prune_non_nullable_null_properties(&schema, &mut instance);
    apply_defaults(&schema, &mut instance);

    assert_eq!(instance, json!({"items": [{"mode": "safe"}]}));
}

#[test]
fn filter_list_openapi_bounds_are_enforced() {
    let contract = http_route_transform_contract().unwrap();
    assert!(contract
        .validate_filter_list_bounds(&Value::Array(vec![json!({}); 16]))
        .unwrap()
        .is_empty());
    assert!(!contract
        .validate_filter_list_bounds(&Value::Array(vec![json!({}); 17]))
        .unwrap()
        .is_empty());
}
