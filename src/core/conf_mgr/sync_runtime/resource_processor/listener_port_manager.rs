//! Listener Port Manager
//!
//! Tracks port usage across all Gateways for conflict detection.
//! Similar to SecretRefManager but for port→listener relationships.
//!
//! ## Design
//!
//! - Forward index: port_key → Set of ListenerRefs (all listeners using this port)
//! - Reverse index: gateway_key → Set of port_keys (all ports used by this gateway)
//!
//! ## Port Key Format
//!
//! According to Gateway API spec (distinct listener rules):
//! - HTTP/HTTPS/TLS: Port + Hostname must be different → key = "port:hostname"
//! - TCP/UDP: Port must be different → key = "port:"

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, RwLock};

/// Global ListenerPortManager instance
pub static LISTENER_PORT_MANAGER: LazyLock<ListenerPortManager> = LazyLock::new(ListenerPortManager::new);

/// Get the global ListenerPortManager
pub fn get_listener_port_manager() -> &'static ListenerPortManager {
    &LISTENER_PORT_MANAGER
}

/// Listener reference (gateway + listener name)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListenerRef {
    /// Gateway key in "namespace/name" format
    pub gateway_key: String,
    /// Listener name within the gateway
    pub listener_name: String,
}

impl ListenerRef {
    /// Create a new ListenerRef
    pub fn new(gateway_key: impl Into<String>, listener_name: impl Into<String>) -> Self {
        Self {
            gateway_key: gateway_key.into(),
            listener_name: listener_name.into(),
        }
    }

    /// Get display string for this listener reference
    pub fn display(&self) -> String {
        format!("{}/{}", self.gateway_key, self.listener_name)
    }
}

/// Port key for distinct check
/// Format: "port:hostname" for HTTP/HTTPS/TLS, "port:" for TCP/UDP
pub type PortKey = String;

/// Build port key from listener attributes
///
/// According to Gateway API spec:
/// - HTTP, HTTPS, TLS: Port + Hostname must be different
/// - TCP, UDP: Port must be different (hostname not considered)
pub fn make_port_key(port: i32, protocol: &str, hostname: Option<&str>) -> PortKey {
    match protocol.to_uppercase().as_str() {
        "HTTP" | "HTTPS" | "TLS" => {
            // Port + Hostname must be unique
            format!("{}:{}", port, hostname.unwrap_or(""))
        }
        _ => {
            // TCP/UDP only care about port
            format!("{}:", port)
        }
    }
}

/// Manages listener port usage across all Gateways
///
/// This manager tracks which listeners are using which ports to enable
/// conflict detection across all Gateways in the cluster.
pub struct ListenerPortManager {
    /// Forward index: port_key → listeners using this port
    port_to_listeners: RwLock<HashMap<PortKey, HashSet<ListenerRef>>>,

    /// Reverse index: gateway_key → port_keys it uses
    gateway_to_ports: RwLock<HashMap<String, HashSet<PortKey>>>,
}

impl ListenerPortManager {
    /// Create a new ListenerPortManager
    pub fn new() -> Self {
        Self {
            port_to_listeners: RwLock::new(HashMap::new()),
            gateway_to_ports: RwLock::new(HashMap::new()),
        }
    }

    /// Register a gateway's listeners
    ///
    /// This should be called in GatewayHandler.parse() to register all listeners.
    /// It first clears old registrations for the gateway, then adds new ones.
    ///
    /// # Arguments
    /// * `gateway_key` - Gateway key in "namespace/name" format
    /// * `listeners` - List of (listener_name, port_key) tuples
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    /// to prevent deadlocks.
    pub fn register_gateway(&self, gateway_key: &str, listeners: &[(String, PortKey)]) {
        // Acquire both locks in consistent order to prevent deadlock
        let mut gw_map = self.gateway_to_ports.write().unwrap();
        let mut port_map = self.port_to_listeners.write().unwrap();

        // Clear old registrations first (inline to avoid lock order issues)
        if let Some(old_port_keys) = gw_map.remove(gateway_key) {
            for port_key in &old_port_keys {
                if let Some(listeners_set) = port_map.get_mut(port_key) {
                    listeners_set.retain(|l| l.gateway_key != gateway_key);
                    if listeners_set.is_empty() {
                        port_map.remove(port_key);
                    }
                }
            }
        }

        // Add new registrations
        let mut port_keys = HashSet::new();

        for (listener_name, port_key) in listeners {
            let listener_ref = ListenerRef::new(gateway_key, listener_name.clone());

            port_map.entry(port_key.clone()).or_default().insert(listener_ref);
            port_keys.insert(port_key.clone());
        }

        if !port_keys.is_empty() {
            gw_map.insert(gateway_key.to_string(), port_keys);
        }

        tracing::debug!(
            gateway = %gateway_key,
            listener_count = listeners.len(),
            "Registered gateway listeners to port manager"
        );
    }

