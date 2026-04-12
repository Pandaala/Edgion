//! Region Route API handlers
//!
//! GET /api/v1/region-routes
//!   Lists RegionRoute plugin instances grouped by (serviceGroup, cluster, namespace, kind).
//!
//! GET /api/v1/region-routes/{service_group}/{cluster}/{namespace}/{kind}
//!   Returns full RegionRoute config detail for all matching resources.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::common::api::ApiResponse;
use crate::types::ResourceKind;

use super::types::AdminState;

// ============= Types =============

/// A grouped key representing a unique (serviceGroup, cluster, namespace, kind) tuple.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionRouteKey {
    pub service_group: String,
    pub cluster: String,
    pub namespace: String,
    pub kind: String,
    pub resources: Vec<String>,
}

/// Full RegionRoute config detail for a single resource.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionRouteDetail {
    pub kind: String,
    pub name: String,
    /// HTTPRoutes that reference this resource (only for EdgionPlugins).
    pub http_routes: Vec<String>,
    pub base_info: serde_json::Value,
    pub key_get: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_key_get: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_calc: Option<serde_json::Value>,
    pub route_rules: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_failover: Option<serde_json::Value>,
}

// ============= Internal helpers =============

/// Extracted metadata from a RegionRoute config block.
struct RegionRouteConfigInfo<'a> {
    service_group: String,
    cluster: String,
    config: &'a serde_json::Value,
}

impl<'a> RegionRouteConfigInfo<'a> {
    /// Build a `RegionRouteDetail` from this config info.
    fn to_detail(&self, kind: &str, name: &str, http_routes: Vec<String>) -> RegionRouteDetail {
        RegionRouteDetail {
            kind: kind.to_string(),
            name: name.to_string(),
            http_routes,
            base_info: self
                .config
                .get("baseInfo")
                .cloned()
                .unwrap_or(serde_json::Value::Object(Default::default())),
            key_get: self
                .config
                .get("keyGet")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![])),
            hash_key_get: self.config.get("hashKeyGet").cloned(),
            hash_calc: self.config.get("hashCalc").cloned(),
            route_rules: self
                .config
                .get("routeRules")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![])),
            auto_failover: self.config.get("autoFailover").cloned(),
        }
    }
}

