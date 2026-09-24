use super::*;
use std::path::PathBuf;

#[test]
fn gateway_profile_and_duplicate_registration_are_deterministic() {
    assert_eq!(gateway_profile("HTTPRoute", "v1"), GatewaySchemaProfile::Experimental);
    assert_eq!(gateway_profile("GatewayClass", "v1"), GatewaySchemaProfile::Standard);
    assert_eq!(
        gateway_profile("BackendTLSPolicy", "v1alpha3"),
        GatewaySchemaProfile::Experimental
    );
    assert_eq!(
        gateway_profile("GRPCRoute", "v1alpha2"),
        GatewaySchemaProfile::HistoricalGrpc
    );
    let crd = serde_json::json!({
        "kind": "CustomResourceDefinition",
        "spec": {
            "group": GROUP,
            "names": {"kind": "GatewayClass"},
            "versions": [{"name":"v1", "schema":{"openAPIV3Schema":{"type":"object"}}}]
        }
    });
    let mut validators = HashMap::new();
    SchemaValidator::register_gateway_crd(&crd, GatewaySchemaProfile::Experimental, &mut validators).unwrap();
    assert!(validators.is_empty());
    SchemaValidator::register_gateway_crd(&crd, GatewaySchemaProfile::Standard, &mut validators).unwrap();
    assert_eq!(validators.len(), 1);
    assert!(SchemaValidator::register_gateway_crd(&crd, GatewaySchemaProfile::Standard, &mut validators).is_err());
}

#[test]
fn last_pre_v1_source_schemas_load_from_the_selected_artifacts() {
    let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
    for (kind, version) in [
        ("GRPCRoute", "v1alpha2"),
        ("TLSRoute", "v1alpha3"),
        ("BackendTLSPolicy", "v1alpha3"),
        ("TCPRoute", "v1alpha2"),
        ("UDPRoute", "v1alpha2"),
    ] {
        assert!(
            validator
                .gateway_validators
                .contains_key(&(GROUP.to_string(), version.to_string(), kind.to_string())),
            "missing {kind} {version} schema"
        );
    }
    assert!(!validator.gateway_validators.contains_key(&(
        GROUP.to_string(),
        "v1alpha2".to_string(),
        "TLSRoute".to_string()
    )));
    for kind in ["TCPRoute", "UDPRoute"] {
        let route = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1alpha2",
            "kind": kind,
            "metadata": {"name": "route", "namespace": "default"},
            "spec": {"rules": [
                {"backendRefs": [{"name": "first", "port": 8080}]},
                {"backendRefs": [{"name": "second", "port": 8080}]}
            ]}
        });
        assert!(validator
            .validate_gateway_source(GROUP, "v1alpha2", kind, &route)
            .is_ok());
        assert!(validator.validate_gateway_source(GROUP, "v1", kind, &route).is_err());
    }
}

#[test]
fn loaded_registry_fails_closed_and_errors_hide_submitted_values() {
    let mut validator = SchemaValidator::empty();
    let value = serde_json::json!({"spec":{"controllerName":"private-token"}});
    assert!(validator
        .validate_gateway_source(GROUP, "v1", "GatewayClass", &value)
        .is_ok());
    validator.gateway_schemas_loaded = true;
    assert!(validator
        .validate_gateway_source(GROUP, "v1", "GatewayClass", &value)
        .is_err());
    let schema = serde_json::json!({
        "type":"object",
        "properties":{"spec":{"type":"object","properties":{"controllerName":{"enum":["allowed"]}}}}
    });
    validator.gateway_validators.insert(
        (GROUP.to_string(), "v1".to_string(), "GatewayClass".to_string()),
        Validator::new(&schema).unwrap(),
    );
    let error = validator
        .validate_gateway_source(GROUP, "v1", "GatewayClass", &value)
        .unwrap_err();
    assert!(!error.to_string().contains("private-token"));
    assert!(error.to_string().contains("controllerName"));
}