    /// Unregister a gateway
    ///
    /// This should be called in GatewayHandler.on_delete() to clean up.
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    /// to prevent deadlocks.
    pub fn unregister_gateway(&self, gateway_key: &str) {
        // Acquire both locks in consistent order to prevent deadlock
        let mut gw_map = self.gateway_to_ports.write().unwrap();
        let mut port_map = self.port_to_listeners.write().unwrap();

        let port_keys = gw_map.remove(gateway_key).unwrap_or_default();

        if port_keys.is_empty() {
            return;
        }

        for port_key in &port_keys {
            if let Some(listeners) = port_map.get_mut(port_key) {
                listeners.retain(|l| l.gateway_key != gateway_key);
                if listeners.is_empty() {
                    port_map.remove(port_key);
                }
            }
        }

        tracing::debug!(
            gateway = %gateway_key,
            port_count = port_keys.len(),
            "Unregistered gateway from port manager"
        );
    }

    /// Get all listeners using a specific port
    pub fn get_listeners_for_port(&self, port_key: &str) -> Vec<ListenerRef> {
        self.port_to_listeners
            .read()
            .unwrap()
            .get(port_key)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Detect port conflicts for a specific gateway
    ///
    /// Returns a HashMap where:
    /// - Key: listener_name that has conflicts
    /// - Value: (conflict_reason, list of all conflicting ListenerRefs)
    ///
    /// # Returns
    /// Empty HashMap if no conflicts, otherwise contains conflicting listeners.
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    /// to prevent deadlocks.
    pub fn detect_conflicts(&self, gateway_key: &str) -> HashMap<String, (String, Vec<ListenerRef>)> {
        let mut conflicts = HashMap::new();

        // Acquire locks in consistent order
        let gw_map = self.gateway_to_ports.read().unwrap();
        let port_map = self.port_to_listeners.read().unwrap();

        let port_keys = match gw_map.get(gateway_key) {
            Some(keys) => keys,
            None => return conflicts,
        };

        for port_key in port_keys {
            if let Some(listeners) = port_map.get(port_key) {
                if listeners.len() > 1 {
                    // Conflict detected: multiple listeners using the same port
                    let conflicting: Vec<ListenerRef> = listeners.iter().cloned().collect();

                    let reason = format!(
                        "Port {} conflicts with: {}",
                        port_key,
                        conflicting.iter().map(|l| l.display()).collect::<Vec<_>>().join(", ")
                    );

                    // Mark all listeners of this gateway that use the conflicting port
                    for listener in listeners {
                        if listener.gateway_key == gateway_key {
                            conflicts.insert(listener.listener_name.clone(), (reason.clone(), conflicting.clone()));
                        }
                    }
                }
            }
        }

        conflicts
    }

    /// Get all gateway keys that have conflicts with a given gateway
    ///
    /// Returns a set of gateway keys (excluding the given gateway itself)
    /// that share conflicting ports.
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    /// to prevent deadlocks.
    pub fn get_conflicting_gateways(&self, gateway_key: &str) -> HashSet<String> {
        let mut conflicting_gateways = HashSet::new();

        // Acquire locks in consistent order
        let gw_map = self.gateway_to_ports.read().unwrap();
        let port_map = self.port_to_listeners.read().unwrap();

        let port_keys = match gw_map.get(gateway_key) {
            Some(keys) => keys,
            None => return conflicting_gateways,
        };

        for port_key in port_keys {
            if let Some(listeners) = port_map.get(port_key) {
                if listeners.len() > 1 {
                    for listener in listeners {
                        if listener.gateway_key != gateway_key {
                            conflicting_gateways.insert(listener.gateway_key.clone());
                        }
                    }
                }
            }
        }

        conflicting_gateways
    }

    /// Clear all registrations
    ///
    /// This should be called during reload/re-election to reset state.
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    /// to prevent deadlocks.
    pub fn clear(&self) {
        // Acquire locks in consistent order
        let mut gw_map = self.gateway_to_ports.write().unwrap();
        let mut port_map = self.port_to_listeners.write().unwrap();
        gw_map.clear();
        port_map.clear();
        tracing::info!("ListenerPortManager cleared");
    }

    /// Get statistics for debugging/metrics
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    /// to prevent deadlocks.
    pub fn stats(&self) -> (usize, usize) {
        // Acquire locks in consistent order
        let gw_map = self.gateway_to_ports.read().unwrap();
        let port_map = self.port_to_listeners.read().unwrap();
        let gateway_count = gw_map.len();
        let port_count = port_map.len();
        (port_count, gateway_count)
    }
}

impl Default for ListenerPortManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_port_key_http() {
        // HTTP/HTTPS/TLS use port + hostname
        assert_eq!(make_port_key(80, "HTTP", Some("example.com")), "80:example.com");
        assert_eq!(make_port_key(443, "HTTPS", Some("example.com")), "443:example.com");
        assert_eq!(make_port_key(443, "TLS", None), "443:");
    }

