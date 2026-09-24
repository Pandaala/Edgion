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
    // Gateway API input is checked against its source schema before typed
    // deserialization; validating the serialized model loses authored fields.
    if edgion_resources::gateway_api_support::canonical_sources().any(|source| source.kind == kind.as_str()) {
        return Ok(());
    }
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

/// Validate the original Gateway API document before typed normalization.
pub fn validate_gateway_api_raw(
    validator: &crate::conf_mgr::SchemaValidator,
    kind: ResourceKind,
    content: &str,
    is_k8s_mode: bool,
) -> Result<(), StatusCode> {
    let Some(source) =
        edgion_resources::gateway_api_support::canonical_sources().find(|source| source.kind == kind.as_str())
    else {
        return Ok(());
    };
    let value: serde_json::Value = parse_resource(content)?;
    let raw: serde_yaml::Value = serde_yaml::from_str(content).map_err(|_| StatusCode::BAD_REQUEST)?;
    crate::conf_mgr::conf_center::common::validate_gateway_api_source(kind, &raw)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    validate_gateway_rule_count(kind, &value)?;
    if is_k8s_mode {
        return Ok(());
    }
    let version = gateway_source_version(&raw).unwrap_or(source.version);
    validator
        .validate_gateway_source(
            edgion_resources::gateway_api_support::GROUP,
            version,
            source.kind,
            &value,
        )
        .map_err(|_| {
            tracing::warn!(component = "unified_api", kind = ?kind, "Gateway API schema validation failed");
            StatusCode::BAD_REQUEST
        })
}

pub fn gateway_source_version(raw: &serde_yaml::Value) -> Option<&str> {
    raw.get("apiVersion")
        .and_then(serde_yaml::Value::as_str)
        .and_then(|api_version| api_version.strip_prefix("gateway.networking.k8s.io/"))
}

pub fn validate_gateway_rule_count(kind: ResourceKind, value: &serde_json::Value) -> Result<(), StatusCode> {
    if matches!(kind, ResourceKind::TCPRoute | ResourceKind::UDPRoute)
        && value["spec"]["rules"].as_array().is_some_and(|rules| rules.len() > 1)
    {
        Err(StatusCode::BAD_REQUEST)
    } else {
        Ok(())
    }
}

