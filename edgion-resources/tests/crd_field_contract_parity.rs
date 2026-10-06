//! Selected field contracts, not whole-schema or ingestion-mode equivalence.
//! See skills/01-architecture/05-resources/00-schema-transport-contract.md
//! for the guard's scope and known validation gaps.

use edgion_resources::duration::GEP2257_PATTERN;
use edgion_resources::gwapi_types::{PORT_MAX, PORT_MIN};
use edgion_resources::resources::edgion_backend_traffic_policy::EdgionBackendTrafficPolicySpec;
use edgion_resources::resources::edgion_gateway_config::{
    EdgionGatewayConfigSpec, LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT, MAX_RETRY_ATTEMPTS,
};
use edgion_resources::resources::health_check::ActiveHealthCheckConfig;
use edgion_resources::resources::outlier_detection::OutlierDetectionConfig;
use schemars::{schema_for, JsonSchema};
use serde_json::{json, Map, Value};

const GATEWAY_CRD: &str = include_str!("../../config/crd/edgion-crd/edgion_gateway_config_crd.yaml");
const BACKEND_POLICY_CRD: &str = include_str!("../../config/crd/edgion-crd/edgion_backend_traffic_policy_crd.yaml");

fn served_specs(text: &str) -> Vec<(String, Value)> {
    let crd: Value = serde_yaml::from_str(text).expect("committed CRD must parse");
    let versions = crd["spec"]["versions"].as_array().expect("CRD versions must exist");
    let specs: Vec<_> = versions
        .iter()
        .filter(|version| version["served"] == true)
        .map(|version| {
            let name = version["name"].as_str().expect("version name must exist");
            let spec = &version["schema"]["openAPIV3Schema"]["properties"]["spec"];
            assert!(spec.is_object(), "{name}: spec schema must exist");
            (name.to_string(), spec.clone())
        })
        .collect();
    assert!(!specs.is_empty(), "at least one CRD version must be served");
    specs
}

// Resolve only the shapes emitted for the explicitly selected struct fields.
fn non_null_schema<'a>(mut node: &'a Value, root: &'a Value) -> &'a Value {
    loop {
        if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
            node = root
                .pointer(reference.strip_prefix('#').expect("local schema reference"))
                .expect("referenced schema must exist");
        } else if let Some(branches) = node.get("anyOf").and_then(Value::as_array) {
            node = branches
                .iter()
                .find(|branch| branch["type"] != "null")
                .expect("field must have a non-null branch");
        } else {
            return node;
        }
    }
}

fn field<'a>(root: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(root, |node, name| {
        non_null_schema(node, root)["properties"]
            .get(name)
            .unwrap_or_else(|| panic!("missing schema field {path}"))
    })
}

fn scalar_type(node: &Value) -> &Value {
    match &node["type"] {
        Value::Array(types) => types.iter().find(|kind| **kind != "null").expect("scalar type"),
        kind => kind,
    }
}

fn assert_types<T: JsonSchema>(actual: &Value, paths: &[&str], version: &str) {
    let generated = serde_json::to_value(schema_for!(T)).expect("generated schema must serialize");
    for path in paths {
        let expected = non_null_schema(field(&generated, path), &generated);
        let actual = field(actual, path);
        assert_eq!(scalar_type(expected), &actual["type"], "{version}: type at {path}");
        assert_ne!(actual["type"], Value::Null, "{version}: missing type at {path}");
    }
}

// Construct an explicit specimen from scalar CRD defaults. This does not
// simulate Kubernetes admission, pruning, null handling, or parent defaulting.
fn explicit_defaults(section: &Value) -> Value {
    let properties = section["properties"]
        .as_object()
        .expect("selected section is an object");
    Value::Object(
        properties
            .iter()
            .filter_map(|(name, schema)| schema.get("default").map(|value| (name.clone(), value.clone())))
            .collect::<Map<_, _>>(),
    )
}

