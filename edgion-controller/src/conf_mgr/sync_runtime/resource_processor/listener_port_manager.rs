//! Listener Port Manager
//!
//! Tracks port usage within each Gateway for intra-gateway conflict detection.
//! Per Gateway API spec, Conflicted condition applies only within a single Gateway;
//! different Gateways may independently use the same port.
//!
//! ## Design
//!
//! - Forward index: conflict_key → Set of ListenerRefs (all listeners using this socket address)
//! - Reverse index: gateway_key → Set of conflict_keys (all socket addresses used by this gateway)
//! - Conflict detection considers hostname intersection (wildcard-aware)
//!
//! ## Conflict Key Format
//!
//! Conflict keys group listeners that bind the same OS socket (same IP:port):
//! - Format: "{ip}:{port}" (e.g. "0.0.0.0:443")
//! - Protocol is excluded because HTTP/HTTPS/TLS/TCP all bind the same OS socket on a given (ip, port)
//!
//! Within each conflict key group, hostname intersection determines actual conflicts.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

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
    pub fn new(gateway_key: impl Into<String>, listener_name: impl Into<String>, hostname: Option<String>) -> Self {
        Self {
            gateway_key: gateway_key.into(),
            listener_name: listener_name.into(),
            hostname,
        }
    }
}

/// Conflict group key: listeners on the same socket address can conflict.
/// Format: "{ip}:{port}" — protocol is excluded because HTTP/HTTPS/TLS/TCP
/// all bind the same OS socket on a given (ip, port).
pub type ConflictKey = String;

/// Build conflict key from a socket address.
///
/// Groups listeners by their OS socket address (ip:port) so that hostname
/// intersection can be checked within each group during conflict detection.
/// Protocol is excluded from the key because different protocols on the same
/// (ip, port) bind the same OS socket.
pub fn make_conflict_key(addr: std::net::SocketAddr) -> ConflictKey {
    addr.to_string() // e.g. "0.0.0.0:443"
}

pub type PortKey = ConflictKey;

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
/// Two listeners conflict only when their hostnames are identical
/// (both wildcard, or the same concrete hostname string).
pub struct ListenerPortManager {
    /// Forward index: conflict_key → listeners using this socket address
    port_to_listeners: RwLock<HashMap<ConflictKey, HashSet<ListenerRef>>>,

