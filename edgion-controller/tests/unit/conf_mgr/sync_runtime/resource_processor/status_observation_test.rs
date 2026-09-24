use super::*;

fn identity() -> ObserverIdentity {
    ObserverIdentity {
        controller_name: "example/controller".into(),
    }
}

#[test]
fn projects_only_the_controller_without_flat_fallback() {
    let status = json!({"controllers": [
        {"controllerName": "foreign/controller", "status": {"conditions": []}},
        {"controllerName": "example/controller", "status": {"conditions": []}}
    ]});
    assert_eq!(
        project_status("EdgionPlugins", &status, &identity()).unwrap(),
        Some(json!({"conditions": []}))
    );
    assert_eq!(
        project_status("EdgionPlugins", &json!({"conditions": []}), &identity()).unwrap(),
        None
    );
    assert!(project_status("EdgionPlugins", &json!({"unexpected": true}), &identity()).is_err());
}

#[test]
fn merge_preserves_order_and_foreign_entries() {
    let existing = json!({"controllers": [
        {"controllerName": "foreign", "status": {"value": 1}},
        {"controllerName": "example/controller", "status": {"value": 2}}
    ]});
    let merged = merge_status("EdgionPlugins", Some(&existing), &json!({"value": 3}), &identity())
        .unwrap()
        .unwrap();
    assert_eq!(merged["controllers"][0], existing["controllers"][0]);
    assert_eq!(merged["controllers"][1]["status"]["value"], 3);
}

#[test]
fn capacity_never_evicts_foreign_observations_but_existing_owner_can_update() {
    let mut entries: Vec<Value> = (0..MAX_CONTROLLER_OBSERVATIONS)
        .map(|index| json!({"controllerName": format!("foreign/{index}"), "status": {}}))
        .collect();
    let existing = json!({"controllers": entries});
    assert!(merge_status("EdgionPlugins", Some(&existing), &json!({}), &identity()).is_err());
    entries[0] = json!({"controllerName": "example/controller", "status": {}});
    let existing_owner = json!({"controllers": entries});
    let updated = merge_status(
        "EdgionPlugins",
        Some(&existing_owner),
        &json!({"conditions": []}),
        &identity(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        updated["controllers"].as_array().unwrap().len(),
        MAX_CONTROLLER_OBSERVATIONS
    );
    assert_eq!(&updated["controllers"].as_array().unwrap()[1..], &entries[1..]);
    assert_eq!(updated["controllers"][0]["status"], json!({"conditions": []}));
    entries.push(json!({"controllerName": "foreign/over-cap", "status": {}}));
    assert!(merge_status(
        "EdgionPlugins",
        Some(&json!({"controllers": entries})),
        &json!({}),
        &identity()
    )
    .is_err());
}

#[test]
fn malformed_duplicate_and_retired_observations_are_rejected() {
    for entries in [
        json!([{"controllerName": "example/controller", "gatewayClassName": "old", "gatewayClassUid": "uid", "status": {}}]),
        json!([{"controllerName": "example/controller", "extra": true, "status": {}}]),
        json!([{"controllerName": "example/controller", "status": {}}, {"controllerName": "example/controller", "status": {}}]),
        json!([{"controllerName": "", "status": {}}]),
        json!([{"controllerName": "foreign", "status": null}]),
    ] {
        let status = json!({"controllers": entries});
        assert!(project_status("EdgionPlugins", &status, &identity()).is_err());
        assert!(merge_status("EdgionPlugins", Some(&status), &json!({}), &identity()).is_err());
    }
}

#[test]
fn native_status_is_retired_but_never_projected() {
    let native = json!({"conditions": []});
    assert_eq!(retired_native_fields("EdgionPlugins", &native).unwrap(), ["conditions"]);
    assert_eq!(retired_native_fields("LinkSys", &json!({})).unwrap().len(), 11);
    assert_eq!(
        merge_status("EdgionPlugins", Some(&native), &json!({}), &identity())
            .unwrap()
            .unwrap(),
        json!({"controllers": [{"controllerName": "example/controller", "status": {}}]})
    );
}
