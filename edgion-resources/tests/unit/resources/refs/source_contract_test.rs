use super::*;
use serde_json::json;

#[test]
fn exact_sources_compile_and_default_only_selected_fields() {
    let authored = json!({"spec": {"parentRefs": [{"name":"gateway", "group":null}], "rules":[{"backendRefs":[{"name":"api", "kind":null, "port":80}], "matches":[{"path":{"value":"/unchanged"}}]}]}});
    let view = prepare_gateway_source("gateway.networking.k8s.io", "v1", "HTTPRoute", &authored)
        .unwrap()
        .unwrap();
    assert!(view.violations.is_empty(), "{:?}", view.violations);
    assert_eq!(
        view.normalized["spec"]["parentRefs"][0]["group"],
        "gateway.networking.k8s.io"
    );
    assert_eq!(view.normalized["spec"]["rules"][0]["backendRefs"][0]["kind"], "Service");
    assert_eq!(
        view.normalized["spec"]["rules"][0]["matches"],
        authored["spec"]["rules"][0]["matches"]
    );
    assert!(authored["spec"]["parentRefs"][0]["group"].is_null());
    assert_eq!(CONTRACTS.as_ref().unwrap().len(), 18);
}

#[test]
fn required_empty_group_is_preserved_and_missing_or_null_is_rejected() {
    for group in [json!(null), json!({})] {
        let mut reference = json!({"kind":"ConfigMap", "name":"ca"});
        if !group.is_object() {
            reference["group"] = group;
        }
        let value = json!({"spec":{"targetRefs":[{"group":"", "kind":"Service", "name":"api"}], "validation":{"caCertificateRefs":[reference]}}});
        let view = prepare_gateway_source("gateway.networking.k8s.io", "v1", "BackendTLSPolicy", &value)
            .unwrap()
            .unwrap();
        assert!(!view.violations.is_empty());
    }
    let value = json!({"spec":{"targetRefs":[{"group":"", "kind":"Service", "name":"api"}], "validation":{"caCertificateRefs":[{"group":"", "kind":"ConfigMap", "name":"ca"}]}}});
    let view = prepare_gateway_source("gateway.networking.k8s.io", "v1", "BackendTLSPolicy", &value)
        .unwrap()
        .unwrap();
    assert!(view.violations.is_empty(), "{:?}", view.violations);
    assert_eq!(
        view.normalized["spec"]["validation"]["caCertificateRefs"][0]["group"],
        ""
    );
}

#[test]
fn parent_namespace_escape_and_source_specific_port_rules_are_evaluated() {
    let different_namespaces =
        json!({"spec":{"parentRefs":[{"name":"gw", "namespace":"one"},{"name":"gw", "namespace":"two"}]}});
    let view = prepare_gateway_source("gateway.networking.k8s.io", "v1", "HTTPRoute", &different_namespaces)
        .unwrap()
        .unwrap();
    assert!(view.violations.is_empty(), "{:?}", view.violations);
    let ports = json!({"spec":{"parentRefs":[{"name":"gw", "port":80},{"name":"gw", "port":81}]}});
    let standard = prepare_gateway_source("gateway.networking.k8s.io", "v1", "GRPCRoute", &ports)
        .unwrap()
        .unwrap();
    let historical = prepare_gateway_source("gateway.networking.k8s.io", "v1alpha2", "GRPCRoute", &ports)
        .unwrap()
        .unwrap();
    assert!(!standard.violations.is_empty());
    assert!(historical.violations.is_empty(), "{:?}", historical.violations);
}

#[test]
fn service_port_filter_union_and_grant_empty_name_are_rejected() {
    for value in [
        json!({"spec":{"rules":[{"backendRefs":[{"name":"api"}]}]}}),
        json!({"spec":{"rules":[{"filters":[{"type":"ExtensionRef"}]}]}}),
        json!({"spec":{"rules":[{"backendRefs":[{"name":"api", "port":80,"filters":[{"type":"RequestMirror"}]}]}]}}),
    ] {
        let view = prepare_gateway_source("gateway.networking.k8s.io", "v1", "HTTPRoute", &value)
            .unwrap()
            .unwrap();
        assert!(!view.violations.is_empty());
    }
    let value = json!({"spec":{"from":[{"group":"", "kind":"Service", "namespace":"one"}], "to":[{"group":"", "kind":"Secret", "name":""}]}});
    let view = prepare_gateway_source("gateway.networking.k8s.io", "v1", "ReferenceGrant", &value)
        .unwrap()
        .unwrap();
    assert!(!view.violations.is_empty());
}

#[test]
fn unrelated_large_fields_do_not_consume_reference_projection_budget() {
    let value = json!({"spec":{"other": "x".repeat(300 * 1024), "parentRefs":[{"name":"gw"}]}});
    let view = prepare_gateway_source("gateway.networking.k8s.io", "v1", "HTTPRoute", &value)
        .unwrap()
        .unwrap();
    assert!(view.violations.is_empty());
    assert_eq!(view.normalized["spec"]["other"], value["spec"]["other"]);
}

#[test]
fn reference_projection_rejects_oversized_lists_and_deep_options() {
    let value = json!({"spec":{"parentRefs":vec![json!({"name":"gw"}); 500]}});
    let view = prepare_gateway_source("gateway.networking.k8s.io", "v1", "HTTPRoute", &value)
        .unwrap()
        .unwrap();
    assert!(!view.violations.is_empty());
    assert_eq!(view.normalized["spec"]["parentRefs"].as_array().unwrap().len(), 33);
    let mut deep = json!("leaf");
    for _ in 0..65 {
        deep = json!({"nested":deep});
    }
    let value =
        json!({"spec":{"gatewayClassName":"edgion", "listeners":[{"tls":{"options":{"example.com/deep":deep}}}]}});
    assert!(matches!(
        prepare_gateway_source("gateway.networking.k8s.io", "v1", "Gateway", &value),
        Err(CrdContractError::InputLimit)
    ));
}
