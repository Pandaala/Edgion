use crate::link_sys::providers::webhook::manager::configure_webhook_global_max;
use edgion_common::conf_sync::traits::{arc_map_to_owned, ConfHandler};
use edgion_resources::prelude_resources::EdgionGatewayConfig;
use kube::ResourceExt;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Global store for EdgionGatewayConfig resources (keyed by key_name())
static EDGION_GATEWAY_CONFIG_STORE: std::sync::LazyLock<RwLock<HashMap<String, EdgionGatewayConfig>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// Count EdgionGatewayConfig resources in the store
pub fn count_edgion_gateway_configs() -> usize {
    EDGION_GATEWAY_CONFIG_STORE.read().len()
}

/// Query EdgionGatewayConfig by name (cluster-scoped: key == name)
#[allow(dead_code)]
pub fn get_edgion_gateway_config_by_name(name: &str) -> Option<EdgionGatewayConfig> {
    EDGION_GATEWAY_CONFIG_STORE.read().get(name).cloned()
}

/// List all EdgionGatewayConfig resources
#[allow(dead_code)]
pub fn list_edgion_gateway_configs() -> Vec<EdgionGatewayConfig> {
    let mut items: Vec<EdgionGatewayConfig> = EDGION_GATEWAY_CONFIG_STORE.read().values().cloned().collect();
    items.sort_by(|a, b| a.name_any().cmp(&b.name_any()));
    items
}

/// ConfHandler implementation for EdgionGatewayConfig
pub struct EdgionGatewayConfigHandler;

impl EdgionGatewayConfigHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionGatewayConfigHandler {
    fn default() -> Self {
        Self::new()
    }
}

fn apply_link_sys_config(config: &EdgionGatewayConfig) {
    if let Some(link_sys) = &config.spec.link_sys {
        configure_webhook_global_max(link_sys.webhook_max_response_bytes);
    }
}

fn apply_load_balancing_config(config: &EdgionGatewayConfig) {
    if let Some(lb) = &config.spec.load_balancing {
        crate::backends::panic_threshold::configure_lb_panic_default(lb.panic_threshold);
    }
}

impl ConfHandler<EdgionGatewayConfig> for EdgionGatewayConfigHandler {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionGatewayConfig>>) {
        tracing::info!(
            count = data.len(),
            "EdgionGatewayConfig full_set: received {} EdgionGatewayConfig resources",
            data.len()
        );
        let mut store = EDGION_GATEWAY_CONFIG_STORE.write();
        *store = arc_map_to_owned(data);
        for config in store.values() {
            apply_link_sys_config(config);
            apply_load_balancing_config(config);
        }
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionGatewayConfig>>,
        update: HashMap<String, Arc<EdgionGatewayConfig>>,
        remove: HashSet<String>,
    ) {
        let mut store = EDGION_GATEWAY_CONFIG_STORE.write();

        if !add.is_empty() {
            tracing::info!(
                count = add.len(),
                "EdgionGatewayConfig partial_update: added {} EdgionGatewayConfig resources",
                add.len()
            );
            for (key, config) in add {
                tracing::debug!(key = %key, name = %config.name_any(), "EdgionGatewayConfig added");
                apply_link_sys_config(&config);
                apply_load_balancing_config(&config);
                store.insert(key, (*config).clone());
            }
        }

        if !update.is_empty() {
            tracing::info!(
                count = update.len(),
                "EdgionGatewayConfig partial_update: updated {} EdgionGatewayConfig resources",
                update.len()
            );
            for (key, config) in update {
                tracing::debug!(key = %key, name = %config.name_any(), "EdgionGatewayConfig updated");
                apply_link_sys_config(&config);
                apply_load_balancing_config(&config);
                if store.insert(key.clone(), (*config).clone()).is_none() {
                    tracing::warn!(key = %key, "EdgionGatewayConfig update: key not found in store, inserting");
                }
            }
        }

        if !remove.is_empty() {
            tracing::info!(
                count = remove.len(),
                "EdgionGatewayConfig partial_update: removed {} EdgionGatewayConfig resources",
                remove.len()
            );
            for key in &remove {
                tracing::debug!(key = %key, "EdgionGatewayConfig removed");
                if store.remove(key).is_none() {
                    tracing::debug!(key = %key, "EdgionGatewayConfig remove: key not found in store");
                }
            }
        }
    }
}

