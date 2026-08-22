use crate::conf_mgr::ConfWriterError;
use axum::http::{header::IF_MATCH, HeaderMap, StatusCode};
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

/// Report unknown keys in `content`'s `spec`, for the Admin API write path
/// (item 2 of `tasks/todo/plugin-config-unknown-field-detection/02-implementation.md`
/// §11 / R-11). This is the fourth ingress that hook covers: the raw body is
/// right here at the handler, so no `Value` round-trip hook or gate is
/// needed — unlike the Kubernetes watcher, this always runs.
///
/// Only resources wired to the advisory checker have a schema to check against;
/// every other kind returns an empty list immediately.
///
/// Best-effort: the caller already parsed this exact `content` into the
/// typed resource one line earlier (via `parse_resource_and_update_version`),
/// so re-parsing it here as `Value` should never fail in practice — but if it
/// does, this returns an empty list rather than failing the write, matching
/// the non-fatal contract the FS/etcd loader seam uses (R-2). The write must
/// never be blocked by this diagnostic (§10 — warn, never reject).
pub fn unknown_field_warnings(kind: ResourceKind, content: &str) -> Vec<String> {
    let schema = match kind {
        ResourceKind::EdgionPlugins => edgion_resources::edgion_plugins_spec_schema(),
        ResourceKind::EdgionStreamPlugins => edgion_resources::edgion_stream_plugins_spec_schema(),
        ResourceKind::EdgionConfigData => edgion_resources::edgion_config_data_spec_schema(),
        _ => return Vec::new(),
    };
    let Ok(document) = parse_resource::<serde_json::Value>(content) else {
        return Vec::new();
    };
    let Some(spec) = document.get("spec") else {
        return Vec::new();
    };
    let paths = edgion_resources::schema_check::unknown_key_paths(spec, schema);
    crate::observe::unknown_fields_registry::bounded_unknown_field_paths(&paths)
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

/// Resolve the optimistic-concurrency token for an update request.
///
/// A strong `If-Match` ETag takes precedence. The body resourceVersion is a
/// compatibility fallback for clients that have not started sending the
/// header yet.
///
/// `None` — no header and no body version — is the documented contract, not a
/// concession to old clients: the write becomes unconditional and is
/// last-writer-wins, so another client's update is silently discarded while
/// this caller still receives `200`/`204`. Callers that want `409`-on-conflict
/// must send the precondition from their own `GET`. `If-Match: *` also resolves
/// to `None`; it means only "must exist", which the update/delete path already
/// enforces via `ConfWriterError::NotFound`. Do not "fix" this into a `428` —
/// see `skills/04-review/architecture/admin-api-write-precondition-opt-in.md`.
pub fn expected_resource_version(headers: &HeaderMap, body: &str) -> Result<Option<String>, StatusCode> {
    if let Some(value) = headers.get(IF_MATCH) {
        let value = value.to_str().map_err(|_| StatusCode::BAD_REQUEST)?.trim();
        if value == "*" {
            return Ok(None);
        }
        if value.starts_with("W/") || value.contains(',') || value.is_empty() {
            return Err(StatusCode::BAD_REQUEST);
        }
        if value.starts_with('"') {
            if !value.ends_with('"') || value.len() < 2 {
                return Err(StatusCode::BAD_REQUEST);
            }
            let mut token = String::new();
            let mut chars = value[1..value.len() - 1].chars();
            while let Some(ch) = chars.next() {
                if ch == '\\' {
                    token.push(chars.next().ok_or(StatusCode::BAD_REQUEST)?);
                } else {
                    token.push(ch);
                }
            }
            if token.is_empty() {
                return Err(StatusCode::BAD_REQUEST);
            }
            return Ok(Some(token));
        }
        return Ok(Some(value.to_string()));
    }

    let resource: serde_yaml::Value = serde_yaml::from_str(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(resource
        .get("metadata")
        .and_then(|metadata| metadata.get("resourceVersion"))
        .and_then(serde_yaml::Value::as_str)
        .map(str::to_string))
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

    #[test]
    fn if_match_precedes_payload_resource_version() {
        let mut headers = HeaderMap::new();
        headers.insert(IF_MATCH, r#""header-rv""#.parse().unwrap());
        let body = r#"metadata: {resourceVersion: payload-rv}"#;
        assert_eq!(
            expected_resource_version(&headers, body).unwrap().as_deref(),
            Some("header-rv")
        );
    }

    #[test]
    fn payload_resource_version_is_compatibility_fallback() {
        let body = r#"metadata: {resourceVersion: payload-rv}"#;
        assert_eq!(
            expected_resource_version(&HeaderMap::new(), body).unwrap().as_deref(),
            Some("payload-rv")
        );
    }
}
