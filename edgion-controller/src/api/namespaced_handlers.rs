use crate::conf_mgr::ListOptions;
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

use super::common::{
    expected_resource_version, map_writer_error, parse_kind, parse_resource_and_update_version, unknown_field_warnings,
    validate_gateway_api_raw, validate_http_route_transforms, validate_resource,
};
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

    validate_gateway_api_raw(state.schema_validator.as_ref(), kind, &content, is_k8s)?;

    let metadata = edgion_resources::resource::meta::extract_resource_metadata(&content).ok_or_else(|| {
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
    //
    // Populated only by the EdgionPlugins/EdgionStreamPlugins/EdgionConfigData/LinkSys arms below
    // (`unknown_field_warnings`); every other kind leaves this empty, so
    // `ApiResponse::warnings` stays absent from the response (see `types.rs`).
    let mut warnings: Vec<String> = Vec::new();
    match kind {
        edgion_resources::ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            validate_http_route_transforms(&route)?;
            let json_content = if is_k8s {
                // Preserve future source fields for the API server's selected
                // version while still validating the canonical route above.
                content.clone()
            } else {
                serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            };
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
            warnings = unknown_field_warnings(kind, &content);
            if !warnings.is_empty() {
                tracing::warn!(
                    component = "unified_api",
                    event = "unknown_config_fields",
                    kind = %kind_str,
                    namespace = %ns,
                    name = %name,
                    paths = ?warnings,
                    "Resource contains keys not recognized by the schema"
                );
            }
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionConfigData => {
            let metadata: EdgionConfigData = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &metadata, is_k8s)?;
            warnings = unknown_field_warnings(kind, &content);
            if !warnings.is_empty() {
                tracing::warn!(
                    component = "unified_api",
                    event = "unknown_config_fields",
                    kind = %kind_str,
                    namespace = %ns,
                    name = %name,
                    paths = ?warnings,
                    "Resource contains keys not recognized by the typed schema"
                );
            }
            let json_content = serde_json::to_string(&metadata).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &linksys, is_k8s)?;
            warnings = unknown_field_warnings(kind, &content);
            if !warnings.is_empty() {
                tracing::warn!(
                    component = "unified_api",
                    event = "unknown_config_fields",
                    kind = %kind_str,
                    namespace = %ns,
                    name = %name,
                    paths = ?warnings,
                    "Resource contains keys not recognized by the typed schema"
                );
            }
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
            let json_content = if is_k8s {
                content.clone()
            } else {
                serde_json::to_string(&rg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            };
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
            warnings = unknown_field_warnings(kind, &content);
            if !warnings.is_empty() {
                tracing::warn!(
                    component = "unified_api",
                    event = "unknown_config_fields",
                    kind = %kind_str,
                    namespace = %ns,
                    name = %name,
                    paths = ?warnings,
                    "Resource contains keys not recognized by the schema"
                );
            }
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
        edgion_resources::ResourceKind::EdgionBackendTrafficPolicy => {
            let policy: EdgionBackendTrafficPolicy = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &policy, is_k8s)?;
            let json_content = serde_json::to_string(&policy).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), Some(&ns), &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionBackend => {
            let backend: EdgionBackend = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &backend, is_k8s)?;
            let json_content = serde_json::to_string(&backend).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
        Json(ApiResponse::ok_body_with_warnings(
            format!("{} created", kind_str),
            warnings,
        )),
    ))
}

