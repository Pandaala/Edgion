//! Region Route API handlers.
//!
//! The region-route base table lives in `EdgionPlugins.RegionRouteConfig.regions`; dynamic
//! overrides use `RegionRouteOverride` EdgionConfigData. `GET /api/v1/region-routes/effective`
//! serves the merged view and `POST /api/v1/cluster-region-routes/failover` patches a single
//! region's `failoverTo`. Center reaches both by drilling into one controller through the
//! federation proxy tunnel; it does not aggregate them fleet-wide.

use axum::{extract::State, http::StatusCode, response::Json};
use edgion_resources::resources::edgion_config_data::validate_regions;
use edgion_resources::resources::edgion_plugins::plugin_configs::RegionDef;
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
            let (regions, override_applied) = apply_region_override(&base_regions, override_ref.as_ref(), &config_data);
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
    usages.sort_by(|left, right| left.to_string().cmp(&right.to_string()));
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
        Err(_e) => Err(StatusCode::INTERNAL_SERVER_ERROR),
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

fn apply_region_override(base_regions: &Value, override_ref: Option<&Value>, resources: &[Value]) -> (Value, bool) {
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
    if validate_regions(&regions).is_err() {
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
) -> Result<bool, String> {
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
            Err(error) => return Err(format!("update_one failed: {error}")),
        }
    }

    Err("RegionRouteOverride update retries exhausted".to_string())
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
    let mut matched = false;
    let mut changed = false;

    if let Some(plugins) = resource.pointer_mut("/spec/plugins").and_then(Value::as_array_mut) {
        for plugin in plugins.iter_mut() {
            if plugin.get("type").and_then(Value::as_str) != Some(plugin_type) {
                continue;
            }
            if let Some(regions) = plugin.pointer_mut("/config/regions").and_then(Value::as_array_mut) {
                let result = patch_region_failover(regions, region_name, failover_to);
                matched |= result.matched;
                changed |= result.changed;
            }
        }
    }

    if !matched {
        if let Some(data) = resource.pointer_mut("/spec/data") {
            if data.get("type").and_then(Value::as_str) == Some(plugin_type) {
                if let Some(regions) = data.pointer_mut("/config/regions").and_then(Value::as_array_mut) {
                    let result = patch_region_failover(regions, region_name, failover_to);
                    matched = result.matched;
                    changed = result.changed;
                }
            }
        }
    }

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
fn patch_region_failover(regions: &mut Vec<serde_json::Value>, region_name: &str, failover_to: &str) -> RegionPatch {
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
mod failover_version_tests {
    use super::*;
    use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

    async fn make_fs_state() -> (Arc<AdminState>, tempfile::TempDir) {
        let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
        let fs_config = FileSystemConfig::new(tmp_data.path().to_path_buf());
        let conf_mgr = Arc::new(
            ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
                .await
                .expect("ConfMgr::create"),
        );
        let (tx, _rx) = tokio::sync::watch::channel(crate::config::CenterClientConfig::default());
        let state = Arc::new(AdminState {
            conf_mgr,
            schema_validator: Arc::new(SchemaValidator::empty()),
            config_file: None,
            authorizer: Arc::new(crate::authz::Authorizer::new(crate::authz::CenterPolicy::deny_all())),
            center_reload_tx: tx,
        });
        (state, tmp_data)
    }

    fn override_body(resource_version: &str) -> String {
        serde_json::json!({
            "apiVersion": "edgion.io/v1",
            "kind": "EdgionConfigData",
            "metadata": {
                "name": "override",
                "namespace": "default",
                "resourceVersion": resource_version
            },
            "spec": {
                "enable": true,
                "visibility": "Namespace",
                "data": {
                    "type": "RegionRouteOverride",
                    "config": { "regions": [{ "name": "east" }] }
                }
            }
        })
        .to_string()
    }

    /// A successful failover must leave the stored resource at a *new*
    /// resourceVersion. If it wrote the object back at the version it was read
    /// at, a client still holding that version could satisfy an `If-Match`
    /// precondition afterwards and silently revert the failover.
    #[tokio::test]
    async fn failover_advances_stored_resource_version() {
        let (state, _tmp) = make_fs_state().await;
        state
            .conf_center()
            .create_one("EdgionConfigData", Some("default"), "override", override_body("42"))
            .await
            .expect("seed resource must be created");

        let applied =
            patch_edgion_config_data_failover(&state, "default", "override", "RegionRouteOverride", "east", "west")
                .await
                .expect("failover must succeed");
        assert!(applied, "failover must report that it changed the resource");

        let stored = state
            .conf_center()
            .get_one("EdgionConfigData", Some("default"), "override")
            .await
            .expect("resource must remain readable");
        let stored: Value = serde_yaml::from_str(&stored).expect("stored resource must be valid YAML");

        assert_eq!(
            stored
                .pointer("/spec/data/config/regions/0/failoverTo")
                .and_then(Value::as_str),
            Some("west"),
            "failover must have been persisted"
        );
        assert_ne!(
            stored.pointer("/metadata/resourceVersion").and_then(Value::as_str),
            Some("42"),
            "a persisted failover must advance the stored resourceVersion"
        );
    }

    /// The precondition must still be evaluated against the version the resource
    /// was read at, not against the freshly allocated one.
    #[tokio::test]
    async fn failover_on_stale_stored_version_still_applies() {
        let (state, _tmp) = make_fs_state().await;
        state
            .conf_center()
            .create_one("EdgionConfigData", Some("default"), "override", override_body("7"))
            .await
            .expect("seed resource must be created");

        let applied =
            patch_edgion_config_data_failover(&state, "default", "override", "RegionRouteOverride", "east", "west")
                .await
                .expect("failover must succeed regardless of the seeded version value");
        assert!(applied);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failover_request_rejects_unknown_fields() {
        // Any unknown field must be rejected to keep the contract
        // "failover endpoint can only change failoverTo".
        let json = r#"{
            "namespace": "default",
            "name": "test-override-route",
            "regionName": "east",
            "failoverTo": "west",
            "myRegion": "evil"
        }"#;
        let result: Result<EdgionConfigDataFailoverRequest, _> = serde_json::from_str(json);
        assert!(result.is_err(), "unknown field 'myRegion' must be rejected");
    }

    #[test]
    fn failover_request_accepts_canonical_payload() {
        let json = r#"{
            "namespace": "default",
            "name": "test-override-route",
            "regionName": "east",
            "failoverTo": "west"
        }"#;
        let result: Result<EdgionConfigDataFailoverRequest, _> = serde_json::from_str(json);
        let req = result.expect("canonical 4-field payload must be accepted");
        assert_eq!(req.namespace, "default");
        assert_eq!(req.name, "test-override-route");
        assert_eq!(req.region_name, "east");
        assert_eq!(req.failover_to, "west");
    }

    #[test]
    fn failover_request_accepts_empty_failover_to() {
        let json = r#"{
            "namespace": "default",
            "name": "test-override-route",
            "regionName": "east",
            "failoverTo": ""
        }"#;
        let result: Result<EdgionConfigDataFailoverRequest, _> = serde_json::from_str(json);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().failover_to, "");
    }

    #[test]
    fn patch_region_failover_only_changes_target_failover_to() {
        let original = serde_json::json!([
            {
                "name": "east",
                "weight": 100,
                "endpoints": ["10.0.0.1", "10.0.0.2"],
                "failoverTo": "old-target"
            },
            {
                "name": "west",
                "weight": 50,
                "endpoints": ["10.0.1.1"],
                "failoverTo": "another"
            }
        ]);
        let mut regions = original.as_array().unwrap().clone();

        let modified = patch_region_failover(&mut regions, "east", "west");
        assert_eq!(
            modified,
            RegionPatch {
                matched: true,
                changed: true
            }
        );

        assert_eq!(regions[0]["failoverTo"], serde_json::Value::String("west".into()));
        assert_eq!(regions[0]["name"], original[0]["name"]);
        assert_eq!(regions[0]["weight"], original[0]["weight"]);
        assert_eq!(regions[0]["endpoints"], original[0]["endpoints"]);
        assert_eq!(regions[1], original[1]);
    }

    #[test]
    fn patch_region_failover_clear_removes_field_only() {
        let original = serde_json::json!([
            {
                "name": "east",
                "weight": 100,
                "failoverTo": "west"
            },
            {
                "name": "west",
                "weight": 50,
                "failoverTo": "east"
            }
        ]);
        let mut regions = original.as_array().unwrap().clone();

        let modified = patch_region_failover(&mut regions, "east", "");
        assert_eq!(
            modified,
            RegionPatch {
                matched: true,
                changed: true
            }
        );

        assert!(regions[0].as_object().unwrap().get("failoverTo").is_none());
        assert_eq!(regions[0]["name"], original[0]["name"]);
        assert_eq!(regions[0]["weight"], original[0]["weight"]);
        assert_eq!(regions[1], original[1]);
    }

    #[test]
    fn patch_region_failover_no_match_returns_false() {
        let mut regions = vec![serde_json::json!({"name": "east", "failoverTo": "x"})];
        let modified = patch_region_failover(&mut regions, "nonexistent", "y");
        assert_eq!(
            modified,
            RegionPatch {
                matched: false,
                changed: false
            }
        );
        assert_eq!(regions[0]["failoverTo"], serde_json::Value::String("x".into()));
    }

    #[test]
    fn patch_region_failover_reports_already_applied_without_mutation() {
        let mut regions = vec![serde_json::json!({"name": "east", "failoverTo": "west"})];
        let before = regions.clone();
        assert_eq!(
            patch_region_failover(&mut regions, "east", "west"),
            RegionPatch {
                matched: true,
                changed: false
            }
        );
        assert_eq!(regions, before);

        let mut clear = vec![serde_json::json!({"name": "east"})];
        assert_eq!(
            patch_region_failover(&mut clear, "east", ""),
            RegionPatch {
                matched: true,
                changed: false
            }
        );
    }

    #[test]
    fn route_usage_discovers_rule_and_backend_extension_refs() {
        let http_routes = vec![serde_json::json!({
            "metadata": {"namespace": "shop", "name": "checkout"},
            "spec": {"rules": [
                {
                    "filters": [{"type": "ExtensionRef", "extensionRef": {
                        "group": "edgion.io", "kind": "EdgionPlugins", "name": "region"
                    }}],
                    "backendRefs": [{"name": "checkout-api", "port": 8080}]
                },
                {
                    "backendRefs": [{
                        "name": "payments", "port": 9090,
                        "filters": [{"type": "ExtensionRef", "extensionRef": {
                            "group": "edgion.io", "kind": "EdgionPlugins", "name": "region"
                        }}]
                    }]
                }
            ]}
        })];
        let usages = collect_route_usages("shop", "region", &http_routes, &[]);
        assert_eq!(usages.len(), 2);
        assert_eq!(usages[0]["routeKind"], "HTTPRoute");
        assert_eq!(usages[0]["backendServices"][0]["name"], "checkout-api");
        assert_eq!(usages[1]["ruleIndex"], 1);
        assert_eq!(usages[1]["backendServices"][0]["name"], "payments");
    }

    #[test]
    fn route_usage_rejects_other_namespaces_and_reference_kinds() {
        let routes = vec![serde_json::json!({
            "metadata": {"namespace": "other", "name": "ignored"},
            "spec": {"rules": [{
                "filters": [{"type": "ExtensionRef", "extensionRef": {
                    "group": "example.io", "kind": "EdgionPlugins", "name": "region"
                }}]
            }]}
        })];
        assert!(collect_route_usages("shop", "region", &routes, &[]).is_empty());
    }

    #[test]
    fn route_usage_uses_delegation_resolved_rules() {
        let routes = vec![serde_json::json!({
            "metadata": {"namespace": "shop", "name": "parent"},
            "spec": {
                "rules": [{
                    "filters": [{"type": "ExtensionRef", "extensionRef": {
                        "group": "edgion.io", "kind": "EdgionPlugins", "name": "region"
                    }}],
                    "backendRefs": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": "child"}]
                }],
                "resolvedRules": [{
                    "filters": [{"type": "ExtensionRef", "extensionRef": {
                        "group": "edgion.io", "kind": "EdgionPlugins", "name": "region"
                    }}],
                    "backendRefs": [{"name": "child-service", "port": 8080}]
                }]
            }
        })];
        let usages = collect_route_usages("shop", "region", &routes, &[]);
        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0]["backendServices"][0]["name"], "child-service");
    }

    #[test]
    fn override_ref_keeps_namespace_and_applies_effective_regions() {
        let reference = parse_override_ref(
            &serde_json::json!({"namespace": "operations", "name": "region-overlay"}),
            "shop",
        )
        .expect("structured reference");
        assert_eq!(reference["namespace"], "operations");
        let base = serde_json::json!([{"name": "base"}]);
        let resources = vec![serde_json::json!({
            "metadata": {"namespace": "operations", "name": "region-overlay"},
            "spec": {"enable": true, "data": {"type": "RegionRouteOverride", "config": {
                "regions": [{
                    "name": "effective",
                    "hashRange": [0, 999],
                    "backendEndpoint": "effective.example.test:443",
                    "tls": true,
                    "failoverTo": "backup"
                }]
            }}}
        })];
        let (regions, applied) = apply_region_override(&base, Some(&reference), &resources);
        assert!(applied);
        assert_eq!(regions[0]["name"], "effective");
        assert_eq!(regions[0]["failoverTo"], "backup");
    }

    #[test]
    fn invalid_region_override_falls_back_to_base() {
        let reference = serde_json::json!({
            "namespace": "operations",
            "name": "region-overlay",
            "permitted": true
        });
        let base = serde_json::json!([{
            "name": "base",
            "hashRange": [0, 999],
            "backendEndpoint": "base.example.test:443",
            "tls": true
        }]);
        let resources = vec![serde_json::json!({
            "metadata": {"namespace": "operations", "name": "region-overlay"},
            "spec": {"enable": true, "data": {"type": "RegionRouteOverride", "config": {
                "regions": [{
                    "name": "broken",
                    "hashRange": [999, 0],
                    "backendEndpoint": "broken.example.test:443",
                    "tls": true
                }]
            }}}
        })];
        let (regions, applied) = apply_region_override(&base, Some(&reference), &resources);
        assert!(!applied);
        assert_eq!(regions, base);
    }
}
