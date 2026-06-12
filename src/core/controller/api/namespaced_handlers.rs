use crate::core::controller::conf_mgr::ListOptions;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header::LOCATION, HeaderMap, HeaderValue, StatusCode},
    response::Json,
};
use edgion_resources::prelude_resources::*;
use k8s_openapi::api::core::v1::{ConfigMap, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use serde::Deserialize;
use std::sync::Arc;

use super::common::{map_writer_error, parse_kind, parse_resource_and_update_version, validate_resource};
use super::types::*;

/// Pagination query parameters for list endpoints.
#[derive(Deserialize, Default)]
pub struct PaginationQuery {
    /// Maximum number of items to return (0 or absent = no limit).
    pub limit: Option<u32>,
    /// Continue token from a previous list response.
    #[serde(rename = "continue")]
    pub continue_token: Option<String>,
}

impl PaginationQuery {
    pub(super) fn into_list_options(self) -> Option<ListOptions> {
        if self.limit.is_some() || self.continue_token.is_some() {
            Some(ListOptions {
                limit: self.limit.unwrap_or(0),
                continue_token: self.continue_token,
            })
        } else {
            None
        }
    }
}

/// List all resources of a kind across all namespaces (from ConfCenter/storage)
pub async fn list_all_namespaces(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let (data, continue_token) = state
        .center_list_resources(kind, pagination.into_list_options())
        .await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

/// List namespace-scoped resources (from ConfCenter/storage)
pub async fn list_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns)): Path<(String, String)>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let (data, continue_token) = state
        .center_list_resources_namespaced(kind, &ns, pagination.into_list_options())
        .await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

/// Get a namespace-scoped resource (from ConfCenter/storage)
pub async fn get_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns, name)): Path<(String, String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let resource = state.center_get_resource(kind, &ns, &name).await?;
    resource.map(Json).ok_or(StatusCode::NOT_FOUND)
}

/// Create a namespace-scoped resource.
pub async fn create_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns)): Path<(String, String)>,
    body: Bytes,
) -> Result<(StatusCode, HeaderMap, Json<ApiResponse<String>>), StatusCode> {
    tracing::info!(
        component = "unified_api",
        event = "create_request",
        kind = %kind_str,
        namespace = %ns,
        body_len = body.len(),
        "Received create request"
    );

    let kind = parse_kind(&kind_str).map_err(|e| {
        tracing::warn!("Failed to parse kind: {}", e);
        StatusCode::BAD_REQUEST
    })?;

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();

    let content = String::from_utf8(body.to_vec()).map_err(|e| {
        tracing::warn!("Failed to parse body as UTF-8: {}", e);
        StatusCode::BAD_REQUEST
    })?;

    let metadata = edgion_common::utils::extract_resource_metadata(&content).ok_or_else(|| {
        tracing::warn!("Failed to extract metadata from request body");
        StatusCode::BAD_REQUEST
    })?;

    // The URL namespace is the authoritative storage key, but the body content is
    // what gets persisted. Reject a body whose metadata.namespace contradicts the
    // URL, otherwise the resource would be stored under the URL namespace while its
    // content claims a different one (create_one keys the path on the URL namespace
    // in non-K8s mode), desynchronizing storage and the running cache. Absent body
    // namespace defers to the URL value. Mirrors the guard in update_namespaced.
    // (K8s API server rejects the same mismatch.) The collection POST takes the name
    // from the body by design, so only the namespace dimension is validated here.
    if metadata.namespace.as_ref().is_some_and(|meta_ns| meta_ns != &ns) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let name = metadata.name.ok_or_else(|| {
        tracing::warn!("Resource name is missing in metadata");
        StatusCode::BAD_REQUEST
    })?;

    tracing::info!("Extracted resource name: {}", name);

    // Parse, validate, and persist using create_one (fails if exists)
    // In K8s mode: skip validation (K8s API Server validates) and don't update version
    // In non-K8s mode: validate and update resource version
    match kind {
        edgion_resources::ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::Service => {
            let service: Service = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &service, is_k8s)?;
            let json_content = serde_json::to_string(&service).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &ep, is_k8s)?;
            let json_content = serde_json::to_string(&ep).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &tls, is_k8s)?;
            let json_content = serde_json::to_string(&tls).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &plugins, is_k8s)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &metadata, is_k8s)?;
            let json_content = serde_json::to_string(&metadata).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &linksys, is_k8s)?;
            let json_content = serde_json::to_string(&linksys).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::Secret => {
            let secret: Secret = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &secret, is_k8s)?;
            let json_content = serde_json::to_string(&secret).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::ConfigMap => {
            let cm: ConfigMap = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &cm, is_k8s)?;
            let json_content = serde_json::to_string(&cm).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::Gateway => {
            let gateway: Gateway = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &gateway, is_k8s)?;
            let json_content = serde_json::to_string(&gateway).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::ReferenceGrant => {
            let rg: ReferenceGrant = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &rg, is_k8s)?;
            let json_content = serde_json::to_string(&rg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::BackendTLSPolicy => {
            let policy: BackendTLSPolicy = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &policy, is_k8s)?;
            let json_content = serde_json::to_string(&policy).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionStreamPlugins => {
            let plugins: EdgionStreamPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &plugins, is_k8s)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionAcme => {
            let acme: EdgionAcme = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &acme, is_k8s)?;
            let json_content = serde_json::to_string(&acme).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        // 400, not 501: unknown kind is a client error; 501 is reserved for unknown HTTP methods.
        _ => return Err(StatusCode::BAD_REQUEST),
    }

    tracing::info!(
        component = "unified_api",
        event = "resource_created",
        kind = %kind_str,
        namespace = %ns,
        name = %name,
        is_k8s_mode = is_k8s,
        "Resource created successfully"
    );

    let location = format!("/api/v1/namespaced/{}/{}/{}", kind_str, ns, name);
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::try_from(location) {
        headers.insert(LOCATION, v);
    }
    Ok((
        StatusCode::CREATED,
        headers,
        Json(ApiResponse::ok_body(format!("{} created", kind_str))),
    ))
}

