use super::*;
use serde_json::json;

fn post(query: &str, selector: Option<&str>) -> Result<ParsedGraphqlRequest, GraphqlError> {
    let value = json!({"query":query,"operationName":selector});
    ParsedGraphqlRequest::parse_post(value.to_string().as_bytes(), MAX_ENVELOPE_BYTES)
}
#[test]
fn infers_and_verifies_actual_operation_type_and_name() {
    let p = post("query GetUser { unknownField }", None).unwrap();
    assert_eq!(p.operation_name(), Some("GetUser"));
    assert_eq!(p.operation_type(), GraphqlOperationType::Query);
    let p = post(
        "query GetUser { user } mutation CreateOrder { order }",
        Some("CreateOrder"),
    )
    .unwrap();
    assert_eq!(p.operation_name(), Some("CreateOrder"));
    assert_eq!(p.operation_type(), GraphqlOperationType::Mutation);
    assert_eq!(post("{ user }", None).unwrap().operation_name(), None);
}
#[test]
fn ambiguous_invalid_and_unsupported_documents_cannot_select_a_route() {
    for (query, selector) in [
        ("query A {x} query B {y}", None),
        ("query A {x}", Some("B")),
        ("query A {x} query A {y}", Some("A")),
        ("{x} query A {y}", None),
        ("subscription Watch {x}", None),
        ("type Query {x:String}", None),
        ("query A { ...F } fragment F on Query {...F}", None),
        ("query A { ...Missing }", None),
        ("query A @unknown {x}", None),
        ("query A($v:Int,$v:Int) {x}", None),
        ("query A {x(a:1,a:2)}", None),
    ] {
        assert_eq!(post(query, selector).unwrap_err(), GraphqlError::Invalid, "{query}");
    }
    assert_eq!(
        ParsedGraphqlRequest::parse_get("query=mutation+M%7Bx%7D", MAX_ENVELOPE_BYTES).unwrap_err(),
        GraphqlError::MethodNotAllowed
    );
}
#[test]
fn strict_envelopes_and_hard_caps_apply_without_sdl() {
    for body in [
        r#"[{"query":"{x}"}]"#,
        r#"{"query":"{x}","query":"{y}"}"#,
        r#"{"query":"{x}","extra":1}"#,
        r#"{"query":"{x}","extensions":{"persistedQuery":{}}}"#,
    ] {
        assert_eq!(
            ParsedGraphqlRequest::parse_post(body.as_bytes(), MAX_ENVELOPE_BYTES).unwrap_err(),
            GraphqlError::Invalid
        );
    }
    let valid = br#"{"query":"query Q {x}","variables":{"v":1}}"#;
    assert!(ParsedGraphqlRequest::parse_post(valid, valid.len()).is_ok());
    assert_eq!(
        ParsedGraphqlRequest::parse_post(valid, valid.len() - 1).unwrap_err(),
        GraphqlError::TooLarge
    );
    for query in ["query=%ZZ", "query=%7Bx%7D&%71uery=%7By%7D", "query=%7Bx%7D&other=1"] {
        assert!(ParsedGraphqlRequest::parse_get(query, MAX_ENVELOPE_BYTES).is_err());
    }
    assert!(post(&format!("query {} {{x}}", "a".repeat(257)), None).is_err());
    assert!(post(&format!("{{{}x{}}}", "x{".repeat(65), "}".repeat(65)), None).is_err());
    assert!(post(&format!("{{{}}}", "x ".repeat(4097)), None).is_err());
}
