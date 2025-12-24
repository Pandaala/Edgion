use axum::http::StatusCode;
use crate::types::ResourceKind;

/// Helper function to validate a resource against its schema
pub fn validate_resource<T: serde::Serialize>(
    validator: &crate::core::conf_mgr::SchemaValidator,
    kind: ResourceKind,
    resource: &T,
) -> Result<(), StatusCode> {
    let json_value = serde_json::to_value(resource)
        .map_err(|e| {
            tracing::warn!(
                component = "unified_api",
                error = %e,
                "Failed to convert resource to JSON for validation"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    
    validator.validate(kind, &json_value)
        .map_err(|e| {
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
    ResourceKind::from_kind_name(kind_str)
        .ok_or_else(|| format!("Unknown resource kind: {}", kind_str))
}

/// Determine if a resource kind is cluster-scoped
pub fn is_cluster_scoped(kind: &ResourceKind) -> bool {
    matches!(kind, ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig)
}

/// Helper macro to convert list data to JSON Value Vec
#[macro_export]
macro_rules! list_to_json {
    ($list_data:expr) => {{
        $list_data
            .into_iter()
            .map(|item| serde_json::to_value(item).unwrap_or(serde_json::Value::Null))
            .collect::<Vec<_>>()
    }};
}