/// Update a namespace-scoped resource
pub async fn update_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns, name)): Path<(String, String, String)>,
    body: Bytes,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();

    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;

    // The URL name/namespace are the authoritative storage keys, but the body
    // content is what gets persisted. Reject a body whose metadata identity
    // contradicts the URL, otherwise the resource would be stored under the URL
    // name while its content claims a different name (the writer keys the path on
    // the URL name in non-K8s mode), desynchronizing storage and the running
    // cache. Absent body name/namespace defers to the URL value. (K8s API server
    // rejects the same mismatch.)
    if let Some(metadata) = edgion_common::utils::extract_resource_metadata(&content) {
        if metadata.name.is_some_and(|meta_name| meta_name != name) {
            return Err(StatusCode::BAD_REQUEST);
        }
        if metadata.namespace.is_some_and(|meta_ns| meta_ns != ns) {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    // Parse, validate, and persist using update_one (fails if not exists)
    // In K8s mode: skip validation (K8s API Server validates) and don't update version
    // In non-K8s mode: validate and update resource version
    match kind {
        edgion_resources::ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::Service => {
            let service: Service = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &service, is_k8s)?;
            let json_content = serde_json::to_string(&service).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &ep, is_k8s)?;
            let json_content = serde_json::to_string(&ep).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &tls, is_k8s)?;
            let json_content = serde_json::to_string(&tls).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &plugins, is_k8s)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::PluginMetaData => {
            let metadata: PluginMetaData = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &metadata, is_k8s)?;
            let json_content = serde_json::to_string(&metadata).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            // Serialize this whole-resource replace against the failover
            // read-modify-write on the same PluginMetaData, which would otherwise
            // interleave and silently lose an update. Held across the write below.
            // See `super::resource_locks` and
            // `skills/04-review/concurrency/failover-read-modify-write-per-key-lock.md`.
            let _resource_guard = super::resource_locks::lock_resource(kind.as_str(), &ns, &name).await;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &linksys, is_k8s)?;
            let json_content = serde_json::to_string(&linksys).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::Secret => {
            let secret: Secret = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &secret, is_k8s)?;
            let json_content = serde_json::to_string(&secret).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::ConfigMap => {
            let cm: ConfigMap = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &cm, is_k8s)?;
            let json_content = serde_json::to_string(&cm).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::Gateway => {
            let gateway: Gateway = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &gateway, is_k8s)?;
            let json_content = serde_json::to_string(&gateway).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::ReferenceGrant => {
            let rg: ReferenceGrant = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &rg, is_k8s)?;
            let json_content = serde_json::to_string(&rg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::BackendTLSPolicy => {
            let policy: BackendTLSPolicy = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &policy, is_k8s)?;
            let json_content = serde_json::to_string(&policy).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionStreamPlugins => {
            let plugins: EdgionStreamPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &plugins, is_k8s)?;
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionAcme => {
            let acme: EdgionAcme = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &acme, is_k8s)?;
            let json_content = serde_json::to_string(&acme).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    }

    tracing::info!(
        component = "unified_api",
        event = "resource_updated",
        kind = %kind_str,
        namespace = %ns,
        name = %name,
        is_k8s_mode = is_k8s,
        "Resource updated successfully"
    );

    Ok(Json(ApiResponse::ok_body(format!("{} updated", kind_str))))
}

