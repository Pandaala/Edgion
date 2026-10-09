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

#[test]
fn measurement_reports_all_business_violations_without_relaxing_default_mode() {
    let value = json!({"mode":"measure","maxDepth":1,"maxAliases":0,"maxRootFields":0,"maxFields":0,
        "maxCost":1,"schema":"type Query { user:User ping:Int } type User { id:ID }"});
    let measured = config(value.clone()).compile().unwrap();
    let report = measured.inspect(request("{ a:user { id } ping }")).unwrap();
    assert_eq!(report.shape.depth, 2);
    assert_eq!(report.shape.aliases, 1);
    assert_eq!(report.shape.root_fields, 2);
    assert_eq!(report.shape.fields, 3);
    assert_eq!(report.cost, Some(4));
    assert_eq!(report.exceeded, 31);
    assert!(measured.check(request("{ a:user { id } ping }")).is_ok());
    let mut enforced = value;
    enforced.as_object_mut().unwrap().remove("mode");
    assert_eq!(config(enforced).mode, GraphqlGuardMode::Enforce);
    assert!(config(json!({"maxDepth":1}))
        .compile()
        .unwrap()
        .check(request("{ user { id } }"))
        .is_err());
    assert!(serde_json::from_value::<GraphqlGuardConfig>(json!({"mode":"warn"})).is_err());
    assert!(serde_json::from_value::<GraphqlGuardConfig>(json!({"mode":null})).is_err());
}

#[test]
fn measurement_selection_fragments_directives_zero_and_null_remain_conservative() {
    let measured = config(json!({"mode":"measure","maxAliases":0,"maxFields":null,"maxRootFields":1,"maxDepth":null}))
        .compile()
        .unwrap();
    let query =
        "query Safe { ...F ...F } query Other { a:ping b:ping c:ping } fragment F on Query { x:ping @skip(if:true) }";
    let selected = ParsedGraphqlRequest::parse_post(
        &serde_json::to_vec(&json!({"query":query,"operationName":"Safe"})).unwrap(),
        MAX_ENVELOPE_BYTES,
    )
    .unwrap();
    let report = measured.inspect(selected).unwrap();
    assert_eq!(
        report.shape,
        GraphqlShapeReport {
            depth: 1,
            aliases: 2,
            root_fields: 2,
            fields: 2
        }
    );
    assert_eq!(report.exceeded, 6);
    assert_eq!(report.cost, None);
    assert_eq!(measured.inspect(request("{ ping }")).unwrap().exceeded, 0);
}

#[test]
fn measurement_keeps_introspection_and_root_authorization_hard() {
    let measured = config(json!({"mode":"measure","maxFields":0,"rootFields":{"query":{"deny":["blocked"]}}}))
        .compile()
        .unwrap();
    assert_eq!(measured.inspect(request("{ a:blocked }")), Err(GraphqlError::Forbidden));
    assert_eq!(
        measured.inspect(request("{ a:__schema { queryType { name } } }")),
        Err(GraphqlError::Invalid)
    );
}

#[test]
fn measurement_validates_original_limits_and_independent_cost_bounds() {
    for value in [
        json!({"mode":"measure","maxCost":0,"schema":"type Query { ping:Int }"}),
        json!({"mode":"measure","maxCost":1_000_001,"schema":"type Query { ping:Int }"}),
        json!({"mode":"measure","maxDepth":33}),
        json!({"showQueryCost":true}),
    ] {
        assert!(config(value).validation_error().is_some());
    }
    let measured = config(json!({"mode":"measure","maxDepth":1,"maxCost":1,
        "schema":"type Query { child:Query id:ID }"}))
    .compile()
    .unwrap();
    let deep = format!("{{ {}id{} }}", "child { ".repeat(31), "}".repeat(31));
    let report = measured.inspect(request(&deep)).unwrap();
    assert_eq!(report.shape.depth, 32);
    assert_eq!(report.cost, Some(33));
    assert_eq!(report.exceeded, 17);
    let beyond = format!("{{ {}id{} }}", "child { ".repeat(32), "}".repeat(32));
    assert!(measured.inspect(request(&beyond)).is_err());
    let maximum = config(json!({"mode":"measure","maxCost":1,"schema":"type Query { ping:Int }",
        "costRules":[{"field":"Query.ping","weight":999_999}]}))
    .compile()
    .unwrap();
    assert_eq!(maximum.inspect(request("{ ping }")).unwrap().cost, Some(1_000_000));
    assert!(maximum.inspect(request("{ ping ping }")).is_err());
}

#[test]
fn guard_and_limiter_share_strict_sdl_metadata_validation() {
    use crate::resources::edgion_plugins::plugin_configs::GraphqlRateLimitConfig;
    for (schema, valid) in [
        (
            "type Query { ping:Int @deprecated } scalar Date @specifiedBy(url:\"https://example.com/date\")",
            true,
        ),
        ("type Query { ping:Int @deprecated(reason:null) }", false),
        ("type Query { ping:Int @specifiedBy(url:\"x\") }", false),
    ] {
        let guard = config(json!({"schema":schema,"maxCost":100}));
        let limiter: GraphqlRateLimitConfig = serde_json::from_value(
            json!({"schema":schema,"redisRef":"default/redis","policies":[{"rate":100,"interval":"1s"}]}),
        )
        .unwrap();
        assert_eq!(guard.compile().is_ok(), valid);
        assert_eq!(limiter.compile().is_ok(), valid);
        assert_eq!(limiter.validation_error().is_none(), valid);
    }
}
