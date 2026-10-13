//! Region Route API handlers.
//!
//! The region-route base table lives in `EdgionPlugins.RegionRouteConfig.regions`; dynamic
//! overrides use `RegionRouteOverride` EdgionConfigData. `GET /api/v1/region-routes/effective`
//! serves the merged view and `POST /api/v1/cluster-region-routes/failover` patches a single
//! region's `failoverTo`. Center reaches both by drilling into one controller through the
//! federation proxy tunnel; it does not aggregate them fleet-wide.

use axum::{extract::State, http::StatusCode, response::Json};
use edgion_resources::resources::edgion_config_data::validate_regions;
use edgion_resources::resources::edgion_plugins::plugin_configs::{RegionDef, RegionRouteConfig};
use edgion_resources::resources::edgion_plugins::PluginValidation;
use edgion_resources::ResourceKind;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use super::types::AdminState;

// ============= Types =============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgionConfigDataFailoverRequest {
    pub namespace: String,
    pub name: String,
    pub region_name: String,
    /// Empty string = clear failover.
    pub failover_to: String,
}

// ============= Handlers =============

/// `GET /api/v1/region-routes/effective`
///
/// Returns the local controller's effective RegionRoute view for Center's
/// background poller. S3 moved the RegionRoute base table into EdgionPlugins, so
/// this view is derived from every enabled `requestPlugins[].RegionRoute` entry.
pub async fn list_region_routes_effective(State(state): State<Arc<AdminState>>) -> Result<Json<Value>, StatusCode> {
    let plugins = state.runtime_list_resources(ResourceKind::EdgionPlugins)?;
    let http_routes = list_usage_resources(&state, ResourceKind::HTTPRoute)?;
    let grpc_routes = list_usage_resources(&state, ResourceKind::GRPCRoute)?;
    let config_data = state.runtime_list_resources(ResourceKind::EdgionConfigData)?;
    let mut data = Vec::new();

    for plugin_resource in plugins {
        let namespace = plugin_resource
            .pointer("/metadata/namespace")
            .and_then(Value::as_str)
            .unwrap_or("default");
        let plugin_name = plugin_resource
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or("");
        if plugin_name.is_empty() {
            continue;
        }

        let Some(entries) = plugin_resource
            .pointer("/spec/requestPlugins")
            .and_then(Value::as_array)
        else {
            continue;
        };

        for (entry_index, entry) in entries.iter().enumerate() {
            let enabled = entry.get("enable").and_then(Value::as_bool).unwrap_or(true);
            if !enabled || entry.get("type").and_then(Value::as_str) != Some("RegionRoute") {
                continue;
            }
            let Some(config) = entry.get("config") else {
                continue;
            };
            let Ok(region_config) = serde_json::from_value::<RegionRouteConfig>(config.clone()) else {
                continue;
            };
            // Match the Gateway compile boundary: an invalid base cannot be
            // rescued by an overlay and is not an effective routing entry.
            if region_config.validation_error().is_some() {
                continue;
            }

            let base_regions = config
                .get("regions")
                .or_else(|| config.pointer("/baseInfo/regions"))
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new()));
            let my_region = config
                .get("myRegion")
                .or_else(|| config.pointer("/baseInfo/myRegion"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let override_ref = config
                .get("overrideRef")
                .and_then(|value| parse_override_ref(value, namespace));
            let (regions, override_applied) =
                apply_region_override(&region_config, &base_regions, override_ref.as_ref(), &config_data);
            let usages = collect_route_usages(namespace, plugin_name, &http_routes, &grpc_routes);

            data.push(serde_json::json!({
                "namespace": namespace,
                "pluginName": plugin_name,
                "alias": entry.get("alias").and_then(Value::as_str),
                "entryIndex": entry_index,
                "myRegion": my_region,
                "regions": regions,
                "keyGet": config.get("keyGet").cloned().unwrap_or_else(|| Value::Array(Vec::new())),
                "hashKeyGet": config.get("hashKeyGet").cloned(),
                "hashCalc": config.get("hashCalc").cloned(),
                "routeRules": config.get("routeRules").cloned().unwrap_or_else(|| Value::Array(Vec::new())),
                "routeByKeyConfMatch": config.get("routeByKeyConfMatch").cloned(),
                "dye": entry.get("dye").cloned(),
                "overrideRef": override_ref,
                "overrideApplied": override_applied,
                "serviceUsages": usages,
            }));
        }
    }

    Ok(Json(serde_json::json!({ "success": true, "data": data })))
}

