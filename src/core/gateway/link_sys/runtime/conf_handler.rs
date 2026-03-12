//! ConfHandler implementation for LinkSys resources.
//!
//! Integrates LinkSysStore with the configuration sync system so that
//! LinkSys resource changes are automatically dispatched to the appropriate
//! sub-module managers (Webhook, Redis, Etcd, etc.).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::store::{get_global_link_sys_store, LinkSysStore};
use crate::core::common::conf_sync::traits::ConfHandler;
use crate::types::resources::link_sys::LinkSys;

/// Implement ConfHandler for Arc<LinkSysStore>
impl ConfHandler<LinkSys> for Arc<LinkSysStore> {
    fn full_set(&self, data: &HashMap<String, LinkSys>) {
        (**self).full_set(data)
    }

    fn partial_update(&self, add: HashMap<String, LinkSys>, update: HashMap<String, LinkSys>, remove: HashSet<String>) {
        (**self).partial_update(add, update, remove)
    }
}

/// Create a LinkSysStore handler for registration with ConfigClient
pub fn create_link_sys_handler() -> Box<dyn ConfHandler<LinkSys> + Send + Sync> {
    Box::new(get_global_link_sys_store())
}

impl ConfHandler<LinkSys> for LinkSysStore {
    fn full_set(&self, data: &HashMap<String, LinkSys>) {
        tracing::info!(component = "link_sys_store", cnt = data.len(), "full set");

        // Validate each resource before storing
        let mut prepared_data = data.clone();
        for ls in prepared_data.values_mut() {
            ls.validate_config();
        }

        self.replace_all(prepared_data);
    }

    fn partial_update(&self, add: HashMap<String, LinkSys>, update: HashMap<String, LinkSys>, remove: HashSet<String>) {
        tracing::info!(
            component = "link_sys_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        let mut add_or_update = add;
        add_or_update.extend(update);

        // Validate each resource
        for ls in add_or_update.values_mut() {
            ls.validate_config();
        }

        self.update(add_or_update, &remove);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::link_sys::webhook::WebhookServiceConfig;
    use crate::types::resources::link_sys::{LinkSysSpec, SystemConfig};

    fn create_test_linksys(namespace: &str, name: &str) -> LinkSys {
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
    fn test_conf_handler_full_set() {
        let store = LinkSysStore::new();

        let mut data = HashMap::new();
        data.insert("default/wh1".to_string(), create_test_linksys("default", "wh1"));
        data.insert("default/wh2".to_string(), create_test_linksys("default", "wh2"));

        ConfHandler::full_set(&store, &data);

        assert!(store.contains("default/wh1"));
        assert!(store.contains("default/wh2"));
    }

    #[test]
    fn test_conf_handler_partial_update() {
        let store = LinkSysStore::new();

        // Add initial data
        let mut data = HashMap::new();
        data.insert("default/wh1".to_string(), create_test_linksys("default", "wh1"));
        ConfHandler::full_set(&store, &data);

        // Partial update: add wh2, remove wh1
        let mut add = HashMap::new();
        add.insert("default/wh2".to_string(), create_test_linksys("default", "wh2"));

        let mut remove = HashSet::new();
        remove.insert("default/wh1".to_string());

        ConfHandler::partial_update(&store, add, HashMap::new(), remove);

        assert!(!store.contains("default/wh1"));
        assert!(store.contains("default/wh2"));
    }
}
