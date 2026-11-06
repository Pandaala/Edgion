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
use edgion_common::dns::{egress_answer_cache, DnsAnswerCacheStats};
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
    pub shared_http_listeners:
        crate::conf_mgr::sync_runtime::resource_processor::listener_port_manager::SharedListenerStats,
    pub metrics: edgion_common::observe::metrics_api::DynamicMetricStats,
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
    /// Active generated-secret entries, one per stored source identity. Counts only.
    pub credential_memory_keys: usize,
    pub event_store: EventStoreStats,
    pub watch_objects: WatchObjectsStats,
    /// Egress DNS answer cache. `in_flight` must return to zero once requests
    /// settle; `entries` is capacity-bounded and exempt from residue.
    pub dns_answer_cache: DnsAnswerCacheStats,
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
        shared_http_listeners: runtime.listener_port_manager.shared_stats(),
        metrics: edgion_common::observe::metrics_api::metric_store_stats(),
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
        dns_answer_cache: egress_answer_cache().stats(),
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
#[path = "../../tests/unit/api/debug_handlers_test.rs"]
mod tests;
