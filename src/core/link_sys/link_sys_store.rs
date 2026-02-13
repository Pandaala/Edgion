//! Global store for LinkSys resources.
//!
//! Manages all LinkSys resources (Webhook, Redis, Etcd, etc.) and dispatches
//! configuration changes to the appropriate sub-module managers.
//! Follows the same ArcSwap pattern as PluginStore.
//!
//! Redis runtime clients are stored in a separate ArcSwap store for typed access:
//! callers use `get_redis_client("namespace/name")` to obtain a ready-to-use client.

use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use crate::types::resources::link_sys::LinkSys;

use super::etcd::EtcdLinkClient;
use super::redis::RedisLinkClient;
use super::webhook::get_webhook_manager;

// ============================================================
// Type aliases
// ============================================================

type LinkSysMap = HashMap<String, LinkSys>;

// ============================================================
// LinkSysStore
// ============================================================

pub struct LinkSysStore {
    resources: ArcSwap<Arc<LinkSysMap>>,
}

impl Default for LinkSysStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkSysStore {
    pub fn new() -> Self {
        Self {
            resources: ArcSwap::from_pointee(Arc::new(HashMap::new())),
        }
    }

    /// Get a LinkSys resource by key (namespace/name)
    pub fn get(&self, key: &str) -> Option<LinkSys> {
        let map = self.resources.load();
        map.get(key).cloned()
    }

    /// Check if a LinkSys resource exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.resources.load();
        map.contains_key(key)
    }

    /// Get total count of LinkSys resources
    pub fn count(&self) -> usize {
        let map = self.resources.load();
        map.len()
    }

    /// Replace all LinkSys resources atomically (full sync).
    ///
    /// Dispatches each resource to the appropriate sub-module manager
    /// based on its SystemConfig type.
    pub fn replace_all(&self, data: HashMap<String, LinkSys>) {
        // Dispatch all resources to sub-module managers
        let rt = tokio::runtime::Handle::try_current();
        if let Ok(handle) = rt {
            let data_clone = data.clone();
            handle.spawn(async move {
                dispatch_full_set(&data_clone).await;
            });
        }

        self.resources.store(Arc::new(Arc::new(data)));
    }

    /// Update LinkSys resources atomically (incremental sync).
    ///
    /// Dispatches changes to the appropriate sub-module managers.
    pub fn update(&self, add_or_update: HashMap<String, LinkSys>, remove: &HashSet<String>) {
        let current = self.resources.load();
        let current_map: &LinkSysMap = &current;
        let mut new_map: LinkSysMap = current_map.clone();

        for key in remove {
            new_map.remove(key);
        }
        for (key, ls) in add_or_update.iter() {
            new_map.insert(key.clone(), ls.clone());
        }

        // Dispatch changes to sub-module managers
        let rt = tokio::runtime::Handle::try_current();
        if let Ok(handle) = rt {
            let add_update = add_or_update.clone();
            let remove_set = remove.clone();
            handle.spawn(async move {
                dispatch_partial_update(&add_update, &remove_set).await;
            });
        }

        self.resources.store(Arc::new(Arc::new(new_map)));
    }
}

// ============================================================
// Redis runtime store (ArcSwap for lock-free reads)
// ============================================================

/// Global runtime store for Redis clients.
/// Keyed by "namespace/name", same as LinkSys CRD key.
/// ArcSwap provides lock-free concurrent reads, consistent with PluginStore pattern.
static REDIS_RUNTIME: LazyLock<ArcSwap<HashMap<String, Arc<RedisLinkClient>>>> =
    LazyLock::new(|| ArcSwap::from_pointee(HashMap::new()));

/// Get a Redis client by LinkSys name ("namespace/name").
///
/// This is the primary API for plugins and other callers.
/// Returns None if the LinkSys resource doesn't exist or isn't a Redis type.
pub fn get_redis_client(name: &str) -> Option<Arc<RedisLinkClient>> {
    REDIS_RUNTIME.load().get(name).cloned()
}

/// List all registered Redis client names.
pub fn list_redis_clients() -> Vec<String> {
    REDIS_RUNTIME.load().keys().cloned().collect()
}

/// Health check all registered Redis clients.
pub async fn health_check_all_redis() -> Vec<super::redis::LinkSysHealth> {
    let clients: Vec<Arc<RedisLinkClient>> = REDIS_RUNTIME.load().values().cloned().collect();
    let mut results = Vec::with_capacity(clients.len());
    for client in clients {
        results.push(client.health_status().await);
    }
    results
}

