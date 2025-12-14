use async_trait::async_trait;
use thiserror::Error;

/// Error type for EdgionResourceMgr operations
#[derive(Debug, Error)]
pub enum ResourceMgrError {
    #[error("Failed to parse resource YAML/JSON: {0}")]
    ParseError(String),
    
    #[error("Resource not found: {0}")]
    NotFound(String),
    
    #[error("Resource already exists: {0}")]
    AlreadyExists(String),
    
    #[error("Unsupported resource kind: {0}")]
    UnsupportedKind(String),
    
    #[error("Invalid resource: {0}")]
    InvalidResource(String),
    
    #[error("Internal error: {0}")]
    InternalError(String),
}

/// Trait for Kubernetes-style resource management operations
/// 
/// This trait provides a unified interface for managing resources in ConfigServer/ConfigClient,
/// supporting standard Kubernetes API operations: GET, CREATE, UPDATE, PATCH, DELETE.
/// 
/// - **GET/DELETE**: Accept kind, namespace, and name as separate parameters
/// - **CREATE/UPDATE**: Accept complete YAML/JSON resource definitions
/// - **PATCH**: Accept kind, namespace, name, and partial YAML/JSON data
#[async_trait]
pub trait EdgionResourceMgr: Send + Sync {
    /// GET - Retrieve a resource
    /// 
    /// # Arguments
    /// * `kind` - Resource kind (e.g., "HTTPRoute", "Service")
    /// * `namespace` - Resource namespace
    /// * `name` - Resource name
    /// 
    /// Returns Ok(()) if resource exists, Err otherwise
    async fn get(&self, kind: String, namespace: String, name: String) -> Result<(), ResourceMgrError>;
    
    /// CREATE/POST - Create a new resource
    /// 
    /// The YAML should contain a complete resource definition
    /// Returns Ok(()) if created successfully, Err if already exists or invalid
    async fn create(&self, resource_yaml: String) -> Result<(), ResourceMgrError>;
    
    /// UPDATE/PUT - Replace an existing resource
    /// 
    /// The YAML should contain a complete resource definition
    /// Returns Ok(()) if updated successfully, Err if not found or invalid
    async fn update(&self, resource_yaml: String) -> Result<(), ResourceMgrError>;
    
    /// PATCH - Partially update a resource
    /// 
    /// # Arguments
    /// * `kind` - Resource kind (e.g., "HTTPRoute", "Service")
    /// * `namespace` - Resource namespace
    /// * `name` - Resource name
    /// * `patch_data` - Partial YAML/JSON data to merge with existing resource
    /// 
    /// Returns Ok(()) if patched successfully, Err if not found or invalid
    async fn patch(&self, kind: String, namespace: String, name: String, patch_data: String) -> Result<(), ResourceMgrError>;
    
    /// DELETE - Remove a resource
    /// 
    /// # Arguments
    /// * `kind` - Resource kind (e.g., "HTTPRoute", "Service")
    /// * `namespace` - Resource namespace
    /// * `name` - Resource name
    /// 
    /// Returns Ok(()) if deleted successfully, Err if not found
    async fn delete(&self, kind: String, namespace: String, name: String) -> Result<(), ResourceMgrError>;
}

