use super::*;

// -----------------------------------------------------------------------
// Helper: build an AdminState backed by a FileSystemCenter on a temp dir.
// Does NOT start the lifecycle (no file-watcher, no gRPC server).
// The CRUD layer (FileSystemStorage) is synchronous I/O and works without
// the lifecycle being started, so wipe_kind / wipe_all can exercise it.
// -----------------------------------------------------------------------
#[cfg(test)]
async fn make_test_state(tmp_dir: &std::path::Path) -> Arc<AdminState> {
    use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

    let fs_config = FileSystemConfig::new(tmp_dir.to_path_buf()).with_controller_name("edgion.io/gateway-controller");
    let conf_mgr = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
            .await
            .expect("ConfMgr::create failed"),
    );
    let schema_validator = Arc::new(SchemaValidator::empty());
    let (tx, _rx) = tokio::sync::watch::channel(crate::config::CenterClientConfig::default());
    let authorizer = std::sync::Arc::new(crate::authz::Authorizer::new(crate::authz::CenterPolicy::deny_all()));
    Arc::new(AdminState {
        conf_mgr,
        schema_validator,
        config_file: None,
        authorizer,
        center_reload_tx: tx,
    })
}

/// Seed a minimal YAML resource into the conf center.
async fn seed_resource(state: &AdminState, kind: &str, namespace: Option<&str>, name: &str) {
    let ns_field = namespace.map(|n| format!("  namespace: {}\n", n)).unwrap_or_default();
    let yaml = format!("apiVersion: v1\nkind: {kind}\nmetadata:\n  name: {name}\n{ns_field}",);
    state
        .conf_center()
        .set_one(kind, namespace, name, yaml)
        .await
        .expect("seed_resource set_one failed");
}

#[tokio::test]
async fn wipe_all_clears_all_resources() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = make_test_state(tmp.path()).await;

    // Seed one resource from each of three different kinds/tiers.
    seed_resource(&state, "HTTPRoute", Some("default"), "route-a").await;
    seed_resource(&state, "Service", Some("default"), "svc-a").await;
    seed_resource(&state, "Secret", Some("default"), "sec-a").await;

    // Execute wipe_all.
    let resp = wipe_all(axum::extract::State(state.clone())).await.0;
    assert_eq!(resp.error, None, "wipe_all returned an API error");
    let data = resp.data.unwrap();
    assert_eq!(data.total, 3, "expected 3 deletions, got {}", data.total);
    assert!(
        data.errors.is_empty(),
        "unexpected wipe errors: {:?}",
        data.errors.iter().map(|e| &e.reason).collect::<Vec<_>>()
    );

    // Verify every seeded kind is now empty in the storage layer.
    let cc = state.conf_center();
    for kind in ["HTTPRoute", "Service", "Secret"] {
        let items = cc
            .get_list_by_kind(kind, None)
            .await
            .expect("get_list_by_kind failed")
            .items;
        assert!(items.is_empty(), "expected {kind} list to be empty after wipe");
    }
}

#[tokio::test]
async fn wipe_drains_conflict_loser() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = make_test_state(tmp.path()).await;
    let root = tmp.path();

    // Winner via the API (canonical path); registry is still empty here.
    seed_resource(&state, "HTTPRoute", Some("default"), "r1").await;
    let winner = root.join("default").join("HTTPRoute_default_r1.yaml");

    // Second file for the SAME key at a non-canonical subdir.
    let yaml = "apiVersion: v1\nkind: HTTPRoute\nmetadata:\n  name: r1\n  namespace: default\n";
    let loser = root.join("legacy").join("HTTPRoute_default_r1.yaml");
    std::fs::create_dir_all(loser.parent().unwrap()).unwrap();
    std::fs::write(&loser, yaml).unwrap();

    // Populate the registry as the watcher would: winner first, loser second.
    let reg = state.conf_dir_registry().expect("fs registry available");
    let key = crate::conf_mgr::conf_center::file_system::ResourceKey::new("HTTPRoute", "default/r1");
    reg.record(key.clone(), winner.clone());
    reg.record(key, loser.clone());
    assert!(winner.exists() && loser.exists(), "both files seeded");

    // Wipe.
    let resp = wipe_all(axum::extract::State(state.clone())).await.0;
    assert_eq!(resp.error, None, "wipe_all returned an API error");

    // Drain loop must remove BOTH files (a single pass would orphan the loser;
    // a non-pending-aware is_recorded_loser would also orphan it).
    assert!(!winner.exists(), "winner must be deleted");
    assert!(!loser.exists(), "loser must be deleted by the drain loop");

    let items = state
        .conf_center()
        .get_list_by_kind("HTTPRoute", None)
        .await
        .expect("get_list_by_kind")
        .items;
    assert!(items.is_empty(), "HTTPRoute must be empty after wipe");
}

#[test]
fn empty_wipe_response_serializes() {
    let resp = WipeAllResponse {
        deleted: BTreeMap::new(),
        total: 0,
        errors: vec![],
        duration_ms: 1,
    };
    let v = serde_json::to_value(&resp).unwrap();
    assert!(v.get("deleted").is_some());
    assert_eq!(v.get("total").unwrap().as_u64(), Some(0));
    assert!(v.get("errors").unwrap().as_array().unwrap().is_empty());
}

#[test]
fn tiers_cover_all_kinds() {
    use edgion_resources::resource_defs::ALL_RESOURCE_INFOS;

    let mut all: Vec<ResourceKind> = Vec::new();
    all.extend(TIER_1);
    all.extend(TIER_2);
    all.extend(TIER_3);
    let actual = all.into_iter().collect::<std::collections::HashSet<_>>();
    let expected = ALL_RESOURCE_INFOS
        .iter()
        .map(|info| info.kind)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        actual.len(),
        TIER_1.len() + TIER_2.len() + TIER_3.len(),
        "duplicate kind in tiers"
    );
    assert_eq!(actual, expected, "wipe tiers must cover every registered resource kind");
}
