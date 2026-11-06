use super::*;

#[tokio::test]
async fn raw_diagnostics_include_filtered_objects_and_observe_native_store_wipe() {
    use crate::conf_mgr::sync_runtime::resource_processor::{ConfigMapHandler, KubeRawStore, ResourceProcessor};
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::runtime::{reflector, watcher::Event};
    let runtime = Arc::new(crate::conf_mgr::ControllerRuntime::new());
    let processor = Arc::new(ResourceProcessor::new(
        "ConfigMap",
        10,
        Arc::new(ConfigMapHandler::new()),
        runtime.clone(),
    ));
    processor.set_namespace_filter(vec!["allowed".into()]);
    let (reader, mut writer) = reflector::store();
    processor.set_raw_store(Arc::new(KubeRawStore::new(reader)));
    runtime.processor_registry.register(processor.clone());
    assert_eq!(collect_raw_counts(&runtime).0["ConfigMap"], 0);
    let mut obj = ConfigMap::default();
    obj.metadata.name = Some("filtered".into());
    obj.metadata.namespace = Some("other".into());
    writer.apply_watcher_event(&Event::Init);
    writer.apply_watcher_event(&Event::InitApply(obj.clone()));
    writer.apply_watcher_event(&Event::InitDone);
    processor.on_init_apply(obj, None).await;
    assert!(processor.get("other/filtered").is_none());
    let (counts, keys) = collect_raw_counts(&runtime);
    assert_eq!(counts["ConfigMap"], 1);
    assert_eq!(keys["ConfigMap"], vec!["other/filtered"]);
    writer.apply_watcher_event(&Event::Init);
    writer.apply_watcher_event(&Event::InitDone);
    assert_eq!(collect_raw_counts(&runtime).0["ConfigMap"], 0);
}

async fn make_test_state(tmp_dir: &std::path::Path) -> Arc<AdminState> {
    use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

    let conf_mgr = Arc::new(
        ConfMgr::create(ConfCenterConfig::FileSystem(
            FileSystemConfig::new(tmp_dir.to_path_buf()).with_controller_name("edgion.io/gateway-controller"),
        ))
        .await
        .expect("ConfMgr::create failed"),
    );
    Arc::new(AdminState {
        conf_mgr,
        schema_validator: Arc::new(SchemaValidator::empty()),
    })
}

#[tokio::test]
async fn empty_conf_center_reports_every_resource_kind_as_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = make_test_state(tmp.path()).await;

    let counts = collect_conf_center_counts(&state).await;

    assert_eq!(counts.len(), ALL_RESOURCE_INFOS.len());
    for info in ALL_RESOURCE_INFOS {
        assert_eq!(
            counts.get(info.kind.as_str()),
            Some(&0),
            "missing kind: {:?}",
            info.kind
        );
    }
}

#[test]
fn response_serializes_with_expected_keys() {
    let resp = ControllerStoreStats {
        shared_http_listeners:
            crate::conf_mgr::sync_runtime::resource_processor::listener_port_manager::ListenerPortManager::new()
                .shared_stats(),
        metrics: edgion_common::observe::metrics_api::DynamicMetricStats::default(),
        conf_center_storage: BTreeMap::from_iter([("HTTPRoute".to_string(), 0)]),
        raw_cache: BTreeMap::new(),
        raw_cache_keys: BTreeMap::new(),
        secret_store: 0,
        secret_store_keys: Vec::new(),
        secret_ref_manager: SecretRefManagerStats {
            source_count: 0,
            value_count: 0,
            total_references: 0,
        },
        credential_source_registry: CredentialSourceRegistryStats::default(),
        credential_memory_sources: 0,
        credential_memory_keys: 0,
        event_store: EventStoreStats {
            next_resource_version: 1,
        },
        watch_objects: WatchObjectsStats { count: 19 },
        dns_answer_cache: edgion_common::dns::DnsAnswerCacheStats::default(),
    };
    let v = serde_json::to_value(&resp).unwrap();
    // Residue detection exempts `entries` and requires `in_flight` to reach zero.
    assert_eq!(v["dns_answer_cache"]["entries"], 0);
    assert_eq!(v["dns_answer_cache"]["in_flight"], 0);
    assert!(v["dns_answer_cache"]["in_flight_keys"].is_array());
    assert!(v.get("conf_center_storage").is_some());
    assert!(v.get("raw_cache").is_some());
    assert!(v.get("raw_cache_keys").is_some());
    assert_eq!(v.get("secret_store").unwrap().as_u64(), Some(0));
    assert_eq!(v.get("secret_store_keys").unwrap().as_array().unwrap().len(), 0);
    assert!(v.get("secret_ref_manager").is_some());
    assert!(v.get("credential_source_registry").is_some());
    assert_eq!(v.get("credential_memory_sources").unwrap().as_u64(), Some(0));
    assert_eq!(v.get("credential_memory_keys").unwrap().as_u64(), Some(0));
    assert!(v.get("event_store").is_some());
    // Credential stores expose counts only: no generated-secret entries may be
    // listed. The DNS answer cache's numeric `entries` count is unrelated.
    let mut credential_view = v.clone();
    credential_view.as_object_mut().unwrap().remove("dns_answer_cache");
    let dump = credential_view.to_string();
    assert!(!dump.contains("entries"));
    assert!(!dump.contains("generated"));
}

fn memory_entry(key: &str, token: &[u8]) -> crate::services::credential_manager::memory::GeneratedSecretEntry {
    crate::services::credential_manager::memory::GeneratedSecretEntry {
        key: key.to_string(),
        value: zeroize::Zeroizing::new(token.to_vec()),
        issued_at: "2026-08-27T00:00:00Z".to_string(),
        valid_until: "2026-08-27T01:00:00Z".to_string(),
        valid_until_mono: std::time::Instant::now(),
    }
}

#[tokio::test]
async fn empty_credential_memory_store_stats_reports_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = make_test_state(tmp.path()).await;

    let Json(resp) = store_stats(State(state)).await;
    let stats = resp.data.expect("ok body");
    assert_eq!(stats.credential_memory_sources, 0);
    assert_eq!(stats.credential_memory_keys, 0);
}

#[tokio::test]
async fn store_stats_counts_one_active_credential_after_rotation_then_reset_returns_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = make_test_state(tmp.path()).await;
    let runtime = state.conf_mgr.runtime();
    runtime
        .credential_memory
        .insert_oauth_publication("ops/src/uid", memory_entry("pub-1", b"token-bytes"));
    runtime
        .credential_memory
        .insert_oauth_publication("ops/src/uid", memory_entry("pub-2", b"rotated-token"));

    let Json(after_insert) = store_stats(State(state.clone())).await;
    let stats = after_insert.data.expect("ok body");
    assert_eq!(stats.credential_memory_sources, 1);
    assert_eq!(stats.credential_memory_keys, 1);
    let dump = serde_json::to_string(&stats).expect("serialize stats");
    assert!(
        !dump.contains("ops/src/uid")
            && !dump.contains("pub-1")
            && !dump.contains("pub-2")
            && !dump.contains("token-bytes")
            && !dump.contains("rotated-token"),
        "store-stats must expose counts only, never source identities, secret keys, or values: {dump}"
    );

    runtime.reset();
    let Json(after_reset) = store_stats(State(state)).await;
    let stats = after_reset.data.expect("ok body");
    assert_eq!(stats.credential_memory_sources, 0);
    assert_eq!(stats.credential_memory_keys, 0);
}