fn assert_scalar_defaults(actual: &Value, expected: &Value, paths: &[&str], version: &str) {
    for path in paths {
        let schema = field(actual, path);
        let value = expected.pointer(&format!("/{}", path.replace('.', "/")));
        assert_eq!(schema.get("default"), value, "{version}: default at {path}");
    }
}

fn admits(schema: &Value, value: &Value) -> bool {
    jsonschema::validator_for(schema)
        .expect("selected scalar schema must compile")
        .is_valid(value)
}

#[test]
fn selected_field_types_match_every_served_crd_version() {
    for (version, spec) in served_specs(GATEWAY_CRD) {
        assert_types::<EdgionGatewayConfigSpec>(
            &spec,
            &[
                "retry.attempts",
                "linkSys.webhookMaxResponseBytes",
                "linkSys.maxInstancesPerKind",
                "requestBody.enabled",
                "requestBody.defaultMemoryBufferSize",
                "requestBody.maxMemoryBufferSize",
                "requestBody.defaultMaxBodySize",
                "requestBody.maxBodySize",
                "requestBody.storageOperationTimeout",
            ],
            &version,
        );
    }
    for (version, spec) in served_specs(BACKEND_POLICY_CRD) {
        assert_types::<EdgionBackendTrafficPolicySpec>(
            &spec,
            &[
                "outlierDetection.consecutiveErrors",
                "outlierDetection.consecutiveGatewayErrors",
                "outlierDetection.consecutiveLocalOriginFailures",
                "outlierDetection.ejectionTime",
                "outlierDetection.maxEjectionTime",
                "outlierDetection.maxEjectionPercent",
                "healthCheck.active.port",
            ],
            &version,
        );
    }
}

#[test]
fn gateway_scalar_defaults_match_deserialization_and_preserve_overrides() {
    let omitted: EdgionGatewayConfigSpec = serde_json::from_value(json!({})).expect("omitted sections are valid");
    let empty: EdgionGatewayConfigSpec = serde_json::from_value(json!({
        "retry": {}, "requestBody": {}, "linkSys": {}
    }))
    .expect("empty sections are valid");
    assert_eq!(omitted.retry.attempts, empty.retry.attempts);
    assert_eq!(
        serde_json::to_value(&omitted.request_body).expect("body config serializes"),
        serde_json::to_value(&empty.request_body).expect("body config serializes")
    );
    assert!(omitted.link_sys.is_none());
    let defaults = serde_json::to_value(&empty).expect("defaulted spec serializes");
    for (version, spec) in served_specs(GATEWAY_CRD) {
        assert_scalar_defaults(
            &spec,
            &defaults,
            &[
                "retry.attempts",
                "linkSys.webhookMaxResponseBytes",
                "linkSys.maxInstancesPerKind",
                "requestBody.enabled",
                "requestBody.defaultMemoryBufferSize",
                "requestBody.defaultMaxBodySize",
                "requestBody.storageOperationTimeout",
            ],
            &version,
        );
        for section in ["retry", "requestBody", "linkSys"] {
            assert!(field(&spec, section).get("default").is_none(), "{version}: {section}");
        }
        for path in ["requestBody.maxMemoryBufferSize", "requestBody.maxBodySize"] {
            assert!(
                field(&spec, path).get("default").is_none(),
                "{version}: override at {path}"
            );
        }
        let explicit: EdgionGatewayConfigSpec = serde_json::from_value(json!({
            "retry": explicit_defaults(field(&spec, "retry")),
            "requestBody": explicit_defaults(field(&spec, "requestBody")),
            "linkSys": explicit_defaults(field(&spec, "linkSys"))
        }))
        .expect("CRD defaults must deserialize");
        assert_eq!(
            serde_json::to_value(explicit).expect("spec serializes"),
            defaults,
            "{version}"
        );
    }
    let override_values = json!({
        "retry": {"attempts": 0},
        "linkSys": {"webhookMaxResponseBytes": 1024, "maxInstancesPerKind": 1},
        "requestBody": {"enabled": false, "defaultMemoryBufferSize": "64KiB",
            "maxMemoryBufferSize": "256KiB", "defaultMaxBodySize": "1MiB",
            "maxBodySize": "2MiB", "storageOperationTimeout": "750ms"}
    });
    let explicit: EdgionGatewayConfigSpec = serde_json::from_value(override_values.clone()).expect("valid overrides");
    assert!(explicit.validate_retry().is_empty());
    assert!(explicit.validate_link_sys().is_empty());
    assert!(explicit.validate_request_body().is_empty());
    let serialized = serde_json::to_value(explicit).expect("spec serializes");
    for section in ["retry", "linkSys", "requestBody"] {
        assert_eq!(serialized[section], override_values[section], "explicit {section}");
    }
}