/// Search through plugin arrays in a spec for the first RegionRoute config block,
/// extracting `baseInfo.serviceGroup` and `baseInfo.cluster`.
fn find_region_route_config_info(spec: &serde_json::Value) -> Option<RegionRouteConfigInfo<'_>> {
    // EdgionPlugins uses requestPlugins (RegionRoute is a request-stage plugin)
    // EdgionStreamPlugins uses plugins / tlsRoutePlugins
    for arr_name in &["requestPlugins", "plugins", "tlsRoutePlugins"] {
        if let Some(arr) = spec.get(arr_name).and_then(|a| a.as_array()) {
            for item in arr {
                if item.get("type").and_then(|t| t.as_str()) == Some("RegionRoute") {
                    if let Some(config) = item.get("config") {
                        let base_info = config.get("baseInfo");
                        let service_group = base_info
                            .and_then(|b| b.get("serviceGroup"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let cluster = base_info
                            .and_then(|b| b.get("cluster"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        return Some(RegionRouteConfigInfo {
                            service_group,
                            cluster,
                            config,
                        });
                    }
                }
            }
        }
    }
    None
}

/// Collect RegionRoute resource names from a list of resources into the grouped map.
fn collect_region_route_refs(
    resources: &[serde_json::Value],
    kind: &str,
    grouped: &mut HashMap<(String, String, String, String), Vec<String>>,
) {
    for resource in resources {
        let ns = resource
            .pointer("/metadata/namespace")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name = resource
            .pointer("/metadata/name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let spec = match resource.get("spec") {
            Some(s) => s,
            None => continue,
        };

        let info = match find_region_route_config_info(spec) {
            Some(i) => i,
            None => continue,
        };

        let key = (info.service_group, info.cluster, ns.to_string(), kind.to_string());
        grouped.entry(key).or_default().push(name.to_string());
    }
}

/// Collect RegionRoute details from a list of resources, filtering by service_group and cluster.
fn collect_region_route_details(
    resources: &[serde_json::Value],
    kind: &str,
    service_group: &str,
    cluster: &str,
    plugin_to_routes: Option<&HashMap<String, Vec<String>>>,
    details: &mut Vec<RegionRouteDetail>,
) {
    for resource in resources {
        let ns = resource
            .pointer("/metadata/namespace")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name = resource
            .pointer("/metadata/name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let spec = match resource.get("spec") {
            Some(s) => s,
            None => continue,
        };

        let info = match find_region_route_config_info(spec) {
            Some(i) => i,
            None => continue,
        };

        if info.service_group != service_group || info.cluster != cluster {
            continue;
        }

        let http_routes = plugin_to_routes
            .and_then(|m| m.get(&format!("{}/{}", ns, name)))
            .cloned()
            .unwrap_or_default();

        details.push(info.to_detail(kind, name, http_routes));
    }
}

/// Build a map of "{namespace}/{plugin_name}" -> Vec<route_name> from HTTPRoute resources.
fn build_plugin_to_routes_map(
    routes: &[serde_json::Value],
    plugin_kind_str: &str,
) -> HashMap<String, Vec<String>> {
    let mut plugin_to_routes: HashMap<String, Vec<String>> = HashMap::new();

    for route in routes {
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
                            if kind == plugin_kind_str && !ref_name.is_empty() {
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

    plugin_to_routes
}

// ============= Handlers =============

/// List all RegionRoute plugin instances grouped by (serviceGroup, cluster, namespace).
pub async fn list_region_routes(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<ApiResponse<Vec<RegionRouteKey>>>, StatusCode> {
    let (plugins_result, stream_plugins_result) = tokio::join!(
        state.center_list_resources(ResourceKind::EdgionPlugins, None),
        state.center_list_resources(ResourceKind::EdgionStreamPlugins, None),
    );
    let (plugins, _) = plugins_result?;
    let (stream_plugins, _) = stream_plugins_result?;

    let mut grouped: HashMap<(String, String, String, String), Vec<String>> = HashMap::new();
    collect_region_route_refs(&plugins, ResourceKind::EdgionPlugins.as_str(), &mut grouped);
    collect_region_route_refs(&stream_plugins, ResourceKind::EdgionStreamPlugins.as_str(), &mut grouped);

    let mut entries: Vec<RegionRouteKey> = grouped
        .into_iter()
        .map(|((service_group, cluster, namespace, kind), resources)| RegionRouteKey {
            service_group,
            cluster,
            namespace,
            kind,
            resources,
        })
        .collect();

    entries.sort_by(|a, b| {
        a.service_group
            .cmp(&b.service_group)
            .then_with(|| a.cluster.cmp(&b.cluster))
            .then_with(|| a.namespace.cmp(&b.namespace))
            .then_with(|| a.kind.cmp(&b.kind))
    });

    Ok(Json(ApiResponse::success(entries)))
}

/// Get full RegionRoute config detail for all resources matching the given
/// (serviceGroup, cluster, namespace, kind) composite key.
pub async fn get_region_route_detail(
    State(state): State<Arc<AdminState>>,
    Path((service_group, cluster, namespace, kind)): Path<(String, String, String, String)>,
) -> Result<Json<ApiResponse<Vec<RegionRouteDetail>>>, StatusCode> {
    // Resolve the requested kind to a ResourceKind.
    let resource_kind = match kind.as_str() {
        k if k == ResourceKind::EdgionPlugins.as_str() => ResourceKind::EdgionPlugins,
        k if k == ResourceKind::EdgionStreamPlugins.as_str() => ResourceKind::EdgionStreamPlugins,
        _ => return Ok(Json(ApiResponse::success(vec![]))),
    };

    // Use namespaced fetch to avoid loading resources from all namespaces.
    let (resources_result, routes_result) = tokio::join!(
        state.center_list_resources_namespaced(resource_kind, &namespace, None),
        state.center_list_resources_namespaced(ResourceKind::HTTPRoute, &namespace, None),
    );
    let (resources, _) = resources_result?;
    let (routes, _) = routes_result?;

    let plugin_to_routes = if resource_kind == ResourceKind::EdgionPlugins {
        Some(build_plugin_to_routes_map(&routes, ResourceKind::EdgionPlugins.as_str()))
    } else {
        None
    };

    let mut details = Vec::new();
    collect_region_route_details(
        &resources,
        &kind,
        &service_group,
        &cluster,
        plugin_to_routes.as_ref(),
        &mut details,
    );

    Ok(Json(ApiResponse::success(details)))
}
