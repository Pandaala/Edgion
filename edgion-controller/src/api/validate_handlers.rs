//! `POST /api/v1/validate/{kind}` — a synchronous, read-only verdict on a
//! submitted resource document, so an operator who wants blocking can build
//! it themselves (a `ValidatingAdmissionWebhook`, a CI check, a GitOps
//! pre-merge step) instead of relying on the controller, which never rejects
//! a resource for a schema/config-key mismatch (warn-not-reject policy, see
//! `skills/01-architecture/01-controller/02-admin-api.md`).
//!
//! Reuses exactly what the Admin API write path already does, minus the
//! write:
//! 1. [`extract_resource_metadata`] on the raw body (same early sanity check
//!    the write path performs);
//! 2. the original Gateway API source document is checked against its
//!    registered schema before typed parsing; other kinds retain the existing
//!    typed-then-schema order;
//! 3. a typed parse via [`parse_resource_and_update_version`] with
//!    `update_version: false` — no version bump, nothing persisted. The
//!    CRD-schema validator (`conf_mgr::schema_validator::SchemaValidator`) is
//!    called directly so schema diagnostics reach the query response;
//! 4. the unknown-field walk on the raw document, via
//!    [`unknown_field_warnings`] from the Admin write path (`api/common.rs`).
//!
//! `validationErrors` additionally carries side-effect-free shared business
//! validation for `EdgionPlugins`, `LinkSys` and HTTPRoute request transforms. The plugin
//! check matches the conf_sync preparse path except for DSL VM initialization,
//! which requires the Gateway runtime. No equivalent exists yet for
//! `EdgionStreamPlugins`; its check mutates plugin-policy state and requires a
//! full `HandlerContext`. Other kinds leave `validationErrors` empty. This is
//! independent of `unknownFieldCheck`, which only tracks the walk in step 4.
//!
//! Strictly read-only: no store is touched, no version is bumped.

