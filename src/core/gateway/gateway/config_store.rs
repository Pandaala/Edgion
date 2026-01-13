//! Gateway Configuration Store
//!
//! Provides dynamic Gateway configuration lookup with two-layer structure:
//! - `listener_map`: For routes with sectionName (exact listener match)
//! - `host_map`: For routes without sectionName (hostname-based match)
//!
//! This module enables dynamic updates of Gateway listener configurations
//! without requiring server restart.

use crate::core::matcher::HashHost;
use crate::types::resources::gateway::{AllowedRoutes, GatewayTLSConfig};
use crate::types::Gateway;
use arc_swap::ArcSwap;
use kube::ResourceExt;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

/// Gateway information for route matching context
///
/// Used to pass gateway context during route matching to support
/// both sectionName-based and hostname-based lookup strategies.
///
/// This struct should be created once when EdgionHttp is constructed,
/// not per-request, to avoid allocation overhead.
#[derive(Clone, Debug)]
pub struct GatewayInfo {
    /// Gateway namespace (None for cluster-scoped or default namespace)
    pub namespace: Option<String>,
    /// Gateway name
    pub name: String,
    /// Current listener name (None for hostname-based lookup)
    pub listener_name: Option<String>,
}

impl GatewayInfo {
    /// Create a new GatewayInfo
    pub fn new(namespace: Option<String>, name: String, listener_name: Option<String>) -> Self {
        Self {
            namespace,
            name,
            listener_name,
        }
    }

    /// Get namespace as &str, returns empty string if None
    #[inline]
    pub fn namespace_str(&self) -> &str {
        self.namespace.as_deref().unwrap_or("")
    }

    /// Build Gateway Key: "{namespace}/{name}" or just "{name}" if no namespace
    pub fn gateway_key(&self) -> String {
        match &self.namespace {
            Some(ns) if !ns.is_empty() => format!("{}/{}", ns, self.name),
            _ => self.name.clone(),
        }
    }
}

/// Single Listener's dynamic configuration
#[derive(Clone, Debug)]
pub struct ListenerConfig {
    /// Listener name
    pub name: String,
    /// Listening port
    pub port: i32,
    /// Protocol (HTTP, HTTPS, TCP, etc.)
    pub protocol: String,
    /// Hostname for SNI matching (optional)
    pub hostname: Option<String>,
    /// TLS configuration
    pub tls: Option<GatewayTLSConfig>,
    /// Allowed routes configuration
    pub allowed_routes: Option<AllowedRoutes>,
}

/// Single Gateway's configuration with two-layer structure
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
                &self
                    .host_map
                    .as_ref()
                    .map(|m| m.keys().collect::<Vec<_>>()),
            )
            .field(
                "listener_map_keys",
                &self
                    .listener_map
                    .as_ref()
                    .map(|m| m.keys().collect::<Vec<_>>()),
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
        self.listener_map
            .as_ref()
            .and_then(|m| m.get(listener_name).cloned())
    }

    /// Get listener config by hostname (exact or wildcard)
    #[inline]
    pub fn get_by_host(&self, hostname: &str) -> Option<Arc<ListenerConfig>> {
        // Try exact match first (O(1))
        if let Some(ref host_map) = self.host_map {
            if let Some(config) = host_map.get(hostname) {
                return Some(config.clone());
            }
        }
        // Try wildcard match
        if let Some(ref wildcard_map) = self.wildcard_host_map {
            if let Some(config) = wildcard_map.get(hostname) {
                return Some(config.clone());
            }
        }
        None
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
                // Wildcard hostname - add to wildcard_host_map
                self.wildcard_host_map
                    .get_or_insert_with(HashHost::new)
                    .insert(&hostname, config);
            } else {
                // Exact hostname - add to host_map
                self.host_map
                    .get_or_insert_with(HashMap::new)
                    .insert(hostname, config);
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

    /// Check if store is empty
    pub fn is_empty(&self) -> bool {
        self.gateways.load().is_empty()
    }

    /// Lookup listener config with two-layer strategy
    ///
    /// - If section_name is Some: lookup from listener_map
    /// - If section_name is None: lookup from host_map by hostname
    pub fn lookup(
        &self,
        namespace: &str,
        gateway_name: &str,
        section_name: Option<&str>,
        hostname: &str,
    ) -> Option<Arc<ListenerConfig>> {
        let gateway_key = if namespace.is_empty() {
            gateway_name.to_string()
        } else {
            format!("{}/{}", namespace, gateway_name)
        };

        let gateways = self.gateways.load();
        let gateway_config = gateways.get(&gateway_key)?;

        match section_name {
            // Has sectionName: direct lookup from listener_map
            Some(listener_name) => gateway_config.get_listener(listener_name),
            // No sectionName: lookup from host_map by hostname
            None => gateway_config.get_by_host(hostname),
        }
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

    /// Check if a hostname exists for a gateway
    pub fn has_host(&self, namespace: &str, gateway_name: &str, hostname: &str) -> bool {
        let gateway_key = if namespace.is_empty() {
            gateway_name.to_string()
        } else {
            format!("{}/{}", namespace, gateway_name)
        };

        let gateways = self.gateways.load();
        gateways
            .get(&gateway_key)
            .map(|config| config.has_host(hostname))
            .unwrap_or(false)
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
                protocol: listener.protocol.clone(),
                hostname: listener.hostname.clone(),
                tls: listener.tls.clone(),
                allowed_routes: listener.allowed_routes.clone(),
            });

            config.add_listener(listener_config);
        }
    }

    config
}

