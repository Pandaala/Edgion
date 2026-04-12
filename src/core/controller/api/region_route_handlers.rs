//! Region Route API handlers
//!
//! GET /api/v1/region-routes
//!
//! Lists all EdgionPlugins resources that contain a RegionRoute plugin,
//! along with the HTTPRoutes that reference each plugin.

use axum::{extract::State, http::StatusCode, response::Json};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::common::api::ApiResponse;
use crate::types::ResourceKind;

use super::types::AdminState;

/// A single RegionRoute plugin instance found in a controller.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionRouteEntry {
    pub namespace: String,
    pub name: String,
    /// HTTPRoutes that reference this EdgionPlugins via extensionRef.
    pub http_routes: Vec<String>,
    pub my_region: String,
    pub regions: serde_json::Value,
    pub key_get: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_key_get: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_calc: Option<serde_json::Value>,
    pub route_rules: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_failover: Option<serde_json::Value>,
}

/// List all RegionRoute plugin instances across all namespaces.
///
/// Returns all EdgionPlugins resources whose spec contains a plugin of type
/// "RegionRoute", together with the HTTPRoutes that reference each plugin
/// via `spec.rules[].filters[].extensionRef`.
pub async fn list_region_routes(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<ApiResponse<Vec<RegionRouteEntry>>>, StatusCode> {
    // Fetch EdgionPlugins and HTTPRoutes in parallel.
    let (plugins_result, routes_result) = tokio::join!(
        state.center_list_resources(ResourceKind::EdgionPlugins, None),
        state.center_list_resources(ResourceKind::HTTPRoute, None),
    );
    let (plugins, _) = plugins_result?;
    let (routes, _) = routes_result?;

    // Build map: "{namespace}/{plugin_name}" -> Vec<route_name>
    let mut plugin_to_routes: HashMap<String, Vec<String>> = HashMap::new();

    let edgion_plugins_kind = ResourceKind::EdgionPlugins.as_str();
    for route in &routes {
        let route_ns = route
            .pointer("/metadata/namespace")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let route_name = route
            .pointer("/metadata/name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if let Some(rules) = route.pointer("/spec/rules").and_then(|r| r.as_array()) {
            for rule in rules {
                if let Some(filters) = rule.get("filters").and_then(|f| f.as_array()) {
                    for filter in filters {
                        if let Some(ext_ref) = filter.get("extensionRef") {
                            let kind = ext_ref
                                .get("kind")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let ref_name = ext_ref
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if kind == edgion_plugins_kind && !ref_name.is_empty() {
                                let key = format!("{}/{}", route_ns, ref_name);
                                plugin_to_routes
                                    .entry(key)
                                    .or_default()
                                    .push(route_name.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    let mut entries = Vec::new();

    for plugin_resource in &plugins {
        let ns = plugin_resource
            .pointer("/metadata/namespace")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name = plugin_resource
            .pointer("/metadata/name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let spec = match plugin_resource.get("spec") {
            Some(s) => s,
            None => continue,
        };

        let config = match find_region_route_config(spec) {
            Some(c) => c,
            None => continue,
        };

        let key = format!("{}/{}", ns, name);
        let http_routes = plugin_to_routes.get(&key).cloned().unwrap_or_default();

        entries.push(RegionRouteEntry {
            namespace: ns.to_string(),
            name: name.to_string(),
            http_routes,
            my_region: config
                .get("myRegion")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            regions: config
                .get("regions")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![])),
            key_get: config
                .get("keyGet")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![])),
            hash_key_get: config.get("hashKeyGet").cloned(),
            hash_calc: config.get("hashCalc").cloned(),
            route_rules: config
                .get("routeRules")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![])),
            auto_failover: config.get("autoFailover").cloned(),
        });
    }

    Ok(Json(ApiResponse::success(entries)))
}

/// Search through plugin arrays in a spec for the first RegionRoute config block.
fn find_region_route_config(spec: &serde_json::Value) -> Option<&serde_json::Value> {
    for arr_name in &["requestPlugins", "responsePlugins"] {
        if let Some(arr) = spec.get(arr_name).and_then(|a| a.as_array()) {
            for item in arr {
                if item.get("type").and_then(|t| t.as_str()) == Some("RegionRoute") {
                    return item.get("config");
                }
            }
        }
    }
    None
}
