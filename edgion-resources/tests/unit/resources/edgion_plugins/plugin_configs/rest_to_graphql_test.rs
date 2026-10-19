use super::*;
use serde_json::json;

#[test]
fn enabled_validation_and_body_policy_match_disabled_inert_semantics() {
    use crate::EdgionPlugins;
    for (enabled, body, rejected) in [
        (true, json!({"maxBodySize":"128k"}), true),
        (true, json!({"onReadFailure":"failOpen"}), true),
        (false, json!({"maxBodySize":"128k"}), false),
    ] {
        let resource:EdgionPlugins=serde_json::from_value(json!({"apiVersion":"edgion.io/v1","kind":"EdgionPlugins",
            "metadata":{"name":"adapter","namespace":"default"},"spec":{"requestPlugins":[{"type":"RestToGraphql","enable":enabled,"config":{"query":"{ping}"},"body":body}]}})).unwrap();
        assert_eq!(!resource.validate_plugins().is_empty(), rejected);
    }
}

fn config(value: Value) -> RestToGraphqlConfig {
    serde_json::from_value(value).unwrap()
}

#[test]
fn validation_is_stateless_and_rejects_invalid_sources_documents_and_targets() {
    let nested_type = format!("{}Int{}", "[".repeat(8), "]".repeat(8));
    assert!(
        config(json!({"query":format!("query($unused:{nested_type}){{ping}}") }))
            .compile()
            .is_err()
    );
    for value in [
        json!({"query":"bad @@"}),
        json!({"query":"{ping}","path":"https://example.com/graphql"}),
        json!({"query":"{ping}","path":"/graphql?secret=1"}),
        json!({"query":"query($id:ID!){user(id:$id){id}}"}),
        json!({"query":"{ping}","variables":[{"name":"id","from":{"type":"ctx","name":"id"}}]}),
        json!({"query":"query($id:ID){user(id:$id){id}}","variables":[{"name":"id"}]}),
        json!({"query":"query($id:ID){user(id:$id){id}}","variables":[{"name":"id","from":{"type":"header","name":"bad header"}}]}),
        json!({"query":"query($id:ID){user(id:$id){id}}","variables":[{"name":"id","bodyPointer":"bad"}]}),
        json!({"query":"query($id:ID){user(id:$id){id}}","variables":[{"name":"id","bodyPointer":"/~2"}]}),
        json!({"query":"query($id:ID){user(id:$id){id}}","variables":[{"name":"id","from":{"type":"ctx","name":"id"},"bodyPointer":"/id"}]}),
        json!({"query":"query($id:ID){user(id:$id){id}}","variables":[{"name":"id","from":{"type":"ctx","name":"id"}},{"name":"id","bodyPointer":"/id"}]}),
    ] {
        assert!(config(value).validation_error().is_some());
    }
    assert!(
        config(json!({"query":"query A{ping} query B{pong}","operationName":"B"}))
            .compile()
            .is_ok()
    );
    assert!(config(json!({"query":"query($n:Int!=2){ping(n:$n)}"}))
        .compile()
        .is_ok());
}

#[test]
fn missing_null_defaults_and_text_coercion_retain_distinct_semantics() {
    let compiled = config(
        json!({"query":"query($n:Int!=2,$id:ID,$b:Boolean!){ping(n:$n,id:$id,b:$b)}",
        "variables":[{"name":"n","from":{"type":"ctx","name":"n"}},
            {"name":"id","from":{"type":"query","name":"id"}},
            {"name":"b","from":{"type":"header","name":"X-Flag"}}]}),
    )
    .compile()
    .unwrap();
    assert_eq!(compiled.bind(0, None).unwrap(), None);
    assert!(compiled.bind(0, Some(Cow::Borrowed(&json!("null")))).is_err());
    assert_eq!(
        compiled.bind(0, Some(Cow::Borrowed(&json!("10")))).unwrap(),
        Some(json!(10))
    );
    for value in ["01", "2147483648", "10.0", "\"10\"", "false", ""] {
        assert!(compiled.bind(0, Some(Cow::Borrowed(&json!(value)))).is_err());
    }
    assert_eq!(compiled.bind(1, None).unwrap(), None);
    assert_eq!(
        compiled.bind(1, Some(Cow::Borrowed(&json!("42")))).unwrap(),
        Some(json!("42"))
    );
    assert_eq!(
        compiled.bind(2, Some(Cow::Borrowed(&json!("true")))).unwrap(),
        Some(json!(true))
    );
    assert!(compiled.bind(2, None).is_err());
    assert!(compiled.bind(2, Some(Cow::Borrowed(&json!("1")))).is_err());
}

#[test]
fn native_pointer_values_and_explicit_json_lists_are_typed_and_bounded() {
    let compiled = config(
        json!({"query":"mutation($input:CreateInput!,$ids:[Int!]!){create(input:$input,ids:$ids){id}}",
        "variables":[{"name":"input","bodyPointer":"/a~1b/~0input"},
            {"name":"ids","from":{"type":"ctx","name":"ids"},"format":"json"}]}),
    )
    .compile()
    .unwrap();
    let body = json!({"a/b":{"~input":{"name":"safe"}}});
    assert_eq!(
        compiled
            .bind(0, compiled.body_value(0, &body).map(Cow::Borrowed))
            .unwrap(),
        Some(json!({"name":"safe"}))
    );
    assert!(compiled.bind(0, Some(Cow::Borrowed(&Value::Null))).is_err());
    assert_eq!(
        compiled.bind(1, Some(Cow::Borrowed(&json!("[1,2]")))).unwrap(),
        Some(json!([1, 2]))
    );
    for value in ["[1,null]", "[1,\"2\"]", "1", "null"] {
        assert!(compiled.bind(1, Some(Cow::Borrowed(&json!(value)))).is_err());
    }
    assert!(compiled
        .bind(
            1,
            Some(Cow::Borrowed(&json!(serde_json::to_string(&vec![1; 33]).unwrap())))
        )
        .is_err());
    assert!(compiled
        .bind(0, Some(Cow::Borrowed(&json!("x".repeat(16 * 1024 + 1)))))
        .is_err());
}

