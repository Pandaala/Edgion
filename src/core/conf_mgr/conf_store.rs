use async_trait::async_trait;
use thiserror::Error;

/// Configuration store trait for persistent configuration storage
#[async_trait]
pub trait ConfStore: Send + Sync {
    /// Set a single configuration (create or update)
    async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfStoreError>;
    
    /// Get a single configuration YAML content
    async fn get_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
    ) -> Result<String, ConfStoreError>;
    
    /// List all configurations by kind
    async fn get_list_by_kind(&self, kind: &str) -> Result<Vec<ConfEntry>, ConfStoreError>;
    
    /// List configurations by kind and namespace
    async fn get_list_by_kind_ns(
        &self,
        kind: &str,
        namespace: &str,
    ) -> Result<Vec<ConfEntry>, ConfStoreError>;
    
    /// Count configurations by kind
    async fn cnt_by_kind(&self, kind: &str) -> Result<usize, ConfStoreError>;
    
    /// Count configurations by kind and namespace
    async fn cnt_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<usize, ConfStoreError>;
    
    /// Delete a single configuration
    async fn delete_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
    ) -> Result<(), ConfStoreError>;
    
    /// List all configurations (for initialization)
    async fn list_all(&self) -> Result<Vec<ConfEntry>, ConfStoreError>;
}

/// Configuration entry with metadata and content
#[derive(Debug, Clone)]
pub struct ConfEntry {
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
    pub content: String,  // Raw YAML content
}

/// Error types for configuration store operations
#[derive(Debug, Error)]
pub enum ConfStoreError {
    #[error("Configuration not found: {0}")]
    NotFound(String),
    
    #[error("Configuration already exists: {0}")]
    AlreadyExists(String),
    
    #[error("Parse error: {0}")]
    ParseError(String),
    
    #[error("IO error: {0}")]
    IOError(String),
    
    #[error("Internal error: {0}")]
    InternalError(String),
}

