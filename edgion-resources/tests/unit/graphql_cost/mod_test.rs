use super::*;
use apollo_parser::{Lexer, Parser};

fn model(schema: &str, rules: &[(&str, u32, &[&str], u32)]) -> CompiledGraphqlCost {
    let rules: Vec<_> = rules
        .iter()
        .map(|(field, weight, args, fallback)| GraphqlCostRule {
            field: field.to_string(),
            weight: *weight,
            multipliers: args.iter().map(|s| s.to_string()).collect(),
            default_multiplier: *fallback,
        })
        .collect();
    CompiledGraphqlCost::compile(schema, &rules, 1_000_000, 32).unwrap()
}
fn estimate(model: &CompiledGraphqlCost, query: &str) -> Result<u32, GraphqlError> {
    model.estimate_post(
        serde_json::json!({"query":query}).to_string().as_bytes(),
        MAX_ENVELOPE_BYTES,
    )
}

#[test]
fn aliases_defaults_and_nested_multipliers() {
    let m = model(
        "type Query { users(first: Int = 10): [User] } type User { name: String friends(first: Int): [User] }",
        &[("Query.users", 1, &["first"], 100), ("User.friends", 1, &["first"], 5)],
    );
    assert_eq!(estimate(&m, "{ alias: users { name } }"), Ok(12));
    assert_eq!(
        estimate(&m, "{ users(first: 2) { friends(first: 3) { name } } }"),
        Ok(10)
    );
    assert_eq!(estimate(&m, "{ users(first: 0) { name } }"), Ok(3));
    assert_eq!(
        estimate(&m, "{ users(first: -1) { name } }"),
        Err(GraphqlError::Invalid)
    );
    assert_eq!(
        estimate(&m, "{ users(first: 1.0) { name } }"),
        Err(GraphqlError::Invalid)
    );
}

#[test]
fn leaf_multipliers_are_rejected_but_fixed_weights_remain_chargeable() {
    for return_type in ["Int", "String!", "Money", "E!", "[Int]", "[Money!]!", "[[E!]!]!"] {
        for interface_rule in [false, true] {
            let schema = if interface_rule {
                format!("type Query {{ item: I }} interface I {{ value(first:Int=100):{return_type} }} type C implements I {{ value(first:Int=100):{return_type} }} scalar Money enum E {{ ONE }}")
            } else {
                format!("type Query {{ value(first:Int=100):{return_type} }} scalar Money enum E {{ ONE }}")
            };
            let mut rule = GraphqlCostRule {
                field: if interface_rule { "I.value" } else { "Query.value" }.into(),
                weight: 7,
                multipliers: vec!["first".into()],
                default_multiplier: 100,
            };
            assert!(
                CompiledGraphqlCost::compile(&schema, &[rule.clone()], 1000, 20).is_err(),
                "{return_type}, interface_rule={interface_rule}"
            );
            rule.multipliers.clear();
            let m = CompiledGraphqlCost::compile(&schema, &[rule], 1000, 20).unwrap();
            for arguments in ["", "(first:1)", "(first:1000000)"] {
                let query = if interface_rule {
                    format!("{{item{{value{arguments}}}}}")
                } else {
                    format!("{{value{arguments}}}")
                };
                assert_eq!(estimate(&m, &query), Ok(if interface_rule { 9 } else { 8 }));
            }
        }
    }
}

#[test]
fn composite_return_multipliers_preserve_child_charges() {
    for return_type in ["Leaf", "[I!]!", "[[U!]!]!"] {
        let schema = format!("type Query {{ items(first:Int=2):{return_type} }} interface I {{ name:String }} type Leaf implements I {{ name:String }} union U = Leaf");
        let m = model(&schema, &[("Query.items", 3, &["first"], 100)]);
        assert_eq!(estimate(&m, "{items(first:4){... on Leaf{name}}}"), Ok(8));
        assert_eq!(estimate(&m, "{items{... on Leaf{name}}}"), Ok(6));
    }
}

