use std::collections::HashMap;
use std::path::Path;
use std::fs;
use anyhow::{Context, Result};
use jsonschema::Validator;
use serde_json::Value as JsonValue;
use crate::types::ResourceKind;

/// Schema validation error
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Schema validation failed: {0}")]
    SchemaViolation(String),
    
    #[error("No schema found for resource kind: {0:?}")]
    NoSchema(ResourceKind),
    
    #[error("Failed to convert resource to JSON: {0}")]
    JsonConversion(#[from] serde_json::Error),
}

/// Schema validator for Kubernetes resources based on CRD definitions
pub struct SchemaValidator {
    validators: HashMap<ResourceKind, Validator>,
}

impl SchemaValidator {
    /// Create an empty SchemaValidator with no schemas
    pub fn empty() -> Self {
        Self {
            validators: HashMap::new(),
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
        
        // Load Gateway API CRDs
        let gateway_api_file = crd_dir.join("gateway-api/gateway-api-standard-v1.4.0.yaml");
        if gateway_api_file.exists() {
            Self::load_gateway_api_crds(&gateway_api_file, &mut validators)?;
        }
        
        tracing::info!(
            component = "schema_validator",
            schema_count = validators.len(),
            "Loaded CRD schemas"
        );
        
        Ok(Self { validators })
    }
    
    /// Load CRD files from a directory
    fn load_crd_files(dir: &Path, validators: &mut HashMap<ResourceKind, Validator>) -> Result<()> {
        let entries = fs::read_dir(dir)
            .with_context(|| format!("Failed to read CRD directory: {:?}", dir))?;
        
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("yaml") {
                Self::load_single_crd(&path, validators)?;
            }
        }
        
        Ok(())
    }
    
    /// Load a single CRD file
    fn load_single_crd(path: &Path, validators: &mut HashMap<ResourceKind, Validator>) -> Result<()> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read CRD file: {:?}", path))?;
        
        let crd: JsonValue = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse CRD YAML: {:?}", path))?;
        
        // Extract kind from CRD
        if let Some(kind_str) = crd.get("spec")
            .and_then(|s| s.get("names"))
            .and_then(|n| n.get("kind"))
            .and_then(|k| k.as_str())
        {
            if let Some(resource_kind) = ResourceKind::from_kind_name(kind_str) {
                // Extract schema
                if let Some(schema) = crd.get("spec")
                    .and_then(|s| s.get("versions"))
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|ver| ver.get("schema"))
                    .and_then(|s| s.get("openAPIV3Schema"))
                {
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
            }
        }
        
        Ok(())
    }
    
    /// Load Gateway API CRDs from a single file containing multiple CRD definitions
    fn load_gateway_api_crds(path: &Path, validators: &mut HashMap<ResourceKind, Validator>) -> Result<()> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read Gateway API CRD file: {:?}", path))?;
        
        // Split by document separator
        for doc in content.split("\n---\n") {
            let doc = doc.trim();
            if doc.is_empty() || doc.starts_with('#') {
                continue;
            }
            
            if let Ok(crd) = serde_yaml::from_str::<JsonValue>(doc) {
                // Check if this is a CRD
                if crd.get("kind").and_then(|k| k.as_str()) != Some("CustomResourceDefinition") {
                    continue;
                }
                
                // Extract kind from CRD
                if let Some(kind_str) = crd.get("spec")
                    .and_then(|s| s.get("names"))
                    .and_then(|n| n.get("kind"))
                    .and_then(|k| k.as_str())
                {
                    if let Some(resource_kind) = ResourceKind::from_kind_name(kind_str) {
                        // Extract schema
                        if let Some(schema) = crd.get("spec")
                            .and_then(|s| s.get("versions"))
                            .and_then(|v| v.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|ver| ver.get("schema"))
                            .and_then(|s| s.get("openAPIV3Schema"))
                        {
                            match Validator::new(schema) {
                                Ok(validator) => {
                                    tracing::debug!(
                                        component = "schema_validator",
                                        kind = ?resource_kind,
                                        "Loaded Gateway API schema"
                                    );
                                    validators.insert(resource_kind, validator);
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        component = "schema_validator",
                                        kind = ?resource_kind,
                                        error = %e,
                                        "Failed to compile Gateway API schema"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        
        Ok(())
    }
    
    /// Validate a resource against its schema
    pub fn validate(&self, kind: ResourceKind, value: &JsonValue) -> Result<(), ValidationError> {
        match self.validators.get(&kind) {
            Some(validator) => {
                if validator.is_valid(value) {
                    Ok(())
                } else {
                    // Collect validation errors for logging
                    let error_iter = validator.iter_errors(value);
                    let mut error_messages = Vec::new();
                    
                    for error in error_iter {
                        let msg = format!(
                            "Path: {}, Error: {}",
                            error.instance_path,
                            error
                        );
                        error_messages.push(msg);
                        tracing::debug!(
                            component = "schema_validator",
                            kind = ?kind,
                            path = %error.instance_path,
                            error = %error,
                            "Schema validation error"
                        );
                    }
                    
                    Err(ValidationError::SchemaViolation(
                        if error_messages.is_empty() {
                            "Schema validation failed".to_string()
                        } else {
                            error_messages.join("; ")
                        }
                    ))
                }
            }
            None => {
                // No schema available for this resource kind
                // Log a warning but don't fail validation
                tracing::warn!(
                    component = "schema_validator",
                    kind = ?kind,
                    "No schema available, skipping validation"
                );
                Ok(())
            }
        }
    }
    
    /// Check if a schema is available for a resource kind
    pub fn has_schema(&self, kind: ResourceKind) -> bool {
        self.validators.contains_key(&kind)
    }
    
    /// Get the number of loaded schemas
    pub fn schema_count(&self) -> usize {
        self.validators.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    
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

