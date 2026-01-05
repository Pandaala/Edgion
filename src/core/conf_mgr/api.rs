use super::{ConfEntry, ConfStore, ConfStoreError};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Resource manager API supporting multiple storage backends
pub struct ResourceMgrAPI {
    backends: RwLock<HashMap<String, Arc<dyn ConfStore>>>,
    default_backend: RwLock<Option<String>>,
}

impl ResourceMgrAPI {
    pub fn new() -> Self {
        Self {
            backends: RwLock::new(HashMap::new()),
            default_backend: RwLock::new(None),
        }
    }

    /// Register a storage backend
    pub fn register_backend(&self, name: String, backend: Arc<dyn ConfStore>) {
        let mut backends = self.backends.write().unwrap();
        backends.insert(name.clone(), backend);
        tracing::info!(component = "conf_mgr_api", backend = name, "Storage backend registered");
    }

    /// Set default backend
    pub fn set_default_backend(&self, name: String) -> Result<(), String> {
        let backends = self.backends.read().unwrap();
        if !backends.contains_key(&name) {
            return Err(format!("Backend '{}' not registered", name));
        }
        drop(backends);

        let mut default = self.default_backend.write().unwrap();
        *default = Some(name.clone());
        tracing::info!(component = "conf_mgr_api", backend = name, "Default backend set");
        Ok(())
    }

    /// Get backend by name (or default if None)
    pub fn get_backend(&self, name: Option<&str>) -> Result<Arc<dyn ConfStore>, String> {
        let backends = self.backends.read().unwrap();
        let backend_name: String = match name {
            Some(n) => n.to_string(),
            None => {
                let default = self.default_backend.read().unwrap();
                default.as_ref().ok_or("No default backend set")?.clone()
            }
        };
        backends
            .get(&backend_name)
            .cloned()
            .ok_or_else(|| format!("Backend '{}' not found", backend_name))
    }

    // Proxy methods to default backend
    pub async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfStoreError> {
        let backend = self.get_backend(None).map_err(|e| ConfStoreError::InternalError(e))?;
        backend.set_one(kind, namespace, name, content).await
    }

    pub async fn get_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<String, ConfStoreError> {
        let backend = self.get_backend(None).map_err(|e| ConfStoreError::InternalError(e))?;
        backend.get_one(kind, namespace, name).await
    }

    pub async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfStoreError> {
        let backend = self.get_backend(None).map_err(|e| ConfStoreError::InternalError(e))?;
        backend.delete_one(kind, namespace, name).await
    }

    pub async fn list_all(&self) -> Result<Vec<ConfEntry>, ConfStoreError> {
        let backend = self.get_backend(None).map_err(|e| ConfStoreError::InternalError(e))?;
        backend.list_all().await
    }

    pub async fn get_list_by_kind(&self, kind: &str) -> Result<Vec<ConfEntry>, ConfStoreError> {
        let backend = self.get_backend(None).map_err(|e| ConfStoreError::InternalError(e))?;
        backend.get_list_by_kind(kind).await
    }

    pub async fn cnt_by_kind(&self, kind: &str) -> Result<usize, ConfStoreError> {
        let backend = self.get_backend(None).map_err(|e| ConfStoreError::InternalError(e))?;
        backend.cnt_by_kind(kind).await
    }
}
