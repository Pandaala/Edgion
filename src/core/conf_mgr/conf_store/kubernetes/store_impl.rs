//! KubernetesStore implementation
//!
//! Implements the ConfStore trait using Kubernetes API as the backend

use anyhow::Result;
use async_trait::async_trait;
use kube::Client;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::conf_mgr::conf_store::{ConfEntry, ConfStore, ConfStoreError};

/// Kubernetes-based configuration store
///
/// Maintains an in-memory cache of all resources watched from Kubernetes API.
/// The cache is updated by the KubernetesController which watches for resource changes.
#[derive(Clone)]
pub struct KubernetesStore {
    pub(crate) client: Client,
    /// Internal cache: key = "kind/namespace/name" or "kind//name" for cluster-scoped
    cache: Arc<RwLock<HashMap<String, ConfEntry>>>,
}

impl KubernetesStore {
    /// Create a new KubernetesStore with default Kubernetes client
    pub async fn new() -> Result<Arc<Self>> {
        let client = Client::try_default().await?;
        Ok(Arc::new(Self {
            client,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }))
    }

    /// Apply a resource to the cache (called by controller on watch events)
    pub async fn apply_resource(&self, kind: String, namespace: Option<String>, name: String, content: String) {
        let key = Self::make_key(&kind, namespace.as_deref(), &name);
        let entry = ConfEntry {
            kind,
            namespace,
            name,
            content,
        };
        self.cache.write().await.insert(key, entry);
    }

    /// Remove a resource from the cache (called by controller on delete events)
    pub async fn remove_resource(&self, kind: &str, namespace: Option<&str>, name: &str) {
        let key = Self::make_key(kind, namespace, name);
        self.cache.write().await.remove(&key);
    }

    /// Make a unique key for cache storage
    fn make_key(kind: &str, namespace: Option<&str>, name: &str) -> String {
        match namespace {
            Some(ns) => format!("{}/{}/{}", kind, ns, name),
            None => format!("{}//{}", kind, name),
        }
    }
}

#[async_trait]
impl ConfStore for KubernetesStore {
    async fn set_one(
        &self,
        kind: &str,
        namespace: Option<&str>,
        name: &str,
        content: String,
    ) -> Result<(), ConfStoreError> {
        // For K8s mode, writes typically go through kubectl/K8s API
        // This method updates the local cache
        self.apply_resource(
            kind.to_string(),
            namespace.map(|s| s.to_string()),
            name.to_string(),
            content,
        )
        .await;
        Ok(())
    }

    async fn get_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<String, ConfStoreError> {
        let key = Self::make_key(kind, namespace, name);
        let cache = self.cache.read().await;
        cache.get(&key).map(|entry| entry.content.clone()).ok_or_else(|| {
            ConfStoreError::NotFound(format!(
                "Resource not found: kind={}, namespace={:?}, name={}",
                kind, namespace, name
            ))
        })
    }

    async fn get_list_by_kind(&self, kind: &str) -> Result<Vec<ConfEntry>, ConfStoreError> {
        let cache = self.cache.read().await;
        let entries: Vec<ConfEntry> = cache.values().filter(|entry| entry.kind == kind).cloned().collect();
        Ok(entries)
    }

    async fn get_list_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<Vec<ConfEntry>, ConfStoreError> {
        let cache = self.cache.read().await;
        let entries: Vec<ConfEntry> = cache
            .values()
            .filter(|entry| entry.kind == kind && entry.namespace.as_deref() == Some(namespace))
            .cloned()
            .collect();
        Ok(entries)
    }

    async fn cnt_by_kind(&self, kind: &str) -> Result<usize, ConfStoreError> {
        let cache = self.cache.read().await;
        let count = cache.values().filter(|entry| entry.kind == kind).count();
        Ok(count)
    }

    async fn cnt_by_kind_ns(&self, kind: &str, namespace: &str) -> Result<usize, ConfStoreError> {
        let cache = self.cache.read().await;
        let count = cache
            .values()
            .filter(|entry| entry.kind == kind && entry.namespace.as_deref() == Some(namespace))
            .count();
        Ok(count)
    }

    async fn delete_one(&self, kind: &str, namespace: Option<&str>, name: &str) -> Result<(), ConfStoreError> {
        // For K8s mode, deletes typically go through kubectl/K8s API
        // This method removes from local cache
        self.remove_resource(kind, namespace, name).await;
        Ok(())
    }

    async fn list_all(&self) -> Result<Vec<ConfEntry>, ConfStoreError> {
        let cache = self.cache.read().await;
        Ok(cache.values().cloned().collect())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
