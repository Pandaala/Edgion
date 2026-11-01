use super::*;
use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

async fn make_fs_state() -> (Arc<AdminState>, tempfile::TempDir) {
    let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
    let fs_config =
        FileSystemConfig::new(tmp_data.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller");
    let conf_mgr = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
            .await
            .expect("ConfMgr::create"),
    );
    let (tx, _rx) = tokio::sync::watch::channel(crate::config::CenterClientConfig::default());
    let state = Arc::new(AdminState {
        conf_mgr,
        schema_validator: Arc::new(SchemaValidator::empty()),
        config_file: None,
        authorizer: Arc::new(crate::authz::Authorizer::new(crate::authz::CenterPolicy::deny_all())),
        center_reload_tx: tx,
    });
    (state, tmp_data)
}

async fn seed_resource(state: &AdminState, body: String) {
    let seed: Value = serde_json::from_str(&body).expect("seed resource JSON");
    let version = seed["metadata"]["resourceVersion"]
        .as_str()
        .expect("seed resource version")
        .parse()
        .expect("numeric seed resource version");
    state
        .conf_center()
        .create_one("EdgionConfigData", Some("default"), "override", body)
        .await
        .expect("seed resource must be created");
    // Direct storage fixtures bypass processor initialization, which restores
    // the persisted version floor before the real admin API accepts writes.
    crate::conf_mgr::sync_runtime::resource_version::bump_resource_version_floor(version);
}

fn override_body(resource_version: &str) -> String {
    serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionConfigData",
        "metadata": {
            "name": "override",
            "namespace": "default",
            "resourceVersion": resource_version
        },
        "spec": {
            "enable": true,
            "visibility": "Namespace",
            "data": {
                "type": "RegionRouteOverride",
                "config": { "regions": [
                    { "name": "east", "hashRange": [0, 499], "backendEndpoint": "east.example.test" },
                    { "name": "west", "hashRange": [500, 999], "backendEndpoint": "west.example.test" }
                ] }
            }
        }
    })
    .to_string()
}

/// A successful failover must leave the stored resource at a *new*
/// resourceVersion. If it wrote the object back at the version it was read
/// at, a client still holding that version could satisfy an `If-Match`
/// precondition afterwards and silently revert the failover.
#[tokio::test]
async fn failover_advances_stored_resource_version() {
    let (state, _tmp) = make_fs_state().await;
    seed_resource(&state, override_body("42")).await;

    let applied =
        patch_edgion_config_data_failover(&state, "default", "override", "RegionRouteOverride", "east", "west")
            .await
            .expect("failover must succeed");
    assert!(applied, "failover must report that it changed the resource");

    let stored = state
        .conf_center()
        .get_one("EdgionConfigData", Some("default"), "override")
        .await
        .expect("resource must remain readable");
    let stored: Value = yaml_serde::from_str(&stored).expect("stored resource must be valid YAML");

    assert_eq!(
        stored
            .pointer("/spec/data/config/regions/0/failoverTo")
            .and_then(Value::as_str),
        Some("west"),
        "failover must have been persisted"
    );
    assert_ne!(
        stored.pointer("/metadata/resourceVersion").and_then(Value::as_str),
        Some("42"),
        "a persisted failover must advance the stored resourceVersion"
    );
}

/// The precondition must still be evaluated against the version the resource
/// was read at, not against the freshly allocated one.
#[tokio::test]
async fn failover_on_stale_stored_version_still_applies() {
    let (state, _tmp) = make_fs_state().await;
    seed_resource(&state, override_body("7")).await;

    let applied =
        patch_edgion_config_data_failover(&state, "default", "override", "RegionRouteOverride", "east", "west")
            .await
            .expect("failover must succeed regardless of the seeded version value");
    assert!(applied);
}

#[tokio::test]
async fn failover_rejects_self_dangling_and_cyclic_targets_without_writing() {
    for target in ["east", "missing", "west"] {
        let (state, _tmp) = make_fs_state().await;
        let mut seed: Value = serde_json::from_str(&override_body("42")).unwrap();
        if target == "west" {
            seed["spec"]["data"]["config"]["regions"][1]["failoverTo"] = Value::String("east".into());
        }
        seed_resource(&state, seed.to_string()).await;
        let result = cluster_edgion_config_data_failover(
            State(state.clone()),
            Json(EdgionConfigDataFailoverRequest {
                namespace: "default".into(),
                name: "override".into(),
                region_name: "east".into(),
                failover_to: target.into(),
            }),
        )
        .await;
        assert_eq!(result.unwrap_err(), StatusCode::UNPROCESSABLE_ENTITY, "target={target}");
        let stored = state
            .conf_center()
            .get_one("EdgionConfigData", Some("default"), "override")
            .await
            .unwrap();
        let stored: Value = yaml_serde::from_str(&stored).unwrap();
        assert_eq!(stored["metadata"]["resourceVersion"], "42");
        assert_eq!(stored["spec"]["data"], seed["spec"]["data"]);
    }
}

