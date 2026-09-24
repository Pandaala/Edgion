use anyhow::{Context, Result};
use edgion_resources::gateway_api_support::{canonical_sources, SourceState, GROUP, SOURCES};
use edgion_resources::ResourceKind;
use jsonschema::Validator;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Schema validation error
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Schema validation failed: {0}")]
    SchemaViolation(String),
}

/// Schema validator for Kubernetes resources based on CRD definitions
pub struct SchemaValidator {
    validators: HashMap<ResourceKind, Validator>,
    gateway_validators: HashMap<(String, String, String), Validator>,
    gateway_schemas_loaded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GatewaySchemaProfile {
    Standard,
    Experimental,
    HistoricalGrpc,
}

fn gateway_profile(kind: &str, version: &str) -> GatewaySchemaProfile {
    match (kind, version) {
        ("GRPCRoute", "v1alpha2") => GatewaySchemaProfile::HistoricalGrpc,
        ("BackendTLSPolicy", "v1alpha3") => GatewaySchemaProfile::Experimental,
        // These models represent fields shipped only in the Experimental CRD.
        ("HTTPRoute" | "TLSRoute" | "TCPRoute" | "UDPRoute", _) => GatewaySchemaProfile::Experimental,
        _ => GatewaySchemaProfile::Standard,
    }
}

/// Return only locations composed of shipped property names and array indexes.
/// Map keys supplied by a user become `*`, even when jsonschema reports them
/// in its instance path. Both paths and the number of diagnostics are bounded.
fn safe_gateway_error_path(instance_path: &str, schema_path: &str, value: &JsonValue) -> String {
    let schema_segments: Vec<_> = schema_path.split('/').collect();
    let properties: Vec<_> = schema_segments
        .windows(2)
        .filter_map(|pair| (pair[0] == "properties").then_some(pair[1]))
        .collect();
    if instance_path.is_empty() {
        return "/".to_string();
    }
    let mut safe = String::new();
    let mut raw_prefix = String::new();
    for segment in instance_path.trim_start_matches('/').split('/').take(16) {
        if safe.len() >= 240 {
            break;
        }
        safe.push('/');
        let parent = value.pointer(&raw_prefix);
        let array_index = parent.is_some_and(JsonValue::is_array)
            && segment.len() <= 8
            && !segment.is_empty()
            && segment.bytes().all(|byte| byte.is_ascii_digit());
        let label = if array_index || properties.contains(&segment) {
            segment
        } else {
            "*"
        };
        for character in label.chars() {
            if safe.len() + character.len_utf8() > 240 {
                break;
            }
            safe.push(character);
        }
        raw_prefix.push('/');
        raw_prefix.push_str(segment);
    }
    safe
}

impl SchemaValidator {
    /// Create an empty SchemaValidator with no schemas
    pub fn empty() -> Self {
        Self {
            validators: HashMap::new(),
            gateway_validators: HashMap::new(),
            gateway_schemas_loaded: false,
        }
    }

    /// Create a new SchemaValidator by loading CRD schemas from a directory
    pub fn from_crd_dir(crd_dir: &Path) -> Result<Self> {
        let mut validators = HashMap::new();

        tracing::info!(
            component = "schema_validator",
            crd_dir = ?crd_dir,
            "Loading CRD schemas"
        );

        // Load Edgion custom CRDs
        let edgion_crd_dir = crd_dir.join("edgion-crd");
        if edgion_crd_dir.exists() {
            Self::load_crd_files(&edgion_crd_dir, &mut validators)?;
        }

        // Source profiles are explicit. Directory enumeration must not choose a
        // schema when Standard and Experimental define the same GVK.
        let gateway_api_dir = crd_dir.join("gateway-api");
        anyhow::ensure!(
            gateway_api_dir.is_dir(),
            "Missing Gateway API CRD directory: {gateway_api_dir:?}"
        );
        let gateway_validators = Self::load_gateway_schemas(&gateway_api_dir)?;

        tracing::info!(
            component = "schema_validator",
            schema_count = validators.len() + gateway_validators.len(),
            "Loaded CRD schemas"
        );

        Ok(Self {
            validators,
            gateway_validators,
            gateway_schemas_loaded: true,
        })
    }

    /// Load CRD files from a directory
    fn load_crd_files(dir: &Path, validators: &mut HashMap<ResourceKind, Validator>) -> Result<()> {
        let entries = fs::read_dir(dir).with_context(|| format!("Failed to read CRD directory: {:?}", dir))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_file() && path.extension().and_then(|s| s.to_str()) == Some("yaml") {
                Self::load_single_crd(&path, validators)?;
            }
        }