fn list_usage_resources(state: &AdminState, kind: ResourceKind) -> Result<Vec<Value>, StatusCode> {
    state.runtime_list_resources(kind).inspect_err(|status| {
        tracing::error!(
            resource_kind = kind.as_str(),
            status = status.as_u16(),
            "RegionRoute effective snapshot aborted because the processed usage snapshot is incomplete"
        );
    })
}

fn collect_route_usages(
    plugin_namespace: &str,
    plugin_name: &str,
    http_routes: &[Value],
    grpc_routes: &[Value],
) -> Vec<Value> {
    let mut usages = Vec::new();
    for (kind, routes) in [("HTTPRoute", http_routes), ("GRPCRoute", grpc_routes)] {
        for route in routes {
            let namespace = route
                .pointer("/metadata/namespace")
                .and_then(Value::as_str)
                .unwrap_or("default");
            if namespace != plugin_namespace {
                continue;
            }
            let Some(route_name) = route.pointer("/metadata/name").and_then(Value::as_str) else {
                continue;
            };
            let Some(rules) = route
                .pointer("/spec/resolvedRules")
                .or_else(|| route.pointer("/spec/rules"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for (rule_index, rule) in rules.iter().enumerate() {
                let backends = rule
                    .get("backendRefs")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if filters_reference_plugin(rule.get("filters"), plugin_name) {
                    usages.push(route_usage(kind, namespace, route_name, rule_index, &backends));
                    continue;
                }
                let matching_backends: Vec<Value> = backends
                    .into_iter()
                    .filter(|backend| filters_reference_plugin(backend.get("filters"), plugin_name))
                    .collect();
                if !matching_backends.is_empty() {
                    usages.push(route_usage(kind, namespace, route_name, rule_index, &matching_backends));
                }
            }
        }
    }
    usages.sort_by_key(|left| left.to_string());
    usages
}

fn filters_reference_plugin(filters: Option<&Value>, plugin_name: &str) -> bool {
    filters.and_then(Value::as_array).is_some_and(|items| {
        items.iter().any(|filter| {
            filter.get("type").and_then(Value::as_str) == Some("ExtensionRef")
                && filter.pointer("/extensionRef/group").and_then(Value::as_str) == Some("edgion.io")
                && filter.pointer("/extensionRef/kind").and_then(Value::as_str) == Some("EdgionPlugins")
                && filter.pointer("/extensionRef/name").and_then(Value::as_str) == Some(plugin_name)
        })
    })
}

fn route_usage(kind: &str, namespace: &str, name: &str, rule_index: usize, backends: &[Value]) -> Value {
    let backend_services: Vec<Value> = backends
        .iter()
        .filter(|backend| {
            backend.get("group").and_then(Value::as_str).is_none_or(str::is_empty)
                && backend
                    .get("kind")
                    .and_then(Value::as_str)
                    .is_none_or(|kind| kind == "Service")
        })
        .filter_map(|backend| {
            let backend_name = backend.get("name").and_then(Value::as_str)?;
            Some(serde_json::json!({
                "namespace": backend.get("namespace").and_then(Value::as_str).unwrap_or(namespace),
                "name": backend_name,
                "port": backend.get("port").cloned(),
            }))
        })
        .collect();
    serde_json::json!({
        "routeKind": kind,
        "routeNamespace": namespace,
        "routeName": name,
        "ruleIndex": rule_index,
        "backendServices": backend_services,
    })
}

/// `POST /api/v1/cluster-region-routes/failover`
///
/// Patches a single region's `failoverTo` in a `RegionRouteOverride` EdgionConfigData.
/// The `name` field must refer to a `RegionRouteOverride` EdgionConfigData.
pub async fn cluster_edgion_config_data_failover(
    State(state): State<Arc<AdminState>>,
    Json(body): Json<EdgionConfigDataFailoverRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match patch_edgion_config_data_failover(
        &state,
        &body.namespace,
        &body.name,
        "RegionRouteOverride",
        &body.region_name,
        &body.failover_to,
    )
    .await
    {
        Ok(true) => Ok(Json(serde_json::json!({ "success": true, "modified": true }))),
        Ok(false) => Err(StatusCode::NOT_FOUND),
        Err(FailoverError::InvalidTable(_)) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(FailoverError::Storage(_)) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

// ============= Helpers =============

fn parse_override_ref(value: &Value, default_namespace: &str) -> Option<Value> {
    if let Some(s) = value.as_str() {
        let (namespace, name) = s
            .split_once('/')
            .map_or((default_namespace, s), |(namespace, name)| (namespace, name));
        return Some(serde_json::json!({ "namespace": namespace, "name": name, "permitted": true }));
    }
    let name = value.get("name").and_then(Value::as_str)?;
    let namespace = value
        .get("namespace")
        .and_then(Value::as_str)
        .filter(|namespace| !namespace.is_empty())
        .unwrap_or(default_namespace);
    let permitted = value.get("refDenied").is_none();
    Some(serde_json::json!({ "namespace": namespace, "name": name, "permitted": permitted }))
}

fn apply_region_override(
    config: &RegionRouteConfig,
    base_regions: &Value,
    override_ref: Option<&Value>,
    resources: &[Value],
) -> (Value, bool) {
    let Some(reference) = override_ref else {
        return (base_regions.clone(), false);
    };
    let Some(namespace) = reference.get("namespace").and_then(Value::as_str) else {
        return (base_regions.clone(), false);
    };
    let Some(name) = reference.get("name").and_then(Value::as_str) else {
        return (base_regions.clone(), false);
    };
    if reference.get("permitted").and_then(Value::as_bool) == Some(false) {
        return (base_regions.clone(), false);
    }
    let Some(resource) = resources.iter().find(|resource| {
        resource.pointer("/metadata/namespace").and_then(Value::as_str) == Some(namespace)
            && resource.pointer("/metadata/name").and_then(Value::as_str) == Some(name)
    }) else {
        return (base_regions.clone(), false);
    };
    if resource.pointer("/spec/enable").and_then(Value::as_bool) == Some(false)
        || resource.pointer("/spec/data/type").and_then(Value::as_str) != Some("RegionRouteOverride")
    {
        return (base_regions.clone(), false);
    }
    let Some(regions_value) = resource.pointer("/spec/data/config/regions").cloned() else {
        return (base_regions.clone(), false);
    };
    let Ok(regions) = serde_json::from_value::<Vec<RegionDef>>(regions_value) else {
        return (base_regions.clone(), false);
    };
    if config.validate_effective_regions(&regions).is_err() {
        return (base_regions.clone(), false);
    }
    match serde_json::to_value(regions) {
        Ok(regions) => (regions, true),
        Err(_) => (base_regions.clone(), false),
    }
}

/// GET the EdgionConfigData from local storage, modify the region's failoverTo, then PUT back.
async fn patch_edgion_config_data_failover(
    state: &AdminState,
    namespace: &str,
    edgion_config_data_name: &str,
    plugin_type: &str,
    region_name: &str,
    failover_to: &str,
) -> Result<bool, FailoverError> {
    use crate::conf_mgr::conf_center::traits::ConfWriterError;
    use edgion_resources::ResourceKind;

    // Serialize the whole get -> patch -> write below against other writers to the
    // same resource so concurrent failover requests and a concurrent generic
    // `update_namespaced` PUT cannot interleave and silently lose an update.
    let _resource_guard = super::resource_locks::lock_resource(
        ResourceKind::EdgionConfigData.as_str(),
        namespace,
        edgion_config_data_name,
    )
    .await;

    let api = state.conf_center();
    for attempt in 0..3 {
        let mut resource = state
            .center_get_resource(ResourceKind::EdgionConfigData, namespace, edgion_config_data_name)
            .await
            .map_err(|e| format!("failed to get EdgionConfigData: {e}"))?
            .ok_or_else(|| format!("EdgionConfigData {}/{} not found", namespace, edgion_config_data_name))?;
        let resource_version = resource
            .pointer("/metadata/resourceVersion")
            .and_then(Value::as_str)
            .map(str::to_string);

        let patch_result = patch_failover_resource(&mut resource, plugin_type, region_name, failover_to);
        if patch_result != FailoverPatch::Missing {
            validate_failover_resource(&resource).map_err(FailoverError::InvalidTable)?;
        }
        match patch_result {
            FailoverPatch::Missing => return Ok(false),
            // A concurrent writer may already have established the requested
            // state. Treat that as success instead of issuing a redundant PUT.
            FailoverPatch::AlreadyApplied => return Ok(true),
            FailoverPatch::Changed => {}
        }

        // Non-Kubernetes backends persist the request's resourceVersion verbatim,
        // so this write must carry a freshly allocated one exactly like
        // `update_namespaced` does. Writing the object back with the version it
        // was read at would leave the stored version unchanged across a failover,
        // and a client holding that same version could then satisfy an `If-Match`
        // precondition and silently overwrite this failover. Kubernetes assigns
        // the version server-side on replace, so it must not be touched there.
        if !state.is_k8s_mode() {
            set_fresh_resource_version(&mut resource)?;
        }

        let json_content = serde_json::to_string(&resource).map_err(|e| format!("JSON serialize failed: {e}"))?;
        match api
            .update_one_if_version(
                ResourceKind::EdgionConfigData.as_str(),
                Some(namespace),
                edgion_config_data_name,
                json_content,
                resource_version.as_deref(),
            )
            .await
        {
            Ok(()) => return Ok(true),
            Err(ConfWriterError::Conflict(_)) if attempt < 2 => {
                tracing::debug!(
                    namespace,
                    name = edgion_config_data_name,
                    attempt = attempt + 1,
                    "RegionRouteOverride changed concurrently; retrying failover update"
                );
            }
            Err(error) => return Err(FailoverError::Storage(format!("update_one failed: {error}"))),
        }
    }

    Err(FailoverError::Storage(
        "RegionRouteOverride update retries exhausted".to_string(),
    ))
}

#[derive(Debug, thiserror::Error)]
enum FailoverError {
    #[error("invalid region table: {0}")]
    InvalidTable(String),
    #[error("{0}")]
    Storage(String),
}

impl From<String> for FailoverError {
    fn from(value: String) -> Self {
        Self::Storage(value)
    }
}

/// The dedicated failover command validates the complete proposed table, even
/// for an idempotent request. Generic resource ingest remains advisory.
fn validate_failover_resource(resource: &Value) -> Result<(), String> {
    let value = resource
        .pointer("/spec/data/config/regions")
        .ok_or_else(|| "RegionRouteOverride regions are missing".to_string())?;
    let regions: Vec<RegionDef> =
        serde_json::from_value(value.clone()).map_err(|error| format!("invalid regions: {error}"))?;
    validate_regions(&regions)
}

/// Assign a newly allocated `metadata.resourceVersion` to a resource that is
/// about to be written to a non-Kubernetes configuration center.
fn set_fresh_resource_version(resource: &mut Value) -> Result<(), String> {
    let metadata = resource
        .pointer_mut("/metadata")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "resource metadata must be an object".to_string())?;
    let version = crate::conf_mgr::sync_runtime::resource_version::next_resource_version();
    metadata.insert("resourceVersion".to_string(), Value::String(version.to_string()));
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum FailoverPatch {
    Missing,
    AlreadyApplied,
    Changed,
}

fn patch_failover_resource(
    resource: &mut Value,
    plugin_type: &str,
    region_name: &str,
    failover_to: &str,
) -> FailoverPatch {
    let Some(data) = resource.pointer_mut("/spec/data") else {
        return FailoverPatch::Missing;
    };
    if data.get("type").and_then(Value::as_str) != Some(plugin_type) {
        return FailoverPatch::Missing;
    }
    let Some(regions) = data.pointer_mut("/config/regions").and_then(Value::as_array_mut) else {
        return FailoverPatch::Missing;
    };
    let RegionPatch { matched, changed } = patch_region_failover(regions, region_name, failover_to);

    match (matched, changed) {
        (false, _) => FailoverPatch::Missing,
        (true, false) => FailoverPatch::AlreadyApplied,
        (true, true) => FailoverPatch::Changed,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct RegionPatch {
    matched: bool,
    changed: bool,
}

/// Modify a single region's failoverTo while distinguishing a missing region
/// from an idempotent request whose target state is already present.
fn patch_region_failover(regions: &mut [serde_json::Value], region_name: &str, failover_to: &str) -> RegionPatch {
    let mut matched = false;
    let mut changed = false;
    for region in regions.iter_mut() {
        if region.get("name").and_then(|n| n.as_str()) != Some(region_name) {
            continue;
        }
        matched = true;
        let current = region.get("failoverTo").and_then(Value::as_str).unwrap_or("");
        if current == failover_to {
            continue;
        }
        if failover_to.is_empty() {
            if let Some(obj) = region.as_object_mut() {
                obj.remove("failoverTo");
            }
        } else {
            region["failoverTo"] = serde_json::Value::String(failover_to.to_string());
        }
        changed = true;
    }
    RegionPatch { matched, changed }
}

#[cfg(test)]
#[path = "../../tests/unit/api/region_route_handlers_test/failover_version_tests.rs"]
mod failover_version_tests;

#[cfg(test)]
#[path = "../../tests/unit/api/region_route_handlers_test.rs"]
mod tests;
