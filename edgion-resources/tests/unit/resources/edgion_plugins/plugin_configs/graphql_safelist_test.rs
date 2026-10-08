use super::*;
use serde_json::json;

fn config(entries: serde_json::Value) -> GraphqlSafelistConfig {
    serde_json::from_value(json!({"operations":entries})).unwrap()
}
fn request(query: &str, selector: Option<&str>, variables: serde_json::Value) -> ParsedGraphqlRequest {
    ParsedGraphqlRequest::parse_post(
        &serde_json::to_vec(&json!({"query":query,"operationName":selector,"variables":variables})).unwrap(),
        MAX_ENVELOPE_BYTES,
    )
    .unwrap()
}

#[test]
fn safelist_binds_whole_document_actual_kind_and_actual_selected_name() {
    let document = "query Allowed { ping } mutation Other { update }";
    let compiled = config(json!([{"query":document,"operationName":"Allowed"}]))
        .compile()
        .unwrap();
    assert!(compiled.check(request(document, Some("Allowed"), json!({}))).is_ok());
    assert_eq!(
        compiled.check(request(document, Some("Other"), json!({}))),
        Err(GraphqlError::Forbidden)
    );
    assert_eq!(
        compiled.check(request("query Allowed { update }", None, json!({}))),
        Err(GraphqlError::Forbidden)
    );
    assert_eq!(
        compiled.check(request("mutation Allowed { ping }", None, json!({}))),
        Err(GraphqlError::Forbidden)
    );
    let both = config(json!([{"query":document,"operationName":"Allowed"},{"query":document,"operationName":"Other"}]))
        .compile()
        .unwrap();
    assert!(both.check(request(document, Some("Other"), json!({}))).is_ok());
}

#[test]
fn variables_are_parameter_values_but_definitions_aliases_literals_and_fragments_are_bound() {
    let query = "query Read($n:Int) { ping(n:$n) }";
    let compiled = config(json!([{"query":query}])).compile().unwrap();
    for n in [1, 100] {
        assert!(compiled
            .check(request(
                "# ignored\nquery Read ($n: Int) {ping(n: $n)}",
                Some("Read"),
                json!({"n":n})
            ))
            .is_ok());
    }
    for changed in [
        "query Read($n:Int=1) { ping(n:$n) }",
        "query Read($n:Int) { a:ping(n:$n) }",
        "query Read($n:Int) { ping(n:1) }",
        "query Read($n:Int) { ...F } fragment F on Query { ping(n:$n) }",
    ] {
        assert_eq!(
            compiled.check(request(changed, None, json!({"n":1}))),
            Err(GraphqlError::Forbidden)
        );
    }
    let anonymous = config(json!([{"query":"{ ping }"}])).compile().unwrap();
    assert!(anonymous.check(request("{ping}", None, json!({}))).is_ok());
    assert_eq!(
        anonymous.check(request("query Named { ping }", None, json!({}))),
        Err(GraphqlError::Forbidden)
    );
}

#[test]
fn invalid_or_duplicate_registrations_fail_cold_and_bounds_are_checked_first() {
    for entries in [
        json!([]),
        json!([{"query":"invalid @@"}]),
        json!([{"query":"query A { ping } query B { pong }"}]),
        json!([{"query":"query A { ping }","operationName":"Other"}]),
        json!([{"query":"{ ping }"},{"query":"# ignored\n{ping}"}]),
    ] {
        let cfg = config(entries);
        assert!(cfg.compile().is_err());
        assert!(cfg.validation_error().is_some());
    }
    let entry = GraphqlTrustedOperation {
        query: "{ ping }".into(),
        operation_name: None,
    };
    assert!(GraphqlSafelistConfig {
        operations: vec![entry.clone(); 129]
    }
    .compile()
    .is_err());
    let mut too_large = entry.clone();
    too_large.query = " ".repeat(MAX_ENVELOPE_BYTES + 1);
    assert!(GraphqlSafelistConfig {
        operations: vec![too_large]
    }
    .compile()
    .unwrap_err()
    .contains("byte limit"));
    let mut selector = entry.clone();
    selector.operation_name = Some("a".repeat(257));
    assert!(GraphqlSafelistConfig {
        operations: vec![selector]
    }
    .compile()
    .unwrap_err()
    .contains("byte limit"));
    let mut source = entry;
    source.query = " ".repeat(MAX_ENVELOPE_BYTES);
    assert!(GraphqlSafelistConfig {
        operations: vec![source; 9]
    }
    .compile()
    .unwrap_err()
    .contains("512KiB"));
}