#[test]
fn gateway_schema_error_path_masks_user_map_keys() {
    let mut validator = SchemaValidator::empty();
    validator.gateway_schemas_loaded = true;
    let schema = serde_json::json!({
        "type":"object",
        "properties":{"spec":{"type":"object","additionalProperties":{"type":"integer"}}}
    });
    validator.gateway_validators.insert(
        (GROUP.to_string(), "v1".to_string(), "GatewayClass".to_string()),
        Validator::new(&schema).unwrap(),
    );
    let value = serde_json::json!({"spec":{"private-customer-token":"wrong", "12345678":"wrong"}});
    let error = validator
        .validate_gateway_source(GROUP, "v1", "GatewayClass", &value)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Path: /spec/*"), "{error}");
    assert!(!error.contains("private-customer-token"));
    assert!(!error.contains("12345678"));
    assert!(!error.contains("wrong"));
}

#[test]
fn gateway_schema_error_path_keeps_real_array_indexes() {
    let mut validator = SchemaValidator::empty();
    validator.gateway_schemas_loaded = true;
    let schema = serde_json::json!({
        "type":"object",
        "properties":{"spec":{"type":"object","properties":{"rules":{"type":"array","items":{"type":"integer"}}}}}
    });
    validator.gateway_validators.insert(
        (GROUP.to_string(), "v1".to_string(), "HTTPRoute".to_string()),
        Validator::new(&schema).unwrap(),
    );
    let value = serde_json::json!({"spec":{"rules":["wrong"]}});
    let error = validator
        .validate_gateway_source(GROUP, "v1", "HTTPRoute", &value)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Path: /spec/rules/0"), "{error}");
    assert!(!error.contains("wrong"));
}

#[test]
fn reference_grant_beta_uses_its_own_schema_and_requires_both_rule_lists() {
    let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
    let source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1",
        "kind": "ReferenceGrant",
        "metadata": {"name": "allow", "namespace": "backend"},
        "spec": {
            "from": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": "app"}],
            "to": [{"group": "", "kind": "Service"}]
        }
    });
    assert!(validator
        .validate_gateway_source(GROUP, "v1beta1", "ReferenceGrant", &source)
        .is_ok());
    for missing in ["from", "to"] {
        let mut invalid = source.clone();
        invalid["spec"].as_object_mut().unwrap().remove(missing);
        assert!(validator
            .validate_gateway_source(GROUP, "v1beta1", "ReferenceGrant", &invalid)
            .is_err());
    }
}

#[test]
fn gateway_beta_uses_standard_source_schema() {
    let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
    let source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "Gateway",
        "metadata": {"name": "edge", "namespace": "demo"},
        "spec": {"gatewayClassName": "edgion",
            "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}]}
    });
    assert!(validator
        .validate_gateway_source(GROUP, "v1beta1", "Gateway", &source)
        .is_ok());
    let mut missing = source.clone();
    missing["spec"].as_object_mut().unwrap().remove("gatewayClassName");
    assert!(validator
        .validate_gateway_source(GROUP, "v1beta1", "Gateway", &missing)
        .is_err());
    let mut invalid = source;
    invalid["spec"]["listeners"][0]["port"] = serde_json::json!(0);
    assert!(validator
        .validate_gateway_source(GROUP, "v1beta1", "Gateway", &invalid)
        .is_err());
}

#[test]
fn http_route_beta_uses_experimental_source_schema_and_shared_transform_contract() {
    let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
    let mut source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "HTTPRoute",
        "metadata": {"name": "route", "namespace": "demo"},
        "spec": {"parentRefs": [{"name": "edge"}], "useDefaultGateways": "None",
            "rules": [{"matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
                "filters": [{"type": "RequestRedirect", "requestRedirect": {
                    "path": {"type": "ReplacePrefixMatch", "replacePrefixMatch": "/new"}}}],
                "retry": {"codes": [503]},
                "sessionPersistence": {"type": "Cookie", "sessionName": "route"}}
            ]}
    });
    assert!(validator
        .validate_gateway_source(GROUP, "v1beta1", "HTTPRoute", &source)
        .is_ok());
    let route: edgion_resources::HTTPRoute =
        crate::conf_mgr::conf_center::common::parse_resource_content(&source.to_string()).unwrap();
    assert!(
        edgion_resources::authored_request_transform_violations(&route.spec.rules.as_ref().unwrap()[0])
            .unwrap()
            .is_empty()
    );

    source["spec"]["rules"][0]["filters"][0]["requestRedirect"]["path"]["replacePrefixMatch"] =
        serde_json::json!("relative");
    // JSON Schema does not execute the CRD's CEL expression. The shared
    // transform contract supplies that verdict for every source version.
    assert!(validator
        .validate_gateway_source(GROUP, "v1beta1", "HTTPRoute", &source)
        .is_ok());
    let route: edgion_resources::HTTPRoute =
        crate::conf_mgr::conf_center::common::parse_resource_content(&source.to_string()).unwrap();
    assert!(
        !edgion_resources::authored_request_transform_violations(&route.spec.rules.as_ref().unwrap()[0])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn gateway_class_beta_uses_standard_source_schema() {
    let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
    let source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "GatewayClass",
        "metadata": {"name": "edge"},
        "spec": {"controllerName": "example.com/edge"}
    });
    assert!(validator
        .validate_gateway_source(GROUP, "v1beta1", "GatewayClass", &source)
        .is_ok());
    let mut missing = source.clone();
    missing["spec"].as_object_mut().unwrap().remove("controllerName");
    assert!(validator
        .validate_gateway_source(GROUP, "v1beta1", "GatewayClass", &missing)
        .is_err());
    let mut invalid = source;
    invalid["spec"]["controllerName"] = serde_json::json!(42);
    assert!(validator
        .validate_gateway_source(GROUP, "v1beta1", "GatewayClass", &invalid)
        .is_err());
}

#[test]
fn test_schema_validator_creation() {
    // This test requires the CRD files to be present
    let crd_dir = PathBuf::from("config/crd");
    if !crd_dir.exists() {
        eprintln!("Skipping test: CRD directory not found");
        return;
    }

    let validator = SchemaValidator::from_crd_dir(&crd_dir);
    assert!(validator.is_ok(), "Failed to create validator: {:?}", validator.err());

    let validator = validator.unwrap();
    assert!(validator.schema_count() > 0, "No schemas loaded");
}
