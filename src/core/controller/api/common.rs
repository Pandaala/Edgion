use crate::core::controller::conf_mgr::ConfWriterError;
use crate::types::ResourceKind;
use axum::http::StatusCode;

/// Helper function to validate a resource against its schema
///
/// In K8s mode, validation is skipped (handled by K8s API Server)
pub fn validate_resource<T: serde::Serialize>(
    validator: &crate::core::controller::conf_mgr::SchemaValidator,
    kind: ResourceKind,
    resource: &T,
    is_k8s_mode: bool,
) -> Result<(), StatusCode> {
    // Skip validation in K8s mode - K8s API Server will validate
    if is_k8s_mode {
        tracing::debug!(
            component = "unified_api",
            kind = ?kind,
            "K8s mode: skipping local schema validation"
        );
        return Ok(());
    }

    let json_value = serde_json::to_value(resource).map_err(|e| {
        tracing::warn!(
            component = "unified_api",
            error = %e,
            "Failed to convert resource to JSON for validation"
        );
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    validator.validate(kind, &json_value).map_err(|e| {
        tracing::warn!(
            component = "unified_api",
            kind = ?kind,
            error = %e,
            "Schema validation failed"
        );
        StatusCode::BAD_REQUEST
    })?;

    Ok(())
}

/// Map ConfWriterError to HTTP StatusCode
///
/// Provides fine-grained error mapping for both K8s and non-K8s modes
pub fn map_writer_error(e: ConfWriterError) -> StatusCode {
    match e {
        ConfWriterError::NotFound(msg) => {
            tracing::warn!(
                component = "unified_api",
                error = %msg,
                "Resource not found"
            );
            StatusCode::NOT_FOUND
        }
        ConfWriterError::AlreadyExists(msg) => {
            tracing::warn!(
                component = "unified_api",
                error = %msg,
                "Resource already exists"
            );
            StatusCode::CONFLICT
        }
        ConfWriterError::ValidationError(msg) => {
            tracing::warn!(
                component = "unified_api",
                error = %msg,
                "Validation error from backend"
            );
            StatusCode::BAD_REQUEST
        }
        ConfWriterError::PermissionDenied(msg) => {
            tracing::warn!(
                component = "unified_api",
                error = %msg,
                "Permission denied"
            );
            StatusCode::FORBIDDEN
        }
        ConfWriterError::Conflict(msg) => {
            tracing::warn!(
                component = "unified_api",
                error = %msg,
                "Conflict detected"
            );
            StatusCode::CONFLICT
        }
        ConfWriterError::ParseError(msg) => {
            tracing::warn!(
                component = "unified_api",
                error = %msg,
                "Parse error"
            );
            StatusCode::BAD_REQUEST
        }
        ConfWriterError::IOError(msg) => {
            tracing::error!(
                component = "unified_api",
                error = %msg,
                "IO error"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        }
        ConfWriterError::KubeError(msg) => {
            tracing::error!(
                component = "unified_api",
                error = %msg,
                "Kubernetes API error"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        }
        ConfWriterError::InternalError(msg) => {
            tracing::error!(
                component = "unified_api",
                error = %msg,
                "Internal error"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Parse request body as either JSON or YAML
/// Tries JSON first, falls back to YAML if JSON parsing fails
pub fn parse_resource<T>(body: &str) -> Result<T, StatusCode>
where
    T: serde::de::DeserializeOwned,
{
    // Try JSON first (more common in API calls)
    if let Ok(resource) = serde_json::from_str::<T>(body) {
        return Ok(resource);
    }

    // Fall back to YAML
    serde_yaml::from_str::<T>(body).map_err(|e| {
        tracing::warn!(
            component = "unified_api",
            error = %e,
            "Failed to parse request body as JSON or YAML"
        );
        StatusCode::BAD_REQUEST
    })
}

/// Parse ResourceKind from string (case-insensitive)
pub fn parse_kind(kind_str: &str) -> Result<ResourceKind, String> {
    ResourceKind::from_kind_name(kind_str).ok_or_else(|| format!("Unknown resource kind: {}", kind_str))
}

/// Update resource_version for a resource in non-k8s mode
pub fn update_resource_version<T>(resource: &mut T)
where
    T: kube::ResourceExt,
{
    let version = crate::core::common::utils::next_resource_version();
    resource.meta_mut().resource_version = Some(version.to_string());
}

/// Parse resource and optionally update resource_version
///
/// If `update_version` is true, automatically updates the resource_version
/// by calling next_resource_version(). This should be true in non-k8s mode
/// for create/update operations.
pub fn parse_resource_and_update_version<T>(body: &str, update_version: bool) -> Result<T, StatusCode>
where
    T: serde::de::DeserializeOwned + kube::ResourceExt,
{
    let mut resource = parse_resource(body)?;

    if update_version {
        update_resource_version(&mut resource);
    }

    Ok(resource)
}