use crate::api::common::{parse_kind, parse_resource_and_update_version, unknown_field_warnings};
use crate::api::types::AdminState;
use crate::api::ApiResponse;
use crate::conf_mgr::ValidationError;
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use edgion_resources::prelude_resources::*;
use edgion_resources::ResourceKind;
use k8s_openapi::api::core::v1::{ConfigMap, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateResponse {
    pub ok: bool,
    /// Whether step 4 (the unknown-field walk) actually ran for this kind.
    /// `"performed"` for resource kinds wired to the advisory schema walk;
    /// `"unsupported"` for
    /// every other kind. A caller must never read an empty `unknownFields`
    /// as "checked, nothing found" when it actually means "not checked".
    pub unknown_field_check: &'static str,
    pub unknown_fields: Vec<String>,
    pub schema_errors: Vec<String>,
    pub validation_errors: Vec<String>,
}

/// `POST /api/v1/validate/{kind}`. Body is the resource document, YAML or
/// JSON, exactly as it would be applied.
pub async fn validate_kind(
    State(state): State<Arc<AdminState>>,
    Path(kind_str): Path<String>,
    body: Bytes,
) -> Result<Json<ApiResponse<ValidateResponse>>, StatusCode> {
    let kind = parse_kind(&kind_str).map_err(|_| StatusCode::BAD_REQUEST)?;

    let content = String::from_utf8(body.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Step 1: same early sanity check the write path performs. A document
    // that doesn't even parse as YAML/JSON with a `metadata` block cannot be
    // meaningfully validated any further.
    edgion_resources::resource::meta::extract_resource_metadata(&content).ok_or(StatusCode::BAD_REQUEST)?;

    // Gateway API source validation precedes typed parsing, which can discard
    // authored fields or reject a field before its schema error is reported.
    let gateway_source =
        edgion_resources::gateway_api_support::canonical_sources().find(|source| source.kind == kind.as_str());
    let gateway_schema_errors = if let Some(source) = gateway_source {
        let raw_yaml: yaml_serde::Value = yaml_serde::from_str(&content).map_err(|_| StatusCode::BAD_REQUEST)?;
        crate::conf_mgr::conf_center::common::validate_gateway_api_source(kind, &raw_yaml)
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        let raw_json: serde_json::Value = crate::api::common::parse_resource(&content)?;
        let version = crate::api::common::gateway_source_version(&raw_yaml).unwrap_or(source.version);
        let mut errors = match state.schema_validator.validate_gateway_source(
            edgion_resources::gateway_api_support::GROUP,
            version,
            source.kind,
            &raw_json,
        ) {
            Ok(()) => Vec::new(),
            Err(ValidationError::SchemaViolation(message)) => message.split("; ").map(str::to_string).collect(),
        };
        if crate::api::common::validate_gateway_rule_count(kind, &raw_json).is_err() {
            errors.push(format!("{kind:?} v1 supports at most one rule"));
        }
        errors
    } else {
        Vec::new()
    };
    if !gateway_schema_errors.is_empty() {
        return Ok(Json(ApiResponse::ok_body(ValidateResponse {
            ok: false,
            unknown_field_check: "unsupported",
            unknown_fields: Vec::new(),
            schema_errors: gateway_schema_errors,
            validation_errors: Vec::new(),
        })));
    }

    // Non-Gateway API kinds retain their existing typed-then-schema flow.
    // `update_version: false` keeps this read-only.
    let json_value: serde_json::Value = match kind {
        ResourceKind::HTTPRoute => parse_and_serialize::<HTTPRoute>(&content)?,
        ResourceKind::GRPCRoute => parse_and_serialize::<GRPCRoute>(&content)?,
        ResourceKind::TCPRoute => parse_and_serialize::<TCPRoute>(&content)?,
        ResourceKind::UDPRoute => parse_and_serialize::<UDPRoute>(&content)?,
        ResourceKind::TLSRoute => parse_and_serialize::<TLSRoute>(&content)?,
        ResourceKind::Service => parse_and_serialize::<Service>(&content)?,
        ResourceKind::EndpointSlice => parse_and_serialize::<EndpointSlice>(&content)?,
        ResourceKind::EdgionTls => parse_and_serialize::<EdgionTls>(&content)?,
        ResourceKind::EdgionPlugins => parse_and_serialize::<EdgionPlugins>(&content)?,
        ResourceKind::EdgionConfigData => parse_and_serialize::<EdgionConfigData>(&content)?,
        ResourceKind::LinkSys => parse_and_serialize::<LinkSys>(&content)?,
        ResourceKind::Secret => parse_and_serialize::<Secret>(&content)?,
        ResourceKind::ConfigMap => parse_and_serialize::<ConfigMap>(&content)?,
        ResourceKind::Gateway => parse_and_serialize::<Gateway>(&content)?,
        ResourceKind::ReferenceGrant => parse_and_serialize::<ReferenceGrant>(&content)?,
        ResourceKind::BackendTLSPolicy => parse_and_serialize::<BackendTLSPolicy>(&content)?,
        ResourceKind::EdgionStreamPlugins => parse_and_serialize::<EdgionStreamPlugins>(&content)?,
        ResourceKind::EdgionAcme => parse_and_serialize::<EdgionAcme>(&content)?,
        ResourceKind::GatewayClass => parse_and_serialize::<GatewayClass>(&content)?,
        ResourceKind::EdgionGatewayConfig => parse_and_serialize::<EdgionGatewayConfig>(&content)?,
        ResourceKind::EdgionBackendTrafficPolicy => parse_and_serialize::<EdgionBackendTrafficPolicy>(&content)?,
        ResourceKind::EdgionBackend => parse_and_serialize::<EdgionBackend>(&content)?,
        ResourceKind::Unspecified => return Err(StatusCode::BAD_REQUEST),
    };

    let schema_result = if gateway_source.is_some() {
        Ok(())
    } else {
        state.schema_validator.validate(kind, &json_value)
    };
    let schema_errors: Vec<String> = match schema_result {
        Ok(()) => Vec::new(),
        Err(ValidationError::SchemaViolation(msg)) => msg.split("; ").map(str::to_string).collect(),
    };

    // Step 4: the unknown-field walk. `unknown_field_warnings` itself
    // returns an empty list for any kind without a wired schema, so the
    // `unknown_field_check` discriminator is computed independently to keep
    // "checked, clean" distinguishable from "not checked" (the honesty
    // requirement of the public validation response contract).
    let (unknown_field_check, unknown_fields) = match kind {
        ResourceKind::EdgionPlugins
        | ResourceKind::EdgionStreamPlugins
        | ResourceKind::EdgionConfigData
        | ResourceKind::LinkSys => ("performed", unknown_field_warnings(kind, &content)),
        _ => ("unsupported", Vec::new()),
    };

    // `validationErrors`: only kinds with side-effect-free shared business
    // validation available outside the full apply handler are included here.
    let validation_errors: Vec<String> = match kind {
        ResourceKind::LinkSys => {
            let linksys: LinkSys = parse_resource_and_update_version(&content, false)?;
            linksys.validate_config()
        }
        ResourceKind::EdgionPlugins => {
            let plugins: EdgionPlugins = parse_resource_and_update_version(&content, false)?;
            plugins.validate_plugins()
        }
        ResourceKind::HTTPRoute => {
            let route: HTTPRoute = parse_resource_and_update_version(&content, false)?;
            let mut errors = route
                .spec
                .rules
                .iter()
                .flatten()
                .enumerate()
                .flat_map(
                    |(index, rule)| match edgion_resources::authored_request_transform_violations(rule) {
                        Ok(violations) => violations
                            .into_iter()
                            .map(|violation| format!("rules[{index}]: {violation}"))
                            .collect::<Vec<_>>(),
                        Err(_) => vec![format!(
                            "rules[{index}]: request transform validation contract is unavailable"
                        )],
                    },
                )
                .collect::<Vec<_>>();
            if let Some(reason) = route.spec.unsupported_default_gateway_attachment() {
                errors.push(reason.to_string());
            }
            errors
        }
        _ => Vec::new(),
    };

    let ok = schema_errors.is_empty() && unknown_fields.is_empty() && validation_errors.is_empty();

    Ok(Json(ApiResponse::ok_body(ValidateResponse {
        ok,
        unknown_field_check,
        unknown_fields,
        schema_errors,
        validation_errors,
    })))
}

/// Parse `content` into `T` (no version bump, per the read-only contract)
/// and serialize it back to JSON for the schema validator, matching what
/// `api::common::validate_resource` does internally for the write path.
fn parse_and_serialize<T>(content: &str) -> Result<serde_json::Value, StatusCode>
where
    T: serde::de::DeserializeOwned + kube::ResourceExt + edgion_resources::ResourceMeta + serde::Serialize,
{
    let obj: T = parse_resource_and_update_version(content, false)?;
    serde_json::to_value(&obj).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
#[path = "../../tests/unit/api/validate_handlers_test.rs"]
mod tests;