#[test]
fn abstract_maximum_and_concrete_default_cannot_be_bypassed() {
    let m = model("type Query { item: I } interface I { children(first: Int = 2): [Leaf] } type A implements I { children(first: Int = 7): [Leaf] } type B implements I { children(first: Int = 3): [Leaf] } type Leaf { name: String }", &[("I.children",10,&["first"],100), ("A.children",1,&["first"],100)]);
    assert_eq!(estimate(&m, "{ item { children { name } } }"), Ok(19));
    assert_eq!(estimate(&m, "{ item { ... on A { children { name } } } }"), Ok(19));
    assert_eq!(estimate(&m, "{ item { ... on B { children { name } } } }"), Ok(15));
    let crossed = model("type Query { item: I } interface I { children(a:Int,b:Int): [Leaf] } type A implements I { children(a:Int,b:Int): [Leaf] } type Leaf { name:String }", &[("I.children",100,&["a"],2), ("A.children",1,&["b"],200)]);
    assert_eq!(estimate(&crossed, "{ item { children { name } } }"), Ok(203));
}

#[test]
fn variables_preserve_undefined_null_and_integer_semantics() {
    let m = model(
        "type Query { list(n: Int = 4): [Leaf] } type Leaf { name: String }",
        &[("Query.list", 1, &["n"], 100)],
    );
    assert_eq!(estimate(&m, "query($n:Int){ list(n:$n){name} }"), Ok(6));
    assert_eq!(estimate(&m, "query($n:Int=3){ list(n:$n){name} }"), Ok(5));
    assert_eq!(
        estimate(&m, "query($n:Int!){ list(n:$n){name} }"),
        Err(GraphqlError::Invalid)
    );
    assert_eq!(estimate(&m, "{ list(n:$missing){name} }"), Err(GraphqlError::Invalid));
    for number in ["1.0", "1e0"] {
        let body = format!("{{\"query\":\"query($n:Int){{list(n:$n){{name}}}}\",\"variables\":{{\"n\":{number}}}}}");
        assert_eq!(m.estimate_post(body.as_bytes(), MAX_ENVELOPE_BYTES), Ok(3));
    }
    for value in ["null", "-1", "2147483648", "true", "\"1\"", "[]", "1.5"] {
        let body = format!("{{\"query\":\"query($n:Int){{list(n:$n){{name}}}}\",\"variables\":{{\"n\":{value}}}}}");
        assert_eq!(
            m.estimate_post(body.as_bytes(), MAX_ENVELOPE_BYTES),
            Err(GraphqlError::Invalid)
        );
    }
}

#[test]
fn covariance_preserves_static_fragment_validity_and_runtime_applicability() {
    let m = model("type Query { parent: I } interface I { child: J } type A implements I { child: L } interface J { value: String } type L implements J { value: String } type R implements J { value: String }", &[("L.value",7,&[],100), ("R.value",100,&[],100)]);
    assert_eq!(
        estimate(&m, "{parent{child{... on L {value} ... on R {value}}}}"),
        Ok(10)
    );
    assert_eq!(
        estimate(&m, "{parent{child{... on Query {__typename}}}}"),
        Err(GraphqlError::Invalid)
    );
    for query in [
        "{parent{child{... on R {value @custom}}}}",
        "{parent{child{... on R {missing}}}}",
        "{parent{child{... on R {value(unknown:1)}}}}",
    ] {
        assert_eq!(estimate(&m, query), Err(GraphqlError::Invalid));
    }
    let union = model(
        "type Query { item: U } union U = L | R type L { one:String two:String } type R { three:String }",
        &[("L.one", 3, &[], 100), ("L.two", 5, &[], 100), ("R.three", 7, &[], 100)],
    );
    assert_eq!(estimate(&union, "{item{... on L{one two} ... on R{three}}}"), Ok(10));
}

