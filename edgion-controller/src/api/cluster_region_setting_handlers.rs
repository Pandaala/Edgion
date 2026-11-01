//! Source-only setting discovery. Directory keys never include routing tables.
use super::bounded_json::BoundedJson as Json;
use super::namespaced_handlers::PaginationQuery;
use super::types::{AdminState, ListResponse};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
};
use edgion_resources::{resources::edgion_config_data::region_route::validate_setting_identity, ResourceKind};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingKey {
    namespace: String,
    name: String,
    uid: Option<String>,
    resource_version: Option<String>,
    cluster_name: Option<String>,
    region_name: Option<String>,
    identity_diagnostic: Option<IdentityDiagnostic>,
}
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum IdentityDiagnostic {
    InvalidIdentity,
}

fn bounded_field(value: &Value, pointer: &str, limit: usize, required: bool) -> Result<Option<String>, StatusCode> {
    match value.pointer(pointer) {
        Some(Value::String(text)) if !text.is_empty() && text.len() <= limit && !text.chars().any(char::is_control) => {
            Ok(Some(text.clone()))
        }
        None | Some(Value::Null) if !required => Ok(None),
        _ => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
fn project(value: Value) -> Result<Option<SettingKey>, StatusCode> {
    if value.pointer("/spec/data/type").and_then(Value::as_str) != Some("ClusterRegionSetting") {
        return Ok(None);
    }
    let cluster_name = value.pointer("/spec/data/config/clusterName").and_then(Value::as_str);
    let region_name = value.pointer("/spec/data/config/regionName").and_then(Value::as_str);
    let identity = cluster_name
        .zip(region_name)
        .filter(|(cluster, region)| validate_setting_identity(cluster, region).is_ok());
    Ok(Some(SettingKey {
        namespace: bounded_field(&value, "/metadata/namespace", 63, true)?.ok_or(StatusCode::INTERNAL_SERVER_ERROR)?,
        name: bounded_field(&value, "/metadata/name", 253, true)?.ok_or(StatusCode::INTERNAL_SERVER_ERROR)?,
        uid: bounded_field(&value, "/metadata/uid", 128, false)?,
        resource_version: bounded_field(&value, "/metadata/resourceVersion", 256, false)?,
        cluster_name: identity.map(|(cluster, _)| cluster.to_owned()),
        region_name: identity.map(|(_, region)| region.to_owned()),
        identity_diagnostic: identity.is_none().then_some(IdentityDiagnostic::InvalidIdentity),
    }))
}

/// Scan one bounded source page; an empty matching page may still continue.
pub async fn keys(
    State(state): State<Arc<AdminState>>,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<ListResponse<SettingKey>>, StatusCode> {
    let (values, cursor) = state
        .center_list_resources(ResourceKind::EdgionConfigData, query.into_list_options()?)
        .await?;
    let rows = values
        .into_iter()
        .map(project)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    Ok(Json(ListResponse::success_with_token(rows, cursor)))
}

pub async fn detail(
    State(state): State<Arc<AdminState>>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<Value>, StatusCode> {
    let value = state
        .center_get_resource(ResourceKind::EdgionConfigData, &namespace, &name)
        .await?
        .ok_or(StatusCode::NOT_FOUND)?;
    if value.pointer("/spec/data/type").and_then(Value::as_str) != Some("ClusterRegionSetting") {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(value))
}

#[cfg(test)]
#[path = "../../tests/unit/api/cluster_region_setting_handlers_test.rs"]
mod tests;