/// Insert a Redis client into the runtime store.
fn redis_runtime_insert(key: String, client: Arc<RedisLinkClient>) {
    let current = REDIS_RUNTIME.load();
    let mut new_map = (**current).clone();
    new_map.insert(key, client);
    REDIS_RUNTIME.store(Arc::new(new_map));
}

/// Remove a Redis client from the runtime store, returning the old client (if any).
fn redis_runtime_remove(key: &str) -> Option<Arc<RedisLinkClient>> {
    let current = REDIS_RUNTIME.load();
    if !current.contains_key(key) {
        return None;
    }
    let mut new_map = (**current).clone();
    let old = new_map.remove(key);
    REDIS_RUNTIME.store(Arc::new(new_map));
    old
}

/// Replace all Redis clients in the runtime store atomically.
fn redis_runtime_replace_all(new_map: HashMap<String, Arc<RedisLinkClient>>) -> HashMap<String, Arc<RedisLinkClient>> {
    let old = REDIS_RUNTIME.swap(Arc::new(new_map));
    (*old).clone()
}

// ============================================================
// Etcd runtime store (ArcSwap for lock-free reads)
// ============================================================

/// Global runtime store for Etcd clients.
/// Keyed by "namespace/name", same as LinkSys CRD key.
/// ArcSwap provides lock-free concurrent reads, consistent with Redis pattern.
static ETCD_RUNTIME: LazyLock<ArcSwap<HashMap<String, Arc<EtcdLinkClient>>>> =
    LazyLock::new(|| ArcSwap::from_pointee(HashMap::new()));

/// Get an Etcd client by LinkSys name ("namespace/name").
///
/// This is the primary API for plugins and other callers.
/// Returns None if the LinkSys resource doesn't exist or isn't an Etcd type.
pub fn get_etcd_client(name: &str) -> Option<Arc<EtcdLinkClient>> {
    ETCD_RUNTIME.load().get(name).cloned()
}

/// List all registered Etcd client names.
pub fn list_etcd_clients() -> Vec<String> {
    ETCD_RUNTIME.load().keys().cloned().collect()
}

/// Health check all registered Etcd clients.
pub async fn health_check_all_etcd() -> Vec<super::redis::LinkSysHealth> {
    let clients: Vec<Arc<EtcdLinkClient>> = ETCD_RUNTIME.load().values().cloned().collect();
    let mut results = Vec::with_capacity(clients.len());
    for client in clients {
        results.push(client.health_status().await);
    }
    results
}

/// Insert an Etcd client into the runtime store.
fn etcd_runtime_insert(key: String, client: Arc<EtcdLinkClient>) {
    let current = ETCD_RUNTIME.load();
    let mut new_map = (**current).clone();
    new_map.insert(key, client);
    ETCD_RUNTIME.store(Arc::new(new_map));
}

/// Remove an Etcd client from the runtime store, returning the old client (if any).
fn etcd_runtime_remove(key: &str) -> Option<Arc<EtcdLinkClient>> {
    let current = ETCD_RUNTIME.load();
    if !current.contains_key(key) {
        return None;
    }
    let mut new_map = (**current).clone();
    let old = new_map.remove(key);
    ETCD_RUNTIME.store(Arc::new(new_map));
    old
}

/// Replace all Etcd clients in the runtime store atomically.
fn etcd_runtime_replace_all(new_map: HashMap<String, Arc<EtcdLinkClient>>) -> HashMap<String, Arc<EtcdLinkClient>> {
    let old = ETCD_RUNTIME.swap(Arc::new(new_map));
    (*old).clone()
}

// ============================================================
// Dispatch to sub-module managers
// ============================================================

