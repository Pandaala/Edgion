use super::bounded_json::BoundedJson as Json;
use crate::conf_mgr::ListOptions;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header::LOCATION, HeaderMap, HeaderValue, StatusCode},
};
use edgion_resources::prelude_resources::*;
use k8s_openapi::api::core::v1::{ConfigMap, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use serde::Deserialize;
use std::sync::Arc;

use super::common::{
    expected_resource_version, map_writer_error, parse_kind, parse_resource, parse_resource_and_update_version,
    unknown_field_warnings, validate_gateway_api_raw, validate_http_route_transforms, validate_resource,
};
use super::types::*;

/// The watcher can publish a fail-closed marker for a stored resource. Reject
/// that lossy conversion before an Admin write can overwrite the source.
fn admit_http_plugins(state: &AdminState, content: &str) -> Result<String, StatusCode> {
    let is_k8s = state.is_k8s_mode();
    let plugins: EdgionPlugins = parse_resource_and_update_version(content, !is_k8s)?;
    if plugins.has_undecodable_plugin_config() {
        return Err(StatusCode::BAD_REQUEST);
    }
    validate_resource(
        &state.schema_validator,
        edgion_resources::ResourceKind::EdgionPlugins,
        &plugins,
        is_k8s,
    )?;
    serde_json::to_string(&plugins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Pagination query parameters for list endpoints.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PaginationQuery {
    /// Maximum returned items, default 10 and hard maximum 100.
    pub limit: Option<u32>,
    /// Continue token from a previous list response.
    #[serde(rename = "continue")]
    pub continue_token: Option<String>,
}

impl PaginationQuery {
    pub(super) fn into_list_options(self) -> Result<Option<ListOptions>, StatusCode> {
        use crate::conf_mgr::conf_center::source_query::{DEFAULT_PAGE_SIZE, MAX_CURSOR_BYTES, MAX_PAGE_SIZE};
        let limit = self.limit.unwrap_or(DEFAULT_PAGE_SIZE);
        if limit == 0
            || limit > MAX_PAGE_SIZE
            || self
                .continue_token
                .as_ref()
                .is_some_and(|t| t.is_empty() || t.len() > MAX_CURSOR_BYTES)
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        Ok(Some(ListOptions {
            limit,
            continue_token: self.continue_token,
        }))
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
        .center_list_resources(kind, pagination.into_list_options()?)
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
        .center_list_resources_namespaced(kind, &ns, pagination.into_list_options()?)
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

/// Establish the URL namespace before any backend persists the admitted body.
/// Stored FS/etcd content must already match its key; ingest never backfills it.
fn admit_namespace(content: String, namespace: &str) -> Result<String, StatusCode> {
    let mut document: serde_json::Value = parse_resource(&content)?;
    let metadata = document
        .get_mut("metadata")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or(StatusCode::BAD_REQUEST)?;
    match metadata.get("namespace") {
        Some(serde_json::Value::String(value)) if value == namespace && !value.is_empty() => Ok(content),
        None | Some(serde_json::Value::Null) => {
            metadata.insert(
                "namespace".to_string(),
                serde_json::Value::String(namespace.to_string()),
            );
            // Preserve the whole source document, including fields unknown to
            // local types that Kubernetes HTTPRoute writes forward verbatim.
            serde_json::to_string(&document).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
        _ => Err(StatusCode::BAD_REQUEST),
    }
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

    let content = admit_namespace(content, &ns)?;

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
            let json_content = admit_http_plugins(&state, &content)?;
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

    // A present body name must agree with the URL. The writer preserves the
    // current identity when an update omits its name.
    if let Some(metadata) = edgion_resources::resource::meta::extract_resource_metadata(&content) {
        if metadata.name.is_some_and(|meta_name| meta_name != name) {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    let content = admit_namespace(content, &ns)?;

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
            let json_content = admit_http_plugins(&state, &content)?;
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
    let (data, continue_token) = state.center_list_keys(kind, pagination.into_list_options()?).await?;
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
        .center_list_keys_namespaced(kind, &ns, pagination.into_list_options()?)
        .await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

#[cfg(test)]
#[path = "../../tests/unit/api/namespaced_handlers_test/update_identity_tests.rs"]
mod update_identity_tests;

/// The Admin API write path warns (never rejects) on an unrecognized config
/// key and surfaces the finding through `ApiResponse::warnings`.
#[cfg(test)]
#[path = "../../tests/unit/api/namespaced_handlers_test/unknown_field_warning_tests.rs"]
mod unknown_field_warning_tests;

#[cfg(test)]
#[path = "../../tests/unit/api/namespaced_handlers_test/namespace_admission_tests.rs"]
mod namespace_admission_tests;

#[cfg(test)]
#[path = "../../tests/unit/api/namespaced_handlers_test/source_paging_tests.rs"]
mod source_paging_tests;
