//! POST /api/v1/admin/wipe-all — testing-mode only. Deletes every resource
//! from ConfCenter in tier order so downstream caches receive delete events
//! through the normal watch path.
//!
//! Tier 1 (consumers): HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLSRoute,
//!                     EdgionPlugins, EdgionStreamPlugins, BackendTLSPolicy
//! Tier 2 (config):    Gateway, GatewayClass, EdgionGatewayConfig, EdgionTls, EdgionAcme,
//!                     PluginMetaData, LinkSys, ReferenceGrant
//! Tier 3 (low-level): Service, Endpoints, EndpointSlice, Secret, ConfigMap

use crate::core::common::api::ApiResponse;
use crate::core::controller::api::types::AdminState;
use crate::types::ResourceKind;
use axum::{extract::State, response::Json};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Serialize)]
pub struct WipeAllResponse {
    pub deleted: BTreeMap<String, Vec<String>>,
    pub total: usize,
    pub errors: Vec<WipeError>,
    pub duration_ms: u128,
}

#[derive(Serialize)]
pub struct WipeError {
    pub kind: String,
    pub name: String,
    pub reason: String,
}

const TIER_1: &[ResourceKind] = &[
    ResourceKind::HTTPRoute,
    ResourceKind::GRPCRoute,
    ResourceKind::TCPRoute,
    ResourceKind::UDPRoute,
    ResourceKind::TLSRoute,
    ResourceKind::EdgionPlugins,
    ResourceKind::EdgionStreamPlugins,
    ResourceKind::BackendTLSPolicy,
];

const TIER_2: &[ResourceKind] = &[
    ResourceKind::Gateway,
    ResourceKind::GatewayClass,
    ResourceKind::EdgionGatewayConfig,
    ResourceKind::EdgionTls,
    ResourceKind::EdgionAcme,
    ResourceKind::PluginMetaData,
    ResourceKind::LinkSys,
    ResourceKind::ReferenceGrant,
];

const TIER_3: &[ResourceKind] = &[
    ResourceKind::Service,
    ResourceKind::EndpointSlice,
    ResourceKind::Secret,
    ResourceKind::ConfigMap,
];

pub async fn wipe_all(State(state): State<Arc<AdminState>>) -> Json<ApiResponse<WipeAllResponse>> {
    let start = Instant::now();
    tracing::info!(component = "wipe_all", "wipe_all started");
    let mut deleted: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut errors: Vec<WipeError> = Vec::new();

    for tier in [TIER_1, TIER_2, TIER_3] {
        wipe_tier(&state, tier, &mut deleted, &mut errors).await;
    }

    let total: usize = deleted.values().map(|v| v.len()).sum();
    let duration_ms = start.elapsed().as_millis();
    tracing::info!(
        component = "wipe_all",
        total = total,
        errors = errors.len(),
        duration_ms = duration_ms as u64,
        "wipe_all finished"
    );
    Json(ApiResponse::success(WipeAllResponse {
        deleted,
        total,
        errors,
        duration_ms,
    }))
}

async fn wipe_tier(
    state: &AdminState,
    kinds: &[ResourceKind],
    deleted: &mut BTreeMap<String, Vec<String>>,
    errors: &mut Vec<WipeError>,
) {
    let futures: Vec<_> = kinds.iter().map(|k| wipe_kind(state, *k)).collect();
    let results = futures::future::join_all(futures).await;
    for (kind, (kind_deleted, kind_errors)) in kinds.iter().zip(results) {
        deleted
            .entry(kind.as_str().to_string())
            .or_default()
            .extend(kind_deleted);
        errors.extend(kind_errors);
    }
}

async fn wipe_kind(state: &AdminState, kind: ResourceKind) -> (Vec<String>, Vec<WipeError>) {
    let writer = state.conf_center();
    let items = match writer.get_list_by_kind(kind.as_str(), None).await {
        Ok(list) => list.items,
        Err(e) => {
            return (
                Vec::new(),
                vec![WipeError {
                    kind: kind.as_str().to_string(),
                    name: String::new(),
                    reason: format!("list failed: {}", e),
                }],
            );
        }
    };

    let mut deleted = Vec::new();
    let mut errors = Vec::new();

    for item in items {
        let (ns, name) = (item.namespace.clone(), item.name.clone());
        let key = match &ns {
            Some(n) => format!("{}/{}", n, name),
            None => name.clone(),
        };
        match writer.delete_one(kind.as_str(), ns.as_deref(), &name).await {
            Ok(_) => deleted.push(key),
            Err(e) => errors.push(WipeError {
                kind: kind.as_str().to_string(),
                name: key,
                reason: format!("{}", e),
            }),
        }
    }

    (deleted, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Helper: build an AdminState backed by a FileSystemCenter on a temp dir.
    // Does NOT start the lifecycle (no file-watcher, no gRPC server).
    // The CRUD layer (FileSystemStorage) is synchronous I/O and works without
    // the lifecycle being started, so wipe_kind / wipe_all can exercise it.
    // -----------------------------------------------------------------------
    #[cfg(test)]
    async fn make_test_state(tmp_dir: &std::path::Path) -> Arc<AdminState> {
        use crate::core::controller::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

        let fs_config = FileSystemConfig::new(tmp_dir.to_path_buf());
        let conf_mgr = Arc::new(
            ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
                .await
                .expect("ConfMgr::create failed"),
        );
        let schema_validator = Arc::new(SchemaValidator::empty());
        Arc::new(AdminState {
            conf_mgr,
            schema_validator,
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
        let mut all: Vec<ResourceKind> = Vec::new();
        all.extend(TIER_1);
        all.extend(TIER_2);
        all.extend(TIER_3);
        assert_eq!(all.len(), 20, "tiers must cover all 20 ResourceKind variants");
        // Verify no duplicates
        let mut seen = std::collections::HashSet::new();
        for k in &all {
            assert!(seen.insert(*k), "duplicate kind in tiers: {:?}", k);
        }
    }
}