#[test]
fn operation_selection_and_strict_http_envelopes() {
    let m = model("type Query { name:String } type Mutation { save:Boolean }", &[]);
    assert_eq!(m.estimate_get("query=%7Bname%7D", MAX_ENVELOPE_BYTES), Ok(2));
    assert_eq!(
        m.estimate_get("query=mutation%7Bsave%7D", MAX_ENVELOPE_BYTES),
        Err(GraphqlError::MethodNotAllowed)
    );
    for q in [
        "query=%GG",
        "query=%FF",
        "query=%7Bname%7D&%71uery=%7Bname%7D",
        "query=%7Bname%7D&unknown=1",
    ] {
        assert_eq!(m.estimate_get(q, MAX_ENVELOPE_BYTES), Err(GraphqlError::Invalid));
    }
    assert_eq!(m.estimate_get("query=%7Bname%7D", 3), Err(GraphqlError::TooLarge));
    for body in [
        r#"[{"query":"{name}"}]"#,
        r#"{"query":"{name}","query":"{name}"}"#,
        r#"{"query":"{name}","variables":{"a":1,"a":2}}"#,
        r#"{"query":"{name}","extensions":{"x":1}}"#,
        r#"{"query":"{name}","extra":1}"#,
    ] {
        assert_eq!(
            m.estimate_post(body.as_bytes(), MAX_ENVELOPE_BYTES),
            Err(GraphqlError::Invalid)
        );
    }
    assert_eq!(estimate(&m, "query A{name} query B{name}"), Err(GraphqlError::Invalid));
    let body = br#"{"query":"query A{name} mutation B{save}","operationName":"B"}"#;
    assert_eq!(m.estimate_post(body, MAX_ENVELOPE_BYTES), Ok(2));
    for q in [
        "query A{name} query A{name}",
        "{name} query A{name}",
        "subscription S{name}",
        "type Query{name:String}",
        "{__schema}",
        "{name{ x }}",
        "{missing}",
    ] {
        assert_eq!(estimate(&m, q), Err(GraphqlError::Invalid));
    }
}

#[test]
fn fragments_directives_and_work_are_bounded() {
    let m = model("type Query { name:String next:Query }", &[]);
    assert_eq!(estimate(&m, "{ name @skip(if:true) name @include(if:false) }"), Ok(3));
    assert_eq!(estimate(&m, "{ ...A ...A } fragment A on Query {name}"), Ok(3));
    for q in [
        "{...Missing}",
        "{...A} fragment A on Query {...B} fragment B on Query {...A}",
        "{name @custom}",
        "{... on String {name}}",
        "{name(a:1,a:2)}",
    ] {
        assert_eq!(estimate(&m, q), Err(GraphqlError::Invalid));
    }
    let mut query = "{...F0}".to_string();
    for i in 0..18 {
        query.push_str(&format!(" fragment F{i} on Query {{...F{} ...F{}}}", i + 1, i + 1));
    }
    query.push_str(" fragment F18 on Query {name}");
    assert_eq!(estimate(&m, &query), Err(GraphqlError::Invalid));
    assert_eq!(
        estimate(&m, &format!("{}{{name}}{}", "{next".repeat(33), "}".repeat(33))),
        Err(GraphqlError::Invalid)
    );
}

