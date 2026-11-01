//! Controller-owned RegionRoute observation and validated setting mutations.
//! Effective observations describe Controller state, not Gateway acknowledgment.

use axum::{extract::State, http::StatusCode, response::Json};
use edgion_resources::resources::edgion_config_data::ClusterRegionSetting;
use edgion_resources::resources::edgion_plugins::plugin_configs::{RegionDef, RegionRouteConfig};
use edgion_resources::resources::edgion_plugins::PluginValidation;
use edgion_resources::ResourceKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use super::bounded_json::{self, BoundedJson, ProjectionBudget};
use super::types::AdminState;

// ============= Types =============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingFailoverRequest {
    pub region_name: String,
    /// Empty string clears the one-hop failover target.
    pub failover_to: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingFailoverApplyRequest {
    pub region_name: String,
    pub failover_to: String,
    pub precondition: crate::conf_mgr::conf_center::source_query::SourcePrecondition,
}

/// Narrow observation contract; no internal resource or plugin config is serialized.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RegionRouteObservation<'a> {
    namespace: &'a str,
    plugin_name: &'a str,
    alias: Option<&'a str>,
    entry_index: usize,
    cluster_name: Option<&'a str>,
    region_name: Option<&'a str>,
    regions: Option<&'a [RegionDef]>,
    cluster_region_ref: RegionSettingReference<'a>,
    configuration_available: bool,
    fail_open: bool,
    status_on_error: u16,
    observation: &'static str,
    service_usages: Vec<Value>,
}
#[derive(Serialize)]
struct RegionSettingReference<'a> {
    namespace: &'a str,
    name: &'a str,
    permitted: bool,
}

// ============= Handlers =============

/// `GET /api/v1/region-routes/effective`
///
/// On-demand Controller observations for enabled RegionRoute consumers.
pub async fn list_region_routes_effective(
    State(state): State<Arc<AdminState>>,
) -> Result<BoundedJson<Value>, StatusCode> {
    let plugins = state.runtime_list_resources(ResourceKind::EdgionPlugins)?;
    let http_routes = list_usage_resources(&state, ResourceKind::HTTPRoute)?;
    let grpc_routes = list_usage_resources(&state, ResourceKind::GRPCRoute)?;
    let config_data = state.runtime_list_resources(ResourceKind::EdgionConfigData)?;
    let mut data = Vec::new();
    let mut budget = ProjectionBudget::new();

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
            let Ok(region_config) = RegionRouteConfig::deserialize(config) else {
                continue;
            };
            let reference = &region_config.cluster_region_ref;
            let target_namespace = reference.namespace.as_deref().unwrap_or(namespace);
            let setting = read_region_setting(&region_config, namespace, &config_data);
            let available = region_config.validation_error().is_none()
                && setting.as_ref().is_some_and(|setting| {
                    setting
                        .validation_proof()
                        .is_ok_and(|proof| proof.accepts_modulo(region_config.effective_modulo()))
                });
            let usages = collect_route_usages(namespace, plugin_name, &http_routes, &grpc_routes, &mut budget)?;

            let row = RegionRouteObservation {
                namespace,
                plugin_name,
                alias: entry.get("alias").and_then(Value::as_str),
                entry_index,
                cluster_name: setting.as_ref().map(|setting| setting.cluster_name.as_str()),
                region_name: setting.as_ref().map(|setting| setting.region_name.as_str()),
                regions: setting
                    .as_ref()
                    .filter(|_| available)
                    .map(|setting| setting.regions.as_slice()),
                cluster_region_ref: RegionSettingReference {
                    namespace: target_namespace,
                    name: &reference.name,
                    permitted: reference.ref_denied.is_none(),
                },
                configuration_available: available,
                fail_open: region_config.fail_open,
                status_on_error: region_config.status_on_error,
                observation: "controller",
                service_usages: usages,
            };
            budget.reserve(&row)?;
            data.push(serde_json::to_value(row).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?);
        }
    }

    Ok(BoundedJson(serde_json::json!({ "success": true, "data": data })))
}