/// Update a namespace-scoped resource
pub async fn update_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns, name)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();

    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;
    validate_gateway_api_raw(state.schema_validator.as_ref(), kind, &content, is_k8s)?;
    // Capture the client's precondition before non-Kubernetes parsing assigns
    // the new resourceVersion that will be persisted by this update.
    let expected_resource_version = expected_resource_version(&headers, &content)?;

    // The URL name/namespace are the authoritative storage keys, but the body
    // content is what gets persisted. Reject a body whose metadata identity
    // contradicts the URL, otherwise the resource would be stored under the URL
    // name while its content claims a different name (the writer keys the path on
    // the URL name in non-K8s mode), desynchronizing storage and the running
    // cache. Absent body name/namespace defers to the URL value. (K8s API server
    // rejects the same mismatch.)
    if let Some(metadata) = edgion_resources::resource::meta::extract_resource_metadata(&content) {
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
    //
    // Populated only by the EdgionPlugins/EdgionStreamPlugins/EdgionConfigData/LinkSys arms below
    // (`unknown_field_warnings`); every other kind leaves this empty, so
    // `ApiResponse::warnings` stays absent from the response (see `types.rs`).
    let mut warnings: Vec<String> = Vec::new();
    match kind {
        edgion_resources::ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            validate_http_route_transforms(&route)?;
            let json_content = if is_k8s {
                content.clone()
            } else {
                serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            };
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::GRPCRoute => {
            let route: GRPCRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::TCPRoute => {
            let route: TCPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::UDPRoute => {
            let route: UDPRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::TLSRoute => {
            let route: TLSRoute = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &route, is_k8s)?;
            let json_content = serde_json::to_string(&route).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::Service => {
            let service: Service = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &service, is_k8s)?;
            let json_content = serde_json::to_string(&service).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EndpointSlice => {
            let ep: EndpointSlice = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &ep, is_k8s)?;
            let json_content = serde_json::to_string(&ep).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionTls => {
            let tls: EdgionTls = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &tls, is_k8s)?;
            let json_content = serde_json::to_string(&tls).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &plugins, is_k8s)?;
            warnings = unknown_field_warnings(kind, &content);
            if !warnings.is_empty() {
                tracing::warn!(
                    component = "unified_api",
                    event = "unknown_config_fields",
                    kind = %kind_str,
                    namespace = %ns,
                    name = %name,
                    paths = ?warnings,
                    "Resource contains keys not recognized by the schema"
                );
            }
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionConfigData => {
            let metadata: EdgionConfigData = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &metadata, is_k8s)?;
            warnings = unknown_field_warnings(kind, &content);
            if !warnings.is_empty() {
                tracing::warn!(
                    component = "unified_api",
                    event = "unknown_config_fields",
                    kind = %kind_str,
                    namespace = %ns,
                    name = %name,
                    paths = ?warnings,
                    "Resource contains keys not recognized by the typed schema"
                );
            }
            let json_content = serde_json::to_string(&metadata).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            // Serialize this whole-resource replace against the failover
            // read-modify-write on the same EdgionConfigData, which would otherwise
            // interleave and silently lose an update. Held across the write below.
            // See `super::resource_locks` and
            // `skills/04-review/concurrency/regression-guards/failover-read-modify-write-per-key-lock.md`.
            let _resource_guard = super::resource_locks::lock_resource(kind.as_str(), &ns, &name).await;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &linksys, is_k8s)?;
            warnings = unknown_field_warnings(kind, &content);
            if !warnings.is_empty() {
                tracing::warn!(
                    component = "unified_api",
                    event = "unknown_config_fields",
                    kind = %kind_str,
                    namespace = %ns,
                    name = %name,
                    paths = ?warnings,
                    "Resource contains keys not recognized by the typed schema"
                );
            }
            let json_content = serde_json::to_string(&linksys).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::Secret => {
            let secret: Secret = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &secret, is_k8s)?;
            let json_content = serde_json::to_string(&secret).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::ConfigMap => {
            let cm: ConfigMap = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &cm, is_k8s)?;
            let json_content = serde_json::to_string(&cm).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::Gateway => {
            let gateway: Gateway = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &gateway, is_k8s)?;
            let json_content = serde_json::to_string(&gateway).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::ReferenceGrant => {
            let rg: ReferenceGrant = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &rg, is_k8s)?;
            let json_content = if is_k8s {
                content.clone()
            } else {
                serde_json::to_string(&rg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            };
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::BackendTLSPolicy => {
            let policy: BackendTLSPolicy = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &policy, is_k8s)?;
            let json_content = serde_json::to_string(&policy).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionStreamPlugins => {
            let plugins: EdgionStreamPlugins = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &plugins, is_k8s)?;
            warnings = unknown_field_warnings(kind, &content);
            if !warnings.is_empty() {
                tracing::warn!(
                    component = "unified_api",
                    event = "unknown_config_fields",
                    kind = %kind_str,
                    namespace = %ns,
                    name = %name,
                    paths = ?warnings,
                    "Resource contains keys not recognized by the schema"
                );
            }
            let json_content = serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionAcme => {
            let acme: EdgionAcme = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &acme, is_k8s)?;
            let json_content = serde_json::to_string(&acme).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionBackendTrafficPolicy => {
            let policy: EdgionBackendTrafficPolicy = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &policy, is_k8s)?;
            let json_content = serde_json::to_string(&policy).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionBackend => {
            let backend: EdgionBackend = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &backend, is_k8s)?;
            let json_content = serde_json::to_string(&backend).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    Some(&ns),
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
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

    Ok(Json(ApiResponse::ok_body_with_warnings(
        format!("{} updated", kind_str),
        warnings,
    )))
}

/// Delete a namespace-scoped resource.
pub async fn delete_namespaced(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns, name)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();

    // delete_one returns NotFound if the resource does not exist.
    let expected_resource_version = expected_resource_version(&headers, "{}")?;
    writer
        .delete_one_if_version(kind.as_str(), Some(&ns), &name, expected_resource_version.as_deref())
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
) -> Result<Json<ListResponse<crate::conf_mgr::conf_center::traits::ResourceKey>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let (data, continue_token) = state.center_list_keys(kind, pagination.into_list_options()).await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

/// List metadata-only keys for a kind scoped to a namespace (keys endpoint)
pub async fn list_namespaced_keys(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, ns)): Path<(String, String)>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Json<ListResponse<crate::conf_mgr::conf_center::traits::ResourceKey>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let (data, continue_token) = state
        .center_list_keys_namespaced(kind, &ns, pagination.into_list_options())
        .await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

#[cfg(test)]
mod update_identity_tests {
    use super::*;
    use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

    /// Build a minimal FileSystem-backed AdminState (non-K8s mode, the mode in
    /// which the URL-vs-body name desync is observable). The returned TempDir
    /// must be kept alive for the storage path to stay valid.
    pub(super) async fn make_fs_state() -> (Arc<AdminState>, tempfile::TempDir) {
        let tmp_data = tempfile::tempdir().expect("tempdir for conf data");
        let fs_config =
            FileSystemConfig::new(tmp_data.path().to_path_buf()).with_controller_name("edgion.io/gateway-controller");
        let conf_mgr = Arc::new(
            ConfMgr::create(ConfCenterConfig::FileSystem(fs_config))
                .await
                .expect("ConfMgr::create"),
        );
        let schema_validator = Arc::new(SchemaValidator::empty());
        let (tx, _rx) = tokio::sync::watch::channel(crate::config::CenterClientConfig::default());
        let authorizer = Arc::new(crate::authz::Authorizer::new(crate::authz::CenterPolicy::deny_all()));
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

    fn backend_tls_policy_body(name: &str, api_version: &str) -> Bytes {
        Bytes::from(format!(
            r#"{{
                "apiVersion":"{api_version}",
                "kind":"BackendTLSPolicy",
                "metadata":{{"name":"{name}","namespace":"default"}},
                "spec":{{
                    "targetRefs":[{{"group":"","kind":"Service","name":"backend"}}],
                    "validation":{{"hostname":"backend.default.svc","wellKnownCACertificates":"System"}}
                }}
            }}"#
        ))
    }

    #[tokio::test]
    async fn update_rejects_mismatched_name() {
        let (state, _tmp) = make_fs_state().await;
        let result = update_namespaced(
            State(state),
            Path(("HTTPRoute".to_string(), "default".to_string(), "route-a".to_string())),
            HeaderMap::new(),
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
            HeaderMap::new(),
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
            HeaderMap::new(),
            http_route_body("route-a", "default"),
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::NOT_FOUND));
    }

    #[tokio::test]
    async fn stale_if_match_returns_conflict_and_keeps_current_resource() {
        let (state, _tmp) = make_fs_state().await;
        let current = r#"{"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute","metadata":{"name":"route-a","namespace":"default","resourceVersion":"42"},"spec":{}}"#;
        state
            .conf_center()
            .create_one("HTTPRoute", Some("default"), "route-a", current.to_string())
            .await
            .expect("seed resource must be created");
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::IF_MATCH, r#""41""#.parse().unwrap());

        let result = update_namespaced(
            State(Arc::clone(&state)),
            Path(("HTTPRoute".to_string(), "default".to_string(), "route-a".to_string())),
            headers,
            http_route_body("route-a", "default"),
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::CONFLICT));
        let stored = state
            .conf_center()
            .get_one("HTTPRoute", Some("default"), "route-a")
            .await
            .expect("current resource must remain readable");
        let stored: serde_yaml::Value = serde_yaml::from_str(&stored).unwrap();
        assert_eq!(stored["metadata"]["resourceVersion"], "42");
    }

    #[tokio::test]
    async fn stale_delete_if_match_returns_conflict_and_keeps_current_resource() {
        let (state, _tmp) = make_fs_state().await;
        let current = r#"{"apiVersion":"gateway.networking.k8s.io/v1","kind":"HTTPRoute","metadata":{"name":"route-a","namespace":"default","resourceVersion":"42"},"spec":{}}"#;
        state
            .conf_center()
            .create_one("HTTPRoute", Some("default"), "route-a", current.to_string())
            .await
            .expect("seed resource must be created");
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::IF_MATCH, r#""41""#.parse().unwrap());

        let result = delete_namespaced(
            State(Arc::clone(&state)),
            Path(("HTTPRoute".to_string(), "default".to_string(), "route-a".to_string())),
            headers,
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::CONFLICT));
        assert!(state
            .conf_center()
            .get_one("HTTPRoute", Some("default"), "route-a")
            .await
            .is_ok());
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

    #[tokio::test]
    async fn create_rejects_non_v1_backend_tls_policy_before_persistence() {
        let (state, _tmp) = make_fs_state().await;
        let result = create_namespaced(
            State(Arc::clone(&state)),
            Path(("BackendTLSPolicy".to_string(), "default".to_string())),
            backend_tls_policy_body("backend-tls", "gateway.networking.k8s.io/v1alpha3"),
        )
        .await;

        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
        assert!(state
            .conf_center()
            .get_one("BackendTLSPolicy", Some("default"), "backend-tls")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn tls_route_create_and_update_reject_invalid_versions_before_persistence() {
        let (state, _tmp) = make_fs_state().await;
        let canonical = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "TLSRoute",
            "metadata": {"name": "tls-route", "namespace": "default"},
            "spec": {"hostnames": ["example.com"]}
        });
        for version in [
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::json!(42)),
            Some(serde_json::json!("gateway.networking.k8s.io/v1alpha3")),
        ] {
            let mut invalid = canonical.clone();
            match version {
                Some(value) => invalid["apiVersion"] = value,
                None => {
                    invalid.as_object_mut().unwrap().remove("apiVersion");
                }
            }
            let result = create_namespaced(
                State(state.clone()),
                Path(("TLSRoute".to_string(), "default".to_string())),
                Bytes::from(invalid.to_string()),
            )
            .await;
            assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
        }
        assert!(state
            .conf_center()
            .get_one("TLSRoute", Some("default"), "tls-route")
            .await
            .is_err());
        let (status, _, _) = create_namespaced(
            State(state.clone()),
            Path(("TLSRoute".to_string(), "default".to_string())),
            Bytes::from(canonical.to_string()),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::CREATED);
        let mut invalid = canonical;
        invalid["apiVersion"] = serde_json::json!("gateway.networking.k8s.io/v1alpha3");
        let result = update_namespaced(
            State(state.clone()),
            Path(("TLSRoute".to_string(), "default".to_string(), "tls-route".to_string())),
            HeaderMap::new(),
            Bytes::from(invalid.to_string()),
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
        let stored = state
            .conf_center()
            .get_one("TLSRoute", Some("default"), "tls-route")
            .await
            .unwrap();
        let stored: serde_yaml::Value = serde_yaml::from_str(&stored).unwrap();
        assert_eq!(stored["apiVersion"], "gateway.networking.k8s.io/v1");
    }

    #[tokio::test]
    async fn update_rejects_non_v1_backend_tls_policy_and_keeps_canonical_object() {
        let (state, _tmp) = make_fs_state().await;
        let (status, _headers, _response) = create_namespaced(
            State(Arc::clone(&state)),
            Path(("BackendTLSPolicy".to_string(), "default".to_string())),
            backend_tls_policy_body("backend-tls", "gateway.networking.k8s.io/v1"),
        )
        .await
        .expect("canonical BackendTLSPolicy create must succeed");
        assert_eq!(status, StatusCode::CREATED);

        let result = update_namespaced(
            State(Arc::clone(&state)),
            Path((
                "BackendTLSPolicy".to_string(),
                "default".to_string(),
                "backend-tls".to_string(),
            )),
            HeaderMap::new(),
            backend_tls_policy_body("backend-tls", "gateway.networking.k8s.io/v1alpha3"),
        )
        .await;

        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
        let stored = state
            .conf_center()
            .get_one("BackendTLSPolicy", Some("default"), "backend-tls")
            .await
            .expect("canonical object must remain stored");
        let stored: serde_yaml::Value = serde_yaml::from_str(&stored).unwrap();
        assert_eq!(stored["apiVersion"], "gateway.networking.k8s.io/v1");
    }
}