/// Apply the shared Gateway API request-transform contract to authored rules.
/// Admin writes reject the whole submitted document synchronously; the
/// filesystem/etcd handler uses the same rule verdicts to retain valid
/// siblings and report status asynchronously.
pub fn validate_http_route_transforms(route: &edgion_resources::HTTPRoute) -> Result<(), StatusCode> {
    if route.spec.unsupported_default_gateway_attachment().is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    for rule in route.spec.rules.iter().flatten() {
        match edgion_resources::authored_request_transform_violations(rule) {
            Ok(violations) if violations.is_empty() => {}
            Ok(_) => return Err(StatusCode::BAD_REQUEST),
            Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }
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

/// Report unknown keys in `content`'s `spec`, for the Admin API write path.
/// This is the fourth ingress that hook covers: the raw body is
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
/// the non-fatal contract the FS/etcd loader seam uses. The write must never
/// be blocked by this diagnostic: unknown-field detection warns, never rejects.
pub fn unknown_field_warnings(kind: ResourceKind, content: &str) -> Vec<String> {
    let schema = match kind {
        ResourceKind::EdgionPlugins => edgion_resources::edgion_plugins_spec_schema(),
        ResourceKind::EdgionStreamPlugins => edgion_resources::edgion_stream_plugins_spec_schema(),
        ResourceKind::EdgionConfigData => edgion_resources::edgion_config_data_spec_schema(),
        ResourceKind::LinkSys => edgion_resources::link_sys_spec_schema(),
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
/// see `skills/04-review/admin-api/intentional-design/admin-api-write-precondition-opt-in.md`.
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
    T: serde::de::DeserializeOwned + kube::ResourceExt + edgion_resources::ResourceMeta,
{
    crate::conf_mgr::conf_center::common::validate_resource_api_version::<T>(body).map_err(|e| {
        tracing::warn!(
            component = "unified_api",
            error = %e,
            "Resource API version validation failed"
        );
        StatusCode::BAD_REQUEST
    })?;

    let mut resource = if matches!(
        T::resource_kind(),
        ResourceKind::ReferenceGrant | ResourceKind::GatewayClass | ResourceKind::Gateway | ResourceKind::HTTPRoute
    ) {
        let canonical: serde_json::Value = if T::resource_kind() == ResourceKind::GatewayClass {
            serde_json::to_value(
                crate::conf_mgr::conf_center::common::parse_resource_content::<edgion_resources::GatewayClass>(body)
                    .map_err(|_| StatusCode::BAD_REQUEST)?,
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        } else if T::resource_kind() == ResourceKind::Gateway {
            serde_json::to_value(
                crate::conf_mgr::conf_center::common::parse_resource_content::<edgion_resources::Gateway>(body)
                    .map_err(|_| StatusCode::BAD_REQUEST)?,
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        } else if T::resource_kind() == ResourceKind::HTTPRoute {
            serde_json::to_value(
                crate::conf_mgr::conf_center::common::parse_resource_content::<edgion_resources::HTTPRoute>(body)
                    .map_err(|_| StatusCode::BAD_REQUEST)?,
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        } else {
            serde_json::to_value(
                crate::conf_mgr::conf_center::common::parse_resource_content::<edgion_resources::ReferenceGrant>(body)
                    .map_err(|_| StatusCode::BAD_REQUEST)?,
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        };
        serde_json::from_value(canonical).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else {
        parse_resource(body)?
    };

    if update_version {
        update_resource_version(&mut resource);
    }

    Ok(resource)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::prelude_resources::BackendTLSPolicy;

    #[test]
    fn admin_beta_http_route_rejects_unserved_default_gateway_attachment() {
        let source = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "HTTPRoute",
            "metadata": {"name": "route", "namespace": "demo"},
            "spec": {"useDefaultGateways": "All", "rules": [{}]}
        });
        let route: edgion_resources::HTTPRoute = parse_resource_and_update_version(&source.to_string(), false).unwrap();
        assert_eq!(route.spec.use_default_gateways.as_deref(), Some("All"));
        assert_eq!(validate_http_route_transforms(&route), Err(StatusCode::BAD_REQUEST));
    }

    fn backend_tls_policy_body(api_version: &str) -> String {
        format!(
            r#"{{
                "apiVersion": "{api_version}",
                "kind": "BackendTLSPolicy",
                "metadata": {{"name": "backend-tls", "namespace": "default"}},
                "spec": {{
                    "targetRefs": [{{"group": "", "kind": "Service", "name": "backend"}}],
                    "validation": {{
                        "hostname": "backend.default.svc",
                        "wellKnownCACertificates": "System"
                    }}
                }}
            }}"#
        )
    }

    #[test]
    fn admin_parser_converts_reference_grant_beta_in_both_modes() {
        let source = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1",
            "kind": "ReferenceGrant",
            "metadata": {"name": "allow", "namespace": "backend", "labels": {"team": "edge"}},
            "spec": {
                "from": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": "app"}],
                "to": [{"group": "", "kind": "Service", "name": "api"}]
            }
        });
        for update_version in [false, true] {
            let grant: edgion_resources::ReferenceGrant =
                parse_resource_and_update_version(&source.to_string(), update_version).unwrap();
            assert_eq!(grant.metadata.labels.as_ref().unwrap()["team"], "edge");
            assert_eq!(grant.spec.from[0].namespace, "app");
            assert_eq!(grant.spec.to[0].name.as_deref(), Some("api"));
            assert_eq!(
                serde_json::to_value(grant).unwrap()["apiVersion"],
                "gateway.networking.k8s.io/v1"
            );
        }
    }

    #[test]
    fn admin_parser_accepts_backend_tls_policy_alpha3_in_all_modes() {
        let legacy = backend_tls_policy_body("gateway.networking.k8s.io/v1alpha3");

        for update_version in [false, true] {
            let result = parse_resource_and_update_version::<BackendTLSPolicy>(&legacy, update_version);
            assert!(result.is_ok());
        }

        let canonical = backend_tls_policy_body("gateway.networking.k8s.io/v1");
        assert!(parse_resource_and_update_version::<BackendTLSPolicy>(&canonical, false).is_ok());
        assert!(parse_resource_and_update_version::<BackendTLSPolicy>(&canonical, true).is_ok());
    }

    #[test]
    fn admin_parser_tls_route_version_matrix() {
        use edgion_resources::TLSRoute;
        let canonical = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "TLSRoute",
            "metadata": {"name": "tls-route", "namespace": "default"},
            "spec": {"hostnames": ["example.com"]}
        });
        for version in [
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::json!(42)),
            Some(serde_json::json!("")),
            Some(serde_json::json!("gateway.networking.k8s.io/v1alpha3")),
            Some(serde_json::json!("gateway.networking.k8s.io/v1")),
        ] {
            let valid = matches!(
                version.as_ref().and_then(serde_json::Value::as_str),
                Some("gateway.networking.k8s.io/v1" | "gateway.networking.k8s.io/v1alpha3")
            );
            let mut document = canonical.clone();
            match version {
                Some(value) => document["apiVersion"] = value,
                None => {
                    document.as_object_mut().unwrap().remove("apiVersion");
                }
            }
            for body in [
                serde_json::to_string(&document).unwrap(),
                serde_yaml::to_string(&document).unwrap(),
            ] {
                for update_version in [false, true] {
                    let result = parse_resource_and_update_version::<TLSRoute>(&body, update_version);
                    if valid {
                        assert!(result.is_ok());
                    } else {
                        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
                    }
                }
            }
        }
    }

    #[test]
    fn admin_parser_rejects_missing_or_malformed_backend_tls_policy_version() {
        let canonical: serde_json::Value =
            serde_json::from_str(&backend_tls_policy_body("gateway.networking.k8s.io/v1")).unwrap();
        for version in [
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::json!(1)),
            Some(serde_json::json!("")),
            Some(serde_json::json!("gateway.networking.k8s.io/v1beta1")),
        ] {
            let mut document = canonical.clone();
            match version {
                Some(value) => document["apiVersion"] = value,
                None => {
                    document.as_object_mut().unwrap().remove("apiVersion");
                }
            }
            for body in [
                serde_json::to_string(&document).unwrap(),
                serde_yaml::to_string(&document).unwrap(),
            ] {
                for update_version in [false, true] {
                    let result = parse_resource_and_update_version::<BackendTLSPolicy>(&body, update_version);
                    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST), "input: {body}");
                }
            }
        }
    }

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
    fn exhausted_backend_occ_conflict_maps_to_409() {
        let status = map_writer_error(ConfWriterError::Conflict("object keeps changing".to_string()));
        assert_eq!(status, StatusCode::CONFLICT);
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

    #[test]
    fn if_match_star_overrides_payload_resource_version() {
        let mut headers = HeaderMap::new();
        headers.insert(IF_MATCH, "*".parse().unwrap());
        let body = r#"metadata: {resourceVersion: payload-rv}"#;
        assert_eq!(expected_resource_version(&headers, body).unwrap(), None);
    }

    #[test]
    fn absent_update_precondition_remains_unconditional() {
        let body = r#"metadata: {name: route-a}"#;
        assert_eq!(expected_resource_version(&HeaderMap::new(), body).unwrap(), None);
    }
}
