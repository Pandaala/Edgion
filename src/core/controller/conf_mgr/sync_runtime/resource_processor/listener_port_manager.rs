//! Listener Port Manager
//!
//! Tracks port usage within each Gateway for intra-gateway conflict detection.
//! Per Gateway API spec, Conflicted condition applies only within a single Gateway;
//! different Gateways may independently use the same port.
//!
//! ## Design
//!
//! - Forward index: port_key → Set of ListenerRefs (all listeners using this port)
//! - Reverse index: gateway_key → Set of port_keys (all ports used by this gateway)
//! - Conflict detection considers hostname intersection (wildcard-aware)
//!
//! ## Port Key Format
//!
//! Port keys group listeners that *could* conflict (same port+protocol group):
//! - HTTP/HTTPS/TLS: "port" (e.g. "443")
//! - TCP/UDP: "PROTO:port" (e.g. "TCP:53", "UDP:53")
//!
//! Within each port key group, hostname intersection determines actual conflicts.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, RwLock};

/// Global ListenerPortManager instance
pub static LISTENER_PORT_MANAGER: LazyLock<ListenerPortManager> = LazyLock::new(ListenerPortManager::new);

/// Get the global ListenerPortManager
pub fn get_listener_port_manager() -> &'static ListenerPortManager {
    &LISTENER_PORT_MANAGER
}

/// Listener reference (gateway + listener name + hostname for conflict checking)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListenerRef {
    /// Gateway key in "namespace/name" format
    pub gateway_key: String,
    /// Listener name within the gateway
    pub listener_name: String,
    /// Hostname (None = wildcard, matches everything)
    pub hostname: Option<String>,
}

impl ListenerRef {
    pub fn new(
        gateway_key: impl Into<String>,
        listener_name: impl Into<String>,
        hostname: Option<String>,
    ) -> Self {
        Self {
            gateway_key: gateway_key.into(),
            listener_name: listener_name.into(),
            hostname,
        }
    }
}

/// Port key for grouping listeners that could potentially conflict.
pub type PortKey = String;

/// Build port key from listener attributes.
///
/// Groups listeners by port+protocol so that hostname intersection can be
/// checked within each group during conflict detection.
///
/// - HTTP, HTTPS, TLS: grouped by port only (hostname checked separately)
/// - TCP, UDP: grouped by protocol+port (hostname irrelevant)
pub fn make_port_key(port: i32, protocol: &str) -> PortKey {
    match protocol.to_uppercase().as_str() {
        "HTTP" | "HTTPS" | "TLS" => format!("{}", port),
        proto => format!("{}:{}", proto, port),
    }
}

/// Check if two listener hostnames are identical (i.e. truly conflict).
///
/// Per Gateway API spec, Conflicted means two listeners have the same
/// port+protocol+hostname combination — not merely overlapping hostnames.
/// Overlapping hostnames (e.g. wildcard vs specific) are handled by routing
/// priority and are explicitly allowed when GatewayHTTPListenerIsolation is supported.
///
/// - None == None → conflict (both are wildcard)
/// - `foo.example.com` == `foo.example.com` → conflict (identical)
/// - None vs `foo.example.com` → no conflict (different, just overlapping)
/// - `*.example.com` vs `foo.example.com` → no conflict (different, just overlapping)
fn hostnames_conflict(a: Option<&str>, b: Option<&str>) -> bool {
    a == b
}

/// Manages listener port usage for intra-gateway conflict detection.
///
/// Tracks which listeners are using which ports across all Gateways,
/// but only reports conflicts within the same Gateway.
/// Conflict detection is hostname-aware: wildcard listeners conflict
/// with any hostname-specific listener on the same port.
pub struct ListenerPortManager {
    /// Forward index: port_key → listeners using this port
    port_to_listeners: RwLock<HashMap<PortKey, HashSet<ListenerRef>>>,

    /// Reverse index: gateway_key → port_keys it uses
    gateway_to_ports: RwLock<HashMap<String, HashSet<PortKey>>>,
}