/// The Admin API write path warns (never rejects) on an unrecognized config
/// key and surfaces the finding through `ApiResponse::warnings`.
#[cfg(test)]
mod unknown_field_warning_tests {
    use super::update_identity_tests::make_fs_state;
    use super::*;

    /// `config.denys` is a typo for `config.deny` on `IpRestrictionConfig`.
    fn plugins_body_with_typo(name: &str, namespace: &str) -> Bytes {
        Bytes::from(format!(
            r#"{{"apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{{"name":"{name}","namespace":"{namespace}"}},"spec":{{"requestPlugins":[{{"type":"IpRestriction","config":{{"defaultAction":"allow","denys":[]}}}}]}}}}"#
        ))
    }

    fn plugins_body_clean(name: &str, namespace: &str) -> Bytes {
        Bytes::from(format!(
            r#"{{"apiVersion":"edgion.io/v1","kind":"EdgionPlugins","metadata":{{"name":"{name}","namespace":"{namespace}"}},"spec":{{"requestPlugins":[{{"type":"IpRestriction","config":{{"defaultAction":"allow","deny":[]}}}}]}}}}"#
        ))
    }

    fn config_data_body(name: &str, typo: bool) -> Bytes {
        let typo_field = if typo { r#","cidrz":[]"# } else { "" };
        Bytes::from(format!(
            r#"{{"apiVersion":"edgion.io/v1","kind":"EdgionConfigData","metadata":{{"name":"{name}","namespace":"default"}},"spec":{{"data":{{"type":"IpList","config":{{"items":[{{"name":"office","cidrs":[]{typo_field}}}]}}}}}}}}"#
        ))
    }

    fn link_sys_body(name: &str, typo: bool) -> Bytes {
        let typo_field = if typo { r#", "conect": 20"# } else { "" };
        Bytes::from(
            r#"{"apiVersion":"edgion.io/v1","kind":"LinkSys","metadata":{"name":"NAME","namespace":"default"},"spec":{"type":"redis","config":{"endpoints":["redis://127.0.0.1:6379"],"timeout":{"connect":"3s"TYPO}}}}"#
                .replace("NAME", name)
                .replace("TYPO", typo_field),
        )
    }

    #[tokio::test]
    async fn create_with_typo_is_not_rejected_and_carries_warning() {
        let (state, _tmp) = make_fs_state().await;
        let result = create_namespaced(
            State(state),
            Path(("EdgionPlugins".to_string(), "default".to_string())),
            plugins_body_with_typo("auth-chain", "default"),
        )
        .await;

        let (status, _headers, body) = result.expect("a typo'd key must warn, never reject the write");
        assert_eq!(
            status,
            StatusCode::CREATED,
            "warn-not-reject: the write must still succeed"
        );
        assert_eq!(body.0.warnings, vec!["requestPlugins[0].config.denys".to_string()]);

        let serialized = serde_json::to_value(&body.0).expect("ApiResponse must serialize");
        assert_eq!(
            serialized["warnings"][0], "requestPlugins[0].config.denys",
            "the finding must also be visible in the serialized payload"
        );
    }

    #[tokio::test]
    async fn create_with_clean_document_has_no_warnings_key_in_serialized_payload() {
        let (state, _tmp) = make_fs_state().await;
        let result = create_namespaced(
            State(state),
            Path(("EdgionPlugins".to_string(), "default".to_string())),
            plugins_body_clean("clean-chain", "default"),
        )
        .await;

        let (status, _headers, body) = result.expect("a clean document must create successfully");
        assert_eq!(status, StatusCode::CREATED);
        assert!(body.0.warnings.is_empty());

        let serialized = serde_json::to_value(&body.0).expect("ApiResponse must serialize");
        assert!(
            serialized.get("warnings").is_none(),
            "empty warnings must be entirely absent from the JSON payload, not an empty array"
        );
    }

    #[tokio::test]
    async fn update_with_typo_is_not_rejected_and_carries_warning() {
        let (state, _tmp) = make_fs_state().await;
        let _ = create_namespaced(
            State(Arc::clone(&state)),
            Path(("EdgionPlugins".to_string(), "default".to_string())),
            plugins_body_clean("auth-chain", "default"),
        )
        .await
        .expect("seed create must succeed");

        let result = update_namespaced(
            State(state),
            Path((
                "EdgionPlugins".to_string(),
                "default".to_string(),
                "auth-chain".to_string(),
            )),
            HeaderMap::new(),
            plugins_body_with_typo("auth-chain", "default"),
        )
        .await;

        let body = result.expect("a typo'd key must warn, never reject the update");
        assert_eq!(body.0.warnings, vec!["requestPlugins[0].config.denys".to_string()]);
    }

    #[tokio::test]
    async fn config_data_create_and_update_carry_typed_unknown_field_warning() {
        let (state, _tmp) = make_fs_state().await;
        let created = create_namespaced(
            State(Arc::clone(&state)),
            Path(("EdgionConfigData".to_string(), "default".to_string())),
            config_data_body("cidrs", true),
        )
        .await
        .expect("advisory finding must not reject create");
        assert_eq!(created.2 .0.warnings, ["data.config.items[0].cidrz"]);

        let updated = update_namespaced(
            State(state),
            Path((
                "EdgionConfigData".to_string(),
                "default".to_string(),
                "cidrs".to_string(),
            )),
            HeaderMap::new(),
            config_data_body("cidrs", true),
        )
        .await
        .expect("advisory finding must not reject update");
        assert_eq!(updated.0.warnings, ["data.config.items[0].cidrz"]);
    }

    #[tokio::test]
    async fn config_data_clean_typed_and_open_misc_writes_have_no_warnings() {
        let (state, _tmp) = make_fs_state().await;
        let clean = create_namespaced(
            State(Arc::clone(&state)),
            Path(("EdgionConfigData".to_string(), "default".to_string())),
            config_data_body("clean-cidrs", false),
        )
        .await
        .expect("clean typed ConfigData must be accepted");
        assert!(clean.2 .0.warnings.is_empty());

        let misc = Bytes::from(
            r#"{"apiVersion":"edgion.io/v1","kind":"EdgionConfigData","metadata":{"name":"vendor-data","namespace":"default"},"spec":{"data":{"type":"Misc","config":{"vendor":{"arbitrary":[1,2,3]}}}}}"#,
        );
        let misc = create_namespaced(
            State(state),
            Path(("EdgionConfigData".to_string(), "default".to_string())),
            misc,
        )
        .await
        .expect("open Misc ConfigData must be accepted");
        assert!(misc.2 .0.warnings.is_empty());
    }

    #[tokio::test]
    async fn link_sys_create_and_update_warn_without_rejecting() {
        let (state, _tmp) = make_fs_state().await;
        let created = create_namespaced(
            State(Arc::clone(&state)),
            Path(("LinkSys".to_string(), "default".to_string())),
            link_sys_body("redis", true),
        )
        .await
        .expect("LinkSys advisory must not reject create");
        assert_eq!(created.2 .0.warnings, ["config.timeout.conect"]);

        let updated = update_namespaced(
            State(state),
            Path(("LinkSys".to_string(), "default".to_string(), "redis".to_string())),
            HeaderMap::new(),
            link_sys_body("redis", true),
        )
        .await
        .expect("LinkSys advisory must not reject update");
        assert_eq!(updated.0.warnings, ["config.timeout.conect"]);
    }

    #[tokio::test]
    async fn link_sys_clean_create_has_no_warnings() {
        let (state, _tmp) = make_fs_state().await;
        let (status, _headers, body) = create_namespaced(
            State(state),
            Path(("LinkSys".to_string(), "default".to_string())),
            link_sys_body("clean", false),
        )
        .await
        .expect("clean LinkSys must create successfully");
        assert_eq!(status, StatusCode::CREATED);
        assert!(body.0.warnings.is_empty());

        let serialized = serde_json::to_value(&body.0).expect("ApiResponse must serialize");
        assert!(serialized.get("warnings").is_none());
    }
}
