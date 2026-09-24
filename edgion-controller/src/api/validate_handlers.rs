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
//! validation for `EdgionPlugins` and HTTPRoute request transforms. The plugin
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
        let raw_yaml: serde_yaml::Value = serde_yaml::from_str(&content).map_err(|_| StatusCode::BAD_REQUEST)?;
        crate::conf_mgr::conf_center::common::validate_gateway_api_source(kind, &raw_yaml)
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        let raw_json: serde_json::Value = crate::api::common::parse_resource(&content)?;
        let version = if matches!(
            kind,
            ResourceKind::ReferenceGrant | ResourceKind::GatewayClass | ResourceKind::Gateway | ResourceKind::HTTPRoute
        ) && raw_yaml.get("apiVersion").and_then(serde_yaml::Value::as_str)
            == Some("gateway.networking.k8s.io/v1beta1")
        {
            "v1beta1"
        } else {
            source.version
        };
        match state.schema_validator.validate_gateway_source(
            edgion_resources::gateway_api_support::GROUP,
            version,
            source.kind,
            &raw_json,
        ) {
            Ok(()) => Vec::new(),
            Err(ValidationError::SchemaViolation(message)) => message.split("; ").map(str::to_string).collect(),
        }
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
mod tests {
    use super::*;
    use crate::conf_mgr::{ConfCenterConfig, ConfMgr, FileSystemConfig, SchemaValidator};

    async fn make_state() -> (Arc<AdminState>, tempfile::TempDir) {
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

    fn clean_http_route_body() -> Bytes {
        Bytes::from(
            r#"{
                "apiVersion": "gateway.networking.k8s.io/v1",
                "kind": "HTTPRoute",
                "metadata": {"name": "r1", "namespace": "default"},
                "spec": {}
            }"#,
        )
    }

    #[tokio::test]
    async fn original_gateway_class_field_reports_schema_error() {
        let (mut state, _tmp) = make_state().await;
        let crd_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
        Arc::get_mut(&mut state).unwrap().schema_validator = Arc::new(SchemaValidator::from_crd_dir(&crd_dir).unwrap());
        let body = Bytes::from(
            r#"{"apiVersion":"gateway.networking.k8s.io/v1","kind":"GatewayClass","metadata":{"name":"gc-a"},"spec":{"controllerName":"invalid"}}"#,
        );
        let result = validate_kind(State(state), Path("GatewayClass".to_string()), body)
            .await
            .unwrap()
            .0
            .data
            .unwrap();
        assert!(!result.ok);
        assert!(!result.schema_errors.is_empty());
    }

    #[tokio::test]
    async fn gateway_schema_error_precedes_typed_deserialization_error() {
        let (mut state, _tmp) = make_state().await;
        let crd_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
        Arc::get_mut(&mut state).unwrap().schema_validator = Arc::new(SchemaValidator::from_crd_dir(&crd_dir).unwrap());
        let body = Bytes::from(
            r#"{"apiVersion":"gateway.networking.k8s.io/v1","kind":"GatewayClass","metadata":{"name":"gc-a"},"spec":{"controllerName":42}}"#,
        );
        let result = validate_kind(State(state), Path("GatewayClass".to_string()), body)
            .await
            .expect("schema verdict must be returned before typed parsing")
            .0
            .data
            .unwrap();
        assert!(!result.ok);
        assert!(result
            .schema_errors
            .iter()
            .any(|error| error.contains("/spec/controllerName")));
    }

    #[tokio::test]
    async fn clean_document_reports_ok_and_unsupported_walk() {
        let (state, _tmp) = make_state().await;
        let resp = validate_kind(State(state), Path("HTTPRoute".to_string()), clean_http_route_body())
            .await
            .expect("validate_kind must succeed");
        let body = resp.0.data.expect("ok_body always carries data");
        assert!(body.ok);
        assert_eq!(body.unknown_field_check, "unsupported");
        assert!(body.unknown_fields.is_empty());
        assert!(body.schema_errors.is_empty());
        assert!(body.validation_errors.is_empty());
    }

    #[tokio::test]
    async fn http_route_transform_contract_is_reported_without_persistence() {
        let (state, tmp) = make_state().await;
        let body = Bytes::from(
            r#"{
                "apiVersion": "gateway.networking.k8s.io/v1",
                "kind": "HTTPRoute",
                "metadata": {"name": "invalid-transform", "namespace": "default"},
                "spec": {"rules": [{"backendRefs": [{
                    "name": "backend",
                    "filters": [{"type": "URLRewrite", "urlRewrite": {
                        "path": {"type": "ReplaceFullPath", "replacePrefixMatch": "/v2"}
                    }}]
                }]}]}
            }"#,
        );