#[test]
fn retry_and_link_sys_capacity_bounds_match_semantic_validation() {
    let generated = serde_json::to_value(schema_for!(EdgionGatewayConfigSpec)).expect("Gateway config schema");
    for (version, spec) in served_specs(GATEWAY_CRD) {
        for (path, maximum) in [
            ("retry.attempts", u64::from(MAX_RETRY_ATTEMPTS)),
            (
                "linkSys.maxInstancesPerKind",
                LINK_SYS_MAX_INSTANCES_PER_KIND_LIMIT as u64,
            ),
        ] {
            let schema = field(&spec, path);
            let minimum = u64::from(path.starts_with("linkSys"));
            for bounds in [schema, non_null_schema(field(&generated, path), &generated)] {
                assert_eq!(bounds["minimum"].as_u64(), Some(minimum), "{version}: {path}");
                assert_eq!(bounds["maximum"].as_u64(), Some(maximum), "{version}: {path}");
            }
            for value in [
                json!(-1),
                json!(0),
                json!(1),
                json!(maximum),
                json!(maximum + 1),
                json!("1"),
                json!(1.5),
            ] {
                let document = if path.starts_with("retry") {
                    json!({"retry": {"attempts": value}})
                } else {
                    json!({"linkSys": {"maxInstancesPerKind": value}})
                };
                let runtime_valid = serde_json::from_value::<EdgionGatewayConfigSpec>(document)
                    .is_ok_and(|config| config.validate_retry().is_empty() && config.validate_link_sys().is_empty());
                assert_eq!(admits(schema, &value), runtime_valid, "{version}: {path} = {value}");
            }
        }
    }
}

#[test]
fn request_body_duration_and_sizes_keep_semantic_validation_boundaries() {
    for (version, spec) in served_specs(GATEWAY_CRD) {
        let timeout = field(&spec, "requestBody.storageOperationTimeout");
        assert_eq!(timeout["pattern"], GEP2257_PATTERN, "{version}");
        for value in [json!("1ms"), json!("750ms"), json!("5s"), json!("invalid"), json!(5)] {
            let valid = serde_json::from_value::<EdgionGatewayConfigSpec>(json!({
                "requestBody": {"storageOperationTimeout": value}
            }))
            .is_ok_and(|config| config.validate_request_body().is_empty());
            assert_eq!(admits(timeout, &value), valid, "{version}: timeout = {value}");
        }
        // The canonical grammar permits zero; the body policy requires a
        // positive deadline. Keep this semantic rule out of the shared regex.
        assert!(admits(timeout, &json!("0s")));
        let zero: EdgionGatewayConfigSpec = serde_json::from_value(json!({
            "requestBody": {"storageOperationTimeout": "0s"}
        }))
        .expect("zero is representable before semantic validation");
        assert!(!zero.validate_request_body().is_empty());
        for (values, expected) in [
            (json!({"maxMemoryBufferSize": "1KiB", "maxBodySize": "1KiB"}), true),
            (json!({"maxMemoryBufferSize": "2KiB", "maxBodySize": "1KiB"}), false),
            (json!({"maxBodySize": "0"}), false),
            (json!({"maxMemoryBufferSize": "invalid"}), false),
        ] {
            assert!(
                admits(field(&spec, "requestBody"), &values),
                "{version}: schema-only input"
            );
            let config: EdgionGatewayConfigSpec = serde_json::from_value(json!({"requestBody": values}))
                .expect("size strings deserialize before semantic validation");
            assert_eq!(
                config.validate_request_body().is_empty(),
                expected,
                "{version}: {values}"
            );
        }
    }
}

