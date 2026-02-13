//! Global store for LinkSys resources.
//!
//! Manages all LinkSys resources (Webhook, Redis, Etcd, etc.) and dispatches
//! configuration changes to the appropriate sub-module managers.
//! Follows the same ArcSwap pattern as PluginStore.

use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use crate::types::resources::link_sys::LinkSys;

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
// Dispatch to sub-module managers
// ============================================================

/// Dispatch a full set of LinkSys resources to sub-module managers.
async fn dispatch_full_set(data: &HashMap<String, LinkSys>) {
    let webhook_manager = get_webhook_manager();

    for (key, ls) in data {
        dispatch_single_upsert(webhook_manager, key, ls).await;
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
        dispatch_single_upsert(webhook_manager, key, ls).await;
    }

    // Handle remove — we don't know the type of removed resources,
    // so we try to remove from all managers (no-op if not found).
    for key in remove {
        webhook_manager.remove(key).await;
        // Future: redis_manager.remove(key).await;
        // Future: etcd_manager.remove(key).await;
    }
}

/// Dispatch a single LinkSys resource upsert to the appropriate manager.
async fn dispatch_single_upsert(
    webhook_manager: &super::webhook::WebhookManager,
    key: &str,
    ls: &LinkSys,
) {
    match &ls.spec.config {
        crate::types::resources::link_sys::SystemConfig::Webhook(config) => {
            webhook_manager.upsert(key, config.clone()).await;
        }
        crate::types::resources::link_sys::SystemConfig::Redis(_) => {
            tracing::debug!(key, "LinkSys Redis resource registered (runtime not yet implemented)");
        }
        crate::types::resources::link_sys::SystemConfig::Etcd(_) => {
            tracing::debug!(key, "LinkSys Etcd resource registered (runtime not yet implemented)");
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
