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

#[test]
fn shared_fragment_graphs_preserve_frontend_work() {
    for (query, count, steps) in [
        ("query Q {...A ...A} fragment A on Query {...B} fragment B on Query {x}", 2, 12),
        ("query Q {...A ...A} fragment A on Query {...B ...C} fragment B on Query {...D} fragment C on Query {...D} fragment D on Query {x}", 4, 22),
        ("query Q {x} fragment Unused on Query {y}", 1, 5),
        ("query Q {...MixedCase ...MixedCase} fragment MixedCase on Query {...lower} fragment lower on Query {x}", 2, 12),
    ] {
        let parsed = post(query, None).unwrap();
        assert_eq!(parsed.operation_name(), Some("Q"));
        assert_eq!(parsed.fragments.len(), count);
        assert_eq!(parsed.work.0, steps, "{query}");
    }
}

#[test]
fn fragment_graph_validation_includes_unused_and_unselected_definitions() {
    for query in [
        "query Q {x} fragment A on Query {...A}",
        "query Q {x} fragment A on Query {...B} fragment B on Query {...A}",
        "query Q {x} fragment A on Query {...Missing}",
        "query Q {x} fragment A on Query {y} fragment A on Query {z}",
        "query Q {x} query Other {...Missing}",
        "query Q {...mixedCase} fragment MixedCase on Query {x}",
        "query Q {x} query Other {...mixedCase} fragment MixedCase on Query {x}",
        "query Q {x} fragment Unused on Query {...mixedCase} fragment MixedCase on Query {x}",
        "query Q {x} query Other {...A} fragment A on Query {...A}",
    ] {
        assert_eq!(post(query, Some("Q")).unwrap_err(), GraphqlError::Invalid, "{query}");
    }
}

#[test]
fn directive_names_preserve_acceptance_and_exact_work() {
    for (suffix, accepted, steps) in [
        ("", true, 0),
        ("@skip(if:true)", true, 2),
        ("@include(if:false)", true, 2),
        ("@skip(if:$flag)", true, 2),
        ("@skip(if:true) @include(if:false)", true, 4),
        ("@skip(if:true) @skip(if:false)", true, 4),
        ("@Skip(if:true)", false, 1),
        ("@unknown(if:true)", false, 1),
        ("@skip", false, 1),
        ("@skip(IF:true)", false, 2),
        ("@skip(other:true)", false, 2),
        ("@skip(if:null)", false, 2),
        ("@skip(if:1)", false, 2),
        ("@skip(if:true,if:false)", false, 3),
        ("@skip(if:true) @unknown(if:false)", false, 3),
    ] {
        let document = parse_document(&format!("{{x {suffix}}}")).unwrap();
        let operation = document
            .definitions()
            .find_map(|definition| match definition {
                cst::Definition::OperationDefinition(operation) => Some(operation),
                _ => None,
            })
            .unwrap();
        let field = operation
            .selection_set()
            .unwrap()
            .selections()
            .find_map(|selection| match selection {
                cst::Selection::Field(field) => Some(field),
                _ => None,
            })
            .unwrap();
        let expected = if accepted { Ok(()) } else { Err(GraphqlError::Invalid) };
        let mut work = Work::default();
        assert_eq!(directives(field.directives(), &mut work), expected, "{suffix}");
        assert_eq!(work.0, steps, "{suffix}");
        if accepted {
            let mut exact = Work(MAX_WORK - steps);
            assert_eq!(directives(field.directives(), &mut exact), Ok(()), "{suffix}");
            assert_eq!(exact.0, MAX_WORK);
            if steps > 0 {
                let mut short = Work(MAX_WORK - steps + 1);
                assert_eq!(
                    directives(field.directives(), &mut short),
                    Err(GraphqlError::Invalid),
                    "{suffix}"
                );
            }
        }
    }
}
