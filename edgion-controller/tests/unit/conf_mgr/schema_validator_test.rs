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

#[test]
fn unsupported_plugin_schema_admits_future_types_in_all_stages() {
    let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
    for stage in [
        "requestPlugins",
        "upstreamResponseFilterPlugins",
        "upstreamResponseBodyFilterPlugins",
        "upstreamResponsePlugins",
    ] {
        let mut value = serde_json::json!({"apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
            "metadata":{"name":"future"}, "spec":{stage:[{"type":"FuturePlugin", "config":{"futureField":true}}]}});
        assert!(
            validator.validate(ResourceKind::EdgionPlugins, &value).is_ok(),
            "{stage}"
        );
        value["spec"][stage][0]["type"] = serde_json::json!(123);
        assert!(validator.validate(ResourceKind::EdgionPlugins, &value).is_err());
    }
}

#[test]
fn unsupported_stream_plugin_schema_admits_future_types() {
    let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
    for stage in ["plugins", "tlsRoutePlugins"] {
        let value = serde_json::json!({"apiVersion":"edgion.io/v1", "kind":"EdgionStreamPlugins",
            "metadata":{"name":"future"}, "spec":{stage:[{"type":"FuturePlugin", "config":{"futureField":true}}]}});
        assert!(validator.validate(ResourceKind::EdgionStreamPlugins, &value).is_ok());
    }
}

#[test]
fn reference_contract_errors_mask_listener_options_keys() {
    let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
    let private_key = format!("credential-{}", "x".repeat(400));
    let value = serde_json::json!({
        "apiVersion":"gateway.networking.k8s.io/v1", "kind":"Gateway",
        "metadata":{"name":"gw", "namespace":"default"},
        "spec":{"gatewayClassName":"edgion", "listeners":[{
            "name":"https", "port":443, "protocol":"HTTPS",
            "tls":{"mode":"Terminate", "options":{private_key.clone(): 42}}
        }]}
    });
    let error = validator
        .validate_gateway_source("gateway.networking.k8s.io", "v1", "Gateway", &value)
        .unwrap_err()
        .to_string();
    assert!(!error.contains(&private_key));
    assert!(!error.contains("credential-"));
    assert!(error.contains("/spec/listeners/0/tls/options/*"), "{error}");
    assert!(error.len() < 1000);
}

#[test]
fn custom_tls_schema_errors_hide_typed_authored_host() {
    let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
    let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
    let raw = serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionTls",
        "metadata": {"name": "edge", "namespace": "default"},
        "spec": {"hosts": ["tenant-internal_host"], "secretRef": {"name": "certificate"}}
    });
    // Admin custom-resource writes serialize the typed document before validation.
    let typed: edgion_resources::EdgionTls = serde_json::from_value(raw).unwrap();
    let mut value = serde_json::to_value(typed).unwrap();
    let error = validator
        .validate(ResourceKind::EdgionTls, &value)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Path: /spec/hosts/0"), "{error}");
    assert!(error.contains("/pattern"), "{error}");
    assert!(!error.contains("tenant-internal_host"), "{error}");
    value["spec"]["hosts"][0] = serde_json::json!("example.com");
    assert!(validator.validate(ResourceKind::EdgionTls, &value).is_ok());
}

#[test]
fn custom_schema_errors_mask_authored_map_keys_and_bound_count() {
    let schema = serde_json::json!({
        "type": "object", "properties": {"spec": {
            "type": "object", "additionalProperties": {"type": "array", "items": {"type": "integer"}}
        }}
    });
    let mut validator = SchemaValidator::empty();
    validator
        .validators
        .insert(ResourceKind::EdgionTls, Validator::new(&schema).unwrap());
    let value = serde_json::json!({"spec": {
        "private-customer-token": ["private-value"], "12345678": ["private-value"]
    }});
    let error = validator
        .validate(ResourceKind::EdgionTls, &value)
        .unwrap_err()
        .to_string();
    assert_eq!(error.matches("Path: /spec/*/0").count(), 2, "{error}");
    for hidden in ["private-customer-token", "12345678", "private-value"] {
        assert!(!error.contains(hidden), "{error}");
    }
    assert!(!error.contains("Additional schema errors omitted"));

    for count in [16, 17, 100] {
        let value = serde_json::json!({"spec": {"private-customer-token": vec!["private-value"; count]}});
        let error = validator
            .validate(ResourceKind::EdgionTls, &value)
            .unwrap_err()
            .to_string();
        assert_eq!(error.matches("Path:").count(), 16, "{error}");
        assert_eq!(error.contains("Additional schema errors omitted"), count > 16);
        assert!(error.contains("Path: /spec/*/15"), "{error}");
        assert!(!error.contains("private-customer-token"));
        assert!(!error.contains("private-value"));
    }
}

#[test]
fn custom_schema_diagnostic_paths_remain_bounded_with_long_unicode_properties() {
    let property = "é".repeat(300);
    let mut schema = serde_json::json!({"type": "integer"});
    let mut value = serde_json::json!("private-value");
    for _ in 0..20 {
        schema = serde_json::json!({"type": "object", "properties": {property.clone(): schema}});
        value = serde_json::json!({property.clone(): value});
    }
    let mut validator = SchemaValidator::empty();
    validator
        .validators
        .insert(ResourceKind::EdgionTls, Validator::new(&schema).unwrap());
    let ValidationError::SchemaViolation(error) = validator.validate(ResourceKind::EdgionTls, &value).unwrap_err();
    let (instance, schema) = error.strip_prefix("Path: ").unwrap().split_once(", Schema: ").unwrap();
    assert!(instance.starts_with("/é"));
    assert!(instance.len() <= 240);
    assert!(instance.matches('/').count() <= 16);
    assert_eq!(schema.chars().count(), 200);
    assert!(schema.len() <= 800);
    assert!(!error.contains("private-value"));
}
