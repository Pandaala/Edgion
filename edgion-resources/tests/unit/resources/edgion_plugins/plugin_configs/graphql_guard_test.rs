use super::*;
use crate::graphql_request::MAX_ENVELOPE_BYTES;
use serde_json::{json, Value};

fn config(value: Value) -> GraphqlGuardConfig {
    serde_json::from_value(value).unwrap()
}
fn request(query: &str) -> ParsedGraphqlRequest {
    ParsedGraphqlRequest::parse_post(json!({"query":query}).to_string().as_bytes(), MAX_ENVELOPE_BYTES).unwrap()
}
#[test]
fn defaults_and_null_survive_serialized_transport() {
    let c = config(json!({}));
    assert_eq!(c.max_depth, Some(20));
    assert_eq!(c.max_fields, Some(1024));
    assert!(c.compile().unwrap().check(request("{ unknownField } ")).is_ok());
    let c = config(json!({"maxDepth":null,"maxAliases":null,"maxRootFields":null,"maxFields":null}));
    let transmitted: GraphqlGuardConfig = serde_json::from_value(serde_json::to_value(c).unwrap()).unwrap();
    assert_eq!(transmitted.max_depth, None);
    assert_eq!(transmitted.max_fields, None);
}
#[test]
fn bounds_and_unused_or_inconsistent_cost_settings_fail_validation() {
    for value in [
        json!({"maxDepth":0}),
        json!({"maxDepth":33}),
        json!({"maxAliases":4097}),
        json!({"maxFields":4097}),
        json!({"maxRootFields":4097}),
        json!({"schema":"type Query { x: Int }"}),
        json!({"costRules":[{"field":"Query.x"}]}),
        json!({"maxCost":10}),
        json!({"maxCost":0,"schema":"type Query { x: Int }"}),
        json!({"maxCost":10,"schema":"type Query { x: Int }","allowIntrospection":true}),
    ] {
        assert!(config(value).validation_error().is_some());
    }
    assert!(serde_json::from_value::<GraphqlGuardConfig>(json!({"extra":1})).is_err());
}
#[test]
fn existing_sdl_cost_ceiling_is_enforced_without_redis() {
    let c = config(
        json!({"schema":"type Query { users(first:Int):[User] } type User { id:ID }","maxCost":5,"costRules":[{"field":"Query.users","multipliers":["first"]}]}),
    );
    let guard = c.compile().unwrap();
    assert!(guard.check(request("{ users(first:3) { id } } ")).is_ok());
    assert!(guard.check(request("{ users(first:4) { id } } ")).is_err());
}
#[test]
fn null_depth_disables_shape_limit_but_cost_retains_independent_32_ceiling() {
    let query = format!("{{ {}id{} }}", "child { ".repeat(33), "}".repeat(33));
    let c = config(json!({"maxDepth":null}));
    assert!(c.compile().unwrap().check(request(&query)).is_ok());
    let c = config(json!({"maxDepth":null,"maxCost":100,"schema":"type Query { child:Query id:ID }"}));
    assert!(c.compile().unwrap().check(request(&query)).is_err());
    let at_32 = format!("{{ {}id{} }}", "child { ".repeat(31), "}".repeat(31));
    assert!(c.compile().unwrap().check(request(&at_32)).is_ok());
}
#[test]
fn root_policies_match_all_underlying_fields_through_fragments_and_directives() {
    use crate::graphql_request::{GraphqlError, ParsedGraphqlRequest, MAX_ENVELOPE_BYTES};
    let config: GraphqlGuardConfig = serde_json::from_value(serde_json::json!({
        "rootFields":{"query":{"allow":["ping","user","__typename"],"deny":["blocked"]},"mutation":{"allow":["update"],"deny":["update"]}}
    })).unwrap();
    let compiled = config.compile().unwrap();
    let check = |query: &str| {
        compiled.check(
            ParsedGraphqlRequest::parse_post(
                &serde_json::to_vec(&serde_json::json!({"query":query})).unwrap(),
                MAX_ENVELOPE_BYTES,
            )
            .unwrap(),
        )
    };
    for query in [
        "{ a:ping user { blocked } }",
        "{ ...F } fragment F on Query { ping }",
        "{ __typename }",
    ] {
        assert_eq!(check(query), Ok(()));
    }
    for query in [
        "{ ping blocked }",
        "{ a:blocked }",
        "{ ...F } fragment F on Query { blocked }",
        "{ ... on Query { blocked } }",
        "{ blocked @skip(if:true) }",
        "{ blocked @include(if:false) }",
        "{ unknown }",
        "mutation M { update }",
    ] {
        assert_eq!(check(query), Err(GraphqlError::Forbidden));
    }
    let selected = ParsedGraphqlRequest::parse_post(
        br#"{"query":"query Q { ping } mutation M { update }","operationName":"Q"}"#,
        MAX_ENVELOPE_BYTES,
    )
    .unwrap();
    assert_eq!(compiled.check(selected), Ok(()));
}

