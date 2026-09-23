//! Structural guards for Kubernetes controller-partitioned status envelopes.

use std::path::PathBuf;

use edgion_resources::resources::common::{ConditionsStatus, ControllerObservation, ControllerStatus};
use edgion_resources::resources::edgion_backend::EdgionBackendStatus;
use edgion_resources::resources::edgion_plugins::EdgionPluginsStatus;
use edgion_resources::resources::edgion_stream_plugins::EdgionStreamPluginsStatus;
use edgion_resources::resources::link_sys::{CredentialSourceState, LinkSysLocalStatus, LinkSysStatus};
use schemars::schema_for;
use serde_json::{json, Value};

fn controller() -> ControllerObservation<ConditionsStatus> {
    ControllerObservation {
        controller_name: "example.io/controller".into(),
        status: ConditionsStatus::default(),
    }
}

#[test]
fn condition_statuses_serialize_native_or_envelope_without_mixing() {
    for status in [
        serde_json::to_value(EdgionPluginsStatus::default()).unwrap(),
        serde_json::to_value(EdgionStreamPluginsStatus::default()).unwrap(),
        serde_json::to_value(EdgionBackendStatus::default()).unwrap(),
    ] {
        assert_eq!(status, json!({}));
    }
    let envelope = ControllerStatus::Partitioned(vec![controller()]);
    let encoded = serde_json::to_value(envelope).unwrap();
    assert!(encoded.get("controllers").is_some());
    assert_eq!(encoded.as_object().unwrap().len(), 1);
}

#[test]
fn link_sys_serializes_native_or_envelope_without_mixing() {
    assert_eq!(serde_json::to_value(LinkSysStatus::default()).unwrap(), json!({}));
    let envelope = ControllerStatus::Partitioned(vec![ControllerObservation {
        controller_name: "example.io/controller".into(),
        status: LinkSysLocalStatus::default(),
    }]);
    let encoded = serde_json::to_value(envelope).unwrap();
    assert!(encoded.get("controllers").is_some());
    assert_eq!(encoded.as_object().unwrap().len(), 1);
}

#[test]
fn link_sys_envelope_retains_every_local_lifecycle_field() {
    let local = LinkSysLocalStatus {
        conditions: Vec::new(),
        observed_generation: Some(1),
        credential_state: Some(CredentialSourceState::Valid),
        published_generation: Some("published".into()),
        issued_at: Some("2026-09-21T00:00:00Z".into()),
        valid_until: Some("2026-09-22T00:00:00Z".into()),
        last_successful_refresh_time: Some("2026-09-21T00:00:00Z".into()),
        next_refresh_time: Some("2026-09-21T01:00:00Z".into()),
        desired_consumer_count: Some(1),
        published_consumer_count: Some(2),
        conflicted_consumer_count: Some(3),
    };
    let status = ControllerStatus::Partitioned(vec![ControllerObservation {
        controller_name: "example.io/controller".into(),
        status: local,
    }]);
    let local = &serde_json::to_value(status).unwrap()["controllers"][0]["status"];
    for field in [
        "observedGeneration",
        "credentialState",
        "publishedGeneration",
        "issuedAt",
        "validUntil",
        "lastSuccessfulRefreshTime",
        "nextRefreshTime",
        "desiredConsumerCount",
        "publishedConsumerCount",
        "conflictedConsumerCount",
    ] {
        assert!(local.get(field).is_some(), "missing {field}");
    }
}

#[test]
fn all_status_schemas_expose_only_the_envelope() {
    for schema in [
        serde_json::to_value(schema_for!(EdgionPluginsStatus)).unwrap(),
        serde_json::to_value(schema_for!(EdgionStreamPluginsStatus)).unwrap(),
        serde_json::to_value(schema_for!(EdgionBackendStatus)).unwrap(),
        serde_json::to_value(schema_for!(LinkSysStatus)).unwrap(),
    ] {
        let properties = &schema["properties"];
        assert!(properties.get("controllers").is_some(), "missing controllers: {schema}");
        assert_eq!(
            properties.as_object().unwrap().len(),
            1,
            "native fields leaked: {schema}"
        );
    }
}

#[test]
fn committed_crds_define_bounded_structural_controller_maps() {
    for file in [
        "edgion_plugins_crd.yaml",
        "edgion_stream_plugins_crd.yaml",
        "edgion_backend_crd.yaml",
        "link_sys_crd.yaml",
    ] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../config/crd/edgion-crd")
            .join(file);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {path:?}: {error}"));
        let doc: Value = serde_yaml::from_str(&text).unwrap_or_else(|error| panic!("parse {path:?}: {error}"));
        let controllers = &doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["status"]
            ["properties"]["controllers"];
        assert_eq!(controllers["type"], "array", "{file}");
        assert_eq!(controllers["maxItems"], 32, "{file}");
        assert_eq!(controllers["x-kubernetes-list-type"], "map", "{file}");
        assert_eq!(
            controllers["x-kubernetes-list-map-keys"],
            json!(["controllerName"]),
            "{file}"
        );
        let item = &controllers["items"];
        assert_eq!(item["required"], json!(["controllerName", "status"]), "{file}");
        assert_eq!(item["properties"]["controllerName"]["type"], "string", "{file}");
        assert_eq!(item["properties"]["controllerName"]["minLength"], 1, "{file}");
        assert!(item["properties"].get("gatewayClassName").is_none(), "{file}");
        assert!(item["properties"].get("gatewayClassUid").is_none(), "{file}");
        assert_eq!(item["properties"]["status"]["type"], "object", "{file}");
        let conditions = &item["properties"]["status"]["properties"]["conditions"];
        assert_eq!(conditions["x-kubernetes-list-type"], "map", "{file}");
        assert_eq!(conditions["x-kubernetes-list-map-keys"], json!(["type"]), "{file}");
        assert!(
            conditions["items"]["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field == "type"),
            "{file}"
        );
    }
}