fn read_region_setting(
    config: &RegionRouteConfig,
    namespace: &str,
    resources: &[Value],
) -> Option<ClusterRegionSetting> {
    let reference = &config.cluster_region_ref;
    let target_namespace = reference.namespace.as_deref().unwrap_or(namespace);
    if reference.ref_denied.is_some() {
        return None;
    }
    resources
        .iter()
        .find(|resource| {
            resource.pointer("/metadata/namespace").and_then(Value::as_str) == Some(target_namespace)
                && resource.pointer("/metadata/name").and_then(Value::as_str) == Some(reference.name.as_str())
                && resource.pointer("/spec/enable").and_then(Value::as_bool) != Some(false)
                && resource.pointer("/spec/data/type").and_then(Value::as_str) == Some("ClusterRegionSetting")
        })
        .and_then(|resource| resource.pointer("/spec/data/config"))
        .and_then(|value| ClusterRegionSetting::deserialize(value).ok())
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
    budget: &mut ProjectionBudget,
) -> Result<Vec<Value>, StatusCode> {
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
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                if filters_reference_plugin(rule.get("filters"), plugin_name) {
                    usages.push(route_usage(
                        kind,
                        namespace,
                        route_name,
                        rule_index,
                        backends.iter(),
                        budget,
                    )?);
                    continue;
                }
                let matching_backends = backends
                    .iter()
                    .filter(|backend| filters_reference_plugin(backend.get("filters"), plugin_name));
                if matching_backends.clone().next().is_some() {
                    usages.push(route_usage(
                        kind,
                        namespace,
                        route_name,
                        rule_index,
                        matching_backends,
                        budget,
                    )?);
                }
            }
        }
    }
    usages.sort_by(|left, right| {
        (
            left["routeKind"].as_str(),
            left["routeNamespace"].as_str(),
            left["routeName"].as_str(),
            left["ruleIndex"].as_u64(),
        )
            .cmp(&(
                right["routeKind"].as_str(),
                right["routeNamespace"].as_str(),
                right["routeName"].as_str(),
                right["ruleIndex"].as_u64(),
            ))
    });
    Ok(usages)
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

fn route_usage<'a>(
    kind: &str,
    namespace: &str,
    name: &str,
    rule_index: usize,
    backends: impl Iterator<Item = &'a Value>,
    budget: &mut ProjectionBudget,
) -> Result<Value, StatusCode> {
    #[derive(Serialize)]
    struct Backend<'a> {
        namespace: &'a str,
        name: &'a str,
        port: Option<u64>,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Usage<'a> {
        route_kind: &'a str,
        route_namespace: &'a str,
        route_name: &'a str,
        rule_index: usize,
        backend_services: Vec<Value>,
    }
    let mut backend_services = Vec::new();
    for backend in backends {
        if !backend.get("group").and_then(Value::as_str).is_none_or(str::is_empty)
            || backend
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind != "Service")
        {
            continue;
        }
        let Some(backend_name) = backend.get("name").and_then(Value::as_str) else {
            continue;
        };
        let row = Backend {
            namespace: backend.get("namespace").and_then(Value::as_str).unwrap_or(namespace),
            name: backend_name,
            port: backend.get("port").and_then(Value::as_u64),
        };
        budget.reserve(&row)?;
        backend_services.push(serde_json::to_value(row).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?);
    }
    let row = Usage {
        route_kind: kind,
        route_namespace: namespace,
        route_name: name,
        rule_index,
        backend_services,
    };
    budget.reserve(&row)?;
    serde_json::to_value(row).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingConsumer<T = String> {
    namespace: T,
    plugin_name: T,
    entry_index: usize,
    enabled: bool,
    reference_permitted: Option<bool>,
    compatible_with_table: bool,
    diagnostic: Option<&'static str>,
}