/// Global GatewayConfigStore instance
static GLOBAL_GATEWAY_CONFIG_STORE: LazyLock<GatewayConfigStore> =
    LazyLock::new(GatewayConfigStore::new);

/// Get the global GatewayConfigStore instance
pub fn get_global_gateway_config_store() -> &'static GatewayConfigStore {
    &GLOBAL_GATEWAY_CONFIG_STORE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::gateway::{GatewaySpec, Listener};
    use kube::api::ObjectMeta;

    fn create_test_gateway(name: &str, namespace: &str, listeners: Vec<Listener>) -> Gateway {
        Gateway {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            spec: GatewaySpec {
                gateway_class_name: "test-class".to_string(),
                listeners: Some(listeners),
                addresses: None,
            },
            status: None,
        }
    }

    fn create_test_listener(name: &str, port: i32, hostname: Option<&str>) -> Listener {
        Listener {
            name: name.to_string(),
            hostname: hostname.map(|s| s.to_string()),
            port,
            protocol: "HTTPS".to_string(),
            tls: None,
            allowed_routes: None,
        }
    }

    #[test]
    fn test_gateway_info() {
        let info = GatewayInfo::new(
            Some("default".to_string()),
            "my-gateway".to_string(),
            Some("https".to_string()),
        );
        assert_eq!(info.gateway_key(), "default/my-gateway");
        assert_eq!(info.namespace_str(), "default");

        let info_no_ns = GatewayInfo::new(None, "my-gateway".to_string(), None);
        assert_eq!(info_no_ns.gateway_key(), "my-gateway");
        assert_eq!(info_no_ns.namespace_str(), "");
    }

    #[test]
    fn test_parse_gateway_to_config() {
        let listeners = vec![
            create_test_listener("http", 80, Some("example.com")),
            create_test_listener("https", 443, Some("*.example.com")),
            create_test_listener("admin", 8443, None),
        ];
        let gateway = create_test_gateway("test-gw", "default", listeners);

        let config = parse_gateway_to_config(&gateway);

        // Check listener_map
        assert!(config.has_listener("http"));
        assert!(config.has_listener("https"));
        assert!(config.has_listener("admin"));
        assert!(!config.has_listener("unknown"));

        // Check host_map (exact)
        assert!(config.has_host("example.com"));

        // Check wildcard
        assert!(config.has_host("api.example.com"));
        assert!(config.has_host("www.example.com"));
    }

    #[test]
    fn test_store_full_set() {
        let store = GatewayConfigStore::new();

        let gw1 = create_test_gateway(
            "gw1",
            "ns1",
            vec![create_test_listener("https", 443, Some("api.example.com"))],
        );
        let gw2 = create_test_gateway(
            "gw2",
            "ns2",
            vec![create_test_listener("https", 443, Some("www.example.com"))],
        );

        store.full_set(&[gw1, gw2]);

        // Check lookup with sectionName
        let result = store.lookup("ns1", "gw1", Some("https"), "");
        assert!(result.is_some());
        assert_eq!(result.unwrap().hostname, Some("api.example.com".to_string()));

        // Check lookup without sectionName (by hostname)
        let result = store.lookup("ns2", "gw2", None, "www.example.com");
        assert!(result.is_some());

        // Check non-existent
        let result = store.lookup("ns1", "gw1", Some("unknown"), "");
        assert!(result.is_none());
    }

    #[test]
    fn test_store_update_and_remove() {
        let store = GatewayConfigStore::new();

        let gw = create_test_gateway(
            "test-gw",
            "default",
            vec![create_test_listener("https", 443, Some("example.com"))],
        );

        // Update
        store.update_gateway(&gw);
        assert!(store.has_listener("default", "test-gw", "https"));

        // Remove
        store.remove_gateway("default", "test-gw");
        assert!(!store.has_listener("default", "test-gw", "https"));
    }
}