#[test]
fn root_missing_null_empty_and_introspection_semantics_survive_transport() {
    use crate::graphql_request::{GraphqlError, ParsedGraphqlRequest, MAX_ENVELOPE_BYTES};
    for (root_fields, expected) in [
        (serde_json::Value::Null, Ok(())),
        (serde_json::json!({"query":null}), Ok(())),
        (serde_json::json!({"query":{"allow":null}}), Ok(())),
        (serde_json::json!({"query":{"allow":[]}}), Err(GraphqlError::Forbidden)),
        (
            serde_json::json!({"query":{"deny":["ping"]}}),
            Err(GraphqlError::Forbidden),
        ),
    ] {
        let config: GraphqlGuardConfig = serde_json::from_value(serde_json::json!({"rootFields":root_fields})).unwrap();
        let roundtrip: GraphqlGuardConfig = serde_json::from_value(serde_json::to_value(config).unwrap()).unwrap();
        assert_eq!(
            roundtrip
                .compile()
                .unwrap()
                .check(ParsedGraphqlRequest::parse_post(br#"{"query":"{ping}"}"#, MAX_ENVELOPE_BYTES).unwrap()),
            expected
        );
        assert_eq!(
            roundtrip.compile().unwrap().check(
                ParsedGraphqlRequest::parse_post(br#"{"query":"mutation M { ping }"}"#, MAX_ENVELOPE_BYTES).unwrap()
            ),
            Ok(())
        );
    }
    let config: GraphqlGuardConfig =
        serde_json::from_value(serde_json::json!({"rootFields":{"query":{"allow":["__schema","__typename"]}}}))
            .unwrap();
    let compiled = config.compile().unwrap();
    assert_eq!(
        compiled.check(
            ParsedGraphqlRequest::parse_post(br#"{"query":"{__schema { queryType }}"}"#, MAX_ENVELOPE_BYTES).unwrap()
        ),
        Err(GraphqlError::Invalid)
    );
    assert_eq!(
        compiled.check(ParsedGraphqlRequest::parse_post(br#"{"query":"{__typename}"}"#, MAX_ENVELOPE_BYTES).unwrap()),
        Ok(())
    );
}

#[test]
fn root_policy_configuration_is_strict_bounded_and_unique() {
    use serde_json::json;
    for names in [
        vec!["".to_owned()],
        vec![" ping".into()],
        vec!["ping # comment".into()],
        vec!["Query.ping".into()],
        vec!["1ping".into()],
        vec!["ping".into(), "ping".into()],
        vec!["a".repeat(257)],
        vec!["ping".into(); 257],
    ] {
        let config: GraphqlGuardConfig =
            serde_json::from_value(json!({"rootFields":{"query":{"allow":names}}})).unwrap();
        assert!(config.compile().is_err());
        assert!(config.validation_error().is_some());
    }
    for value in [
        json!({"rootFields":{"unknown":{}}}),
        json!({"rootFields":{"query":{"unknown":[]}}}),
    ] {
        assert!(serde_json::from_value::<GraphqlGuardConfig>(value).is_err());
    }
    let names: Vec<String> = (0..256).map(|n| format!("field_{n}")).collect();
    let config: GraphqlGuardConfig =
        serde_json::from_value(json!({"rootFields":{"query":{"allow":names,"deny":["_private"]}}})).unwrap();
    assert!(config.compile().is_ok());
}
