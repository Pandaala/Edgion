use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header::LOCATION, HeaderMap, HeaderValue, StatusCode},
    response::Json,
};
use edgion_resources::prelude_resources::*;
use std::sync::Arc;

use super::common::{
    expected_resource_version, map_writer_error, parse_kind, parse_resource_and_update_version, validate_resource,
};
use super::types::*;
use edgion_resources::resource::is_kind_cluster_scoped;

/// List all cluster-scoped resources of a kind
pub async fn list_cluster(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
) -> Result<Json<ListResponse<serde_json::Value>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !is_kind_cluster_scoped(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let (data, continue_token) = state.center_list_resources(kind, None).await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

/// Get a cluster-scoped resource
pub async fn get_cluster(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !is_kind_cluster_scoped(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let resource = state.center_get_cluster_resource(kind, &name).await?;
    resource.map(Json).ok_or(StatusCode::NOT_FOUND)
}

/// Create a cluster-scoped resource.
pub async fn create_cluster(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, HeaderMap, Json<ApiResponse<String>>), StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !is_kind_cluster_scoped(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();
    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;

    let metadata =
        edgion_resources::resource::meta::extract_resource_metadata(&content).ok_or(StatusCode::BAD_REQUEST)?;
    let name = metadata.name.ok_or(StatusCode::BAD_REQUEST)?;

    // Parse, validate, and persist using create_one (fails if exists)
    // In K8s mode: skip validation (K8s API Server validates) and don't update version
    // In non-K8s mode: validate and update resource version
    match kind {
        edgion_resources::ResourceKind::GatewayClass => {
            let gc: GatewayClass = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &gc, is_k8s)?;
            let json_content = serde_json::to_string(&gc).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), None, &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionGatewayConfig => {
            let cfg: EdgionGatewayConfig = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &cfg, is_k8s)?;
            let json_content = serde_json::to_string(&cfg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .create_one(kind.as_str(), None, &name, json_content)
                .await
                .map_err(map_writer_error)?;
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    }

    tracing::info!(
        component = "unified_api",
        event = "cluster_resource_created",
        kind = %kind_str,
        name = %name,
        is_k8s_mode = is_k8s,
        "Cluster resource created"
    );

    let location = format!("/api/v1/cluster/{}/{}", kind_str, name);
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

/// Update a cluster-scoped resource
pub async fn update_cluster(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !is_kind_cluster_scoped(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();
    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;
    let expected_resource_version = expected_resource_version(&headers, &content)?;

    // The URL name is the authoritative storage key, but the body content is what
    // gets persisted. Reject a body whose metadata.name contradicts the URL,
    // otherwise the resource would be stored under the URL name while its content
    // claims a different name, desynchronizing storage and the running cache. An
    // absent body name defers to the URL value. (K8s API server rejects the same
    // mismatch.)
    if let Some(metadata) = edgion_resources::resource::meta::extract_resource_metadata(&content) {
        if metadata.name.is_some_and(|meta_name| meta_name != name) {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    // Parse, validate, and persist using update_one (fails if not exists)
    // In K8s mode: skip validation (K8s API Server validates) and don't update version
    // In non-K8s mode: validate and update resource version
    match kind {
        edgion_resources::ResourceKind::GatewayClass => {
            let gc: GatewayClass = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &gc, is_k8s)?;
            let json_content = serde_json::to_string(&gc).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    None,
                    &name,
                    json_content,
                    expected_resource_version.as_deref(),
                )
                .await
                .map_err(map_writer_error)?;
        }
        edgion_resources::ResourceKind::EdgionGatewayConfig => {
            let cfg: EdgionGatewayConfig = parse_resource_and_update_version(&content, !is_k8s)?;
            validate_resource(state.schema_validator.as_ref(), kind, &cfg, is_k8s)?;
            let json_content = serde_json::to_string(&cfg).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            writer
                .update_one_if_version(
                    kind.as_str(),
                    None,
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
        event = "cluster_resource_updated",
        kind = %kind_str,
        name = %name,
        is_k8s_mode = is_k8s,
        "Cluster resource updated"
    );

    Ok(Json(ApiResponse::ok_body(format!("{} updated", kind_str))))
}

/// Delete a cluster-scoped resource.
pub async fn delete_cluster(
    State(state): State<Arc<AdminState>>,
    Path((kind_str, name)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !is_kind_cluster_scoped(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let is_k8s = state.is_k8s_mode();
    let writer = state.conf_center();

    // delete_one returns NotFound if the resource does not exist.
    let expected_resource_version = expected_resource_version(&headers, "{}")?;
    writer
        .delete_one_if_version(kind.as_str(), None, &name, expected_resource_version.as_deref())
        .await
        .map_err(map_writer_error)?;

    tracing::info!(
        component = "unified_api",
        event = "cluster_resource_deleted",
        kind = %kind_str,
        name = %name,
        is_k8s_mode = is_k8s,
        "Cluster resource deleted"
    );

    Ok(StatusCode::NO_CONTENT)
}

/// List metadata-only keys for a cluster-scoped kind (keys endpoint)
pub async fn list_cluster_keys(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
    Query(pagination): Query<super::namespaced_handlers::PaginationQuery>,
) -> Result<Json<ListResponse<crate::conf_mgr::conf_center::traits::ResourceKey>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    if !is_kind_cluster_scoped(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let (data, continue_token) = state.center_list_keys(kind, pagination.into_list_options()).await?;
    Ok(Json(ListResponse::success_with_token(data, continue_token)))
}

#[cfg(test)]
mod update_identity_tests {
    use super::*;
    use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

    /// Build a minimal FileSystem-backed AdminState (non-K8s mode, the mode in
    /// which the URL-vs-body name desync is observable). The returned TempDir
    /// must be kept alive for the storage path to stay valid.
    async fn make_fs_state() -> (Arc<AdminState>, tempfile::TempDir) {
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

    fn gateway_class_body(name: &str) -> Bytes {
        Bytes::from(format!(
            r#"{{"apiVersion":"gateway.networking.k8s.io/v1","kind":"GatewayClass","metadata":{{"name":"{name}"}},"spec":{{"controllerName":"edgion.io/gateway"}}}}"#
        ))
    }

    #[tokio::test]
    async fn update_cluster_rejects_mismatched_name() {
        let (state, _tmp) = make_fs_state().await;
        let result = update_cluster(
            State(state),
            Path(("GatewayClass".to_string(), "gc-a".to_string())),
            HeaderMap::new(),
            gateway_class_body("gc-b"),
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    }

    #[tokio::test]
    async fn update_cluster_matching_name_passes_guard() {
        // Identity matches, so the guard must NOT reject; the request reaches the
        // writer, which returns NotFound (404) for the absent resource -- proving
        // the guard does not false-positive on a valid identity.
        let (state, _tmp) = make_fs_state().await;
        let result = update_cluster(
            State(state),
            Path(("GatewayClass".to_string(), "gc-a".to_string())),
            HeaderMap::new(),
            gateway_class_body("gc-a"),
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::NOT_FOUND));
    }
}