/// Dispatch a full set of LinkSys resources to sub-module managers.
async fn dispatch_full_set(data: &HashMap<String, LinkSys>) {
    let webhook_manager = get_webhook_manager();

    // Build new clients maps
    let mut new_redis_map: HashMap<String, Arc<RedisLinkClient>> = HashMap::new();
    let mut new_etcd_map: HashMap<String, Arc<EtcdLinkClient>> = HashMap::new();

    for (key, ls) in data {
        match &ls.spec.config {
            crate::types::resources::link_sys::SystemConfig::Webhook(config) => {
                webhook_manager.upsert(key, config.clone()).await;
            }
            crate::types::resources::link_sys::SystemConfig::Redis(redis_config) => {
                match RedisLinkClient::from_config(key, redis_config) {
                    Ok(client) => {
                        let client = Arc::new(client);
                        let client_ref = client.clone();
                        let key_owned = key.clone();
                        // Init in background — don't block full_set for slow connections
                        tokio::spawn(async move {
                            if let Err(e) = client_ref.init().await {
                                tracing::error!(
                                    redis = %key_owned,
                                    error = %e,
                                    "Failed to initialize Redis client"
                                );
                            }
                        });
                        new_redis_map.insert(key.clone(), client);
                    }
                    Err(e) => {
                        tracing::error!(
                            key,
                            error = %e,
                            "Failed to build Redis client from config"
                        );
                    }
                }
            }
            crate::types::resources::link_sys::SystemConfig::Etcd(etcd_config) => {
                match EtcdLinkClient::from_config(key, etcd_config) {
                    Ok(client) => {
                        let client = Arc::new(client);
                        let client_ref = client.clone();
                        let key_owned = key.clone();
                        // Init in background — don't block full_set for slow connections
                        tokio::spawn(async move {
                            if let Err(e) = client_ref.init().await {
                                tracing::error!(
                                    etcd = %key_owned,
                                    error = %e,
                                    "Failed to initialize Etcd client"
                                );
                            }
                        });
                        new_etcd_map.insert(key.clone(), client);
                    }
                    Err(e) => {
                        tracing::error!(
                            key,
                            error = %e,
                            "Failed to build Etcd client from config"
                        );
                    }
                }
            }
            _ => {
                tracing::debug!(
                    key,
                    system_type = ?ls.spec.config.system_type(),
                    "LinkSys resource registered (runtime not yet implemented)"
                );
            }
        }
    }

    // Atomically swap Redis runtime store; shutdown old clients in background
    let old_redis = redis_runtime_replace_all(new_redis_map);
    if !old_redis.is_empty() {
        tokio::spawn(async move {
            for (key, client) in old_redis {
                if let Err(e) = client.shutdown().await {
                    tracing::warn!(redis = %key, error = %e, "Error shutting down old Redis client");
                }
            }
        });
    }

    // Atomically swap Etcd runtime store; shutdown old clients in background
    let old_etcd = etcd_runtime_replace_all(new_etcd_map);
    if !old_etcd.is_empty() {
        tokio::spawn(async move {
            for (key, client) in old_etcd {
                if let Err(e) = client.shutdown().await {
                    tracing::warn!(etcd = %key, error = %e, "Error shutting down old Etcd client");
                }
            }
        });
    }
}

/// Dispatch incremental changes to sub-module managers.
async fn dispatch_partial_update(
    add_or_update: &HashMap<String, LinkSys>,
    remove: &HashSet<String>,
) {
    let webhook_manager = get_webhook_manager();

    // Handle add/update
    for (key, ls) in add_or_update {
        match &ls.spec.config {
            crate::types::resources::link_sys::SystemConfig::Webhook(config) => {
                webhook_manager.upsert(key, config.clone()).await;
            }
            crate::types::resources::link_sys::SystemConfig::Redis(redis_config) => {
                match RedisLinkClient::from_config(key, redis_config) {
                    Ok(client) => {
                        let client = Arc::new(client);
                        let client_ref = client.clone();
                        let key_owned = key.clone();

                        // Swap into store first (so get_redis_client returns new client immediately)
                        let old = {
                            let current = REDIS_RUNTIME.load();
                            let old = current.get(key).cloned();
                            redis_runtime_insert(key.clone(), client);
                            old
                        };

                        // Init new client in background
                        tokio::spawn(async move {
                            if let Err(e) = client_ref.init().await {
                                tracing::error!(
                                    redis = %key_owned,
                                    error = %e,
                                    "Failed to initialize Redis client"
                                );
                            }
                        });

                        // Shutdown old client in background
                        if let Some(old_client) = old {
                            tokio::spawn(async move {
                                if let Err(e) = old_client.shutdown().await {
                                    tracing::warn!(error = %e, "Error shutting down old Redis client");
                                }
                            });
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            key,
                            error = %e,
                            "Failed to build Redis client from config"
                        );
                    }
                }
            }
            crate::types::resources::link_sys::SystemConfig::Etcd(etcd_config) => {
                match EtcdLinkClient::from_config(key, etcd_config) {
                    Ok(client) => {
                        let client = Arc::new(client);
                        let client_ref = client.clone();
                        let key_owned = key.clone();

                        // Swap into store first (so get_etcd_client returns new client immediately)
                        let old = {
                            let current = ETCD_RUNTIME.load();
                            let old = current.get(key).cloned();
                            etcd_runtime_insert(key.clone(), client);
                            old
                        };

                        // Init new client in background
                        tokio::spawn(async move {
                            if let Err(e) = client_ref.init().await {
                                tracing::error!(
                                    etcd = %key_owned,
                                    error = %e,
                                    "Failed to initialize Etcd client"
                                );
                            }
                        });

                        // Shutdown old client in background
                        if let Some(old_client) = old {
                            tokio::spawn(async move {
                                if let Err(e) = old_client.shutdown().await {
                                    tracing::warn!(error = %e, "Error shutting down old Etcd client");
                                }
                            });
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            key,
                            error = %e,
                            "Failed to build Etcd client from config"
                        );
                    }
                }
            }
            _ => {
                tracing::debug!(
                    key,
                    system_type = ?ls.spec.config.system_type(),
                    "LinkSys resource registered (runtime not yet implemented)"
                );
            }
        }
    }

    // Handle remove — we don't know the type of removed resources,
    // so we try to remove from all managers (no-op if not found).
    for key in remove {
        webhook_manager.remove(key).await;

        // Remove Redis client and shutdown in background
        if let Some(old_client) = redis_runtime_remove(key) {
            let key_owned = key.clone();
            tokio::spawn(async move {
                if let Err(e) = old_client.shutdown().await {
                    tracing::warn!(redis = %key_owned, error = %e, "Error shutting down removed Redis client");
                }
            });
        }

        // Remove Etcd client and shutdown in background
        if let Some(old_client) = etcd_runtime_remove(key) {
            let key_owned = key.clone();
            tokio::spawn(async move {
                if let Err(e) = old_client.shutdown().await {
                    tracing::warn!(etcd = %key_owned, error = %e, "Error shutting down removed Etcd client");
                }
            });
        }
    }
}

