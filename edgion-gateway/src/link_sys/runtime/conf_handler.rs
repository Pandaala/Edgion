//! ConfHandler implementation for LinkSys resources.
//!
//! Integrates LinkSysStore with the configuration sync system so that
//! LinkSys resource changes are automatically dispatched to the appropriate
//! sub-module managers (Webhook, Redis, Etcd, etc.).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::store::{get_global_link_sys_store, LinkSysStore};
use edgion_common::conf_sync::traits::{arc_map_into_owned, arc_map_to_owned, ConfHandler};
use edgion_resources::resources::link_sys::LinkSys;

/// Create a LinkSysStore handler for registration with ConfigClient
pub fn create_link_sys_handler() -> Box<dyn ConfHandler<LinkSys> + Send + Sync> {
    Box::new(get_global_link_sys_store())
}

impl ConfHandler<LinkSys> for LinkSysStore {
    fn full_set(&self, data: &HashMap<String, Arc<LinkSys>>) {
        tracing::info!(component = "link_sys_store", cnt = data.len(), "full set");

        self.replace_all(arc_map_to_owned(data));
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<LinkSys>>,
        update: HashMap<String, Arc<LinkSys>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "link_sys_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        let mut add_or_update = arc_map_into_owned(add);
        add_or_update.extend(arc_map_into_owned(update));

        self.update(add_or_update, &remove);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::link_sys::webhook::{
        ExternalHttpConnection, WebhookServiceConfig, WebhookServiceRef, WebhookTarget,
    };
    use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};

    fn create_test_linksys(namespace: &str, name: &str) -> LinkSys {
        LinkSys {
            metadata: kube::core::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: LinkSysSpec {
                config: SystemConfig::Webhook(WebhookServiceConfig {
                    conn: ExternalHttpConnection {
                        target: WebhookTarget {
                            service: WebhookServiceRef {
                                kind: None,
                                name: Some("resolve-svc".to_string()),
                                namespace: Some(namespace.to_string()),
                                port: Some(8080),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    },
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
        data.insert(
            "default/wh1".to_string(),
            Arc::new(create_test_linksys("default", "wh1")),
        );
        data.insert(
            "default/wh2".to_string(),
            Arc::new(create_test_linksys("default", "wh2")),
        );

        ConfHandler::full_set(&store, &data);

        assert!(store.contains("default/wh1"));
        assert!(store.contains("default/wh2"));
    }

    #[test]
    fn test_conf_handler_partial_update() {
        let store = LinkSysStore::new();

        // Add initial data
        let mut data = HashMap::new();
        data.insert(
            "default/wh1".to_string(),
            Arc::new(create_test_linksys("default", "wh1")),
        );
        ConfHandler::full_set(&store, &data);

        // Partial update: add wh2, remove wh1
        let mut add = HashMap::new();
        add.insert(
            "default/wh2".to_string(),
            Arc::new(create_test_linksys("default", "wh2")),
        );

        let mut remove = HashSet::new();
        remove.insert("default/wh1".to_string());

        ConfHandler::partial_update(&store, add, HashMap::new(), remove);

        assert!(!store.contains("default/wh1"));
        assert!(store.contains("default/wh2"));
    }
}
