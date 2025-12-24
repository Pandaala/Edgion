use async_trait::async_trait;
use thiserror::Error;

/// Error type for EdgionConfMgr operations
#[derive(Debug, Error)]
pub enum ConfMgrError {
    #[error("Failed to parse configuration YAML/JSON: {0}")]
    ParseError(String),
    
    #[error("Configuration not found: {0}")]
    NotFound(String),
    
    #[error("Configuration already exists: {0}")]
    AlreadyExists(String),
    
    #[error("Unsupported configuration kind: {0}")]
    UnsupportedKind(String),
    
    #[error("Invalid configuration: {0}")]
    InvalidResource(String),
    
    #[error("Internal error: {0}")]
    InternalError(String),
}

/// Trait for Kubernetes-style configuration management operations
/// 
/// This trait provides a unified interface for managing configurations in ConfigServer/ConfigClient,
/// supporting standard Kubernetes API operations: GET, CREATE, UPDATE, PATCH, DELETE.
/// 
/// - **GET/DELETE**: Accept kind, namespace, and name as separate parameters
/// - **CREATE/UPDATE**: Accept complete YAML/JSON configuration definitions
/// - **PATCH**: Accept kind, namespace, name, and partial YAML/JSON data
#[async_trait]
pub trait EdgionConfMgr: Send + Sync {
    /// GET - Retrieve a configuration
    /// 
    /// # Arguments
    /// * `kind` - Configuration kind (e.g., "HTTPRoute", "Service")
    /// * `namespace` - Configuration namespace
    /// * `name` - Configuration name
    /// 
    /// Returns Ok(()) if configuration exists, Err otherwise
    async fn get(&self, kind: String, namespace: String, name: String) -> Result<(), ConfMgrError>;
    
    /// CREATE/POST - Create a new configuration
    /// 
    /// The YAML should contain a complete configuration definition
    /// Returns Ok(()) if created successfully, Err if already exists or invalid
    async fn create(&self, resource_yaml: String) -> Result<(), ConfMgrError>;
    
    /// UPDATE/PUT - Replace an existing configuration
    /// 
    /// The YAML should contain a complete configuration definition
    /// Returns Ok(()) if updated successfully, Err if not found or invalid
    async fn update(&self, resource_yaml: String) -> Result<(), ConfMgrError>;
    
    /// PATCH - Partially update a configuration
    /// 
    /// # Arguments
    /// * `kind` - Configuration kind (e.g., "HTTPRoute", "Service")
    /// * `namespace` - Configuration namespace
    /// * `name` - Configuration name
    /// * `patch_data` - Partial YAML/JSON data to merge with existing configuration
    /// 
    /// Returns Ok(()) if patched successfully, Err if not found or invalid
    async fn patch(&self, kind: String, namespace: String, name: String, patch_data: String) -> Result<(), ConfMgrError>;
    
    /// DELETE - Remove a configuration
    /// 
    /// # Arguments
    /// * `kind` - Configuration kind (e.g., "HTTPRoute", "Service")
    /// * `namespace` - Configuration namespace
    /// * `name` - Configuration name
    /// 
    /// Returns Ok(()) if deleted successfully, Err if not found
    async fn delete(&self, kind: String, namespace: String, name: String) -> Result<(), ConfMgrError>;
}