// ============================================================
// Global singleton
// ============================================================

static GLOBAL_LINK_SYS_STORE: LazyLock<Arc<LinkSysStore>> =
    LazyLock::new(|| Arc::new(LinkSysStore::new()));

/// Get the global LinkSys store
pub fn get_global_link_sys_store() -> Arc<LinkSysStore> {
    GLOBAL_LINK_SYS_STORE.clone()
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_webhook_linksys(namespace: &str, name: &str) -> LinkSys {
        use crate::types::resources::link_sys::webhook::WebhookServiceConfig;
        use crate::types::resources::link_sys::{LinkSysSpec, SystemConfig};

        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                config: SystemConfig::Webhook(WebhookServiceConfig {
                    uri: "http://localhost:8080/resolve".to_string(),
                    ..Default::default()
                }),
            },
            status: None,
        }
    }

    #[test]
    fn test_full_set() {
        let store = LinkSysStore::new();

        let mut data = HashMap::new();
        data.insert(
            "default/webhook1".to_string(),
            create_test_webhook_linksys("default", "webhook1"),
        );
        data.insert(
            "default/webhook2".to_string(),
            create_test_webhook_linksys("default", "webhook2"),
        );

        store.replace_all(data);

        assert!(store.contains("default/webhook1"));
        assert!(store.contains("default/webhook2"));
        assert!(!store.contains("default/webhook3"));
        assert_eq!(store.count(), 2);
    }

    #[test]
    fn test_partial_update_add() {
        let store = LinkSysStore::new();

        let mut add = HashMap::new();
        add.insert(
            "default/webhook1".to_string(),
            create_test_webhook_linksys("default", "webhook1"),
        );

        store.update(add, &HashSet::new());

        assert!(store.contains("default/webhook1"));
    }

    #[test]
    fn test_partial_update_remove() {
        let store = LinkSysStore::new();

        let mut data = HashMap::new();
        data.insert(
            "default/webhook1".to_string(),
            create_test_webhook_linksys("default", "webhook1"),
        );
        store.replace_all(data);

        let mut remove = HashSet::new();
        remove.insert("default/webhook1".to_string());
        store.update(HashMap::new(), &remove);

        assert!(!store.contains("default/webhook1"));
    }

    #[test]
    fn test_get() {
        let store = LinkSysStore::new();

        let mut data = HashMap::new();
        data.insert(
            "default/webhook1".to_string(),
            create_test_webhook_linksys("default", "webhook1"),
        );
        store.replace_all(data);

        let ls = store.get("default/webhook1");
        assert!(ls.is_some());

        let ls = store.get("default/nonexistent");
        assert!(ls.is_none());
    }
}