/// Create EdgionGatewayConfig handler
pub fn create_edgion_gateway_config_handler() -> Box<dyn ConfHandler<EdgionGatewayConfig> + Send + Sync> {
    Box::new(EdgionGatewayConfigHandler::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfigSpec;
    use std::collections::HashMap;

    // Serialize tests to avoid global-store interference
    static TEST_MUTEX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    fn make_config(name: &str) -> EdgionGatewayConfig {
        EdgionGatewayConfig::new(
            name,
            EdgionGatewayConfigSpec {
                server: None,
                http_timeout: None,
                max_retries: 3,
                real_ip: None,
                security_protect: None,
                global_plugins_ref: None,
                preflight_policy: None,
                enable_reference_grant_validation: false,
                link_sys: None,
                tcp_timeout: None,
                load_balancing: None,
            },
        )
    }

    fn clear_store() {
        EDGION_GATEWAY_CONFIG_STORE.write().clear();
    }

    #[test]
    fn test_full_set_replaces_existing() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut data = HashMap::new();
        data.insert("foo".to_string(), Arc::new(make_config("foo")));
        data.insert("bar".to_string(), Arc::new(make_config("bar")));
        handler.full_set(&data);
        assert_eq!(count_edgion_gateway_configs(), 2);

        // full_set again with only one item — old entries must be gone
        let mut data2 = HashMap::new();
        data2.insert("baz".to_string(), Arc::new(make_config("baz")));
        handler.full_set(&data2);
        assert_eq!(count_edgion_gateway_configs(), 1);
    }

    #[test]
    fn test_add_deduplicates() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut add = HashMap::new();
        add.insert("foo".to_string(), Arc::new(make_config("foo")));
        handler.partial_update(add.clone(), HashMap::new(), std::collections::HashSet::new());
        // add same key again
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

        assert_eq!(
            count_edgion_gateway_configs(),
            1,
            "duplicate add must not create two entries"
        );
    }

    #[test]
    fn test_update_overwrites_existing() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut add = HashMap::new();
        add.insert("foo".to_string(), Arc::new(make_config("foo")));
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

        let mut updated = make_config("foo");
        updated.spec.max_retries = 99;
        let mut upd = HashMap::new();
        upd.insert("foo".to_string(), Arc::new(updated));
        handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());

        let result = get_edgion_gateway_config_by_name("foo").expect("must exist after update");
        assert_eq!(result.spec.max_retries, 99);
        assert_eq!(count_edgion_gateway_configs(), 1);
    }

    #[test]
    fn test_update_miss_inserts() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        // update on empty store must insert (not silently discard)
        let mut upd = HashMap::new();
        upd.insert("ghost".to_string(), Arc::new(make_config("ghost")));
        handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());

        assert_eq!(count_edgion_gateway_configs(), 1, "update miss must insert");
    }

    #[test]
    fn test_remove_works() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut add = HashMap::new();
        add.insert("foo".to_string(), Arc::new(make_config("foo")));
        add.insert("bar".to_string(), Arc::new(make_config("bar")));
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

        let mut remove = std::collections::HashSet::new();
        remove.insert("foo".to_string());
        handler.partial_update(HashMap::new(), HashMap::new(), remove);

        assert_eq!(count_edgion_gateway_configs(), 1);
        assert!(get_edgion_gateway_config_by_name("foo").is_none());
        assert!(get_edgion_gateway_config_by_name("bar").is_some());
    }

    #[test]
    fn test_remove_miss_does_not_panic() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut remove = std::collections::HashSet::new();
        remove.insert("nonexistent".to_string());
        // must not panic
        handler.partial_update(HashMap::new(), HashMap::new(), remove);
        assert_eq!(count_edgion_gateway_configs(), 0);
    }

    #[test]
    fn test_link_sys_config_webhook_max() {
        use edgion_resources::resources::edgion_gateway_config::{EdgionGatewayConfigSpec, LinkSysGlobalConfig};
        let spec = EdgionGatewayConfigSpec {
            link_sys: Some(LinkSysGlobalConfig {
                webhook_max_response_bytes: 65536,
            }),
            ..Default::default()
        };
        assert_eq!(spec.link_sys.unwrap().webhook_max_response_bytes, 65536);
    }

    #[test]
    fn test_list_and_count() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = EdgionGatewayConfigHandler::new();

        let mut add = HashMap::new();
        add.insert("a".to_string(), Arc::new(make_config("a")));
        add.insert("b".to_string(), Arc::new(make_config("b")));
        add.insert("c".to_string(), Arc::new(make_config("c")));
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

        assert_eq!(count_edgion_gateway_configs(), 3);
        assert_eq!(list_edgion_gateway_configs().len(), 3);
    }

    fn make_config_with_link_sys(name: &str, max_bytes: usize) -> EdgionGatewayConfig {
        use edgion_resources::resources::edgion_gateway_config::LinkSysGlobalConfig;
        let mut config = make_config(name);
        config.spec.link_sys = Some(LinkSysGlobalConfig {
            webhook_max_response_bytes: max_bytes,
        });
        config
    }

    #[test]
    fn full_set_propagates_link_sys_config() {
        use crate::link_sys::providers::webhook::manager::{configure_webhook_global_max, get_webhook_global_max};
        let _g = TEST_MUTEX.lock();
        clear_store();

        // Reset the global to a known sentinel before the test
        configure_webhook_global_max(16384);
        assert_eq!(get_webhook_global_max(), 16384);

        // Construct a config with a non-default linkSys.webhookMaxResponseBytes
        let config = make_config_with_link_sys("test-config", 123_456);

        let mut data = HashMap::new();
        data.insert("default/test-config".to_string(), Arc::new(config));

        let handler = EdgionGatewayConfigHandler;
        handler.full_set(&data);

        // Global should now reflect the input
        assert_eq!(get_webhook_global_max(), 123_456);

        // Reset to default for other tests
        configure_webhook_global_max(16384);
    }

    #[test]
    fn test_load_balancing_panic_threshold_applied() {
        use crate::backends::panic_threshold::get_lb_panic_default;
        use edgion_resources::resources::edgion_gateway_config::LoadBalancingConfig;

        let _g = TEST_MUTEX.lock();
        clear_store();

        let handler = EdgionGatewayConfigHandler::new();
        let mut cfg = make_config("gw-pt");
        cfg.spec.load_balancing = Some(LoadBalancingConfig { panic_threshold: 50 });

        let mut data = HashMap::new();
        data.insert("gw-pt".to_string(), Arc::new(cfg));
        handler.full_set(&data);

        assert_eq!(get_lb_panic_default(), 50);
        crate::backends::panic_threshold::configure_lb_panic_default(0);
    }

    #[test]
    fn partial_update_propagates_link_sys_config_on_add_and_update() {
        use crate::link_sys::providers::webhook::manager::{configure_webhook_global_max, get_webhook_global_max};
        let _g = TEST_MUTEX.lock();
        clear_store();

        configure_webhook_global_max(16384);
        let handler = EdgionGatewayConfigHandler;

        // ADD path
        let cfg_a = make_config_with_link_sys("cfg-a", 33_333);
        let mut add = HashMap::new();
        add.insert("default/cfg-a".to_string(), Arc::new(cfg_a));
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());
        assert_eq!(get_webhook_global_max(), 33_333);

        // UPDATE path
        let cfg_b = make_config_with_link_sys("cfg-a", 44_444);
        let mut upd = HashMap::new();
        upd.insert("default/cfg-a".to_string(), Arc::new(cfg_b));
        handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());
        assert_eq!(get_webhook_global_max(), 44_444);

        // Reset
        configure_webhook_global_max(16384);
    }
}