impl ListenerPortManager {
    pub fn new() -> Self {
        Self {
            port_to_listeners: RwLock::new(HashMap::new()),
            gateway_to_ports: RwLock::new(HashMap::new()),
        }
    }

    /// Register a gateway's listeners.
    ///
    /// Each listener is a tuple of (listener_name, port_key, hostname).
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    pub fn register_gateway(&self, gateway_key: &str, listeners: &[(String, PortKey, Option<String>)]) {
        let mut gw_map = self.gateway_to_ports.write().unwrap();
        let mut port_map = self.port_to_listeners.write().unwrap();

        // Clear old registrations
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

        for (listener_name, port_key, hostname) in listeners {
            // Normalize empty hostname to None (wildcard)
            let hostname = hostname.as_ref().filter(|h| !h.is_empty()).cloned();
            let listener_ref = ListenerRef::new(gateway_key, listener_name.clone(), hostname);
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

    /// Unregister a gateway and clean up all its port registrations.
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    pub fn unregister_gateway(&self, gateway_key: &str) {
        let mut gw_map = self.gateway_to_ports.write().unwrap();
        let mut port_map = self.port_to_listeners.write().unwrap();

        let port_keys = gw_map.remove(gateway_key).unwrap_or_default();

        for port_key in &port_keys {
            if let Some(listeners) = port_map.get_mut(port_key) {
                listeners.retain(|l| l.gateway_key != gateway_key);
                if listeners.is_empty() {
                    port_map.remove(port_key);
                }
            }
        }

        if !port_keys.is_empty() {
            tracing::debug!(
                gateway = %gateway_key,
                port_count = port_keys.len(),
                "Unregistered gateway from port manager"
            );
        }
    }

    /// Detect intra-gateway port conflicts for a specific gateway.
    ///
    /// Two listeners on the same port conflict if their hostnames intersect:
    /// - None (wildcard) conflicts with any hostname
    /// - `*.example.com` conflicts with `foo.example.com`
    /// - `api.example.com` does NOT conflict with `web.example.com`
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    pub fn detect_conflicts(&self, gateway_key: &str) -> HashMap<String, String> {
        let mut conflicts = HashMap::new();

        let gw_map = self.gateway_to_ports.read().unwrap();
        let port_map = self.port_to_listeners.read().unwrap();

        let port_keys = match gw_map.get(gateway_key) {
            Some(keys) => keys,
            None => return conflicts,
        };

        for port_key in port_keys {
            let Some(listeners) = port_map.get(port_key) else {
                continue;
            };

            // Collect same-gateway listeners on this port
            let same_gw: Vec<&ListenerRef> = listeners
                .iter()
                .filter(|l| l.gateway_key == gateway_key)
                .collect();

            if same_gw.len() < 2 {
                continue;
            }

            // For TCP/UDP there's no hostname dimension — all same-port listeners conflict
            // For HTTP/HTTPS/TLS, check hostname intersection pairwise
            for (i, listener) in same_gw.iter().enumerate() {
                let others: Vec<&str> = same_gw
                    .iter()
                    .enumerate()
                    .filter(|(j, other)| {
                        *j != i
                            && hostnames_conflict(
                                listener.hostname.as_deref(),
                                other.hostname.as_deref(),
                            )
                    })
                    .map(|(_, l)| l.listener_name.as_str())
                    .collect();

                if !others.is_empty() {
                    let reason = format!("Port {}: conflicts with: {}", port_key, others.join(", "));
                    conflicts.insert(listener.listener_name.clone(), reason);
                }
            }
        }

        conflicts
    }

    /// Clear all registrations (e.g. during reload/re-election).
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    pub fn clear(&self) {
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
    pub fn stats(&self) -> (usize, usize) {
        let gw_map = self.gateway_to_ports.read().unwrap();
        let port_map = self.port_to_listeners.read().unwrap();
        (port_map.len(), gw_map.len())
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
    fn test_make_port_key() {
        // HTTP/HTTPS/TLS: port only
        assert_eq!(make_port_key(80, "HTTP"), "80");
        assert_eq!(make_port_key(443, "HTTPS"), "443");
        assert_eq!(make_port_key(443, "TLS"), "443");

        // TCP/UDP: protocol:port
        assert_eq!(make_port_key(3306, "TCP"), "TCP:3306");
        assert_eq!(make_port_key(53, "UDP"), "UDP:53");
        assert_ne!(make_port_key(53, "TCP"), make_port_key(53, "UDP"));
    }

    #[test]
    fn test_hostnames_conflict_fn() {
        // Both wildcard (None) → conflict
        assert!(hostnames_conflict(None, None));

        // Wildcard vs concrete → no conflict (overlapping but not identical)
        assert!(!hostnames_conflict(None, Some("example.com")));
        assert!(!hostnames_conflict(Some("example.com"), None));

        // Exact match → conflict
        assert!(hostnames_conflict(Some("example.com"), Some("example.com")));

        // Different concrete hostnames → no conflict
        assert!(!hostnames_conflict(Some("api.example.com"), Some("web.example.com")));

        // Wildcard pattern vs concrete → no conflict (overlapping but not identical)
        assert!(!hostnames_conflict(Some("*.example.com"), Some("foo.example.com")));
        assert!(!hostnames_conflict(Some("foo.example.com"), Some("*.example.com")));

        // Different wildcard patterns → no conflict
        assert!(!hostnames_conflict(Some("*.example.com"), Some("*.bar.example.com")));
        assert!(!hostnames_conflict(Some("*.foo.com"), Some("*.bar.com")));

        // Same wildcard pattern → conflict
        assert!(hostnames_conflict(Some("*.example.com"), Some("*.example.com")));
    }

    #[test]
    fn test_register_and_unregister() {
        let manager = ListenerPortManager::new();

        manager.register_gateway(
            "default/gateway-a",
            &[
                ("http".to_string(), "80".to_string(), None),
                ("https".to_string(), "443".to_string(), Some("example.com".to_string())),
            ],
        );

        manager.register_gateway("default/gateway-b", &[("http".to_string(), "80".to_string(), None)]);

        let (ports, gateways) = manager.stats();
        assert_eq!(gateways, 2);
        assert_eq!(ports, 2); // "80" and "443"

        manager.unregister_gateway("default/gateway-a");

        let (ports, gateways) = manager.stats();
        assert_eq!(gateways, 1);
        assert_eq!(ports, 1);
    }

    #[test]
    fn test_conflict_same_port_same_hostname() {
        let manager = ListenerPortManager::new();

        manager.register_gateway(
            "default/gw",
            &[
                ("http-1".to_string(), "80".to_string(), None),
                ("http-2".to_string(), "80".to_string(), None),
            ],
        );

        let conflicts = manager.detect_conflicts("default/gw");
        assert_eq!(conflicts.len(), 2);
        assert!(conflicts.contains_key("http-1"));
        assert!(conflicts.contains_key("http-2"));
    }

    #[test]
    fn test_no_conflict_wildcard_vs_specific() {
        let manager = ListenerPortManager::new();

        // Wildcard (None) vs concrete hostname — overlapping but not identical, no conflict
        manager.register_gateway(
            "default/gw",
            &[
                ("catchall".to_string(), "443".to_string(), None),
                ("specific".to_string(), "443".to_string(), Some("api.example.com".to_string())),
            ],
        );

        assert!(manager.detect_conflicts("default/gw").is_empty());
    }

    #[test]
    fn test_no_conflict_wildcard_pattern_vs_concrete() {
        let manager = ListenerPortManager::new();

        // *.example.com vs foo.example.com — overlapping but not identical, no conflict
        manager.register_gateway(
            "default/gw",
            &[
                ("wild".to_string(), "443".to_string(), Some("*.example.com".to_string())),
                ("concrete".to_string(), "443".to_string(), Some("foo.example.com".to_string())),
            ],
        );

        assert!(manager.detect_conflicts("default/gw").is_empty());
    }

    #[test]
    fn test_conflict_same_wildcard_pattern() {
        let manager = ListenerPortManager::new();

        // Two listeners with identical wildcard pattern → conflict
        manager.register_gateway(
            "default/gw",
            &[
                ("wild-1".to_string(), "443".to_string(), Some("*.example.com".to_string())),
                ("wild-2".to_string(), "443".to_string(), Some("*.example.com".to_string())),
            ],
        );

        let conflicts = manager.detect_conflicts("default/gw");
        assert_eq!(conflicts.len(), 2);
    }

    #[test]
    fn test_no_conflict_different_hostnames() {
        let manager = ListenerPortManager::new();

        // Different concrete hostnames on same port — no conflict
        manager.register_gateway(
            "default/gw",
            &[
                ("api".to_string(), "443".to_string(), Some("api.example.com".to_string())),
                ("web".to_string(), "443".to_string(), Some("web.example.com".to_string())),
            ],
        );

        assert!(manager.detect_conflicts("default/gw").is_empty());
    }

    #[test]
    fn test_no_conflict_cross_gateway() {
        let manager = ListenerPortManager::new();

        manager.register_gateway("default/gw-a", &[("https".to_string(), "443".to_string(), None)]);
        manager.register_gateway("default/gw-b", &[("https".to_string(), "443".to_string(), None)]);

        assert!(manager.detect_conflicts("default/gw-a").is_empty());
        assert!(manager.detect_conflicts("default/gw-b").is_empty());
    }

    #[test]
    fn test_no_conflict_different_ports() {
        let manager = ListenerPortManager::new();

        manager.register_gateway(
            "default/gw",
            &[
                ("http".to_string(), "80".to_string(), None),
                ("https".to_string(), "443".to_string(), None),
            ],
        );

        assert!(manager.detect_conflicts("default/gw").is_empty());
    }

    #[test]
    fn test_no_conflict_tcp_vs_udp_same_port() {
        let manager = ListenerPortManager::new();

        // TCP and UDP on same port number within same gateway — no conflict
        manager.register_gateway(
            "default/gw",
            &[
                ("tcp-53".to_string(), make_port_key(53, "TCP"), None),
                ("udp-53".to_string(), make_port_key(53, "UDP"), None),
            ],
        );

        assert!(manager.detect_conflicts("default/gw").is_empty());
    }

    #[test]
    fn test_re_register_clears_old() {
        let manager = ListenerPortManager::new();

        manager.register_gateway("default/gw", &[("http".to_string(), "80".to_string(), None)]);
        manager.register_gateway("default/gw", &[]);

        let (ports, gateways) = manager.stats();
        assert_eq!(gateways, 0);
        assert_eq!(ports, 0);
    }

    #[test]
    fn test_clear() {
        let manager = ListenerPortManager::new();
        manager.register_gateway("default/gw", &[("http".to_string(), "80".to_string(), None)]);
        manager.clear();
        let (ports, gateways) = manager.stats();
        assert_eq!(ports, 0);
        assert_eq!(gateways, 0);
    }

    #[test]
    fn test_no_conflict_wildcard_and_two_concretes() {
        let manager = ListenerPortManager::new();

        // Wildcard + two concretes — all different hostnames, no conflicts
        manager.register_gateway(
            "default/gw",
            &[
                ("catchall".to_string(), "443".to_string(), None),
                ("api".to_string(), "443".to_string(), Some("api.example.com".to_string())),
                ("web".to_string(), "443".to_string(), Some("web.example.com".to_string())),
            ],
        );

        assert!(manager.detect_conflicts("default/gw").is_empty());
    }

    #[test]
    fn test_conflict_two_wildcards_and_concrete() {
        let manager = ListenerPortManager::new();

        // Two wildcard (None) listeners → conflict with each other; concrete is fine
        manager.register_gateway(
            "default/gw",
            &[
                ("catchall-1".to_string(), "443".to_string(), None),
                ("catchall-2".to_string(), "443".to_string(), None),
                ("api".to_string(), "443".to_string(), Some("api.example.com".to_string())),
            ],
        );

        let conflicts = manager.detect_conflicts("default/gw");
        assert_eq!(conflicts.len(), 2);
        assert!(conflicts.contains_key("catchall-1"));
        assert!(conflicts.contains_key("catchall-2"));
        assert!(!conflicts.contains_key("api"));
    }
}