#[test]
fn entry_query_preflight_rejects_decoded_duplicates_encoding_and_pair_limits() {
    for query in ["n=1&%6e=2", "%6e=1&n=2", "n=%GG", "n=%ff", "n=%", "n=%C3"] {
        assert!(validate_rest_query(query, 65536).is_err());
    }
    assert!(validate_rest_query("q=hello%20world&n=10", 65536).is_ok());
    assert!(validate_rest_query("q=hello+world", 65536).is_ok());
    assert!(validate_rest_query("empty&x=", 65536).is_ok());
    assert!(validate_rest_query("n=123", 3).is_err());
    let query = (0..257).map(|n| format!("n{n}=1")).collect::<Vec<_>>().join("&");
    assert!(validate_rest_query(&query, 65536).is_err());
}

#[test]
fn leaf_value_bounds_keep_profile_and_text_byte_limits() {
    for value in [
        Value::Null,
        json!(false),
        json!(true),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(1.25),
        json!(""),
        json!("x".repeat(REST_GRAPHQL_MAX_VALUE_BYTES)),
        json!("é".repeat(REST_GRAPHQL_MAX_VALUE_BYTES / 2)),
    ] {
        assert_eq!(check_value_bounds(&value), Ok(()));
    }
    for text in [
        "x".repeat(REST_GRAPHQL_MAX_VALUE_BYTES + 1),
        "é".repeat(REST_GRAPHQL_MAX_VALUE_BYTES / 2 + 1),
    ] {
        assert_eq!(check_value_bounds(&Value::String(text)), Err(GraphqlError::TooLarge));
    }
}

#[test]
fn container_value_bounds_keep_nested_arrays_and_profile_error_precedence() {
    assert_eq!(
        check_value_bounds(&json!({"nested": {"items": vec![Value::Null; 32]}})),
        Ok(())
    );
    assert_eq!(
        check_value_bounds(&json!({"nested": {"items": vec![Value::Null; 33]}})),
        Err(GraphqlError::TooLarge)
    );
    let nested = |depth: usize| (1..depth).fold(Value::Null, |child, _| Value::Array(vec![child]));
    assert_eq!(check_value_bounds(&nested(32)), Ok(()));
    assert_eq!(check_value_bounds(&nested(33)), Err(GraphqlError::TooLarge));
    assert_eq!(check_value_bounds(&nested(65)), Err(GraphqlError::Invalid));
    // Every array is within its length cap; aggregate node count is the rejection.
    let row = Value::Array(vec![Value::Null; 32]);
    let block = Value::Array(vec![row; 32]);
    assert_eq!(check_value_bounds(&Value::Array(vec![block.clone(); 3])), Ok(()));
    assert_eq!(
        check_value_bounds(&Value::Array(vec![block; 4])),
        Err(GraphqlError::TooLarge)
    );
    let many_nodes = Value::Array(vec![Value::Array(vec![Value::Null; 32]); 625]);
    // Universal profiling rejects first, before the stricter adapter array cap.
    assert_eq!(check_value_bounds(&many_nodes), Err(GraphqlError::Invalid));
}

#[test]
fn owned_and_borrowed_binding_inputs_keep_results_and_errors() {
    let compiled = config(json!({
        "query": "query($id:ID,$items:[Int!]!,$input:CustomInput){read(id:$id,items:$items,input:$input)}",
        "variables": [
            {"name":"id","from":{"type":"ctx","name":"id"}},
            {"name":"items","from":{"type":"ctx","name":"items"},"format":"json"},
            {"name":"input","bodyPointer":"/input"}
        ]
    }))
    .compile()
    .unwrap();
    for index in 0..3 {
        for value in [
            Value::Null,
            json!(false),
            json!(42),
            json!(""),
            json!("hello"),
            json!("[1,2]"),
            json!("[1,null]"),
            json!("null"),
            json!({"field":[1,2]}),
            json!("x".repeat(REST_GRAPHQL_MAX_VALUE_BYTES)),
            json!("x".repeat(REST_GRAPHQL_MAX_VALUE_BYTES + 1)),
        ] {
            assert_eq!(
                compiled.bind(index, Some(Cow::Owned(value.clone()))),
                compiled.bind(index, Some(Cow::Borrowed(&value)))
            );
        }
    }
}

#[test]
fn owned_text_binding_moves_storage_and_body_binding_keeps_original() {
    let compiled = config(json!({
        "query":"query($id:ID,$input:CustomInput){read(id:$id,input:$input)}",
        "variables":[{"name":"id","from":{"type":"ctx","name":"id"}},{"name":"input","bodyPointer":"/input"}]
    }))
    .compile()
    .unwrap();
    let text = "stable-owned-text".to_owned();
    let pointer = text.as_ptr();
    let output = compiled
        .bind(0, Some(Cow::Owned(Value::String(text))))
        .unwrap()
        .unwrap();
    assert_eq!(output.as_str(), Some("stable-owned-text"));
    assert_eq!(output.as_str().unwrap().as_ptr(), pointer);

    let body = json!({"input":{"field":"preserved"}});
    let selected = compiled.body_value(1, &body).unwrap();
    let mut output = compiled.bind(1, Some(Cow::Borrowed(selected))).unwrap().unwrap();
    assert_eq!(&output, selected);
    output.as_object_mut().unwrap().clear();
    assert_eq!(body, json!({"input":{"field":"preserved"}}));
}
