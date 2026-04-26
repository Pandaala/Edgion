//! Debug stats endpoint for controller — only mounted in integration testing mode.
//!
//! Used by `cache_residue` validator to assert all in-memory caches are zero
//! after a wipe-all sweep.

use crate::core::common::api::ApiResponse;
use crate::core::controller::api::types::AdminState;
use crate::core::controller::conf_mgr::CenterLifeCycle;
use axum::{extract::State, response::Json};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Serialize)]
pub struct ControllerStoreStats {
    pub conf_center_storage: BTreeMap<String, usize>,
    pub secret_store: usize,
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

    let next_resource_version = crate::core::common::utils::current_resource_version();

    let watch_count = state
        .conf_mgr
        .config_sync_server()
        .map(|s| s.all_kinds().len())
        .unwrap_or(0);

    Json(ApiResponse::success(ControllerStoreStats {
        conf_center_storage,
        secret_store,
        event_store: EventStoreStats { next_resource_version },
        watch_objects: WatchObjectsStats { count: watch_count },
    }))
}

async fn collect_conf_center_counts(state: &AdminState) -> BTreeMap<String, usize> {
    use crate::types::ResourceKind;

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
        ResourceKind::Endpoint,
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
            event_store: EventStoreStats {
                next_resource_version: 1,
            },
            watch_objects: WatchObjectsStats { count: 19 },
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert!(v.get("conf_center_storage").is_some());
        assert_eq!(v.get("secret_store").unwrap().as_u64(), Some(0));
        assert!(v.get("event_store").is_some());
    }
}