        Ok(())
    }

    /// Load a single CRD file
    fn load_single_crd(path: &Path, validators: &mut HashMap<ResourceKind, Validator>) -> Result<()> {
        let content = fs::read_to_string(path).with_context(|| format!("Failed to read CRD file: {:?}", path))?;
        let crd: JsonValue =
            serde_yaml::from_str(&content).with_context(|| format!("Failed to parse CRD YAML: {:?}", path))?;
        Self::try_register_crd(&crd, path, validators);
        Ok(())
    }

    /// Extract CRD schema from a parsed document and register a validator if the kind is known.
    fn try_register_crd(crd: &JsonValue, path: &Path, validators: &mut HashMap<ResourceKind, Validator>) {
        let Some(kind_str) = crd
            .get("spec")
            .and_then(|s| s.get("names"))
            .and_then(|n| n.get("kind"))
            .and_then(|k| k.as_str())
        else {
            return;
        };

        let Some(resource_kind) = ResourceKind::from_kind_name(kind_str) else {
            return;
        };

        let Some(schema) = crd
            .get("spec")
            .and_then(|s| s.get("versions"))
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|ver| ver.get("schema"))
            .and_then(|s| s.get("openAPIV3Schema"))
        else {
            return;
        };

        match Validator::new(schema) {
            Ok(validator) => {
                tracing::debug!(
                    component = "schema_validator",
                    kind = ?resource_kind,
                    file = ?path,
                    "Loaded schema"
                );
                validators.insert(resource_kind, validator);
            }
            Err(e) => {
                tracing::warn!(
                    component = "schema_validator",
                    kind = ?resource_kind,
                    file = ?path,
                    error = %e,
                    "Failed to compile schema"
                );
            }
        }
    }

    fn load_gateway_schemas(dir: &Path) -> Result<HashMap<(String, String, String), Validator>> {
        let mut validators = HashMap::new();
        for (profile, file) in [
            (GatewaySchemaProfile::Standard, "gateway-api-standard-v1.6.2.yaml"),
            (
                GatewaySchemaProfile::Experimental,
                "gateway-api-experimental-v1.6.2.yaml",
            ),
            (
                GatewaySchemaProfile::HistoricalGrpc,
                "grpcroute-v1alpha2-v1.1.1-experimental.yaml",
            ),
        ] {
            let path = if profile == GatewaySchemaProfile::HistoricalGrpc {
                Path::new("embedded Gateway API source schema").join(file)
            } else {
                dir.join(file)
            };
            let content = if profile == GatewaySchemaProfile::HistoricalGrpc {
                include_str!("../../../config/gateway-api-source-schemas/grpcroute-v1alpha2-v1.1.1-experimental.yaml")
                    .to_string()
            } else {
                fs::read_to_string(&path).with_context(|| format!("Failed to read Gateway API CRD bundle: {path:?}"))?
            };
            for doc in serde_yaml::Deserializer::from_str(&content) {
                let crd =
                    JsonValue::deserialize(doc).with_context(|| format!("Invalid Gateway API CRD in {path:?}"))?;
                Self::register_gateway_crd(&crd, profile, &mut validators)?;
            }
        }
        for source in SOURCES.iter().filter(|source| {
            matches!(
                source.state,
                SourceState::Canonical | SourceState::ImplementedUnverified
            )
        }) {
            let key = (GROUP.to_string(), source.version.to_string(), source.kind.to_string());
            anyhow::ensure!(
                validators.contains_key(&key),
                "Missing selected Gateway API schema for {key:?}"
            );
        }
        Ok(validators)
    }

    fn register_gateway_crd(
        crd: &JsonValue,
        profile: GatewaySchemaProfile,
        validators: &mut HashMap<(String, String, String), Validator>,
    ) -> Result<()> {
        if crd.get("kind").and_then(JsonValue::as_str) != Some("CustomResourceDefinition") {
            return Ok(());
        }
        let spec = &crd["spec"];
        let Some(kind) = spec["names"]["kind"].as_str() else {
            return Ok(());
        };
        let Some(group) = spec["group"].as_str() else {
            return Ok(());
        };
        if group != GROUP {
            return Ok(());
        }
        let Some(versions) = spec["versions"].as_array() else {
            return Ok(());
        };
        for version in versions {
            let Some(version_name) = version["name"].as_str() else {
                continue;
            };
            if gateway_profile(kind, version_name) != profile {
                continue;
            }
            if !SOURCES.iter().any(|source| {
                source.kind == kind
                    && source.version == version_name
                    && matches!(
                        source.state,
                        SourceState::Canonical | SourceState::ImplementedUnverified
                    )
            }) {
                continue;
            }
            let Some(schema) = version["schema"]["openAPIV3Schema"].as_object() else {
                anyhow::bail!("Missing Gateway API source schema for {group}/{version_name}/{kind}");
            };
            let key = (group.to_string(), version_name.to_string(), kind.to_string());
            anyhow::ensure!(
                !validators.contains_key(&key),
                "Duplicate Gateway API schema for {key:?}"
            );
            let validator = Validator::new(&JsonValue::Object(schema.clone()))
                .with_context(|| format!("Invalid Gateway API schema for {key:?}"))?;
            validators.insert(key, validator);
        }
        Ok(())
    }

    /// Validate a resource against its schema
    pub fn validate(&self, kind: ResourceKind, value: &JsonValue) -> Result<(), ValidationError> {
        if let Some(source) = canonical_sources().find(|source| source.kind == kind.as_str()) {
            return self.validate_gateway_source(GROUP, source.version, source.kind, value);
        }
        match self.validators.get(&kind) {
            Some(validator) => {
                let mut error_messages = Vec::new();
                for error in validator.iter_errors(value) {
                    tracing::debug!(
                        component = "schema_validator",
                        kind = ?kind,
                        path = %error.instance_path(),
                        error = %error,
                        "Schema validation error"
                    );
                    error_messages.push(format!("Path: {}, Error: {}", error.instance_path(), error));
                }
                if error_messages.is_empty() {
                    Ok(())
                } else {
                    Err(ValidationError::SchemaViolation(error_messages.join("; ")))
                }
            }
            None => {
                // No schema: warn but continue (fail-open by design)
                tracing::warn!(
                    component = "schema_validator",
                    kind = ?kind,
                    "No schema available, skipping validation"
                );
                Ok(())
            }
        }
    }

    /// Validate the authored source document, before typed serde or version bumps.
    /// This checks OpenAPI constraints only; Kubernetes CEL is not evaluated here.
    pub fn validate_gateway_source(
        &self,
        group: &str,
        version: &str,
        kind: &str,
        value: &JsonValue,
    ) -> Result<(), ValidationError> {
        let key = (group.to_string(), version.to_string(), kind.to_string());
        let Some(validator) = self.gateway_validators.get(&key) else {
            // Kubernetes mode intentionally uses SchemaValidator::empty(). A
            // loaded non-Kubernetes registry must fail closed on missing GVKs.
            return if self.gateway_schemas_loaded {
                Err(ValidationError::SchemaViolation(
                    "Gateway API source schema is unavailable".to_string(),
                ))
            } else {
                Ok(())
            };
        };
        let mut errors: Vec<_> = validator
            .iter_errors(value)
            .take(17)
            .map(|error| {
                let schema_path = error.schema_path().to_string();
                let instance_path = safe_gateway_error_path(&error.instance_path().to_string(), &schema_path, value);
                format!(
                    "Path: {instance_path}, Schema: {}",
                    schema_path.chars().take(200).collect::<String>()
                )
            })
            .collect();
        if errors.len() > 16 {
            errors.truncate(16);
            errors.push("Additional schema errors omitted".to_string());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError::SchemaViolation(errors.join("; ")))
        }
    }

    /// Get the number of loaded schemas
    pub fn schema_count(&self) -> usize {
        self.validators.len() + self.gateway_validators.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn gateway_profile_and_duplicate_registration_are_deterministic() {
        assert_eq!(gateway_profile("HTTPRoute", "v1"), GatewaySchemaProfile::Experimental);
        assert_eq!(gateway_profile("GatewayClass", "v1"), GatewaySchemaProfile::Standard);
        assert_eq!(
            gateway_profile("BackendTLSPolicy", "v1alpha3"),
            GatewaySchemaProfile::Experimental
        );
        assert_eq!(
            gateway_profile("GRPCRoute", "v1alpha2"),
            GatewaySchemaProfile::HistoricalGrpc
        );
        let crd = serde_json::json!({
            "kind": "CustomResourceDefinition",
            "spec": {
                "group": GROUP,
                "names": {"kind": "GatewayClass"},
                "versions": [{"name":"v1", "schema":{"openAPIV3Schema":{"type":"object"}}}]
            }
        });
        let mut validators = HashMap::new();
        SchemaValidator::register_gateway_crd(&crd, GatewaySchemaProfile::Experimental, &mut validators).unwrap();
        assert!(validators.is_empty());
        SchemaValidator::register_gateway_crd(&crd, GatewaySchemaProfile::Standard, &mut validators).unwrap();
        assert_eq!(validators.len(), 1);
        assert!(SchemaValidator::register_gateway_crd(&crd, GatewaySchemaProfile::Standard, &mut validators).is_err());
    }

    #[test]
    fn last_pre_v1_source_schemas_load_from_the_selected_artifacts() {
        let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
        let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
        for (kind, version) in [
            ("GRPCRoute", "v1alpha2"),
            ("TLSRoute", "v1alpha3"),
            ("BackendTLSPolicy", "v1alpha3"),
            ("TCPRoute", "v1alpha2"),
            ("UDPRoute", "v1alpha2"),
        ] {
            assert!(
                validator
                    .gateway_validators
                    .contains_key(&(GROUP.to_string(), version.to_string(), kind.to_string())),
                "missing {kind} {version} schema"
            );
        }
        assert!(!validator.gateway_validators.contains_key(&(
            GROUP.to_string(),
            "v1alpha2".to_string(),
            "TLSRoute".to_string()
        )));
        for kind in ["TCPRoute", "UDPRoute"] {
            let route = serde_json::json!({
                "apiVersion": "gateway.networking.k8s.io/v1alpha2",
                "kind": kind,
                "metadata": {"name": "route", "namespace": "default"},
                "spec": {"rules": [
                    {"backendRefs": [{"name": "first", "port": 8080}]},
                    {"backendRefs": [{"name": "second", "port": 8080}]}
                ]}
            });
            assert!(validator
                .validate_gateway_source(GROUP, "v1alpha2", kind, &route)
                .is_ok());
            assert!(validator.validate_gateway_source(GROUP, "v1", kind, &route).is_err());
        }
    }

    #[test]
    fn loaded_registry_fails_closed_and_errors_hide_submitted_values() {
        let mut validator = SchemaValidator::empty();
        let value = serde_json::json!({"spec":{"controllerName":"private-token"}});
        assert!(validator
            .validate_gateway_source(GROUP, "v1", "GatewayClass", &value)
            .is_ok());
        validator.gateway_schemas_loaded = true;
        assert!(validator
            .validate_gateway_source(GROUP, "v1", "GatewayClass", &value)
            .is_err());
        let schema = serde_json::json!({
            "type":"object",
            "properties":{"spec":{"type":"object","properties":{"controllerName":{"enum":["allowed"]}}}}
        });
        validator.gateway_validators.insert(
            (GROUP.to_string(), "v1".to_string(), "GatewayClass".to_string()),
            Validator::new(&schema).unwrap(),
        );
        let error = validator
            .validate_gateway_source(GROUP, "v1", "GatewayClass", &value)
            .unwrap_err();
        assert!(!error.to_string().contains("private-token"));
        assert!(error.to_string().contains("controllerName"));
    }

    #[test]
    fn gateway_schema_error_path_masks_user_map_keys() {
        let mut validator = SchemaValidator::empty();
        validator.gateway_schemas_loaded = true;
        let schema = serde_json::json!({
            "type":"object",
            "properties":{"spec":{"type":"object","additionalProperties":{"type":"integer"}}}
        });
        validator.gateway_validators.insert(
            (GROUP.to_string(), "v1".to_string(), "GatewayClass".to_string()),
            Validator::new(&schema).unwrap(),
        );
        let value = serde_json::json!({"spec":{"private-customer-token":"wrong", "12345678":"wrong"}});
        let error = validator
            .validate_gateway_source(GROUP, "v1", "GatewayClass", &value)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Path: /spec/*"), "{error}");
        assert!(!error.contains("private-customer-token"));
        assert!(!error.contains("12345678"));
        assert!(!error.contains("wrong"));
    }

    #[test]
    fn gateway_schema_error_path_keeps_real_array_indexes() {
        let mut validator = SchemaValidator::empty();
        validator.gateway_schemas_loaded = true;
        let schema = serde_json::json!({
            "type":"object",
            "properties":{"spec":{"type":"object","properties":{"rules":{"type":"array","items":{"type":"integer"}}}}}
        });
        validator.gateway_validators.insert(
            (GROUP.to_string(), "v1".to_string(), "HTTPRoute".to_string()),
            Validator::new(&schema).unwrap(),
        );
        let value = serde_json::json!({"spec":{"rules":["wrong"]}});
        let error = validator
            .validate_gateway_source(GROUP, "v1", "HTTPRoute", &value)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Path: /spec/rules/0"), "{error}");
        assert!(!error.contains("wrong"));
    }

    #[test]
    fn reference_grant_beta_uses_its_own_schema_and_requires_both_rule_lists() {
        let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
        let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
        let source = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1",
            "kind": "ReferenceGrant",
            "metadata": {"name": "allow", "namespace": "backend"},
            "spec": {
                "from": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": "app"}],
                "to": [{"group": "", "kind": "Service"}]
            }
        });
        assert!(validator
            .validate_gateway_source(GROUP, "v1beta1", "ReferenceGrant", &source)
            .is_ok());
        for missing in ["from", "to"] {
            let mut invalid = source.clone();
            invalid["spec"].as_object_mut().unwrap().remove(missing);
            assert!(validator
                .validate_gateway_source(GROUP, "v1beta1", "ReferenceGrant", &invalid)
                .is_err());
        }
    }

    #[test]
    fn gateway_beta_uses_standard_source_schema() {
        let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
        let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
        let source = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "Gateway",
            "metadata": {"name": "edge", "namespace": "demo"},
            "spec": {"gatewayClassName": "edgion",
                "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}]}
        });
        assert!(validator
            .validate_gateway_source(GROUP, "v1beta1", "Gateway", &source)
            .is_ok());
        let mut missing = source.clone();
        missing["spec"].as_object_mut().unwrap().remove("gatewayClassName");
        assert!(validator
            .validate_gateway_source(GROUP, "v1beta1", "Gateway", &missing)
            .is_err());
        let mut invalid = source;
        invalid["spec"]["listeners"][0]["port"] = serde_json::json!(0);
        assert!(validator
            .validate_gateway_source(GROUP, "v1beta1", "Gateway", &invalid)
            .is_err());
    }

    #[test]
    fn http_route_beta_uses_experimental_source_schema_and_shared_transform_contract() {
        let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
        let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
        let mut source = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "HTTPRoute",
            "metadata": {"name": "route", "namespace": "demo"},
            "spec": {"parentRefs": [{"name": "edge"}], "useDefaultGateways": "None",
                "rules": [{"matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
                    "filters": [{"type": "RequestRedirect", "requestRedirect": {
                        "path": {"type": "ReplacePrefixMatch", "replacePrefixMatch": "/new"}}}],
                    "retry": {"codes": [503]},
                    "sessionPersistence": {"type": "Cookie", "sessionName": "route"}}
                ]}
        });
        assert!(validator
            .validate_gateway_source(GROUP, "v1beta1", "HTTPRoute", &source)
            .is_ok());
        let route: edgion_resources::HTTPRoute =
            crate::conf_mgr::conf_center::common::parse_resource_content(&source.to_string()).unwrap();
        assert!(
            edgion_resources::authored_request_transform_violations(&route.spec.rules.as_ref().unwrap()[0])
                .unwrap()
                .is_empty()
        );

        source["spec"]["rules"][0]["filters"][0]["requestRedirect"]["path"]["replacePrefixMatch"] =
            serde_json::json!("relative");
        // JSON Schema does not execute the CRD's CEL expression. The shared
        // transform contract supplies that verdict for every source version.
        assert!(validator
            .validate_gateway_source(GROUP, "v1beta1", "HTTPRoute", &source)
            .is_ok());
        let route: edgion_resources::HTTPRoute =
            crate::conf_mgr::conf_center::common::parse_resource_content(&source.to_string()).unwrap();
        assert!(
            !edgion_resources::authored_request_transform_violations(&route.spec.rules.as_ref().unwrap()[0])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn gateway_class_beta_uses_standard_source_schema() {
        let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd");
        let validator = SchemaValidator::from_crd_dir(&crd_dir).unwrap();
        let source = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "GatewayClass",
            "metadata": {"name": "edge"},
            "spec": {"controllerName": "example.com/edge"}
        });
        assert!(validator
            .validate_gateway_source(GROUP, "v1beta1", "GatewayClass", &source)
            .is_ok());
        let mut missing = source.clone();
        missing["spec"].as_object_mut().unwrap().remove("controllerName");
        assert!(validator
            .validate_gateway_source(GROUP, "v1beta1", "GatewayClass", &missing)
            .is_err());
        let mut invalid = source;
        invalid["spec"]["controllerName"] = serde_json::json!(42);
        assert!(validator
            .validate_gateway_source(GROUP, "v1beta1", "GatewayClass", &invalid)
            .is_err());
    }

    #[test]
    fn test_schema_validator_creation() {
        // This test requires the CRD files to be present
        let crd_dir = PathBuf::from("config/crd");
        if !crd_dir.exists() {
            eprintln!("Skipping test: CRD directory not found");
            return;
        }

        let validator = SchemaValidator::from_crd_dir(&crd_dir);
        assert!(validator.is_ok(), "Failed to create validator: {:?}", validator.err());

        let validator = validator.unwrap();
        assert!(validator.schema_count() > 0, "No schemas loaded");
    }
}