#[tokio::test]
async fn failover_rejects_malformed_complete_table_even_when_already_applied() {
    for (field, value) in [
        ("backendEndpoint", serde_json::json!("east.example.test:abc")),
        ("hashRange", serde_json::json!([400, 999])),
    ] {
        let (state, _tmp) = make_fs_state().await;
        let mut seed: Value = serde_json::from_str(&override_body("42")).unwrap();
        seed["spec"]["data"]["config"]["regions"][0][field] = value;
        seed_resource(&state, seed.to_string()).await;
        // Clearing an already absent failover must not approve a broken table.
        let result =
            patch_edgion_config_data_failover(&state, "default", "override", "RegionRouteOverride", "east", "").await;
        assert!(matches!(result, Err(FailoverError::InvalidTable(_))), "field={field}");
    }
}

fn mixed_override_body() -> Value {
    let mut seed: Value = serde_json::from_str(&override_body("42")).unwrap();
    let mut legacy = seed["spec"]["data"].clone();
    legacy["config"]["regions"][0]["failoverTo"] = serde_json::json!("west");
    seed["spec"]["plugins"] = serde_json::json!([legacy]);
    seed["spec"]["unrelated"] = serde_json::json!({"keep": [1, 2]});
    seed
}

#[tokio::test]
async fn mixed_raw_failover_mutates_only_canonical_table_and_preserves_idempotence() {
    let (state, _tmp) = make_fs_state().await;
    let seed = mixed_override_body();
    seed_resource(&state, seed.to_string()).await;
    let mut previous: Value = yaml_serde::from_str(
        &state
            .conf_center()
            .get_one("EdgionConfigData", Some("default"), "override")
            .await
            .unwrap(),
    )
    .unwrap();
    for (target, changed) in [("west", true), ("west", false), ("", true), ("", false)] {
        assert!(patch_edgion_config_data_failover(
            &state,
            "default",
            "override",
            "RegionRouteOverride",
            "east",
            target
        )
        .await
        .unwrap());
        let stored = state
            .conf_center()
            .get_one("EdgionConfigData", Some("default"), "override")
            .await
            .unwrap();
        let stored: Value = yaml_serde::from_str(&stored).unwrap();
        let region = &stored["spec"]["data"]["config"]["regions"][0];
        if target.is_empty() {
            assert!(region.get("failoverTo").is_none());
        } else {
            assert_eq!(region["failoverTo"], target);
        }
        let mut expected_spec = seed["spec"].clone();
        if !target.is_empty() {
            expected_spec["data"]["config"]["regions"][0]["failoverTo"] = serde_json::json!(target);
        }
        assert_eq!(stored["spec"], expected_spec);
        if changed {
            assert_ne!(
                stored["metadata"]["resourceVersion"],
                previous["metadata"]["resourceVersion"]
            );
        } else {
            assert_eq!(stored, previous);
        }
        previous = stored;
    }
}

#[tokio::test]
async fn mixed_raw_failover_rejects_invalid_or_missing_canonical_table_without_writing() {
    for case in [
        "target",
        "idempotent-invalid-table",
        "missing",
        "wrong-type",
        "missing-region",
    ] {
        let (state, _tmp) = make_fs_state().await;
        let mut seed = mixed_override_body();
        let (target, status) = match case {
            "target" => ("unknown", StatusCode::UNPROCESSABLE_ENTITY),
            "idempotent-invalid-table" => {
                seed["spec"]["data"]["config"]["regions"][0]["backendEndpoint"] = serde_json::json!("host:abc");
                ("", StatusCode::UNPROCESSABLE_ENTITY)
            }
            "missing" => {
                seed["spec"].as_object_mut().unwrap().remove("data");
                ("west", StatusCode::NOT_FOUND)
            }
            "wrong-type" => {
                seed["spec"]["data"]["type"] = serde_json::json!("Misc");
                ("west", StatusCode::NOT_FOUND)
            }
            "missing-region" => {
                seed["spec"]["data"]["config"]["regions"][0]["name"] = serde_json::json!("north");
                ("west", StatusCode::NOT_FOUND)
            }
            _ => unreachable!(),
        };
        seed_resource(&state, seed.to_string()).await;
        let before = state
            .conf_center()
            .get_one("EdgionConfigData", Some("default"), "override")
            .await
            .unwrap();
        let result = cluster_edgion_config_data_failover(
            State(state.clone()),
            Json(EdgionConfigDataFailoverRequest {
                namespace: "default".into(),
                name: "override".into(),
                region_name: "east".into(),
                failover_to: target.into(),
            }),
        )
        .await;
        assert_eq!(result.unwrap_err(), status, "case={case}");
        let after = state
            .conf_center()
            .get_one("EdgionConfigData", Some("default"), "override")
            .await
            .unwrap();
        assert_eq!(after, before, "case={case}");
    }
}
