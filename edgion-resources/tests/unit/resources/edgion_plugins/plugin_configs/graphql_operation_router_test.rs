use super::*;
use serde_json::json;

fn config() -> GraphqlOperationRouterConfig {
    serde_json::from_value(json!({"rules":[
        {"operationType":"query","operationName":"GetUser","backendRef":{"name":"users"}},
        {"operationType":"mutation","operationName":"CreateOrder","backendRef":{"name":"orders"}}
    ]}))
    .unwrap()
}
fn rule(backends: serde_json::Value) -> HTTPRouteRule {
    serde_json::from_value(json!({"backendRefs":backends})).unwrap()
}
fn check(r: &HTTPRouteRule, c: &GraphqlOperationRouterConfig) -> Option<String> {
    graphql_router_attachment_error(r, "apps", &[c], false, 1, false)
}

#[test]
fn validates_exact_keys_names_and_bounds() {
    let mut c = config();
    assert!(c.validation_error().is_none());
    c.rules.push(c.rules[0].clone());
    assert!(c.validation_error().is_some());
    c.rules.pop();
    for name in ["", "1Name", "bad-name"] {
        c.rules[0].operation_name = name.into();
        assert!(c.validation_error().is_some());
    }
    c.rules[0].operation_name = "A".repeat(257);
    assert!(c.validation_error().is_some());
    c.rules[0].operation_name = "_valid123".into();
    c.rules[1].operation_name = "_valid123".into();
    assert!(c.validation_error().is_none());
    c.rules = vec![c.rules[0].clone(); 257];
    assert!(c.validation_error().is_some());
    c.rules.clear();
    assert!(c.validation_error().is_some());
}

#[test]
fn every_mapping_and_default_must_belong_to_the_same_rule() {
    let r = rule(json!([{"name":"users","port":8080,"weight":0},{"name":"orders","port":8081}]));
    let mut c = config();
    assert!(check(&r, &c).is_none());
    c.rules[1].backend_ref.name = "existing-but-undeclared".into();
    assert!(check(&r, &c).unwrap().contains("not declared"));
    c = config();
    c.default_backend_ref = Some(DynUpstreamTarget {
        name: "undeclared".into(),
        namespace: None,
    });
    assert!(check(&r, &c).is_some());
    c.default_backend_ref.as_mut().unwrap().name = "users".into();
    assert!(check(&r, &c).is_none());
    let sibling = rule(json!([{"name":"orders","port":8081}]));
    assert!(check(&sibling, &c).is_some());
}

#[test]
fn namespace_uniqueness_delegation_and_conflicts_are_checked() {
    let r = rule(json!([{"name":"users","namespace":"other","port":8080},{"name":"orders","port":8081}]));
    let mut c = config();
    assert!(check(&r, &c).is_some());
    c.rules[0].backend_ref.namespace = Some("other".into());
    assert!(check(&r, &c).is_none());
    let duplicate =
        rule(json!([{"name":"users","port":8080},{"name":"users","port":8081},{"name":"orders","port":8081}]));
    assert!(check(&duplicate, &config()).unwrap().contains("unique"));
    for (delegated, publishers, incompatible) in [(true, 1, false), (false, 2, false), (false, 1, true)] {
        assert!(graphql_router_attachment_error(&r, "apps", &[&c], delegated, publishers, incompatible).is_some());
    }
    assert!(graphql_router_attachment_error(&r, "apps", &[], true, 2, true).is_none());
}

#[test]
fn raw_binding_is_independent_of_effective_composition_and_disabled_sources() {
    let r = rule(json!([{"name":"users","port":8080},{"name":"orders","port":8081}]));
    let mut c = config();
    assert!(graphql_router_binding_error(&r, "apps", &[&c], false).is_none());
    assert!(graphql_router_attachment_error(&r, "apps", &[&c], false, 2, false).is_some());
    c.default_backend_ref = Some(DynUpstreamTarget {
        name: "undeclared".into(),
        namespace: None,
    });
    assert!(graphql_router_binding_error(&r, "apps", &[&c], false).is_some());
    let mut plugins: crate::EdgionPlugins = serde_json::from_value(json!({
        "apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{"name":"router","namespace":"apps"},
        "spec":{"requestPlugins":[{"type":"GraphqlOperationRouter","enable":false,"config":c}]}
    }))
    .unwrap();
    assert!(graphql_router_rule_binding_error(&r, "apps", &[&plugins], true).is_none());
    plugins.spec.request_plugins.as_mut().unwrap()[0].enable = true;
    assert!(graphql_router_rule_binding_error(&r, "apps", &[&plugins], false).is_some());
}
