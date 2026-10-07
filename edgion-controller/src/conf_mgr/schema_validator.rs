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
    /// Reuses the finite reference normalization/CEL view before existing OpenAPI checks.
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
        let prepared =
            edgion_resources::resources::refs::source_contract::prepare_gateway_source(group, version, kind, value)
                .map_err(|_| ValidationError::SchemaViolation("Gateway API reference validation failed".to_string()))?;
        if let Some(view) = &prepared {
            if !view.violations.is_empty() {
                return Err(ValidationError::SchemaViolation(
                    view.violations
                        .iter()
                        .take(16)
                        .map(|violation| {
                            format!(
                                "Path: {}, Schema: {}",
                                safe_gateway_error_path(
                                    &violation.instance_path,
                                    &violation.schema_path,
                                    &view.normalized
                                ),
                                violation.schema_path.chars().take(200).collect::<String>()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; "),
                ));
            }
        }
        let value = prepared.as_ref().map(|view| &view.normalized).unwrap_or(value);
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
#[path = "../../tests/unit/conf_mgr/schema_validator_test.rs"]
mod tests;
