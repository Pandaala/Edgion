use crate::conf_mgr::ConfWriterError;
use axum::http::StatusCode;
use edgion_resources::ResourceKind;

/// Helper function to validate a resource against its schema
///
/// In K8s mode, validation is skipped (handled by K8s API Server)
pub fn validate_resource<T: serde::Serialize>(
    validator: &crate::conf_mgr::SchemaValidator,
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

/// Map ConfWriterError to HTTP StatusCode.
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
        ConfWriterError::ParseError { context, source } => {
            tracing::warn!(
                component = "unified_api",
                context = %context,
                error = %source,
                "Parse error"
            );
            StatusCode::BAD_REQUEST
        }
        ConfWriterError::IOError { context, source } => {
            tracing::error!(
                component = "unified_api",
                context = %context,
                error = %source,
                "IO error"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        }
        ConfWriterError::KubeError(source) => {
            tracing::error!(
                component = "unified_api",
                error = %source,
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
        ConfWriterError::EtcdError(msg) => {
            tracing::error!(
                component = "unified_api",
                error = %msg,
                "Etcd error"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        }
        ConfWriterError::NotSupported(msg) => {
            tracing::info!(
                component = "unified_api",
                error = %msg,
                "Operation not supported by backend"
            );
            StatusCode::NOT_IMPLEMENTED
        }
        ConfWriterError::StalePagination(msg) => {
            tracing::info!(
                component = "unified_api",
                error = %msg,
                "Pagination token is stale or expired"
            );
            StatusCode::GONE
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
    let version = crate::conf_mgr::sync_runtime::resource_version::next_resource_version();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_supported_maps_to_501() {
        let status = map_writer_error(ConfWriterError::NotSupported("x".to_string()));
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    }

    #[test]
    fn stale_pagination_maps_to_410() {
        let status = map_writer_error(ConfWriterError::StalePagination("expired".to_string()));
        assert_eq!(status, StatusCode::GONE);
    }
}
