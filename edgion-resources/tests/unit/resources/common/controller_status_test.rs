use super::*;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Local {
    value: String,
}

#[test]
fn serde_preserves_native_and_partitioned_shapes_and_rejects_mixing() {
    let native = ControllerStatus::from(Local { value: "native".into() });
    assert_eq!(
        serde_json::to_value(&native).unwrap(),
        serde_json::json!({"value": "native"})
    );
    assert_eq!(
        serde_json::from_value::<ControllerStatus<Local>>(serde_json::json!({"value": "native"})).unwrap(),
        native
    );

    let partitioned = ControllerStatus::Partitioned(vec![ControllerObservation {
        controller_name: "example.io/controller".into(),
        status: Local {
            value: "partitioned".into(),
        },
    }]);
    assert_eq!(
        serde_json::to_value(&partitioned).unwrap(),
        serde_json::json!({"controllers": [{
            "controllerName": "example.io/controller",
            "status": {"value": "partitioned"}
        }]})
    );
    let mixed = serde_json::from_value::<ControllerStatus<Local>>(serde_json::json!({
        "controllers": [], "value": "mixed"
    }))
    .unwrap();
    assert!(matches!(mixed, ControllerStatus::Opaque(_)));
}

#[test]
fn schema_exposes_only_the_controller_envelope() {
    let schema = schemars::schema_for!(ControllerStatus<Local>);
    let value = serde_json::to_value(schema).unwrap();
    assert!(value["properties"].get("controllers").is_some());
    assert!(value["properties"].get("value").is_none());
}
