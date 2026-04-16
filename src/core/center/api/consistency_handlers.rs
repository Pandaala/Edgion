//! Consistency detection endpoints for Center.
//!
//! GET /api/v1/center/cluster-region-routes/consistency
//!   Compares ClusterRegionRoute data across all controllers and reports conflicts.
//!
//! GET /api/v1/center/service-region-routes/consistency
//!   Compares ServiceRegionRoute data across all controllers and reports conflicts.

use std::collections::HashMap;

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use super::ApiState;

// ---------------------------------------------------------------------------
// Report DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsistencyReport {
    pub namespace: String,
    pub name: String,
    pub consistent: bool,
    pub controller_count: usize,
    pub conflicts: Vec<ConflictDetail>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictDetail {
    pub field: String,
    pub values: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/v1/center/cluster-region-routes/consistency`
///
/// For each ClusterRegionRoute entry (identified by namespace/name), compares
/// the `my_region` field and the number of `regions` across all controllers.
/// Returns a list of consistency reports, one per route.
pub async fn cluster_region_routes_consistency(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let routes = state.metadata_store.list_cluster_routes();
    let mut reports: Vec<ConsistencyReport> = routes
        .into_iter()
        .map(|route| {
            let mut conflicts = Vec::new();

            // Compare my_region across controllers
            let my_regions: HashMap<String, String> = route
                .controllers
                .iter()
                .map(|(cid, entry)| (cid.clone(), entry.my_region.clone()))
                .collect();
            if unique_values(&my_regions) > 1 {
                conflicts.push(ConflictDetail {
                    field: "my_region".to_string(),
                    values: my_regions,
                });
            }

            // Compare regions count across controllers
            let region_counts: HashMap<String, String> = route
                .controllers
                .iter()
                .map(|(cid, entry)| (cid.clone(), entry.regions.len().to_string()))
                .collect();
            if unique_values(&region_counts) > 1 {
                conflicts.push(ConflictDetail {
                    field: "regions.length".to_string(),
                    values: region_counts,
                });
            }

            ConsistencyReport {
                namespace: route.namespace,
                name: route.name,
                consistent: conflicts.is_empty(),
                controller_count: route.controllers.len(),
                conflicts,
            }
        })
        .collect();

    reports.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));

    Json(serde_json::json!({ "success": true, "data": reports }))
}

/// `GET /api/v1/center/service-region-routes/consistency`
///
/// For each ServiceRegionRoute entry (identified by namespace/name), compares
/// the number of `regions` across all controllers.
/// Returns a list of consistency reports, one per route.
pub async fn service_region_routes_consistency(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let routes = state.metadata_store.list_service_routes();
    let mut reports: Vec<ConsistencyReport> = routes
        .into_iter()
        .map(|route| {
            let mut conflicts = Vec::new();

            // Compare regions count across controllers
            let region_counts: HashMap<String, String> = route
                .controllers
                .iter()
                .map(|(cid, entry)| (cid.clone(), entry.regions.len().to_string()))
                .collect();
            if unique_values(&region_counts) > 1 {
                conflicts.push(ConflictDetail {
                    field: "regions.length".to_string(),
                    values: region_counts,
                });
            }

            ConsistencyReport {
                namespace: route.namespace,
                name: route.name,
                consistent: conflicts.is_empty(),
                controller_count: route.controllers.len(),
                conflicts,
            }
        })
        .collect();

    reports.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));

    Json(serde_json::json!({ "success": true, "data": reports }))
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Count the number of distinct values in a `controller_id → value` map.
fn unique_values(map: &HashMap<String, String>) -> usize {
    map.values()
        .collect::<std::collections::HashSet<_>>()
        .len()
}
