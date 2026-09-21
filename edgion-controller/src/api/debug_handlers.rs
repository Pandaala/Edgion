//! Debug stats endpoint for controller — only mounted in integration testing mode.
//!
//! Used by `cache_residue` validator to assert all in-memory caches are zero
//! after a wipe-all sweep.

use crate::api::types::AdminState;
use crate::api::ApiResponse;
use crate::conf_mgr::sync_runtime::resource_processor::credential_source_state::CredentialSourceRegistryStats;
use crate::conf_mgr::sync_runtime::resource_processor::ref_manager::RefManagerStats;
use crate::conf_mgr::CenterLifeCycle;
use axum::{extract::State, response::Json};
use edgion_resources::resource_defs::ALL_RESOURCE_INFOS;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Serialize)]
pub struct SecretRefManagerStats {
    pub source_count: usize,
    pub value_count: usize,
    pub total_references: usize,
}

impl From<RefManagerStats> for SecretRefManagerStats {
    fn from(s: RefManagerStats) -> Self {
        Self {
            source_count: s.source_count,
            value_count: s.value_count,
            total_references: s.total_references,
        }
    }
}

#[derive(Serialize)]
pub struct ControllerStoreStats {
    pub conf_center_storage: BTreeMap<String, usize>,
    pub raw_cache: BTreeMap<String, usize>,
    /// Stable-sorted raw identities, including resources filtered from ServerCache.
    pub raw_cache_keys: BTreeMap<String, Vec<String>>,
    pub secret_store: usize,
    /// Stable-sorted Secret identities only; never includes Secret values.
    pub secret_store_keys: Vec<String>,
    pub secret_ref_manager: SecretRefManagerStats,
    pub credential_source_registry: CredentialSourceRegistryStats,
    /// LinkSys identities with an in-memory generated-secret slot. Counts only.
    pub credential_memory_sources: usize,
    /// Sum of generated-secret entries across those slots. Counts only.
    pub credential_memory_keys: usize,
    pub event_store: EventStoreStats,
    pub watch_objects: WatchObjectsStats,
}

#[derive(Serialize)]
pub struct EventStoreStats {
    pub next_resource_version: u64,
}

#[derive(Serialize)]
pub struct WatchObjectsStats {
    pub count: usize,
}

/// GET /api/v1/debug/store-stats — testing-mode only.
pub async fn store_stats(State(state): State<Arc<AdminState>>) -> Json<ApiResponse<ControllerStoreStats>> {
    let conf_center_storage = collect_conf_center_counts(&state).await;

    let runtime = state.conf_mgr.runtime();
    let (raw_cache, raw_cache_keys) = collect_raw_counts(&runtime);
    let secret_store_keys = runtime.secret_store.keys();
    let secret_store = secret_store_keys.len();
    let secret_ref_manager = SecretRefManagerStats::from(runtime.secret_ref_manager.stats());
    let credential_source_registry = runtime.credential_source_registry.stats();
    let credential_memory_sources = runtime.credential_memory.source_count();
    let credential_memory_keys = runtime.credential_memory.key_count();

    let next_resource_version = crate::conf_mgr::sync_runtime::resource_version::current_resource_version();

    let watch_count = state
        .conf_mgr
        .config_sync_server()
        .map(|s| s.all_kinds().len())
        .unwrap_or(0);

    Json(ApiResponse::ok_body(ControllerStoreStats {
        conf_center_storage,
        raw_cache,
        raw_cache_keys,
        secret_store,
        secret_store_keys,
        secret_ref_manager,
        credential_source_registry,
        credential_memory_sources,
        credential_memory_keys,
        event_store: EventStoreStats { next_resource_version },
        watch_objects: WatchObjectsStats { count: watch_count },
    }))
}

fn collect_raw_counts(
    runtime: &crate::conf_mgr::ControllerRuntime,
) -> (BTreeMap<String, usize>, BTreeMap<String, Vec<String>>) {
    let mut counts = BTreeMap::new();
    let mut identities = BTreeMap::new();
    for kind in runtime.processor_registry.all_kinds() {
        if let Some(processor) = runtime.processor_registry.get(kind) {
            let mut keys = processor.raw_keys();
            keys.sort();
            counts.insert(kind.to_string(), keys.len());
            identities.insert(kind.to_string(), keys);
        }
    }
    (counts, identities)
}

async fn collect_conf_center_counts(state: &AdminState) -> BTreeMap<String, usize> {
    let writer = state.conf_center();
    let mut out = BTreeMap::new();
    for info in ALL_RESOURCE_INFOS {
        let kind = info.kind;
        let count = writer
            .get_list_by_kind(kind.as_str(), None)
            .await
            .map(|r| r.items.len())
            .unwrap_or(0);
        out.insert(kind.as_str().to_string(), count);
    }
    out
}

#[cfg(test)]
mod tests {
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
        let (center_reload_tx, _rx) = tokio::sync::watch::channel(crate::config::CenterClientConfig::default());
        Arc::new(AdminState {
            conf_mgr,
            schema_validator: Arc::new(SchemaValidator::empty()),
            config_file: None,
            authorizer: Arc::new(crate::authz::Authorizer::new(crate::authz::CenterPolicy::deny_all())),
            center_reload_tx,
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
        };
        let v = serde_json::to_value(&resp).unwrap();
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
        let dump = v.to_string();
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
    async fn store_stats_counts_one_credential_memory_insert_then_reset_returns_zero() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = make_test_state(tmp.path()).await;
        let runtime = state.conf_mgr.runtime();
        runtime
            .credential_memory
            .insert_oauth_publication("ops/src/uid", memory_entry("pub-1", b"token-bytes"), 10_000)
            .expect("insert");

        let Json(after_insert) = store_stats(State(state.clone())).await;
        let stats = after_insert.data.expect("ok body");
        assert_eq!(stats.credential_memory_sources, 1);
        assert_eq!(stats.credential_memory_keys, 1);
        let dump = serde_json::to_string(&stats).expect("serialize stats");
        assert!(
            !dump.contains("ops/src/uid") && !dump.contains("pub-1") && !dump.contains("token-bytes"),
            "store-stats must expose counts only, never source identities, secret keys, or values: {dump}"
        );

        runtime.reset();
        let Json(after_reset) = store_stats(State(state)).await;
        let stats = after_reset.data.expect("ok body");
        assert_eq!(stats.credential_memory_sources, 0);
        assert_eq!(stats.credential_memory_keys, 0);
    }
}
