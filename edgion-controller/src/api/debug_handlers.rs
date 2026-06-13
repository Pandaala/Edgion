//! Debug stats endpoint for controller — only mounted in integration testing mode.
//!
//! Used by `cache_residue` validator to assert all in-memory caches are zero
//! after a wipe-all sweep.

use crate::api::types::AdminState;
use crate::api::ApiResponse;
use crate::conf_mgr::sync_runtime::resource_processor::ref_manager::RefManagerStats;
use crate::conf_mgr::CenterLifeCycle;
use axum::{extract::State, response::Json};
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
    pub secret_store: usize,
    pub secret_ref_manager: SecretRefManagerStats,
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
    let secret_store = runtime.secret_store.count();
    let secret_ref_manager = SecretRefManagerStats::from(runtime.secret_ref_manager.stats());

    let next_resource_version = crate::conf_mgr::sync_runtime::resource_version::current_resource_version();

    let watch_count = state
        .conf_mgr
        .config_sync_server()
        .map(|s| s.all_kinds().len())
        .unwrap_or(0);

    Json(ApiResponse::ok_body(ControllerStoreStats {
        conf_center_storage,
        secret_store,
        secret_ref_manager,
        event_store: EventStoreStats { next_resource_version },
        watch_objects: WatchObjectsStats { count: watch_count },
    }))
}

async fn collect_conf_center_counts(state: &AdminState) -> BTreeMap<String, usize> {
    use edgion_resources::ResourceKind;

    let kinds = [
        ResourceKind::HTTPRoute,
        ResourceKind::GRPCRoute,
        ResourceKind::TCPRoute,
        ResourceKind::UDPRoute,
        ResourceKind::TLSRoute,
        ResourceKind::EdgionPlugins,
        ResourceKind::EdgionStreamPlugins,
        ResourceKind::BackendTLSPolicy,
        ResourceKind::Gateway,
        ResourceKind::GatewayClass,
        ResourceKind::EdgionGatewayConfig,
        ResourceKind::EdgionTls,
        ResourceKind::PluginMetaData,
        ResourceKind::LinkSys,
        ResourceKind::ReferenceGrant,
        ResourceKind::Service,
        ResourceKind::EndpointSlice,
        ResourceKind::Secret,
        ResourceKind::ConfigMap,
    ];

    let writer = state.conf_center();
    let mut out = BTreeMap::new();
    for kind in kinds {
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

    #[test]
    fn response_serializes_with_expected_keys() {
        let resp = ControllerStoreStats {
            conf_center_storage: BTreeMap::from_iter([("HTTPRoute".to_string(), 0)]),
            secret_store: 0,
            secret_ref_manager: SecretRefManagerStats {
                source_count: 0,
                value_count: 0,
                total_references: 0,
            },
            event_store: EventStoreStats {
                next_resource_version: 1,
            },
            watch_objects: WatchObjectsStats { count: 19 },
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert!(v.get("conf_center_storage").is_some());
        assert_eq!(v.get("secret_store").unwrap().as_u64(), Some(0));
        assert!(v.get("secret_ref_manager").is_some());
        assert!(v.get("event_store").is_some());
    }
}
