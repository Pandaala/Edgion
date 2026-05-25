//! Gateway Configuration Store
//!
//! Provides dynamic Gateway configuration lookup with two-layer structure:
//! - `listener_map`: For routes with sectionName (exact listener match)
//! - `host_map`: For routes without sectionName (hostname-based match)
//!
//! This module enables dynamic updates of Gateway listener configurations
//! without requiring server restart.

pub use super::super::gateway_info::GatewayInfo;
use crate::core::common::matcher::HashHost;
use crate::types::resources::gateway::AllowedRoutes;
use crate::types::Gateway;
use arc_swap::ArcSwap;
use kube::ResourceExt;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

/// Single Listener's dynamic configuration
#[derive(Clone, Debug)]
pub struct ListenerConfig {
    /// Listener name
    pub name: String,
    /// Listener port
    pub port: i32,
    /// Hostname for SNI matching (optional)
    pub hostname: Option<String>,
    /// Allowed routes configuration
    pub allowed_routes: Option<AllowedRoutes>,
}

/// Single Gateway's configuration with two-layer structure
///
/// ## Performance Optimization
/// All internal HashMaps are Option types to avoid unnecessary lookups.
/// Most Gateways don't configure hostname in listeners, so we skip
/// host_map lookups entirely when no hostname-based listeners exist.
#[derive(Clone, Default)]
pub struct GatewayListenerConfig {
    /// Host-based mapping for routes without sectionName (exact hostname match)
    /// Key: hostname (e.g., "api.example.com")
    /// None if no listeners have exact hostname configured
    host_map: Option<HashMap<String, Arc<ListenerConfig>>>,

    /// Wildcard host matching engine (for "*.example.com" patterns)
    /// None if no listeners have wildcard hostname configured
    wildcard_host_map: Option<HashHost<Arc<ListenerConfig>>>,

    /// Listener-based mapping for routes with sectionName
    /// Key: listener_name
    /// None if no listeners configured (shouldn't happen in practice)
    listener_map: Option<HashMap<String, Arc<ListenerConfig>>>,
}

impl std::fmt::Debug for GatewayListenerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayListenerConfig")
            .field(
                "host_map_keys",
                &self.host_map.as_ref().map(|m| m.keys().collect::<Vec<_>>()),
            )
            .field(
                "listener_map_keys",
                &self.listener_map.as_ref().map(|m| m.keys().collect::<Vec<_>>()),
            )
            .finish()
    }
}

impl GatewayListenerConfig {
    /// Create a new empty GatewayListenerConfig
    pub fn new() -> Self {
        Self {
            host_map: None,
            wildcard_host_map: None,
            listener_map: None,
        }
    }

    /// Check if a listener exists by name
    #[inline]
    pub fn has_listener(&self, listener_name: &str) -> bool {
        self.listener_map
            .as_ref()
            .map(|m| m.contains_key(listener_name))
            .unwrap_or(false)
    }

    /// Check if a hostname exists in host_map (exact or wildcard)
    ///
    /// Returns true if:
    /// - No host restrictions configured (both maps are None) -> allow all
    /// - Hostname matches exactly in host_map
    /// - Hostname matches a wildcard in wildcard_host_map
    #[cfg(test)]
    #[inline]
    pub fn has_host(&self, hostname: &str) -> bool {
        // If no host restrictions configured, allow all hostnames
        if self.host_map.is_none() && self.wildcard_host_map.is_none() {
            return true;
        }

        // Try exact match first (fast path)
        if let Some(ref host_map) = self.host_map {
            if host_map.contains_key(hostname) {
                return true;
            }
        }
        // Try wildcard match
        if let Some(ref wildcard_map) = self.wildcard_host_map {
            if wildcard_map.get(hostname).is_some() {
                return true;
            }
        }
        false
    }

    /// Get listener config by name
    #[inline]
    pub fn get_listener(&self, listener_name: &str) -> Option<Arc<ListenerConfig>> {
        self.listener_map.as_ref().and_then(|m| m.get(listener_name).cloned())
    }

