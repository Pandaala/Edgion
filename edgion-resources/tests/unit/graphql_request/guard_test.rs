use super::*;
use crate::graphql_request::{MAX_ENVELOPE_BYTES, MAX_WORK};
use serde_json::json;

fn parsed(query: &str, selector: Option<&str>) -> ParsedGraphqlRequest {
    ParsedGraphqlRequest::parse_post(
        json!({"query":query,"operationName":selector}).to_string().as_bytes(),
        MAX_ENVELOPE_BYTES,
    )
    .unwrap()
}
fn limits() -> GraphqlShapeLimits {
    GraphqlShapeLimits {
        max_depth: Some(2),
        max_aliases: Some(1),
        max_root_fields: Some(2),
        max_fields: Some(3),
        allow_introspection: false,
    }
}
#[test]
fn equality_and_each_overflow_are_independent() {
    assert!(parsed("{ a: user { id } ping }", None)
        .validate_shape(&limits())
        .is_ok());
    for query in [
        "{ user { child { id } } }",
        "{ a:ping b:ping }",
        "{ ping pong other }",
        "{ user { id name } ping }",
    ] {
        assert_eq!(
            parsed(query, None).validate_shape(&limits()),
            Err(GraphqlError::Invalid),
            "{query}"
        );
    }
}
#[test]
fn fragments_keep_semantic_depth_and_count_each_occurrence() {
    let query = "{ ...F ... on Query { ping } } fragment F on Query { a: user { ...G } } fragment G on User { id }";
    assert!(parsed(query, None).validate_shape(&limits()).is_ok());
    assert!(parsed("{ ...F ...F } fragment F on Query { a: ping }", None)
        .validate_shape(&limits())
        .is_err());
    assert!(parsed("{ ...F ...F ...F } fragment F on Query { ping }", None)
        .validate_shape(&limits())
        .is_err());
    assert!(parsed("{ user { ...F } } fragment F on User { child { id } }", None)
        .validate_shape(&limits())
        .is_err());
}
#[test]
fn selected_operation_only_and_directives_are_conservative() {
    let query = "query Safe { ping } query Large { a:ping b:ping c:ping }";
    assert!(parsed(query, Some("Safe")).validate_shape(&limits()).is_ok());
    assert!(parsed(query, Some("Large")).validate_shape(&limits()).is_err());
    assert!(parsed("{ a:ping @skip(if:true) b:ping @include(if:false) }", None)
        .validate_shape(&limits())
        .is_err());
}
#[test]
fn introspection_cannot_hide_behind_alias_or_fragment() {
    for query in [
        "{ s: __schema { queryType { name } } }",
        "{ ...F } fragment F on Query { __type(name:\"User\") { name } }",
    ] {
        let mut permissive = limits();
        permissive.max_depth = None;
        permissive.max_fields = None;
        assert!(parsed(query, None).validate_shape(&permissive).is_err());
        permissive.allow_introspection = true;
        assert!(parsed(query, None).validate_shape(&permissive).is_ok());
    }
    assert!(parsed("{ __typename }", None).validate_shape(&limits()).is_ok());
}
#[test]
fn zero_and_null_limits_have_distinct_meaning() {
    let mut l = limits();
    l.max_fields = Some(0);
    assert!(parsed("{ ping }", None).validate_shape(&l).is_err());
    l.max_fields = None;
    l.max_aliases = Some(0);
    assert!(parsed("{ ping }", None).validate_shape(&l).is_ok());
    assert!(parsed("{ a:ping }", None).validate_shape(&l).is_err());
    l.max_aliases = None;
    l.max_root_fields = Some(0);
    assert!(parsed("{ ping }", None).validate_shape(&l).is_err());
}
#[test]
fn disabled_business_limits_retain_work_budget_and_selected_walk_does_not_reset_it() {
    let mut p = parsed("{ ping }", None);
    p.work = Work(MAX_WORK);
    let l = GraphqlShapeLimits {
        max_depth: None,
        max_aliases: None,
        max_root_fields: None,
        max_fields: None,
        allow_introspection: true,
    };
    assert_eq!(p.validate_shape(&l), Err(GraphqlError::Invalid));
    let mut query = "{ ...F14 } ".to_owned();
    query.push_str("fragment F0 on Query { ping } ");
    for i in 1..=14 {
        query.push_str(&format!("fragment F{i} on Query {{ ...F{} ...F{} }} ", i - 1, i - 1));
    }
    assert_eq!(parsed(&query, None).validate_shape(&l), Err(GraphqlError::Invalid));
}

#[test]
fn shape_work_is_carried_into_optional_costing() {
    use crate::plugin_configs::GraphqlGuardConfig;
    let c: GraphqlGuardConfig =
        serde_json::from_value(json!({"maxCost":10,"schema":"type Query { ping:Int }"})).unwrap();
    let mut p = parsed("{ ping }", None);
    p.work = Work(MAX_WORK - 1);
    assert!(c.compile().unwrap().check(p).is_err());
}