        let payload = validate_kind(State(state), Path("HTTPRoute".to_string()), body)
            .await
            .expect("HTTPRoute validation succeeds")
            .0
            .data
            .expect("response data");

        assert!(!payload.ok);
        assert!(payload
            .validation_errors
            .iter()
            .any(|error| error.contains("rules[0]") && error.contains("backendRefs/0/filters/0")));
        assert!(std::fs::read_dir(tmp.path()).expect("read conf dir").next().is_none());
    }

    #[tokio::test]
    async fn typo_in_edgion_plugins_config_is_reported_and_not_persisted() {
        let (state, tmp) = make_state().await;
        let body = Bytes::from(
            r#"{
                "apiVersion": "edgion.io/v1",
                "kind": "EdgionPlugins",
                "metadata": {"name": "auth-chain", "namespace": "default"},
                "spec": {
                    "requestPlugins": [
                        {"type": "BasicAuth", "config": {"denys": ["x"]}}
                    ]
                }
            }"#,
        );
        let resp = validate_kind(State(state.clone()), Path("EdgionPlugins".to_string()), body)
            .await
            .expect("validate_kind must succeed");
        let payload = resp.0.data.expect("ok_body always carries data");
        assert!(!payload.ok);
        assert_eq!(payload.unknown_field_check, "performed");
        assert_eq!(
            payload.unknown_fields,
            vec!["requestPlugins[0].config.denys".to_string()]
        );

        // Nothing persisted: the FileSystem conf dir must remain empty.
        let mut entries = std::fs::read_dir(tmp.path()).expect("read conf dir");
        assert!(
            entries.next().is_none(),
            "validate must not write anything to the backing store"
        );
    }

    #[tokio::test]
    async fn typed_config_data_typo_is_reported_while_misc_remains_open() {
        let (state, _tmp) = make_state().await;
        let typed = Bytes::from(
            r#"{
                "apiVersion":"edgion.io/v1","kind":"EdgionConfigData",
                "metadata":{"name":"cidrs","namespace":"default"},
                "spec":{"data":{"type":"IpList","config":{"items":[{"name":"office","cidrs":[],"cidrz":[]}]}}}
            }"#,
        );
        let typed = validate_kind(State(state.clone()), Path("EdgionConfigData".to_string()), typed)
            .await
            .expect("typed ConfigData validation succeeds")
            .0
            .data
            .expect("response data");
        assert!(!typed.ok);
        assert_eq!(typed.unknown_field_check, "performed");
        assert_eq!(typed.unknown_fields, ["data.config.items[0].cidrz"]);

        let misc = Bytes::from(
            r#"{
                "apiVersion":"edgion.io/v1","kind":"EdgionConfigData",
                "metadata":{"name":"vendor","namespace":"default"},
                "spec":{"data":{"type":"Misc","config":{"vendor":{"arbitrary":[1,2,3]}}}}
            }"#,
        );
        let misc = validate_kind(State(state), Path("EdgionConfigData".to_string()), misc)
            .await
            .expect("Misc ConfigData validation succeeds")
            .0
            .data
            .expect("response data");
        assert!(misc.ok);
        assert_eq!(misc.unknown_field_check, "performed");
        assert!(misc.unknown_fields.is_empty());
    }

    #[tokio::test]
    async fn url_allow_list_nested_typo_reaches_advisory_validation_result() {
        let (state, _tmp) = make_state().await;
        let body = Bytes::from(
            r#"{
                "apiVersion":"edgion.io/v1","kind":"EdgionConfigData",
                "metadata":{"name":"urls","namespace":"default"},
                "spec":{"data":{"type":"RequestAccessUrlAllowList","config":{"items":[{
                    "name":"health","hosts":["api.example.com"],
                    "paths":[{"type":"Exact","value":"/health","valuez":"/ignored"}]
                }]}}}
            }"#,
        );

        let payload = validate_kind(State(state.clone()), Path("EdgionConfigData".to_string()), body)
            .await
            .expect("typed ConfigData typo must reach the advisory validator")
            .0
            .data
            .expect("response data");

        assert!(!payload.ok);
        assert_eq!(payload.unknown_field_check, "performed");
        assert_eq!(payload.unknown_fields, ["data.config.items[0].paths[0].valuez"]);

        let clean = Bytes::from(
            r#"{
                "apiVersion":"edgion.io/v1","kind":"EdgionConfigData",
                "metadata":{"name":"urls","namespace":"default"},
                "spec":{"data":{"type":"RequestAccessUrlAllowList","config":{"items":[{
                    "name":"health","hosts":["api.example.com"],
                    "paths":[{"type":"Exact","value":"/health"}]
                }]}}}
            }"#,
        );
        let clean = validate_kind(State(state), Path("EdgionConfigData".to_string()), clean)
            .await
            .expect("clean typed ConfigData must validate")
            .0
            .data
            .expect("response data");
        assert!(clean.ok);
        assert_eq!(clean.unknown_field_check, "performed");
        assert!(clean.unknown_fields.is_empty());
    }

    #[tokio::test]
    async fn link_sys_typo_is_reported_without_persistence() {
        let (state, tmp) = make_state().await;
        let body = Bytes::from(
            r#"{
                "apiVersion":"edgion.io/v1","kind":"LinkSys",
                "metadata":{"name":"redis","namespace":"default"},
                "spec":{"type":"redis","config":{"endpoints":[],"timeout":{"conect":10}}}
            }"#,
        );
        let payload = validate_kind(State(state), Path("LinkSys".to_string()), body)
            .await
            .expect("LinkSys validation succeeds")
            .0
            .data
            .expect("response data");
        assert!(!payload.ok);
        assert_eq!(payload.unknown_field_check, "performed");
        assert_eq!(payload.unknown_fields, ["config.timeout.conect"]);
        assert!(std::fs::read_dir(tmp.path()).expect("read conf dir").next().is_none());
    }

    #[tokio::test]
    async fn unparseable_body_is_bad_request() {
        let (state, _tmp) = make_state().await;
        let result = validate_kind(
            State(state),
            Path("HTTPRoute".to_string()),
            Bytes::from("not yaml or json content"),
        )
        .await;
        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    }

    #[tokio::test]
    async fn backend_tls_policy_validation_rejects_non_v1_before_typed_parse() {
        let (state, tmp) = make_state().await;
        let body = Bytes::from(
            r#"{
                "apiVersion":"gateway.networking.k8s.io/v1alpha3",
                "kind":"BackendTLSPolicy",
                "metadata":{"name":"backend-tls","namespace":"default"},
                "spec":{
                    "targetRefs":[{"group":"","kind":"Service","name":"backend"}],
                    "validation":{"hostname":"backend.default.svc","wellKnownCACertificates":"System"}
                }
            }"#,
        );

        let result = validate_kind(State(state), Path("BackendTLSPolicy".to_string()), body).await;
        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
        assert!(std::fs::read_dir(tmp.path()).expect("read conf dir").next().is_none());
    }

    #[tokio::test]
    async fn unknown_kind_is_bad_request() {
        let (state, _tmp) = make_state().await;
        let result = validate_kind(State(state), Path("NoSuchKind".to_string()), clean_http_route_body()).await;
        assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
    }

    #[tokio::test]
    async fn tls_route_validation_rejects_invalid_source_versions() {
        let (state, tmp) = make_state().await;
        for version in [
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::json!(7)),
            Some(serde_json::json!("gateway.networking.k8s.io/v1alpha3")),
        ] {
            let mut document = serde_json::json!({
                "kind": "TLSRoute", "metadata": {"name": "tls-route", "namespace": "default"},
                "spec": {"hostnames": ["example.com"]}
            });
            if let Some(value) = version {
                document["apiVersion"] = value;
            }
            let result = validate_kind(
                State(state.clone()),
                Path("TLSRoute".to_string()),
                Bytes::from(document.to_string()),
            )
            .await;
            assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
        }
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn yaml_and_json_bodies_behave_identically() {
        let (state, _tmp) = make_state().await;
        let yaml_body = Bytes::from(
            "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: r1\n  namespace: default\nspec: {}\n",
        );
        let resp = validate_kind(State(state), Path("HTTPRoute".to_string()), yaml_body)
            .await
            .expect("validate_kind must succeed on YAML");
        let body = resp.0.data.expect("ok_body always carries data");
        assert!(body.ok);
    }
}