    /// Check if another listener **on the same port** has a more specific hostname
    /// that also matches the given request hostname.
    ///
    /// Used for HTTP Listener Isolation: a listener should not serve a request
    /// when a more specific listener on the same gateway+port also matches.
    /// Specificity: exact > wildcard > catch-all (no hostname)
    ///
    /// Listeners on different ports are independent per Gateway API spec,
    /// so cross-port listeners are never considered.
    ///
    /// `current_listener_hostname`: the hostname of the listener being evaluated
    ///   - `None` → catch-all listener
    ///   - `Some("*.example.com")` → wildcard listener
    ///   - `Some("foo.example.com")` → exact listener (no check needed)
    pub fn has_more_specific_listener(
        &self,
        hostname: &str,
        current_listener_hostname: Option<&str>,
        current_port: i32,
    ) -> bool {
        use crate::core::gateway::runtime::matching::route::hostname_matches_listener;

        if matches!(current_listener_hostname, Some(h) if !h.starts_with("*.")) {
            return false;
        }

        let listeners = match self.listener_map {
            Some(ref m) => m,
            None => return false,
        };

        for config in listeners.values() {
            if config.port != current_port {
                continue;
            }
            let other_host = match config.hostname.as_deref() {
                Some(h) => h,
                None => continue,
            };

            if !hostname_matches_listener(hostname, other_host) {
                continue;
            }

            match current_listener_hostname {
                None => {
                    // Catch-all: any listener with a hostname that matches is more specific
                    return true;
                }
                Some(current_host) => {
                    // Wildcard: blocked by exact match, or a longer (more specific) wildcard
                    if !other_host.starts_with("*.") {
                        return true;
                    }
                    if other_host != current_host && other_host.len() > current_host.len() {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Add a listener config
    fn add_listener(&mut self, config: Arc<ListenerConfig>) {
        // Always add to listener_map
        self.listener_map
            .get_or_insert_with(HashMap::new)
            .insert(config.name.clone(), config.clone());

        // If hostname exists, also add to host_map
        if let Some(hostname) = config.hostname.clone() {
            if hostname.starts_with("*.") {
                // Wildcard hostname - add to wildcard_host_map.
                // HashHost::insert rejects patterns whose stripped suffix is not a
                // valid multi-label domain (e.g. "*.localhost"). Surface the rejection
                // so operators can see why a listener never participates in
                // hostname-based routing instead of failing silently.
                let listener_name = config.name.clone();
                let inserted = self
                    .wildcard_host_map
                    .get_or_insert_with(HashHost::new)
                    .insert(&hostname, config);
                if !inserted {
                    tracing::warn!(
                        component = "gateway_config_store",
                        listener = %listener_name,
                        hostname = %hostname,
                        "Wildcard listener hostname rejected by matcher; listener will not match by hostname"
                    );
                }
            } else {
                // Exact hostname - add to host_map
                self.host_map.get_or_insert_with(HashMap::new).insert(hostname, config);
            }
        }
    }
}

/// Global Gateway Configuration Store
///
/// Key: "{namespace}/{name}" (Gateway Key)
/// Value: GatewayListenerConfig (two-layer structure)
pub struct GatewayConfigStore {
    /// Gateway configurations
    gateways: ArcSwap<HashMap<String, Arc<GatewayListenerConfig>>>,
}

impl GatewayConfigStore {
    /// Create a new empty store
    pub fn new() -> Self {
        Self {
            gateways: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Load the current gateways map (for reuse in hot path).
    #[inline]
    pub fn load_gateways(&self) -> arc_swap::Guard<Arc<HashMap<String, Arc<GatewayListenerConfig>>>> {
        self.gateways.load()
    }

    /// Check if a listener exists for a gateway
    pub fn has_listener(&self, namespace: &str, gateway_name: &str, listener_name: &str) -> bool {
        let gateway_key = if namespace.is_empty() {
            gateway_name.to_string()
        } else {
            format!("{}/{}", namespace, gateway_name)
        };

        let gateways = self.gateways.load();
        gateways
            .get(&gateway_key)
            .map(|config| config.has_listener(listener_name))
            .unwrap_or(false)
    }

    /// Get listener configuration by GatewayInfo
    ///
    /// This method enables dynamic lookup of listener config, supporting
    /// hot-reload of Gateway configuration changes.
    pub fn get_listener_config(&self, gateway_info: &GatewayInfo) -> Option<Arc<ListenerConfig>> {
        let listener_name = gateway_info.listener_name.as_deref()?;
        let gateway_key = gateway_info.gateway_key();
        let gateways = self.gateways.load();
        let gateway_config = gateways.get(&gateway_key)?;
        gateway_config.get_listener(listener_name)
    }

    /// Full set of all Gateway configurations
    ///
    /// Parses all Gateways and rebuilds the entire store.
    pub fn full_set(&self, gateways: &[Gateway]) {
        let mut new_map: HashMap<String, Arc<GatewayListenerConfig>> = HashMap::new();

        for gateway in gateways {
            let gateway_key = build_gateway_key(gateway);
            let config = parse_gateway_to_config(gateway);
            new_map.insert(gateway_key, Arc::new(config));
        }

        let gateway_count = new_map.len();
        self.gateways.store(Arc::new(new_map));

        tracing::info!(
            component = "gateway_config_store",
            gateways = gateway_count,
            "Full set of Gateway configurations"
        );
    }

    /// Update a single Gateway configuration
    pub fn update_gateway(&self, gateway: &Gateway) {
        let gateway_key = build_gateway_key(gateway);
        let config = parse_gateway_to_config(gateway);

        // Load current map, clone, update, store back
        let current = self.gateways.load();
        let mut new_map = (**current).clone();
        new_map.insert(gateway_key.clone(), Arc::new(config));
        self.gateways.store(Arc::new(new_map));

        tracing::debug!(
            component = "gateway_config_store",
            gateway = %gateway_key,
            "Updated Gateway configuration"
        );
    }

    /// Remove a Gateway configuration
    pub fn remove_gateway(&self, namespace: &str, name: &str) {
        let gateway_key = if namespace.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", namespace, name)
        };

        let current = self.gateways.load();
        let mut new_map = (**current).clone();
        new_map.remove(&gateway_key);
        self.gateways.store(Arc::new(new_map));

        tracing::debug!(
            component = "gateway_config_store",
            gateway = %gateway_key,
            "Removed Gateway configuration"
        );
    }
}

impl GatewayConfigStore {
    /// Number of Gateway configurations currently held.
    pub fn gateway_count(&self) -> usize {
        self.gateways.load().len()
    }
}

impl Default for GatewayConfigStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Build Gateway Key from Gateway resource
fn build_gateway_key(gateway: &Gateway) -> String {
    let namespace = gateway.namespace().unwrap_or_default();
    let name = gateway.name_any();
    if namespace.is_empty() {
        name
    } else {
        format!("{}/{}", namespace, name)
    }
}

/// Parse Gateway resource into GatewayListenerConfig (two-layer structure)
fn parse_gateway_to_config(gateway: &Gateway) -> GatewayListenerConfig {
    let mut config = GatewayListenerConfig::new();

    if let Some(listeners) = &gateway.spec.listeners {
        for listener in listeners {
            let listener_config = Arc::new(ListenerConfig {
                name: listener.name.clone(),
                port: listener.port,
                hostname: listener.hostname.clone(),
                allowed_routes: listener.allowed_routes.clone(),
            });

            config.add_listener(listener_config);
        }
    }

    config
}

/// Global GatewayConfigStore instance
static GLOBAL_GATEWAY_CONFIG_STORE: LazyLock<GatewayConfigStore> = LazyLock::new(GatewayConfigStore::new);

/// Get the global GatewayConfigStore instance
pub fn get_global_gateway_config_store() -> &'static GatewayConfigStore {
    &GLOBAL_GATEWAY_CONFIG_STORE
}

// Unit tests for this module live in `tests/config_test.rs`.
#[cfg(test)]
#[path = "tests/config_test.rs"]
mod tests;
