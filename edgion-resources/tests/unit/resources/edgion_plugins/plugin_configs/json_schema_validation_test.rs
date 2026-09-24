use super::*;
use serde_json::json;

fn config(schema: Value) -> JsonSchemaValidationConfig {
    JsonSchemaValidationConfig { schema }
}

#[test]
fn compiles_and_validates_local_pointer_schema() {
    let compiled = config(json!({
        "$schema": DRAFT_2020_12_URI,
        "$defs": {"name": {"type": "string", "minLength": 1}},
        "type": "object",
        "properties": {"name": {"$ref": "#/$defs/name"}},
        "required": ["name"]
    }))
    .compile()
    .unwrap();

    let valid = parse_json_instance(br#"{"name":"Ada"}"#).unwrap();
    let invalid = parse_json_instance(br#"{"name":""}"#).unwrap();
    assert_eq!(compiled.validate_instance(&valid), Ok(true));
    assert_eq!(compiled.validate_instance(&invalid), Ok(false));
}

#[test]
fn rejects_wrong_draft_and_restricted_keywords() {
    assert!(
        config(json!({"$schema": "http://json-schema.org/draft-07/schema#", "type": "string"}))
            .compile()
            .unwrap_err()
            .contains("Draft 2020-12")
    );
    for keyword in [
        "$id",
        "$anchor",
        "$dynamicAnchor",
        "$dynamicRef",
        "$recursiveRef",
        "unevaluatedProperties",
    ] {
        let mut schema = Map::new();
        schema.insert(keyword.to_string(), Value::String("#x".to_string()));
        assert!(config(Value::Object(schema)).compile().unwrap_err().contains(keyword));
    }
}

#[test]
fn rejects_external_unresolved_cyclic_and_bad_pointer_refs() {
    for schema in [
        json!({"$ref": "https://example.com/schema"}),
        json!({"$ref": "#/$defs/missing"}),
        json!({"$ref": "#/$defs/a", "$defs": {"a": {"$ref": "#/$defs/a"}}}),
        json!({"$ref": "#/$defs/~2bad", "$defs": {"~2bad": true}}),
    ] {
        assert!(config(schema).compile().is_err());
    }
}

#[test]
fn rejects_schema_shape_and_reference_depth_limits() {
    let mut deep = Value::Bool(true);
    for _ in 0..MAX_SCHEMA_DEPTH {
        deep = json!({"allOf": [deep]});
    }
    assert!(config(deep).compile().unwrap_err().contains("nesting"));

    let too_many_nodes = json!({"examples": vec![Value::Null; MAX_SCHEMA_NODES]});
    assert!(config(too_many_nodes).compile().unwrap_err().contains("node"));

    let mut definitions = Map::new();
    for index in 0..=MAX_REFERENCE_DEPTH {
        let target = if index == MAX_REFERENCE_DEPTH {
            Value::Bool(true)
        } else {
            json!({"$ref": format!("#/$defs/n{}", index + 1)})
        };
        definitions.insert(format!("n{index}"), target);
    }
    assert!(config(json!({
        "$ref": "#/$defs/n0",
        "$defs": definitions
    }))
    .compile()
    .unwrap_err()
    .contains("reference chain"));
}

#[test]
fn referenced_annotation_data_cannot_bypass_schema_policy() {
    for schema in [
        json!({
            "$ref": "#/default",
            "default": {"format": "regex"}
        }),
        json!({
            "$ref": "#/x-internal",
            "x-internal": {"unevaluatedProperties": false}
        }),
        json!({
            "$ref": "#/examples/0",
            "examples": [{"$id": "https://example.com/hidden"}]
        }),
    ] {
        assert!(config(schema).compile().is_err());
    }
}

#[test]
fn rejects_legacy_applicator_keywords() {
    for (keyword, schema) in [
        (
            "dependencies",
            json!({
                "type": "object",
                "dependencies": {
                    "trigger": {
                        "properties": {
                            "payload": {"type": "string", "format": "regex"}
                        }
                    }
                }
            }),
        ),
        (
            "dependencies",
            json!({
                "type": "object",
                "dependencies": {"trigger": ["payload"]}
            }),
        ),
        (
            "additionalItems",
            json!({
                "type": "array",
                "items": [{"type": "string"}],
                "additionalItems": false
            }),
        ),
    ] {
        let error = config(schema).compile().unwrap_err();
        assert!(error.contains(keyword), "{error}");
    }
}

#[test]
fn percent_encoded_ref_cannot_select_a_different_validator_target() {
    let error = config(json!({
        "$ref": "#/$defs/safe%2Fname",
        "$defs": {
            "safe%2Fname": true,
            "safe": {
                "name": {"format": "regex"}
            }
        }
    }))
    .compile()
    .unwrap_err();
    assert!(error.contains("percent-encoded"), "{error}");
}

#[test]
fn referenced_annotation_patterns_are_bounded_and_counted_once() {
    let patterns: Map<String, Value> = (0..=MAX_PATTERN_COUNT)
        .map(|i| (format!("p{i}"), Value::Bool(true)))
        .collect();
    assert!(config(json!({
        "$ref": "#/default",
        "default": {"patternProperties": patterns}
    }))
    .compile()
    .unwrap_err()
    .contains("pattern"));

    config(json!({
        "allOf": [
            {"$ref": "#/$defs/shared"},
            {"$ref": "#/$defs/shared"}
        ],
        "$defs": {
            "shared": {"pattern": "^shared$"}
        }
    }))
    .compile()
    .unwrap();
}

#[test]
fn schema_walker_ignores_keyword_like_annotation_data() {
    config(json!({
        "type": "object",
        "const": {"$id": "not-a-schema", "unevaluatedProperties": false},
        "default": {"$dynamicRef": "not-a-schema"},
        "examples": [{"$anchor": "not-a-schema"}]
    }))
    .compile()
    .unwrap();
}

#[test]
fn strict_parser_rejects_duplicates_depth_and_trailing_data() {
    assert_eq!(
        parse_json_instance(br#"{"a":1,"a":2}"#).unwrap_err(),
        StrictJsonError::DuplicateKey
    );
    assert_eq!(
        parse_json_instance(br#"{} {}"#).unwrap_err(),
        StrictJsonError::Malformed
    );

    let deep = format!(
        "{}0{}",
        "[".repeat(MAX_JSON_INSTANCE_DEPTH),
        "]".repeat(MAX_JSON_INSTANCE_DEPTH)
    );
    assert_eq!(
        parse_json_instance(deep.as_bytes()).unwrap_err(),
        StrictJsonError::DepthLimit
    );
}

#[test]
fn strict_parser_counts_nodes_and_text() {
    let parsed = parse_json_instance(br#"{"name":"Ada","tags":["x","yz"]}"#).unwrap();
    assert_eq!(parsed.node_count(), 5);
    assert_eq!(parsed.text_bytes(), 14);
}

#[test]
fn strict_parser_rejects_instance_node_overflow() {
    let encoded = serde_json::to_vec(&Value::Array(vec![Value::Null; MAX_JSON_INSTANCE_NODES as usize])).unwrap();
    assert_eq!(parse_json_instance(&encoded).unwrap_err(), StrictJsonError::NodeLimit);
}

#[test]
fn strict_parser_preserves_private_token_object_keys() {
    let parsed = parse_json_instance(br#"{"$serde_json::private::Number":"1","other":true}"#)
        .unwrap()
        .into_value();
    assert_eq!(
        parsed.pointer("/$serde_json::private::Number"),
        Some(&Value::String("1".to_string()))
    );
    assert_eq!(parsed.pointer("/other"), Some(&Value::Bool(true)));
}

#[test]
fn format_assertions_are_enabled_and_unknown_formats_are_annotations() {
    let email = config(json!({"type": "string", "format": "email"})).compile().unwrap();
    assert!(!email
        .validate_instance(&parse_json_instance(br#""not-an-email""#).unwrap())
        .unwrap());
    let unknown = config(json!({"type": "string", "format": "edgion-unknown"}))
        .compile()
        .unwrap();
    assert!(unknown
        .validate_instance(&parse_json_instance(br#""anything""#).unwrap())
        .unwrap());
}

#[test]
fn rejects_regex_format_that_compiles_instance_text_at_request_time() {
    assert!(config(json!({"type": "string", "format": "regex"}))
        .compile()
        .unwrap_err()
        .contains("format 'regex'"));
}

#[test]
fn rejects_pattern_and_schema_limits() {
    let patterns: Map<String, Value> = (0..=MAX_PATTERN_COUNT)
        .map(|i| (format!("p{i}"), Value::Bool(true)))
        .collect();
    assert!(config(json!({"patternProperties": patterns}))
        .compile()
        .unwrap_err()
        .contains("pattern"));

    let half_plus_one = MAX_PATTERN_SOURCE_BYTES / 2 + 1;
    let aggregate_patterns = Map::from_iter([
        ("a".repeat(half_plus_one), Value::Bool(true)),
        ("b".repeat(half_plus_one), Value::Bool(true)),
    ]);
    assert!(config(json!({"patternProperties": aggregate_patterns}))
        .compile()
        .unwrap_err()
        .contains("pattern source"));

    assert!(config(json!({"pattern": "a{1000000}"})).compile().is_err());

    let huge = "x".repeat(MAX_SCHEMA_BYTES);
    assert!(config(json!({"description": huge})).compile().is_err());
}

#[test]
fn rejects_absolute_instance_byte_overflow() {
    let bytes = vec![b' '; ABSOLUTE_INSTANCE_MAX_BYTES as usize + 1];
    assert_eq!(parse_json_instance(&bytes).unwrap_err(), StrictJsonError::TooLarge);
}

#[test]
fn profiles_deep_literal_and_property_costs() {
    let compiled = config(json!({
        "type": "object",
        "required": ["kind", "payload"],
        "properties": {
            "kind": {"enum": ["one", "two", "three"]},
            "payload": {"const": {"nested": ["a", "b", "c"]}}
        }
    }))
    .compile()
    .unwrap();
    let profile = compiled.profile();
    assert!(profile.node_ops >= 20, "{profile:?}");
    assert!(profile.scan_ops >= 7, "{profile:?}");
}

#[test]
fn pattern_scan_weight_uses_128_byte_chunks() {
    assert_eq!(pattern_scan_ops(""), 1);
    assert_eq!(pattern_scan_ops("a"), 1);
    assert_eq!(pattern_scan_ops(&"a".repeat(128)), 1);
    assert_eq!(pattern_scan_ops(&"a".repeat(129)), 2);
    assert_eq!(pattern_scan_ops(&"é".repeat(64)), 1);
    assert_eq!(pattern_scan_ops(&"é".repeat(65)), 2);
}

#[test]
fn profile_weights_pattern_and_pattern_properties_without_double_counting() {
    let direct = config(json!({"pattern": "a".repeat(129)})).compile().unwrap();
    assert_eq!(direct.profile().scan_ops, 2);

    let patterns = Map::from_iter([
        ("a".to_string(), Value::Bool(true)),
        ("b".repeat(129), Value::Bool(true)),
    ]);
    let properties = config(json!({"patternProperties": patterns})).compile().unwrap();
    assert_eq!(properties.profile().scan_ops, 3);
}

#[test]
fn long_pattern_and_large_text_exceed_validation_work_budget() {
    let pattern = "(?:a?)".repeat(86);
    assert!(pattern.len() > 512);
    let compiled = config(json!({"type": "string", "pattern": pattern})).compile().unwrap();
    assert_eq!(compiled.profile().scan_ops, 5);

    let encoded = serde_json::to_vec(&"b".repeat(800 * 1024)).unwrap();
    let parsed = parse_json_instance(&encoded).unwrap();
    assert_eq!(
        compiled.validate_instance(&parsed),
        Err(InstanceValidationError::WorkBudgetExceeded)
    );
}

#[test]
fn profiles_unique_items_pairwise_comparison_fanout() {
    let compiled = config(json!({"type": "array", "uniqueItems": true})).compile().unwrap();
    let profile = compiled.profile();
    assert!(profile.node_ops >= 18, "{profile:?}");
    assert!(profile.scan_ops >= 15, "{profile:?}");

    let instance = Value::Array(
        (0..15)
            .map(|index| Value::String(format!("{}{:02}", "x".repeat(20_000), index)))
            .collect(),
    );
    let encoded = serde_json::to_vec(&instance).unwrap();
    let parsed = parse_json_instance(&encoded).unwrap();
    assert_eq!(
        compiled.validate_instance(&parsed),
        Err(InstanceValidationError::WorkBudgetExceeded)
    );
}
