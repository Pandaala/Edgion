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

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::region_route_table::{
    ClusterRegionRouteEntry, ServiceRegionRouteEntry,
};

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
/// several fields across all controllers and reports conflicts.
pub async fn cluster_region_routes_consistency(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let routes = state.metadata_store.list_cluster_routes();
    let mut reports: Vec<ConsistencyReport> = routes
        .into_iter()
        .map(|route| {
            let mut conflicts = Vec::new();

            // my_region is intentionally different per controller — skip it.
            if let Some(c) =
                compare_cluster_field(&route.controllers, "regions.length", |e| e.regions.len().to_string())
            {
                conflicts.push(c);
            }
            if let Some(c) =
                compare_cluster_field(&route.controllers, "key_get.length", |e| e.key_get.len().to_string())
            {
                conflicts.push(c);
            }
            if let Some(c) = compare_cluster_field(&route.controllers, "route_rules.length", |e| {
                e.route_rules.len().to_string()
            }) {
                conflicts.push(c);
            }
            if let Some(c) = compare_cluster_field(&route.controllers, "hash_calc.present", |e| {
                e.hash_calc.is_some().to_string()
            }) {
                conflicts.push(c);
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
/// the number of `regions` and per-region `failover_to` values across all controllers.
/// Returns a list of consistency reports, one per route.
pub async fn service_region_routes_consistency(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let routes = state.metadata_store.list_service_routes();
    let mut reports: Vec<ConsistencyReport> = routes
        .into_iter()
        .map(|route| {
            let mut conflicts = Vec::new();

            if let Some(c) =
                compare_service_field(&route.controllers, "regions.length", |e| e.regions.len().to_string())
            {
                conflicts.push(c);
            }

            // Collect all region names across all controllers
            let all_region_names: std::collections::HashSet<String> = route
                .controllers
                .values()
                .flat_map(|e| e.regions.iter().map(|r| r.name.clone()))
                .collect();

            for region_name in &all_region_names {
                let field = format!("region[{}].failover_to", region_name);
                if let Some(c) = compare_service_field(&route.controllers, &field, |e| {
                    e.regions
                        .iter()
                        .find(|r| &r.name == region_name)
                        .map(|r| r.failover_to.clone().unwrap_or_default())
                        .unwrap_or_else(|| "<absent>".to_string())
                }) {
                    conflicts.push(c);
                }
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
// Helpers
// ---------------------------------------------------------------------------

/// Count the number of distinct values in a `controller_id → value` map.
fn unique_values(map: &HashMap<String, String>) -> usize {
    map.values().collect::<std::collections::HashSet<_>>().len()
}

/// Compare a single field across all ClusterRegionRouteEntry controllers.
/// Returns `Some(ConflictDetail)` if the field differs across controllers, otherwise `None`.
fn compare_cluster_field<F>(
    controllers: &HashMap<String, ClusterRegionRouteEntry>,
    field_name: &str,
    extractor: F,
) -> Option<ConflictDetail>
where
    F: Fn(&ClusterRegionRouteEntry) -> String,
{
    let values: HashMap<String, String> = controllers
        .iter()
        .map(|(cid, entry)| (cid.clone(), extractor(entry)))
        .collect();
    if unique_values(&values) > 1 {
        Some(ConflictDetail {
            field: field_name.to_string(),
            values,
        })
    } else {
        None
    }
}

/// Compare a single field across all ServiceRegionRouteEntry controllers.
/// Returns `Some(ConflictDetail)` if the field differs across controllers, otherwise `None`.
fn compare_service_field<F>(
    controllers: &HashMap<String, ServiceRegionRouteEntry>,
    field_name: &str,
    extractor: F,
) -> Option<ConflictDetail>
where
    F: Fn(&ServiceRegionRouteEntry) -> String,
{
    let values: HashMap<String, String> = controllers
        .iter()
        .map(|(cid, entry)| (cid.clone(), extractor(entry)))
        .collect();
    if unique_values(&values) > 1 {
        Some(ConflictDetail {
            field: field_name.to_string(),
            values,
        })
    } else {
        None
    }
}
