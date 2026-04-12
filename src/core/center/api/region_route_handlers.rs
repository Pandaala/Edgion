//! Center aggregation endpoints for RegionRoute data.
//!
//! These handlers call ALL online controllers in parallel and aggregate
//! region-route information across the federation.

use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::ApiState;
use crate::core::common::api::ListResponse;

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// One entry in the controller's `GET /api/v1/region-routes` response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControllerRegionRouteKey {
    service_group: String,
    cluster: String,
    namespace: String,
    kind: String,
    resources: Vec<String>,
}

/// Wrapper matching the controller's JSON envelope `{ success, data }`.
#[derive(Debug, Deserialize)]
struct ControllerListResponse {
    success: bool,
    data: Option<Vec<ControllerRegionRouteKey>>,
}

/// Wrapper for the controller's detail response `{ success, data }`.
#[derive(Debug, Deserialize)]
struct ControllerDetailResponse {
    success: bool,
    data: Option<Vec<serde_json::Value>>,
}

/// Per-controller contribution to an aggregated key.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ControllerRouteRef {
    controller_id: String,
    resources: Vec<String>,
}

/// Aggregated region-route key across controllers.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CenterRegionRouteKey {
    service_group: String,
    cluster: String,
    namespace: String,
    kind: String,
    controllers: Vec<ControllerRouteRef>,
}

/// Per-controller detail for a specific (serviceGroup, cluster, namespace).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CenterRegionRouteDetail {
    controller_id: String,
    entries: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RegionRouteQuery {
    service_group: Option<String>,
    cluster: Option<String>,
    namespace: Option<String>,
    kind: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Forward a GET request to all online controllers in parallel, parse each
/// response as `T`, and return `(controller_id, parsed)` pairs.
/// Controllers that fail or return unparseable responses are skipped with a warning.
async fn forward_to_all_online<T: serde::de::DeserializeOwned>(
    state: &ApiState,
    path: String,
    endpoint_label: &str,
) -> Vec<(String, T)> {
    let summaries = state.aggregator.controller_summaries();
    let online: Vec<_> = summaries.into_iter().filter(|s| s.online).collect();

    let futs = online.iter().map(|s| {
        let proxy = state.proxy.clone();
        let controller_id = s.controller_id.clone();
        let path = path.clone();
        async move {
            let result = proxy
                .forward(&controller_id, "GET".to_string(), path, HashMap::new(), vec![])
                .await;
            (controller_id, result)
        }
    });

    let results = futures::future::join_all(futs).await;
    let mut parsed_results = Vec::new();

    for (controller_id, result) in results {
        let resp = match result {
            Ok(r) if r.status_code == 200 => r,
            Ok(r) => {
                tracing::warn!(
                    component = "center", controller_id = %controller_id,
                    status = r.status_code,
                    "Non-OK response for {} from controller, skipping", endpoint_label
                );
                continue;
            }
            Err((status, msg)) => {
                tracing::warn!(
                    component = "center", controller_id = %controller_id,
                    status = %status, error = %msg,
                    "Failed to fetch {} from controller, skipping", endpoint_label
                );
                continue;
            }
        };

        match serde_json::from_slice::<T>(&resp.body) {
            Ok(v) => parsed_results.push((controller_id, v)),
            Err(e) => {
                tracing::warn!(
                    component = "center", controller_id = %controller_id, error = %e,
                    "Failed to parse {} response from controller, skipping", endpoint_label
                );
            }
        }
    }

    parsed_results
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/v1/center/region-routes`
///
/// Calls every online controller's `GET /api/v1/region-routes`, then merges
/// results by `(serviceGroup, cluster, namespace)`.
pub async fn list_center_region_routes(
    State(state): State<ApiState>,
    Query(params): Query<RegionRouteQuery>,
) -> impl IntoResponse {
    let responses: Vec<(String, ControllerListResponse)> =
        forward_to_all_online(&state, "/api/v1/region-routes".to_string(), "region-routes").await;

    // Aggregate: (serviceGroup, cluster, namespace, kind) → Vec<ControllerRouteRef>
    let mut map: HashMap<(String, String, String, String), Vec<ControllerRouteRef>> = HashMap::new();

    for (controller_id, parsed) in responses {
        if !parsed.success {
            continue;
        }
        if let Some(keys) = parsed.data {
            for key in keys {
                let group_key = (key.service_group.clone(), key.cluster.clone(), key.namespace.clone(), key.kind.clone());
                map.entry(group_key).or_default().push(ControllerRouteRef {
                    controller_id: controller_id.clone(),
                    resources: key.resources,
                });
            }
        }
    }

    // Build result list and apply optional filters in a single pass.
    let mut items: Vec<CenterRegionRouteKey> = map
        .into_iter()
        .filter(|((sg, cl, ns, k), _)| {
            params.service_group.as_ref().map_or(true, |f| sg == f)
                && params.cluster.as_ref().map_or(true, |f| cl == f)
                && params.namespace.as_ref().map_or(true, |f| ns == f)
                && params.kind.as_ref().map_or(true, |f| k == f)
        })
        .map(|((sg, cl, ns, k), controllers)| CenterRegionRouteKey {
            service_group: sg,
            cluster: cl,
            namespace: ns,
            kind: k,
            controllers,
        })
        .collect();

    items.sort_by(|a, b| {
        (&a.service_group, &a.cluster, &a.namespace, &a.kind)
            .cmp(&(&b.service_group, &b.cluster, &b.namespace, &b.kind))
    });

    Json(ListResponse::success(items))
}

/// `GET /api/v1/center/region-routes/{service_group}/{cluster}/{namespace}`
///
/// Calls every online controller's detail endpoint and returns per-controller
/// entries.
pub async fn get_center_region_route_detail(
    State(state): State<ApiState>,
    Path((service_group, cluster, namespace, kind)): Path<(String, String, String, String)>,
) -> impl IntoResponse {
    let path = format!("/api/v1/region-routes/{}/{}/{}/{}", service_group, cluster, namespace, kind);

    let responses: Vec<(String, ControllerDetailResponse)> =
        forward_to_all_online(&state, path, "region-route detail").await;

    let mut details: Vec<CenterRegionRouteDetail> = responses
        .into_iter()
        .filter(|(_, parsed)| parsed.success)
        .map(|(controller_id, parsed)| CenterRegionRouteDetail {
            controller_id,
            entries: parsed.data.unwrap_or_default(),
        })
        .collect();

    details.sort_by(|a, b| a.controller_id.cmp(&b.controller_id));

    Json(ListResponse::success(details))
}