/// Delete a namespace-scoped resource.
pub async fn delete_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns, name)): Path<(String, String, String)>,
) -> Result<StatusCode, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();

    // Delete from backend - delete_one will return NotFound if resource doesn't exist
    writer
        .delete_one(kind.as_str(), Some(&ns), &name)
        .await
        .map_err(map_writer_error)?;

    tracing::info!(
        component = "unified_api",
        event = "resource_deleted",
        kind = %kind_str,
        namespace = %ns,
        name = %name,
        is_k8s_mode = is_k8s,
        "Resource deleted successfully"
    );

    Ok(StatusCode::NO_CONTENT)
}

/// List metadata-only keys for a kind across all namespaces (keys endpoint)
pub async fn list_all_namespaces_keys(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Json<ListResponse<crate::core::controller::conf_mgr::conf_center::traits::ResourceKey>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let (data, continue_token) = state.center_list_keys(kind, pagination.into_list_options()).await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

/// List metadata-only keys for a kind scoped to a namespace (keys endpoint)
pub async fn list_namespaced_keys(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns)): Path<(String, String)>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Json<ListResponse<crate::core::controller::conf_mgr::conf_center::traits::ResourceKey>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let (data, continue_token) = state
        .center_list_keys_namespaced(kind, &ns, pagination.into_list_options())
        .await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

#[cfg(test)]
mod update_identity_tests {
    use super::*;
    use crate::core::controller::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

    /// Build a minimal FileSystem-backed AdminState (non-K8s mode, the mode in
    /// which the URL-vs-body name desync is observable). The returned TempDir
    /// must be kept alive for the storage path to stay valid.
    async fn make_fs_state() -> (Arc<AdminState>, tempfile::TempDir) {
        let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
        let fs_config = FileSystemConfig::new(tmp_data.path().to_path_buf());
        let conf_mgr = Arc::new(
            ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
                .await
                .expect("ConfMgr::create"),
        );
        let schema_validator = Arc::new(SchemaValidator::empty());
        let (tx, _rx) = tokio::sync::watch::channel(edgion_common::config::CenterClientConfig::default());
        let authorizer = Arc::new(crate::core::controller::authz::Authorizer::new(
            crate::core::controller::authz::CenterPolicy::deny_all(),
        ));
        let state = Arc::new(AdminState {
            conf_mgr,
            schema_validator,
            config_file: None,
            authorizer,
            center_reload_tx: tx,
        });
        (state, tmp_data)
    }

    /// A minimal HTTPRoute body; HTTPRouteSpec fields are all optional so `spec:{}` parses.
    fn http_route_body(name: &str, namespace: &str) -> Bytes {
        Bytes::from(format!(
            r#"{{"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute","metadata":{{"name":"{name}","namespace":"{namespace}"}},"spec":{{}}}}"#
        ))
    }

    #[tokio::test]
    async fn update_rejects_mismatched_name() {
        let (state, _tmp) = make_fs_state().await;
        let result = update_namespaced(
            State(state),
            Path(("HTTPRoute".to_string(), "default".to_string(), "route-a".to_string())),
            http_route_body("route-b", "default"),
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    }

    #[tokio::test]
    async fn update_rejects_mismatched_namespace() {
        let (state, _tmp) = make_fs_state().await;
        let result = update_namespaced(
            State(state),
            Path(("HTTPRoute".to_string(), "default".to_string(), "route-a".to_string())),
            http_route_body("route-a", "other-ns"),
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    }

    #[tokio::test]
    async fn update_matching_identity_passes_guard() {
        // Identity matches, so the guard must NOT reject; the request reaches the
        // writer, which returns NotFound (404) for the absent resource -- proving
        // the guard does not false-positive on a valid identity.
        let (state, _tmp) = make_fs_state().await;
        let result = update_namespaced(
            State(state),
            Path(("HTTPRoute".to_string(), "default".to_string(), "route-a".to_string())),
            http_route_body("route-a", "default"),
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::NOT_FOUND));
    }

    #[tokio::test]
    async fn create_rejects_mismatched_namespace() {
        // Body metadata.namespace ("other-ns") disagrees with the URL namespace
        // ("default"); the create-path guard must reject before persisting,
        // mirroring update_namespaced.
        let (state, _tmp) = make_fs_state().await;
        let result = create_namespaced(
            State(state),
            Path(("HTTPRoute".to_string(), "default".to_string())),
            http_route_body("route-a", "other-ns"),
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    }

    #[tokio::test]
    async fn create_matching_namespace_passes_guard() {
        // Body metadata.namespace matches the URL namespace, so the guard must NOT
        // reject; the request reaches the writer and the resource is created (201),
        // proving the guard does not false-positive on a valid identity.
        let (state, _tmp) = make_fs_state().await;
        let result = create_namespaced(
            State(state),
            Path(("HTTPRoute".to_string(), "default".to_string())),
            http_route_body("route-a", "default"),
        )
        .await;
        let (status, _headers, _body) = result.expect("create should succeed on matching identity");
        assert_eq!(status, StatusCode::CREATED);
    }
}