    #[test]
    fn test_make_port_key_tcp_udp() {
        // TCP/UDP use port only
        assert_eq!(make_port_key(3306, "TCP", Some("mysql.example.com")), "3306:");
        assert_eq!(make_port_key(53, "UDP", None), "53:");
    }

    #[test]
    fn test_register_and_unregister() {
        let manager = ListenerPortManager::new();

        // Register gateway A
        manager.register_gateway(
            "default/gateway-a",
            &[
                ("http".to_string(), "80:".to_string()),
                ("https".to_string(), "443:example.com".to_string()),
            ],
        );

        // Check forward index
        let listeners_80 = manager.get_listeners_for_port("80:");
        assert_eq!(listeners_80.len(), 1);
        assert_eq!(listeners_80[0].gateway_key, "default/gateway-a");

        // Register gateway B with same port
        manager.register_gateway("default/gateway-b", &[("http".to_string(), "80:".to_string())]);

        // Now port 80 should have 2 listeners
        let listeners_80 = manager.get_listeners_for_port("80:");
        assert_eq!(listeners_80.len(), 2);

        // Unregister gateway A
        manager.unregister_gateway("default/gateway-a");

        // Only gateway B should remain
        let listeners_80 = manager.get_listeners_for_port("80:");
        assert_eq!(listeners_80.len(), 1);
        assert_eq!(listeners_80[0].gateway_key, "default/gateway-b");
    }

    #[test]
    fn test_detect_conflicts_single_gateway() {
        let manager = ListenerPortManager::new();

        // Gateway with two listeners on same port (internal conflict)
        manager.register_gateway(
            "default/gateway-a",
            &[
                ("http-1".to_string(), "80:".to_string()),
                ("http-2".to_string(), "80:".to_string()), // Same port!
            ],
        );

        let conflicts = manager.detect_conflicts("default/gateway-a");
        assert_eq!(conflicts.len(), 2); // Both listeners are conflicted
        assert!(conflicts.contains_key("http-1"));
        assert!(conflicts.contains_key("http-2"));
    }

    #[test]
    fn test_detect_conflicts_cross_gateway() {
        let manager = ListenerPortManager::new();

        // Gateway A uses port 443
        manager.register_gateway("default/gateway-a", &[("https".to_string(), "443:".to_string())]);

        // Gateway B also uses port 443 (conflict!)
        manager.register_gateway("default/gateway-b", &[("tls".to_string(), "443:".to_string())]);

        // Both gateways should detect conflicts
        let conflicts_a = manager.detect_conflicts("default/gateway-a");
        let conflicts_b = manager.detect_conflicts("default/gateway-b");

        assert_eq!(conflicts_a.len(), 1);
        assert_eq!(conflicts_b.len(), 1);
        assert!(conflicts_a.contains_key("https"));
        assert!(conflicts_b.contains_key("tls"));
    }

    #[test]
    fn test_get_conflicting_gateways() {
        let manager = ListenerPortManager::new();

        manager.register_gateway("default/gateway-a", &[("https".to_string(), "443:".to_string())]);
        manager.register_gateway("default/gateway-b", &[("tls".to_string(), "443:".to_string())]);
        manager.register_gateway("default/gateway-c", &[("http".to_string(), "80:".to_string())]);

        let conflicting = manager.get_conflicting_gateways("default/gateway-a");
        assert_eq!(conflicting.len(), 1);
        assert!(conflicting.contains("default/gateway-b"));
        assert!(!conflicting.contains("default/gateway-c"));
    }

    #[test]
    fn test_no_conflict_different_hostnames() {
        let manager = ListenerPortManager::new();

        // Same port but different hostnames (allowed for HTTP/HTTPS/TLS)
        manager.register_gateway(
            "default/gateway-a",
            &[("https".to_string(), "443:api.example.com".to_string())],
        );
        manager.register_gateway(
            "default/gateway-b",
            &[("https".to_string(), "443:web.example.com".to_string())],
        );

        // No conflicts because hostnames are different
        let conflicts_a = manager.detect_conflicts("default/gateway-a");
        let conflicts_b = manager.detect_conflicts("default/gateway-b");

        assert!(conflicts_a.is_empty());
        assert!(conflicts_b.is_empty());
    }

    #[test]
    fn test_clear() {
        let manager = ListenerPortManager::new();

        manager.register_gateway("default/gateway-a", &[("http".to_string(), "80:".to_string())]);

        let (ports, gateways) = manager.stats();
        assert_eq!(ports, 1);
        assert_eq!(gateways, 1);

        manager.clear();

        let (ports, gateways) = manager.stats();
        assert_eq!(ports, 0);
        assert_eq!(gateways, 0);
    }
}