#[test]
fn independent_depth_cost_and_json_structural_limits() {
    let m = CompiledGraphqlCost::compile("type Query {next: Query name: String}", &[], 10, 2).unwrap();
    assert_eq!(estimate(&m, "{next{name}}"), Ok(3));
    assert_eq!(estimate(&m, "{next{next{name}}}"), Err(GraphqlError::Invalid));
    assert_eq!(
        estimate(
            &m,
            "{...A} fragment A on Query {next{...B}} fragment B on Query {next{name}}"
        ),
        Err(GraphqlError::Invalid)
    );
    assert_eq!(
        estimate(&m, &format!("{{{}}}", "name ".repeat(10))),
        Err(GraphqlError::Invalid)
    );
    let mut body = "{\"query\":\"{name}\",\"variables\":{\"v\":".to_string();
    body.push_str(&"[".repeat(32));
    body.push('0');
    body.push_str(&"]".repeat(32));
    body.push_str("}}");
    assert_eq!(
        m.estimate_post(body.as_bytes(), MAX_ENVELOPE_BYTES),
        Err(GraphqlError::Invalid)
    );
    let values = (0..4096).map(|_| "0").collect::<Vec<_>>().join(",");
    let body = format!("{{\"query\":\"{{name}}\",\"variables\":{{\"v\":[{values}]}}}}");
    assert_eq!(
        m.estimate_post(body.as_bytes(), MAX_ENVELOPE_BYTES),
        Err(GraphqlError::Invalid)
    );
    assert_eq!(
        m.estimate_post(br#"{"query":"{name}"}"#, 3),
        Err(GraphqlError::TooLarge)
    );
    let full_depth = model("type Query {next: Query name:String}", &[]);
    assert_eq!(
        estimate(
            &full_depth,
            &format!("{}{{name}}{}", "{next".repeat(31), "}".repeat(31))
        ),
        Ok(33)
    );
}

#[test]
fn sdl_consistency_and_cost_rule_validation() {
    for schema in [
        "type Query { field:Missing }",
        "type Query { field: Input } input Input{x:Int}",
        "type Query { f:String f:String }",
        "type Query{f:String} extend type Query{g:Int}",
        "type Query{f:String @x}",
        "type Query{f:String} interface I implements J{f:String} interface J{f:String}",
        "type Query{x:I} interface I{f:String!} type A implements I{f:String}",
        "type Query{x:I} interface I{f(n:Int):String} type A implements I{f(n:String):String}",
    ] {
        assert!(
            CompiledGraphqlCost::compile(schema, &[], 10000, 20).is_err(),
            "{schema}"
        );
    }
    assert!(CompiledGraphqlCost::compile("schema { query: Root } type Root{x:String}", &[], 10000, 20).is_ok());
    let rule = GraphqlCostRule {
        field: "Query.x".into(),
        weight: 1,
        multipliers: vec!["n".into()],
        default_multiplier: 100,
    };
    assert!(CompiledGraphqlCost::compile(
        "type Query{x(n:Int=null):Leaf} type Leaf{name:String}",
        &[rule],
        10000,
        20
    )
    .is_err());
}

#[test]
fn parser_caps_and_malformed_grammar_ledger_guard() {
    assert!(std::mem::size_of::<apollo_parser::Error>() <= 64);
    assert!(parse_document(&format!("{{{}}}", "x".repeat(257))).is_err());
    assert!(parse_document(&format!("{{{}}}", "x ".repeat(3000))).is_err());
    for atom in [
        "!", "$", "@", "query", "type", "fragment", "...", "}", "]", "(", "=", ":",
    ] {
        for count in [1, 4, 16, 128, 512] {
            let source = format!("{{ {} }}", atom.repeat(count));
            let tokens = Lexer::new(&source).count();
            let parsed = Parser::new(&source).recursion_limit(64).token_limit(4096).parse();
            let errors = parsed.errors().count();
            use apollo_parser::cst::CstNode;
            let nodes = parsed.document().syntax().descendants().count();
            assert!(errors <= 8 * (tokens + 1), "errors={errors} tokens={tokens}");
            assert!(nodes <= 8 * (tokens + 1), "nodes={nodes} tokens={tokens}");
        }
    }
}

#[test]
fn standard_sdl_metadata_preserves_cost_and_deprecated_selections() {
    let plain = "type Query { users(first:Int=10, required:Int!=1, input:Filter):[User] } interface I { name:String } type User implements I { name:String } input Filter { old:Int defaulted:Int!=1 } enum E { OLD NEW } scalar Date";
    let annotated = r#"type Query { users(first:Int=10 @deprecated, required:Int!=1 @deprecated(reason:""), input:Filter):[User] @deprecated(reason:"Use people") }
        interface I { name:String @deprecated }
        type User implements I { name:String @deprecated(reason:"""Use displayName""") }
        input Filter { old:Int @deprecated defaulted:Int!=1 @deprecated }
        enum E { OLD @deprecated(reason:"Use NEW") NEW }
        scalar Date @specifiedBy(url:"https://example.com/date")"#;
    let rules = [("Query.users", 1, &["first"][..], 100)];
    let before = model(plain, &rules);
    let after = model(annotated, &rules);
    for query in [
        "{users{name}}",
        "{a:users(first:2){...F}} fragment F on User{name}",
        "{users(first:0){name}}",
    ] {
        assert_eq!(estimate(&after, query), estimate(&before, query));
    }
    assert_eq!(estimate(&after, "{users{name}}"), Ok(12));
    assert_eq!(
        estimate(&after, "{users{name @deprecated}}"),
        Err(GraphqlError::Invalid)
    );
    assert!(CompiledGraphqlCost::compile(
        r#"type Query { date:Date } scalar Date @specifiedBy(url:"""https://example.com/date""")"#,
        &[],
        100,
        20
    )
    .is_ok());
}

#[test]
fn sdl_metadata_rejects_invalid_arguments_locations_and_redefinitions() {
    for suffix in [
        "@deprecated(reason:null)",
        "@deprecated(reason:1)",
        "@deprecated(reason:true)",
        "@deprecated(reason:OLD)",
        "@deprecated(reason:[\"old\"])",
        "@deprecated(reason:{x:1})",
        "@deprecated(reason:$x)",
        "@deprecated(other:\"old\")",
        "@deprecated(reason:\"one\",reason:\"two\")",
        "@deprecated @deprecated",
        "@deprecated @unknown",
        "@unknown",
        "@specifiedBy(url:\"https://example.com\")",
    ] {
        for schema in [
            format!("type Query {{ f:Int {suffix} }}"),
            format!("type Query {{ f(n:Int {suffix}):Int }}"),
            format!("type Query {{ f:Int }} input I {{ n:Int {suffix} }}"),
            format!("type Query {{ f:E }} enum E {{ OLD {suffix} }}"),
        ] {
            assert!(CompiledGraphqlCost::compile(&schema, &[], 100, 20).is_err(), "{schema}");
        }
    }
    for suffix in [
        "@specifiedBy",
        "@specifiedBy(url:null)",
        "@specifiedBy(url:1)",
        "@specifiedBy(url:URL)",
        "@specifiedBy(other:\"x\")",
        "@specifiedBy(url:\"x\",url:\"y\")",
        "@specifiedBy(url:\"x\") @specifiedBy(url:\"y\")",
        "@deprecated",
        "@unknown",
    ] {
        let schema = format!("type Query {{ f:Date }} scalar Date {suffix}");
        assert!(CompiledGraphqlCost::compile(&schema, &[], 100, 20).is_err(), "{schema}");
    }
    for schema in [
        "type Query @deprecated { f:Int }",
        "type Query { f:Int } interface I @deprecated { f:Int }",
        "type Query { f:Int } union U @deprecated = Query",
        "type Query { f:Int } input I @deprecated { f:Int }",
        "type Query { f:Int } enum E @deprecated { OLD }",
        "schema @deprecated { query:Query } type Query { f:Int }",
        "type Query { f(n:Int! @deprecated):Int }",
        "type Query { f:Int } input I { n:Int! @deprecated }",
        "type Query { f(n:Int!=null @deprecated):Int }",
        "type Query { f:Int } scalar Int @specifiedBy(url:\"x\")",
        "directive @deprecated(reason:String!) on FIELD_DEFINITION type Query { f:Int }",
        "type Query { f:Int } extend type Query { g:Int @deprecated }",
        "type Query { f:Int @cost(weight:2) }",
        "type Query { f:Int } input I @oneOf { f:Int }",
    ] {
        assert!(CompiledGraphqlCost::compile(schema, &[], 100, 20).is_err(), "{schema}");
    }
}

#[test]
fn sdl_metadata_retains_token_source_and_model_bounds() {
    let reason = "x".repeat(257);
    assert!(CompiledGraphqlCost::compile(
        &format!("type Query {{ f:Int @deprecated(reason:\"{reason}\") }}"),
        &[],
        100,
        20
    )
    .is_err());
    let values = (0..257).map(|n| format!("V{n} @deprecated ")).collect::<String>();
    assert!(CompiledGraphqlCost::compile(&format!("type Query {{ f:E }} enum E {{{values}}}"), &[], 100, 20).is_err());
}

fn quota_report(model: &CompiledGraphqlCost, query: &str) -> Result<GraphqlQuotaCosts, GraphqlError> {
    let bytes = serde_json::json!({"query":query}).to_string();
    model.estimate_quotas(crate::graphql_request::ParsedGraphqlRequest::parse_post(
        bytes.as_bytes(),
        MAX_ENVELOPE_BYTES,
    )?)
}

#[test]
fn root_quotas_use_complete_weighted_maxima_and_conservative_occurrences() {
    let schema = "type Query { users(first:Int=2):[I] scalar:String nested:Query } interface I { value:String } type C implements I { value:String } type D implements I { value:String }";
    let rules = [
        ("Query.users", 3, &["first"][..], 100),
        ("C.value", 2, &[][..], 100),
        ("D.value", 5, &[][..], 100),
        ("Query.scalar", 7, &[][..], 100),
    ];
    let original = model(schema, &rules);
    let bound = model(schema, &rules)
        .with_root_fields(["Query.users", "Query.scalar", "Query.users", "Query.nested"])
        .unwrap();
    let query =
        "{ a:users{value} ...F scalar nested{users{value}} } fragment F on Query { b:users @skip(if:true){value} }";
    let report = quota_report(&bound, query).unwrap();
    assert_eq!(report.total, 48);
    assert_eq!(&report.roots[..4], &[26, 7, 26, 14]);
    assert_eq!(report.roots[4..], [0; 11]);
    assert_eq!(estimate(&original, query), Ok(report.total));
    assert_eq!(report.total - 1, report.roots[0] + report.roots[1] + report.roots[3]);
    let absent = quota_report(&bound, "{ scalar }").unwrap();
    assert_eq!(absent.total, 8);
    assert_eq!(&absent.roots[..4], &[0, 7, 0, 0]);
}

#[test]
fn root_coordinates_support_custom_shared_query_mutation_objects() {
    let schema = "schema { query:Root mutation:Root } type Root { count:Int self:Root }";
    let bound = model(schema, &[])
        .with_root_fields(["Root.count", "Root.self"])
        .unwrap();
    let query = quota_report(&bound, "{count self{count}}").unwrap();
    assert_eq!(query.total, 4);
    assert_eq!(&query.roots[..2], &[1, 2]);
    let mutation = quota_report(&bound, "mutation {count}").unwrap();
    assert_eq!(mutation.total, 2);
    assert_eq!(&mutation.roots[..2], &[1, 0]);
    for invalid in ["Query.count", "Root.absent", "Root.__typename", "Root.count.extra"] {
        assert!(model(schema, &[]).with_root_fields([invalid]).is_err());
    }
    assert!(model(schema, &[]).with_root_fields(["Root.count"; 16]).is_err());
}

#[test]
fn root_reports_retain_existing_variable_and_hard_cost_validation() {
    let schema = "type Query { users(first:Int):[User] } type User { name:String }";
    let m = CompiledGraphqlCost::compile(
        schema,
        &[GraphqlCostRule {
            field: "Query.users".into(),
            weight: 1,
            multipliers: vec!["first".into()],
            default_multiplier: 3,
        }],
        5,
        4,
    )
    .unwrap()
    .with_root_fields(["Query.users"])
    .unwrap();
    assert_eq!(quota_report(&m, "{users(first:3){name}}").unwrap().roots[0], 4);
    assert_eq!(quota_report(&m, "{users(first:4){name}}"), Err(GraphqlError::Invalid));
    assert_eq!(
        quota_report(&m, "query($n:Int!){users(first:$n){name}}"),
        Err(GraphqlError::Invalid)
    );
}

#[test]
fn root_evaluation_does_not_reset_frontend_work() {
    let m = model("type Query {ok:Int}", &[])
        .with_root_fields(["Query.ok"])
        .unwrap();
    let mut parsed =
        crate::graphql_request::ParsedGraphqlRequest::parse_post(br#"{"query":"{ok}"}"#, MAX_ENVELOPE_BYTES).unwrap();
    while parsed.work.step().is_ok() {}
    assert_eq!(m.estimate_quotas(parsed), Err(GraphqlError::Invalid));
}

#[test]
fn cold_rule_bindings_preserve_empty_sparse_and_missing_interface_fields() {
    let schema = "type Query implements I { x:Int y:Int } interface I { y:Int }";
    for rules in [vec![], vec![("I.y", 7, &[][..], 1)]] {
        let m = model(schema, &rules);
        assert_eq!(estimate(&m, "{x}"), Ok(2));
        assert_eq!(estimate(&m, "{x y}"), Ok(if rules.is_empty() { 3 } else { 9 }));
    }
}

#[test]
fn cold_rule_bindings_reject_duplicates_and_support_all_256_rules() {
    let schema = "type Query implements I { x:Int } interface I { x:Int }";
    for coordinate in ["Query.x", "I.x"] {
        let rule = GraphqlCostRule {
            field: coordinate.into(),
            weight: 7,
            multipliers: vec![],
            default_multiplier: 1,
        };
        assert!(CompiledGraphqlCost::compile(schema, &[rule.clone(), rule], 100, 20).is_err());
    }
    let fields = (0..256).map(|i| format!("f{i}:Int ")).collect::<String>();
    let schema = format!("type Query {{{fields}}}");
    let mut rules: Vec<_> = (0..256)
        .map(|i| GraphqlCostRule {
            field: format!("Query.f{i}"),
            weight: i + 1,
            multipliers: vec![],
            default_multiplier: 1,
        })
        .collect();
    let m = CompiledGraphqlCost::compile(&schema, &rules, 1000, 20).unwrap();
    assert_eq!(estimate(&m, "{f0}"), Ok(2));
    assert_eq!(estimate(&m, "{f255}"), Ok(257));
    assert_eq!(estimate(&m, "{f0 f255}"), Ok(258));
    rules.push(rules[0].clone());
    assert!(CompiledGraphqlCost::compile(&schema, &rules, 1000, 20).is_err());
}

#[test]
fn cold_rule_bindings_stay_with_their_immutable_generation() {
    let schema = "type Query { x:Int y:Int }";
    let first = model(schema, &[("Query.x", 3, &[], 1), ("Query.y", 7, &[], 1)]);
    let second = model(schema, &[("Query.y", 11, &[], 1), ("Query.x", 17, &[], 1)]);
    for _ in 0..3 {
        assert_eq!(estimate(&first, "{x}"), Ok(4));
        assert_eq!(estimate(&second, "{x}"), Ok(18));
        assert_eq!(estimate(&first, "{y}"), Ok(8));
        assert_eq!(estimate(&second, "{y}"), Ok(12));
    }
    drop(first);
    assert_eq!(estimate(&second, "{x y}"), Ok(29));
}

fn estimate_with_remaining_work(
    model: &CompiledGraphqlCost,
    query: &str,
    remaining: usize,
) -> Result<GraphqlQuotaCosts, GraphqlError> {
    use crate::graphql_request::ParsedGraphqlRequest;
    let bytes = serde_json::json!({"query":query}).to_string();
    let mut probe = ParsedGraphqlRequest::parse_post(bytes.as_bytes(), MAX_ENVELOPE_BYTES).unwrap();
    let mut available = 0;
    while probe.work.step().is_ok() {
        available += 1;
    }
    assert!(available >= remaining);
    let mut parsed = ParsedGraphqlRequest::parse_post(bytes.as_bytes(), MAX_ENVELOPE_BYTES).unwrap();
    for _ in 0..available - remaining {
        parsed.work.step().unwrap();
    }
    model.estimate_quotas(parsed)
}

#[test]
fn cold_rule_bindings_preserve_exact_candidate_multiplier_and_root_work() {
    let schema = "type Query implements I { x:Int y:Int } interface I { y:Int }";
    for rules in [vec![], vec![("I.y", 7, &[][..], 1)]] {
        let m = model(schema, &rules);
        // One validation, one possible object, one selection, and two candidates,
        // including the interface without x. Baseline succeeds at 5, fails at 4.
        assert_eq!(estimate_with_remaining_work(&m, "{x}", 5).unwrap().total, 2);
        assert_eq!(estimate_with_remaining_work(&m, "{x}", 4), Err(GraphqlError::Invalid));
        let m = m.with_root_fields(["Query.x", "Query.y"]).unwrap();
        // Both matching and missing root slots still charge one step each.
        let report = estimate_with_remaining_work(&m, "{x}", 7).unwrap();
        assert_eq!(report.total, 2);
        assert_eq!(&report.roots[..2], &[1, 0]);
        assert_eq!(estimate_with_remaining_work(&m, "{x}", 6), Err(GraphqlError::Invalid));
    }
    let m = model(
        "type Query implements I { children(n:Int=3):Leaf other:Int } interface I { children(n:Int=2):Leaf } type Leaf { value:Int }",
        &[("Query.children", 1, &["n"], 1), ("I.children", 5, &["n"], 1), ("Leaf.value", 2, &[], 1)],
    ).with_root_fields(["Query.children", "Query.other"]).unwrap();
    // Two validations, two possible objects, two selections, three candidates,
    // two multipliers, two root slots: 13 steps, with concrete default 3.
    let report = estimate_with_remaining_work(&m, "{children{value}}", 13).unwrap();
    assert_eq!(report.total, 12);
    assert_eq!(&report.roots[..2], &[11, 0]);
    assert_eq!(
        estimate_with_remaining_work(&m, "{children{value}}", 12),
        Err(GraphqlError::Invalid)
    );
}

#[test]
fn populated_out_of_range_rule_index_rejects_instead_of_default_charging() {
    for coordinate in ["Query", "I"] {
        let mut m = model(
            "type Query implements I { x:Int } interface I { x:Int }",
            &[("I.x", 7, &[], 1)],
        );
        m.schema
            .types
            .get_mut(coordinate)
            .unwrap()
            .fields
            .get_mut("x")
            .unwrap()
            .rule_index = std::num::NonZeroU16::new(2);
        assert_eq!(estimate(&m, "{x}"), Err(GraphqlError::Invalid));
    }
}

#[test]
fn inline_fragment_type_context_preserves_cost_and_exact_work() {
    let m = model("type Query { x:Int }", &[])
        .with_root_fields(["Query.x"])
        .unwrap();
    for (query, steps) in [
        ("{x}", 5),
        ("{...{x}}", 11),
        ("{... on Query{x}}", 11),
        ("{...{...{x}}}", 17),
        ("{... on Query{...{x}}}", 17),
        ("{...{... on Query{x}}}", 17),
    ] {
        let report = estimate_with_remaining_work(&m, query, steps).unwrap();
        assert_eq!(report.total, 2, "{query}");
        assert_eq!(report.roots[0], 1, "{query}");
        assert!(report.roots[1..].iter().all(|cost| *cost == 0));
        assert_eq!(
            estimate_with_remaining_work(&m, query, steps - 1),
            Err(GraphqlError::Invalid),
            "{query}"
        );
    }
}

#[test]
fn lookup_names_preserve_case_variables_fragments_and_root_charges() {
    let m = model(
        "type Query { users(first:Int=3):User } type User { name:String }",
        &[("Query.users", 2, &["first"], 4)],
    )
    .with_root_fields(["Query.users"])
    .unwrap();
    let query =
        "query Q($count:Int=5){alias:users(first:$count){name} ...F} fragment F on Query{users(first:$count){name}}";
    for (variables, total) in [
        (serde_json::json!({"count":2}), 9),
        (serde_json::json!({}), 15),
        (serde_json::json!({"Count":2}), 15),
    ] {
        let body = serde_json::json!({"query":query,"variables":variables}).to_string();
        let parsed =
            crate::graphql_request::ParsedGraphqlRequest::parse_post(body.as_bytes(), MAX_ENVELOPE_BYTES).unwrap();
        let report = m.estimate_quotas(parsed).unwrap();
        assert_eq!(report.total, total);
        assert_eq!(report.roots[0], total - 1);
        assert!(report.roots[1..].iter().all(|cost| *cost == 0));
    }
    for invalid in [
        query.replace("alias:users", "alias:Users"),
        query.replace("first:", "First:"),
        query.replace("...F", "...f"),
        query.replace("first:$count", "first:$Count"),
        query.replace("{name}", "{Name}"),
    ] {
        assert_eq!(estimate(&m, &invalid), Err(GraphqlError::Invalid), "{invalid}");
    }
}