#[test]
fn outlier_defaults_preserve_sibling_inheritance_and_duration_resolution() {
    let omitted: EdgionBackendTrafficPolicySpec = serde_json::from_value(json!({
        "targetRefs": [{"kind": "Service", "name": "backend"}]
    }))
    .expect("policy with no outlier detection is valid");
    assert!(omitted.outlier_detection.is_none());
    let empty: OutlierDetectionConfig = serde_json::from_value(json!({})).expect("empty outlier config");
    let defaults = serde_json::to_value(&empty).expect("outlier config serializes");
    for (version, spec) in served_specs(BACKEND_POLICY_CRD) {
        let section = field(&spec, "outlierDetection");
        for name in ["consecutiveErrors", "ejectionTime", "maxEjectionPercent"] {
            assert_eq!(
                section["properties"][name].get("default"),
                defaults.get(name),
                "{version}: {name}"
            );
        }
        assert!(
            section.get("default").is_none(),
            "{version}: absent section stays absent"
        );
        for name in [
            "consecutiveGatewayErrors",
            "consecutiveLocalOriginFailures",
            "maxEjectionTime",
        ] {
            assert!(
                section["properties"][name].get("default").is_none(),
                "{version}: {name} must inherit"
            );
        }
        let explicit: OutlierDetectionConfig =
            serde_json::from_value(explicit_defaults(section)).expect("CRD defaults");
        assert_eq!(explicit.normalize(), empty.clone().normalize(), "{version}");
        for (values, base_seconds, cap_seconds) in [
            (json!({"consecutiveErrors": 9, "ejectionTime": "30s"}), 30, 300),
            (json!({"consecutiveErrors": 9, "ejectionTime": "10m"}), 600, 600),
            (
                json!({"consecutiveErrors": 9, "consecutiveGatewayErrors": 3, "ejectionTime": "10m"}),
                600,
                600,
            ),
            (
                json!({"consecutiveErrors": 9, "consecutiveGatewayErrors": 3,
                "consecutiveLocalOriginFailures": 2, "ejectionTime": "10m", "maxEjectionTime": "1m"}),
                600,
                600,
            ),
        ] {
            assert!(admits(section, &values), "{version}: explicit outlier values");
            let raw: OutlierDetectionConfig = serde_json::from_value(values.clone()).expect("outlier values");
            assert!(raw.get_validation_error().is_none());
            let resolved = raw.normalize();
            assert_eq!(resolved.consecutive_errors, 9);
            assert_eq!(
                resolved.consecutive_gateway_errors,
                values["consecutiveGatewayErrors"].as_u64().unwrap_or(9) as u32
            );
            assert_eq!(
                resolved.consecutive_local_origin_failures,
                values["consecutiveLocalOriginFailures"]
                    .as_u64()
                    .map(|value| value as u32)
                    .unwrap_or(resolved.consecutive_gateway_errors)
            );
            assert_eq!(resolved.ejection_seconds, base_seconds);
            assert_eq!(
                resolved.max_ejection_seconds, cap_seconds,
                "five-minute default, sibling default, and explicit lower-cap clamp"
            );
        }
        for value in ["1s", "500ms", "0s", "invalid"] {
            let raw: OutlierDetectionConfig =
                serde_json::from_value(json!({"ejectionTime": value})).expect("duration string");
            assert_eq!(raw.get_validation_error().is_none(), value == "1s");
            assert_eq!(
                admits(&section["properties"]["ejectionTime"], &json!(value)),
                value != "invalid"
            );
        }
    }
}