    /// Reverse index: gateway_key → conflict_keys it uses
    gateway_to_ports: RwLock<HashMap<String, HashSet<ConflictKey>>>,
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
    /// Each listener is a tuple of (listener_name, conflict_key, hostname).
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    pub fn register_gateway(&self, gateway_key: &str, listeners: &[(String, ConflictKey, Option<String>)]) {
        let mut gw_map = self.gateway_to_ports.write();
        let mut port_map = self.port_to_listeners.write();

        // Clear old registrations
        if let Some(old_conflict_keys) = gw_map.remove(gateway_key) {
            for conflict_key in &old_conflict_keys {
                if let Some(listeners_set) = port_map.get_mut(conflict_key) {
                    listeners_set.retain(|l| l.gateway_key != gateway_key);
                    if listeners_set.is_empty() {
                        port_map.remove(conflict_key);
                    }
                }
            }
        }

        // Add new registrations
        let mut conflict_keys = HashSet::new();

        for (listener_name, conflict_key, hostname) in listeners {
            // Normalize empty hostname to None (wildcard)
            let hostname = hostname.as_ref().filter(|h| !h.is_empty()).cloned();
            let listener_ref = ListenerRef::new(gateway_key, listener_name.clone(), hostname);
            port_map.entry(conflict_key.clone()).or_default().insert(listener_ref);
            conflict_keys.insert(conflict_key.clone());
        }

        if !conflict_keys.is_empty() {
            gw_map.insert(gateway_key.to_string(), conflict_keys);
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
        let mut gw_map = self.gateway_to_ports.write();
        let mut port_map = self.port_to_listeners.write();

        let conflict_keys = gw_map.remove(gateway_key).unwrap_or_default();

        for conflict_key in &conflict_keys {
            if let Some(listeners) = port_map.get_mut(conflict_key) {
                listeners.retain(|l| l.gateway_key != gateway_key);
                if listeners.is_empty() {
                    port_map.remove(conflict_key);
                }
            }
        }

        if !conflict_keys.is_empty() {
            tracing::debug!(
                gateway = %gateway_key,
                socket_count = conflict_keys.len(),
                "Unregistered gateway from port manager"
            );
        }
    }

    /// Detect intra-gateway port conflicts for a specific gateway.
    ///
    /// Two listeners sharing a socket address conflict only if their hostnames are
    /// identical: both wildcard (`None`), or the same concrete string. Overlapping
    /// but distinct hostnames (wildcard vs concrete, or two different concrete
    /// values) are not conflicts — routing priority resolves the overlap per
    /// GatewayHTTPListenerIsolation semantics.
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    pub fn detect_conflicts(&self, gateway_key: &str) -> HashMap<String, String> {
        let mut conflicts = HashMap::new();

        let gw_map = self.gateway_to_ports.read();
        let port_map = self.port_to_listeners.read();

        let Some(port_keys) = gw_map.get(gateway_key) else {
            return conflicts;
        };

        for conflict_key in port_keys {
            let Some(listeners) = port_map.get(conflict_key) else {
                continue;
            };

            // Collect same-gateway listeners on this socket address
            let same_gw: Vec<&ListenerRef> = listeners.iter().filter(|l| l.gateway_key == gateway_key).collect();

            if same_gw.len() < 2 {
                continue;
            }

            // Check hostname intersection pairwise: two listeners on the same socket address
            // conflict if they have the same hostname (including both being wildcard).
            for (i, listener) in same_gw.iter().enumerate() {
                let mut others: Vec<&str> = same_gw
                    .iter()
                    .enumerate()
                    .filter(|(j, other)| {
                        *j != i && hostnames_conflict(listener.hostname.as_deref(), other.hostname.as_deref())
                    })
                    .map(|(_, l)| l.listener_name.as_str())
                    .collect();
                // Condition messages are semantic status: hash iteration must
                // not turn a status-only WATCH into another status write.
                others.sort_unstable();

                if !others.is_empty() {
                    let reason = format!("Socket {}: conflicts with: {}", conflict_key, others.join(", "));
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
        let mut gw_map = self.gateway_to_ports.write();
        let mut port_map = self.port_to_listeners.write();
        gw_map.clear();
        port_map.clear();
        tracing::info!("ListenerPortManager cleared");
    }

    /// Get statistics for debugging/metrics
    ///
    /// # Lock Order
    /// Always acquires locks in order: gateway_to_ports -> port_to_listeners
    pub fn stats(&self) -> (usize, usize) {
        let gw_map = self.gateway_to_ports.read();
        let port_map = self.port_to_listeners.read();
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
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)
    }

    fn key(port: u16) -> ConflictKey {
        make_conflict_key(addr(port))
    }

    #[test]
    fn conflict_messages_are_stable_across_registration_order() {
        let manager = ListenerPortManager::new();
        let mut listeners = vec![
            ("z-http".to_string(), key(80), None),
            ("a-tcp".to_string(), key(80), None),
            ("m-tls".to_string(), key(80), None),
        ];
        let expected = HashMap::from([
            (
                "a-tcp".to_string(),
                "Socket 0.0.0.0:80: conflicts with: m-tls, z-http".to_string(),
            ),
            (
                "m-tls".to_string(),
                "Socket 0.0.0.0:80: conflicts with: a-tcp, z-http".to_string(),
            ),
            (
                "z-http".to_string(),
                "Socket 0.0.0.0:80: conflicts with: a-tcp, m-tls".to_string(),
            ),
        ]);
        for round in 0..12 {
            listeners.rotate_left(1);
            if round % 2 == 0 {
                listeners.reverse();
            }
            manager.register_gateway("default/gateway", &listeners);
            assert_eq!(manager.detect_conflicts("default/gateway"), expected);
        }
        manager.unregister_gateway("default/gateway");
        assert_eq!(manager.stats(), (0, 0));
    }

    #[test]
    fn test_make_conflict_key() {
        // All protocols on the same (ip, port) produce the same conflict key
        assert_eq!(make_conflict_key(addr(80)), "0.0.0.0:80");
        assert_eq!(make_conflict_key(addr(443)), "0.0.0.0:443");

        // Different ports produce different keys
        assert_ne!(make_conflict_key(addr(80)), make_conflict_key(addr(443)));

        // Custom IP address
        let custom = SocketAddr::new("192.168.1.1".parse::<IpAddr>().unwrap(), 8080);
        assert_eq!(make_conflict_key(custom), "192.168.1.1:8080");
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
                ("http".to_string(), key(80), None),
                ("https".to_string(), key(443), Some("example.com".to_string())),
            ],
        );

        manager.register_gateway("default/gateway-b", &[("http".to_string(), key(80), None)]);

        let (sockets, gateways) = manager.stats();
        assert_eq!(gateways, 2);
        assert_eq!(sockets, 2); // "0.0.0.0:80" and "0.0.0.0:443"

        manager.unregister_gateway("default/gateway-a");

        let (sockets, gateways) = manager.stats();
        assert_eq!(gateways, 1);
        assert_eq!(sockets, 1);
    }

    #[test]
    fn test_conflict_same_socket_same_hostname() {
        let manager = ListenerPortManager::new();

        manager.register_gateway(
            "default/gw",
            &[
                ("http-1".to_string(), key(80), None),
                ("http-2".to_string(), key(80), None),
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
                ("catchall".to_string(), key(443), None),
                ("specific".to_string(), key(443), Some("api.example.com".to_string())),
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
                ("wild".to_string(), key(443), Some("*.example.com".to_string())),
                ("concrete".to_string(), key(443), Some("foo.example.com".to_string())),
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
                ("wild-1".to_string(), key(443), Some("*.example.com".to_string())),
                ("wild-2".to_string(), key(443), Some("*.example.com".to_string())),
            ],
        );

        let conflicts = manager.detect_conflicts("default/gw");
        assert_eq!(conflicts.len(), 2);
    }

    #[test]
    fn test_no_conflict_different_hostnames() {
        let manager = ListenerPortManager::new();

        // Different concrete hostnames on same socket address — no conflict
        manager.register_gateway(
            "default/gw",
            &[
                ("api".to_string(), key(443), Some("api.example.com".to_string())),
                ("web".to_string(), key(443), Some("web.example.com".to_string())),
            ],
        );

        assert!(manager.detect_conflicts("default/gw").is_empty());
    }

    #[test]
    fn test_no_conflict_cross_gateway() {
        let manager = ListenerPortManager::new();

        manager.register_gateway("default/gw-a", &[("https".to_string(), key(443), None)]);
        manager.register_gateway("default/gw-b", &[("https".to_string(), key(443), None)]);

        assert!(manager.detect_conflicts("default/gw-a").is_empty());
        assert!(manager.detect_conflicts("default/gw-b").is_empty());
    }

    #[test]
    fn test_no_conflict_different_ports() {
        let manager = ListenerPortManager::new();

        manager.register_gateway(
            "default/gw",
            &[
                ("http".to_string(), key(80), None),
                ("https".to_string(), key(443), None),
            ],
        );

        assert!(manager.detect_conflicts("default/gw").is_empty());
    }

    #[test]
    fn test_conflict_tcp_and_http_same_socket() {
        let manager = ListenerPortManager::new();

        // TCP and HTTP on the same (ip, port) share the same OS socket → same conflict key → conflict
        manager.register_gateway(
            "default/gw",
            &[
                ("tcp-80".to_string(), key(80), None),
                ("http-80".to_string(), key(80), None),
            ],
        );

        let conflicts = manager.detect_conflicts("default/gw");
        assert_eq!(conflicts.len(), 2);
        assert!(conflicts.contains_key("tcp-80"));
        assert!(conflicts.contains_key("http-80"));
    }

    #[test]
    fn test_re_register_clears_old() {
        let manager = ListenerPortManager::new();

        manager.register_gateway("default/gw", &[("http".to_string(), key(80), None)]);
        manager.register_gateway("default/gw", &[]);

        let (sockets, gateways) = manager.stats();
        assert_eq!(gateways, 0);
        assert_eq!(sockets, 0);
    }

    #[test]
    fn test_clear() {
        let manager = ListenerPortManager::new();
        manager.register_gateway("default/gw", &[("http".to_string(), key(80), None)]);
        manager.clear();
        let (sockets, gateways) = manager.stats();
        assert_eq!(sockets, 0);
        assert_eq!(gateways, 0);
    }

    #[test]
    fn test_no_conflict_wildcard_and_two_concretes() {
        let manager = ListenerPortManager::new();

        // Wildcard + two concretes — all different hostnames, no conflicts
        manager.register_gateway(
            "default/gw",
            &[
                ("catchall".to_string(), key(443), None),
                ("api".to_string(), key(443), Some("api.example.com".to_string())),
                ("web".to_string(), key(443), Some("web.example.com".to_string())),
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
                ("catchall-1".to_string(), key(443), None),
                ("catchall-2".to_string(), key(443), None),
                ("api".to_string(), key(443), Some("api.example.com".to_string())),
            ],
        );

        let conflicts = manager.detect_conflicts("default/gw");
        assert_eq!(conflicts.len(), 2);
        assert!(conflicts.contains_key("catchall-1"));
        assert!(conflicts.contains_key("catchall-2"));
        assert!(!conflicts.contains_key("api"));
    }
}
