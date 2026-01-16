//! KubernetesStore - In-memory cache for Controller
//!
//! Maintains an in-memory cache of all resources watched from Kubernetes API.
//! The cache is updated by the KubernetesController which watches for resource changes.
//!
//! Note: This is NOT used for admin-api write operations. Use KubernetesWriter for that.

use anyhow::Result;
use kube::Client;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::conf_mgr::conf_center::ConfEntry;

/// Kubernetes-based configuration store (in-memory cache)
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

    /// Create a new KubernetesStore with existing client
    pub fn with_client(client: Client) -> Arc<Self> {
        Arc::new(Self {
            client,
            cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Get the Kubernetes client
    pub fn client(&self) -> &Client {
        &self.client
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

    /// Get a resource from cache
    pub async fn get_resource(&self, kind: &str, namespace: Option<&str>, name: &str) -> Option<ConfEntry> {
        let key = Self::make_key(kind, namespace, name);
        self.cache.read().await.get(&key).cloned()
    }

    /// List all resources in cache
    pub async fn list_all(&self) -> Vec<ConfEntry> {
        self.cache.read().await.values().cloned().collect()
    }

    /// List resources by kind
    pub async fn list_by_kind(&self, kind: &str) -> Vec<ConfEntry> {
        self.cache
            .read()
            .await
            .values()
            .filter(|e| e.kind == kind)
            .cloned()
            .collect()
    }

    /// Make a unique key for cache storage
    fn make_key(kind: &str, namespace: Option<&str>, name: &str) -> String {
        match namespace {
            Some(ns) => format!("{}/{}/{}", kind, ns, name),
            None => format!("{}//{}", kind, name),
        }
    }
}