#[test]
fn outlier_numeric_limits_are_enforced_by_existing_semantic_validation() {
    for (version, spec) in served_specs(BACKEND_POLICY_CRD) {
        let section = field(&spec, "outlierDetection");
        for name in [
            "consecutiveErrors",
            "consecutiveGatewayErrors",
            "consecutiveLocalOriginFailures",
        ] {
            for value in [0, 1, u32::MAX] {
                let raw: OutlierDetectionConfig =
                    serde_json::from_value(json!({name: value})).expect("unsigned threshold is representable");
                assert_eq!(
                    raw.get_validation_error().is_none(),
                    value > 0,
                    "{version}: {name} = {value}"
                );
            }
        }
        for value in [0, 100, 101] {
            let raw: OutlierDetectionConfig = serde_json::from_value(json!({"maxEjectionPercent": value}))
                .expect("percentage is representable before validation");
            assert_eq!(
                raw.get_validation_error().is_none(),
                value <= 100,
                "{version}: percent = {value}"
            );
        }
        // Semantic rejection remains necessary even if the schema grows
        // stricter later. These assertions do not freeze its current looseness.
        for name in ["consecutiveErrors", "maxEjectionPercent"] {
            assert!(admits(&section["properties"][name], &json!(1)), "{version}: {name}");
        }
    }
}

#[test]
fn health_check_port_schema_uses_canonical_bounds_and_preserves_absence() {
    let generated = serde_json::to_value(schema_for!(ActiveHealthCheckConfig)).expect("health check schema");
    let expected = non_null_schema(field(&generated, "port"), &generated);
    let empty: ActiveHealthCheckConfig = serde_json::from_value(json!({})).expect("empty health check");
    assert!(empty.port.is_none());
    for (version, spec) in served_specs(BACKEND_POLICY_CRD) {
        let port = field(&spec, "healthCheck.active.port");
        assert!(
            port.get("default").is_none(),
            "{version}: port must inherit endpoint port"
        );
        for schema in [port, expected] {
            assert_eq!(schema["minimum"].as_i64(), Some(PORT_MIN), "{version}");
            assert_eq!(schema["maximum"].as_i64(), Some(PORT_MAX), "{version}");
        }
        for value in [PORT_MIN, PORT_MAX] {
            assert!(admits(port, &json!(value)), "{version}: {value}");
            let config: ActiveHealthCheckConfig = serde_json::from_value(json!({"port": value})).expect("valid port");
            assert_eq!(config.port.map(i64::from), Some(value));
        }
        for value in [json!(0), json!(-1), json!(PORT_MAX + 1), json!("80"), json!(80.5)] {
            assert!(!admits(port, &value), "{version}: {value}");
            if value != json!(0) {
                assert!(serde_json::from_value::<ActiveHealthCheckConfig>(json!({"port": value})).is_err());
            }
        }
        // Minimum enforcement on direct file/etcd input remains a separately
        // tracked gap; the u16 representation alone cannot reject port zero.
    }
}

#[test]
fn explicit_null_is_distinct_from_serde_omission() {
    for section in ["retry", "requestBody"] {
        assert!(
            serde_json::from_value::<EdgionGatewayConfigSpec>(json!({section: null})).is_err(),
            "{section}"
        );
    }
    for (section, name) in [
        ("retry", "attempts"),
        ("requestBody", "enabled"),
        ("requestBody", "storageOperationTimeout"),
        ("linkSys", "maxInstancesPerKind"),
    ] {
        assert!(
            serde_json::from_value::<EdgionGatewayConfigSpec>(json!({section: {name: null}})).is_err(),
            "{section}.{name}"
        );
    }
    let optional: EdgionGatewayConfigSpec = serde_json::from_value(json!({
        "linkSys": null, "requestBody": {"maxMemoryBufferSize": null, "maxBodySize": null}
    }))
    .expect("optional serde values accept null");
    assert!(optional.link_sys.is_none());
    assert!(optional.request_body.max_memory_buffer_size.is_none());
    assert!(optional.request_body.max_body_size.is_none());
    // JSON Schema validation does not apply Kubernetes null pruning/defaults.
    for (_, spec) in served_specs(GATEWAY_CRD) {
        for path in [
            "retry.attempts",
            "requestBody.enabled",
            "requestBody.maxBodySize",
            "linkSys.maxInstancesPerKind",
        ] {
            assert!(!admits(field(&spec, path), &Value::Null), "raw schema at {path}");
        }
    }
}