fn setting_consumers(
    state: &AdminState,
    namespace: &str,
    name: &str,
    setting: &ClusterRegionSetting,
) -> Result<Vec<SettingConsumer>, StatusCode> {
    let plugins = state.runtime_list_resources(ResourceKind::EdgionPlugins)?;
    let proof = setting.validation_proof();
    let mut consumers = Vec::new();
    let mut budget = ProjectionBudget::new();
    for resource in plugins {
        let owner_ns = resource
            .pointer("/metadata/namespace")
            .and_then(Value::as_str)
            .unwrap_or("default");
        let owner_name = resource.pointer("/metadata/name").and_then(Value::as_str).unwrap_or("");
        let Some(entries) = resource.pointer("/spec/requestPlugins").and_then(Value::as_array) else {
            continue;
        };
        for (entry_index, entry) in entries.iter().enumerate() {
            if entry.get("type").and_then(Value::as_str) != Some("RegionRoute") {
                continue;
            }
            let Some(config) = entry.get("config") else {
                continue;
            };
            let target_ns = config
                .pointer("/clusterRegionRef/namespace")
                .and_then(Value::as_str)
                .unwrap_or(owner_ns);
            if target_ns != namespace || config.pointer("/clusterRegionRef/name").and_then(Value::as_str) != Some(name)
            {
                continue;
            }
            let parsed = RegionRouteConfig::deserialize(config);
            let (permitted, diagnostic) = match parsed {
                Err(_) => (None, Some("invalid-consumer-config")),
                Ok(config) if config.cluster_region_ref.ref_denied.is_some() => (Some(false), Some("reference-denied")),
                Ok(config) if config.validation_error().is_some() => (Some(true), Some("invalid-consumer-config")),
                Ok(config)
                    if !proof
                        .as_ref()
                        .is_ok_and(|p| p.accepts_modulo(config.effective_modulo())) =>
                {
                    (Some(true), Some("incompatible-region-table"))
                }
                Ok(_) => (Some(true), None),
            };
            if consumers.len() >= 1000 {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            let row = SettingConsumer {
                namespace: owner_ns,
                plugin_name: owner_name,
                entry_index,
                enabled: entry.get("enable").and_then(Value::as_bool).unwrap_or(true),
                reference_permitted: permitted,
                compatible_with_table: diagnostic.is_none(),
                diagnostic,
            };
            budget.reserve(&row)?;
            consumers.push(SettingConsumer {
                namespace: row.namespace.to_owned(),
                plugin_name: row.plugin_name.to_owned(),
                entry_index: row.entry_index,
                enabled: row.enabled,
                reference_permitted: row.reference_permitted,
                compatible_with_table: row.compatible_with_table,
                diagnostic: row.diagnostic,
            });
        }
    }
    Ok(consumers)
}

async fn setting_source(state: &AdminState, namespace: &str, name: &str) -> Result<Value, StatusCode> {
    let value = state
        .center_get_resource(ResourceKind::EdgionConfigData, namespace, name)
        .await?
        .ok_or(StatusCode::NOT_FOUND)?;
    if value.pointer("/spec/data/type").and_then(Value::as_str) != Some("ClusterRegionSetting") {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(value)
}
fn parsed_setting(value: &Value) -> Result<ClusterRegionSetting, StatusCode> {
    ClusterRegionSetting::deserialize(
        value
            .pointer("/spec/data/config")
            .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?,
    )
    .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)
}

pub async fn list_setting_consumers(
    State(state): State<Arc<AdminState>>,
    axum::extract::Path((namespace, name)): axum::extract::Path<(String, String)>,
) -> Result<BoundedJson<Value>, StatusCode> {
    tokio::time::timeout(crate::conf_mgr::conf_center::source_query::SOURCE_TIMEOUT,async {
        let source=setting_source(&state,&namespace,&name).await?;
        let setting=parsed_setting(&source)?;
        let consumers=setting_consumers(&state,&namespace,&name,&setting)?;
        Ok(BoundedJson(serde_json::json!({"success":true,"data":consumers,"settingEnabled":source.pointer("/spec/enable").and_then(Value::as_bool).unwrap_or(true),"tableObservation":"source","consumerObservation":"controller","gatewayApplication":"not-confirmed"})))
    }).await.map_err(|_|StatusCode::GATEWAY_TIMEOUT)?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingPreview {
    namespace: String,
    name: String,
    precondition: crate::conf_mgr::conf_center::source_query::SourcePrecondition,
    region_name: String,
    before: Option<String>,
    after: Option<String>,
    modified: bool,
    consumers: Vec<SettingConsumer>,
    observation: &'static str,
}
fn prepare_failover(
    source: &Value,
    request: &SettingFailoverRequest,
) -> Result<(Value, bool, Option<String>), StatusCode> {
    if source.pointer("/spec/enable").and_then(Value::as_bool) == Some(false) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let mut preview_budget = ProjectionBudget::new();
    preview_budget.reserve(&(&request.region_name, &request.failover_to))?;
    let before = source
        .pointer("/spec/data/config/regions")
        .and_then(Value::as_array)
        .and_then(|regions| {
            regions
                .iter()
                .find(|region| region.get("name").and_then(Value::as_str) == Some(request.region_name.as_str()))
        })
        .and_then(|region| region.get("failoverTo"))
        .and_then(Value::as_str);
    preview_budget.reserve(&before)?;
    let before = before.map(str::to_owned);
    let mut proposed = source.clone();
    let patch = patch_failover_resource(
        &mut proposed,
        "ClusterRegionSetting",
        &request.region_name,
        &request.failover_to,
    );
    if patch == FailoverPatch::Missing {
        return Err(StatusCode::NOT_FOUND);
    }
    validate_failover_resource(&proposed).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    Ok((proposed, patch == FailoverPatch::Changed, before))
}

pub async fn preview_setting_failover(
    State(state): State<Arc<AdminState>>,
    axum::extract::Path((namespace, name)): axum::extract::Path<(String, String)>,
    Json(request): Json<SettingFailoverRequest>,
) -> Result<BoundedJson<Value>, StatusCode> {
    use crate::conf_mgr::conf_center::source_query::{SourcePrecondition, SOURCE_TIMEOUT};
    tokio::time::timeout(SOURCE_TIMEOUT, async {
        let source = setting_source(&state, &namespace, &name).await?;
        let precondition = SourcePrecondition::from_value(&source).map_err(super::common::map_writer_error)?;
        let (proposed, modified, before) = prepare_failover(&source, &request)?;
        let consumers = setting_consumers(&state, &namespace, &name, &parsed_setting(&proposed)?)?;
        let preview = SettingPreview {
            namespace,
            name,
            precondition,
            region_name: request.region_name,
            before,
            after: (!request.failover_to.is_empty()).then_some(request.failover_to),
            modified,
            consumers,
            observation: "source-preview",
        };
        #[derive(Serialize)]
        struct PreviewEnvelope<'a> {
            success: bool,
            data: &'a SettingPreview,
        }
        bounded_json::check(&PreviewEnvelope {
            success: true,
            data: &preview,
        })?;
        Ok(BoundedJson(serde_json::json!({"success":true,"data":preview})))
    })
    .await
    .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
}

pub async fn apply_setting_failover(
    State(state): State<Arc<AdminState>>,
    axum::extract::Path((namespace, name)): axum::extract::Path<(String, String)>,
    Json(request): Json<SettingFailoverApplyRequest>,
) -> Result<BoundedJson<Value>, StatusCode> {
    use crate::conf_mgr::conf_center::source_query::SOURCE_TIMEOUT;
    tokio::time::timeout(SOURCE_TIMEOUT,async {
        let _guard=super::resource_locks::lock_resource(ResourceKind::EdgionConfigData.as_str(),&namespace,&name).await;
        let source=setting_source(&state,&namespace,&name).await?;
        request.precondition.verify(&source.to_string()).map_err(super::common::map_writer_error)?;
        let (mut proposed,modified,_)=prepare_failover(&source,&SettingFailoverRequest {region_name:request.region_name,failover_to:request.failover_to})?;
        let consumers=setting_consumers(&state,&namespace,&name,&parsed_setting(&proposed)?)?;
        bounded_json::check(&serde_json::json!({"success":true,"modified":modified,"persistence":if modified {"persisted"} else {"unchanged"},"controllerObservation":"not-confirmed","gatewayApplication":"not-confirmed","consumers":&consumers}))?;
        if modified {
            if !state.is_k8s_mode() {set_fresh_resource_version(&mut proposed).map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?;}
            state.conf_center().update_one_if_source("EdgionConfigData",Some(&namespace),&name,proposed.to_string(),&request.precondition).await.map_err(super::common::map_writer_error)?;
        }
        // A timeout is an unknown persistence outcome. Callers must read back
        // and create a new preview, never automatically replay this command.
        Ok(BoundedJson(serde_json::json!({"success":true,"modified":modified,"persistence":if modified {"persisted"} else {"unchanged"},"controllerObservation":"not-confirmed","gatewayApplication":"not-confirmed","consumers":consumers})))
    }).await.map_err(|_|StatusCode::GATEWAY_TIMEOUT)?
}

/// The dedicated failover command validates the complete proposed table, even
/// for an idempotent request. Generic resource ingest remains advisory.
fn validate_failover_resource(resource: &Value) -> Result<(), String> {
    let value = resource
        .pointer("/spec/data/config")
        .ok_or_else(|| "ClusterRegionSetting config is missing".to_string())?;
    let setting: ClusterRegionSetting =
        serde_json::from_value(value.clone()).map_err(|_| "invalid ClusterRegionSetting".to_string())?;
    setting.validate_and_init()
}

/// Assign a newly allocated `metadata.resourceVersion` to a resource that is
/// about to be written to a non-Kubernetes configuration center.
fn set_fresh_resource_version(resource: &mut Value) -> Result<(), String> {
    let metadata = resource
        .pointer_mut("/metadata")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "resource metadata must be an object".to_string())?;
    let version = distinct_resource_version(
        metadata.get("resourceVersion").and_then(Value::as_str),
        crate::conf_mgr::sync_runtime::resource_version::next_resource_version,
    );
    metadata.insert("resourceVersion".to_string(), Value::String(version));
    Ok(())
}

fn distinct_resource_version(current: Option<&str>, mut allocate: impl FnMut() -> u64) -> String {
    let first = allocate().to_string();
    if current == Some(first.as_str()) {
        allocate().to_string()
    } else {
        first
    }
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
