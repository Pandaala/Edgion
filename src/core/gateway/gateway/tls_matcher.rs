//! Gateway TLS Matcher
//!
//! Provides fallback TLS certificate lookup based on Gateway Listener configurations.
//! This matcher is used when EdgionTls lookup fails.
//!
//! ## Architecture
//!
//! The matcher uses a two-layer structure: Port -> (SNI -> TLSEntry)
//! This supports Gateway API semantics where the same hostname on different
//! ports can have different certificates.
//!
//! ## Performance Optimization
//!
//! Most users don't configure TLS in Gateway, so we use an `AtomicBool` flag to
//! skip the HashMap lookup when no Gateway TLS is configured. This provides a
//! fast path for the common case.

use crate::core::matcher::HashHost;
use crate::types::err::EdError;
use crate::types::prelude_resources::Gateway;
use crate::types::resources::gateway::SecretObjectReference;
use arc_swap::ArcSwap;
use k8s_openapi::api::core::v1::Secret;
use kube::ResourceExt;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

/// Gateway TLS entry containing certificate references and resolved secrets
#[derive(Debug, Clone)]
pub struct GatewayTlsEntry {
    /// Gateway namespace
    pub gateway_namespace: String,
    /// Gateway name
    pub gateway_name: String,
    /// Listener name
    pub listener_name: String,
    /// Listener port
    pub port: u16,
    /// Hostname pattern (e.g., "*.example.com")
    pub hostname: String,
    /// References to Secrets containing certificates
    pub certificate_refs: Vec<SecretObjectReference>,
    /// Resolved Secret data (filled by Controller)
    pub secrets: Option<Vec<Secret>>,
}

/// Port-based TLS matcher structure
/// Maps port -> (hostname -> TLS entries)
type PortTlsMap = HashMap<u16, HashHost<Vec<GatewayTlsEntry>>>;

/// Gateway TLS Matcher for port and hostname-based certificate lookup
///
/// Uses ArcSwap for lock-free reads during TLS handshake.
/// The inner Option provides fast-path for the common case where
/// no Gateway TLS is configured (avoids HashMap allocation and lookup).
///
/// ## Structure
/// Option<Port -> (Hostname/SNI -> Vec<GatewayTlsEntry>)>
pub struct GatewayTlsMatcher {
    /// Port-based TLS matcher: None if no TLS configured, Some(port -> hostname -> entries) otherwise
    /// Using Option avoids HashMap allocation when no Gateway TLS is configured
    port_matcher: ArcSwap<Option<PortTlsMap>>,
}

impl GatewayTlsMatcher {
    pub fn new() -> Self {
        Self {
            port_matcher: ArcSwap::from_pointee(None),
        }
    }

    /// Check if there are any Gateway TLS configurations
    ///
    /// This is a fast O(1) check.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.port_matcher.load().is_none()
    }

    /// Set the entire Gateway TLS matcher (port-based)
    ///
    /// # Warning
    /// Do not call this method frequently. Maintain at least 100ms interval between calls.
    pub fn set(&self, port_map: Option<PortTlsMap>) {
        self.port_matcher.store(Arc::new(port_map));
    }

    /// Match SNI against Gateway Listener hostnames with port dimension
    ///
    /// Returns the first matching GatewayTlsEntry or an error if not found.
    ///
    /// ## Parameters
    /// - `port`: The listening port (from TCP connection)
    /// - `sni`: Server Name Indication from TLS Client Hello
    ///
    /// ## Performance
    /// - Fast path: If no Gateway TLS configured, returns immediately (Option::None check)
    /// - Slow path: Port lookup O(1) + Hostname lookup O(1)
    #[inline]
    pub fn match_sni_with_port(&self, port: u16, sni: &str) -> Result<GatewayTlsEntry, EdError> {
        // Load port matcher - fast path if None
        let port_map_opt = self.port_matcher.load();
        let port_map = match port_map_opt.as_ref() {
            Some(map) => map,
            None => return Err(EdError::SniNotMatch("Gateway TLS not configured".to_string())),
        };

        // First layer: lookup by port
        let host_matcher = match port_map.get(&port) {
            Some(m) => m,
            None => return Err(EdError::SniNotMatch(format!("No TLS config for port {}", port))),
        };

        // Second layer: lookup by hostname/SNI
        match host_matcher.get(sni) {
            Some(entries) if !entries.is_empty() => Ok(entries[0].clone()),
            _ => Err(EdError::SniNotMatch(format!("Port {}: SNI {}", port, sni))),
        }
    }

    /// Match SNI against Gateway Listener hostnames (without port, searches all ports)
    ///
    /// This is a fallback method that searches across all ports.
    /// Prefer `match_sni_with_port` when port is known.
    #[inline]
    pub fn match_sni(&self, sni: &str) -> Result<GatewayTlsEntry, EdError> {
        // Load port matcher - fast path if None
        let port_map_opt = self.port_matcher.load();
        let port_map = match port_map_opt.as_ref() {
            Some(map) => map,
            None => return Err(EdError::SniNotMatch("Gateway TLS not configured".to_string())),
        };

        // Search across all ports
        for (_port, host_matcher) in port_map.iter() {
            if let Some(entries) = host_matcher.get(sni) {
                if let Some(entry) = entries.first() {
                    return Ok(entry.clone());
                }
            }
        }

        Err(EdError::SniNotMatch(format!("Gateway TLS: {}", sni)))
    }

    /// Rebuild matcher from Gateway list with port dimension
    ///
    /// Extracts TLS configurations from all Gateway Listeners and builds
    /// a port -> hostname-based matcher for certificate lookup.
    pub fn rebuild_from_gateways(&self, gateways: &[Gateway]) {
        let mut port_map: PortTlsMap = HashMap::new();
        let mut entry_count = 0usize;
        let mut port_count = 0usize;

        for gateway in gateways {
            let gateway_namespace = gateway.namespace().unwrap_or_default();
            let gateway_name = gateway.name_any();

            // Get listeners from Gateway spec
            let listeners = match &gateway.spec.listeners {
                Some(listeners) => listeners,
                None => continue,
            };

            for listener in listeners {
                // Skip listeners without TLS config
                let tls_config = match &listener.tls {
                    Some(tls) => tls,
                    None => continue,
                };

                // Skip listeners without certificate refs
                let cert_refs = match &tls_config.certificate_refs {
                    Some(refs) if !refs.is_empty() => refs.clone(),
                    _ => continue,
                };

                // Get hostname, skip if not specified
                let hostname = match &listener.hostname {
                    Some(h) => h.clone(),
                    None => continue,
                };

                let port = listener.port as u16;

                let entry = GatewayTlsEntry {
                    gateway_namespace: gateway_namespace.clone(),
                    gateway_name: gateway_name.clone(),
                    listener_name: listener.name.clone(),
                    port,
                    hostname: hostname.clone(),
                    certificate_refs: cert_refs,
                    secrets: tls_config.secrets.clone(),
                };

                // Get or create host matcher for this port
                let host_matcher = port_map.entry(port).or_insert_with(|| {
                    port_count += 1;
                    HashHost::new()
                });

                // Add entry to host matcher
                if let Some(existing) = host_matcher.get_mut(&hostname) {
                    existing.push(entry);
                } else {
                    host_matcher.insert(&hostname, vec![entry]);
                }
                entry_count += 1;
            }
        }

        let has_entries = entry_count > 0;

        tracing::info!(
            component = "gateway_tls_matcher",
            gateways = gateways.len(),
            ports = port_count,
            entries = entry_count,
            has_entries = has_entries,
            "Rebuilt Gateway TLS matcher with port dimension"
        );

        // Set to None if no entries, avoiding unnecessary HashMap allocation and lookup
        self.set(if has_entries { Some(port_map) } else { None });
    }
}

impl Default for GatewayTlsMatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Global Gateway TLS Matcher instance
pub static GATEWAY_TLS_MATCHER: LazyLock<GatewayTlsMatcher> = LazyLock::new(GatewayTlsMatcher::new);

/// Get a reference to the global Gateway TLS Matcher
pub fn get_gateway_tls_matcher() -> &'static GatewayTlsMatcher {
    &GATEWAY_TLS_MATCHER
}

/// Match SNI against Gateway TLS configurations with port
///
/// This is the preferred method when port is known (during TLS handshake).
pub fn match_gateway_tls_with_port(port: u16, sni: &str) -> Result<GatewayTlsEntry, EdError> {
    get_gateway_tls_matcher().match_sni_with_port(port, sni)
}

/// Match SNI against Gateway TLS configurations (without port, searches all ports)
///
/// Fallback method when port is not available.
pub fn match_gateway_tls(sni: &str) -> Result<GatewayTlsEntry, EdError> {
    get_gateway_tls_matcher().match_sni(sni)
}

/// Rebuild Gateway TLS matcher from Gateway list
pub fn rebuild_gateway_tls_matcher(gateways: &[Gateway]) {
    get_gateway_tls_matcher().rebuild_from_gateways(gateways);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::gateway::{GatewaySpec, GatewayTLSConfig, Listener};
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

    fn create_test_listener(name: &str, hostname: Option<&str>, tls: Option<GatewayTLSConfig>) -> Listener {
        create_test_listener_with_port(name, 443, hostname, tls)
    }

    fn create_test_listener_with_port(
        name: &str,
        port: i32,
        hostname: Option<&str>,
        tls: Option<GatewayTLSConfig>,
    ) -> Listener {
        Listener {
            name: name.to_string(),
            hostname: hostname.map(|s| s.to_string()),
            port,
            protocol: "HTTPS".to_string(),
            tls,
            allowed_routes: None,
        }
    }

    #[test]
    fn test_rebuild_from_gateways() {
        let matcher = GatewayTlsMatcher::new();

        let tls_config = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "test-cert".to_string(),
                namespace: Some("default".to_string()),
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };

        let listener = create_test_listener("https", Some("example.com"), Some(tls_config));
        let gateway = create_test_gateway("test-gw", "default", vec![listener]);

        matcher.rebuild_from_gateways(&[gateway]);

        // Test exact match
        let result = matcher.match_sni("example.com");
        assert!(result.is_ok());
        let entry = result.unwrap();
        assert_eq!(entry.gateway_name, "test-gw");
        assert_eq!(entry.gateway_namespace, "default");
        assert_eq!(entry.listener_name, "https");
        assert_eq!(entry.certificate_refs.len(), 1);
    }

    #[test]
    fn test_no_tls_config() {
        let matcher = GatewayTlsMatcher::new();

        let listener = create_test_listener("http", Some("example.com"), None);
        let gateway = create_test_gateway("test-gw", "default", vec![listener]);

        matcher.rebuild_from_gateways(&[gateway]);

        // Should not find any entry
        let result = matcher.match_sni("example.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_no_hostname() {
        let matcher = GatewayTlsMatcher::new();

        let tls_config = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "test-cert".to_string(),
                namespace: None,
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };

        let listener = create_test_listener("https", None, Some(tls_config));
        let gateway = create_test_gateway("test-gw", "default", vec![listener]);

        matcher.rebuild_from_gateways(&[gateway]);

        // Should not find any entry (no hostname to match)
        let result = matcher.match_sni("example.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_wildcard_match() {
        let matcher = GatewayTlsMatcher::new();

        let tls_config = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "wildcard-cert".to_string(),
                namespace: Some("default".to_string()),
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };

        // Gateway with wildcard hostname
        let listener = create_test_listener("https", Some("*.example.com"), Some(tls_config));
        let gateway = create_test_gateway("wildcard-gw", "default", vec![listener]);

        matcher.rebuild_from_gateways(&[gateway]);

        // Test wildcard match - subdomain should match
        let result = matcher.match_sni("api.example.com");
        assert!(result.is_ok(), "api.example.com should match *.example.com");
        let entry = result.unwrap();
        assert_eq!(entry.gateway_name, "wildcard-gw");
        assert_eq!(entry.certificate_refs[0].name, "wildcard-cert");

        // Another subdomain should also match
        let result = matcher.match_sni("www.example.com");
        assert!(result.is_ok(), "www.example.com should match *.example.com");

        // Root domain should NOT match wildcard
        let result = matcher.match_sni("example.com");
        assert!(result.is_err(), "example.com should NOT match *.example.com");
    }

    #[test]
    fn test_multiple_gateways_multiple_listeners() {
        let matcher = GatewayTlsMatcher::new();

        // Gateway 1: api.example.com
        let tls_config1 = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "api-cert".to_string(),
                namespace: Some("prod".to_string()),
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };
        let listener1 = create_test_listener("api-https", Some("api.example.com"), Some(tls_config1));
        let gateway1 = create_test_gateway("api-gateway", "prod", vec![listener1]);

        // Gateway 2: Multiple listeners (www and admin)
        let tls_config2 = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "www-cert".to_string(),
                namespace: Some("prod".to_string()),
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };
        let tls_config3 = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "admin-cert".to_string(),
                namespace: Some("prod".to_string()),
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };
        let listener2 = create_test_listener("www-https", Some("www.example.com"), Some(tls_config2));
        let listener3 = create_test_listener("admin-https", Some("admin.example.com"), Some(tls_config3));
        let gateway2 = create_test_gateway("web-gateway", "prod", vec![listener2, listener3]);

        matcher.rebuild_from_gateways(&[gateway1, gateway2]);

        // Test api.example.com
        let result = matcher.match_sni("api.example.com");
        assert!(result.is_ok());
        let entry = result.unwrap();
        assert_eq!(entry.gateway_name, "api-gateway");
        assert_eq!(entry.certificate_refs[0].name, "api-cert");

        // Test www.example.com
        let result = matcher.match_sni("www.example.com");
        assert!(result.is_ok());
        let entry = result.unwrap();
        assert_eq!(entry.gateway_name, "web-gateway");
        assert_eq!(entry.listener_name, "www-https");
        assert_eq!(entry.certificate_refs[0].name, "www-cert");

        // Test admin.example.com
        let result = matcher.match_sni("admin.example.com");
        assert!(result.is_ok());
        let entry = result.unwrap();
        assert_eq!(entry.gateway_name, "web-gateway");
        assert_eq!(entry.listener_name, "admin-https");
        assert_eq!(entry.certificate_refs[0].name, "admin-cert");

        // Test unknown domain
        let result = matcher.match_sni("unknown.example.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_global_matcher_functions() {
        // Test global matcher functions
        let tls_config = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "global-cert".to_string(),
                namespace: Some("default".to_string()),
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };

        let listener = create_test_listener("https", Some("global.example.com"), Some(tls_config));
        let gateway = create_test_gateway("global-gw", "default", vec![listener]);

        // Use global function to rebuild
        rebuild_gateway_tls_matcher(&[gateway]);

        // Use global function to match
        let result = match_gateway_tls("global.example.com");
        assert!(result.is_ok());
        let entry = result.unwrap();
        assert_eq!(entry.gateway_name, "global-gw");
        assert_eq!(entry.certificate_refs[0].name, "global-cert");
    }

    #[test]
    fn test_certificate_ref_namespace_fallback() {
        let matcher = GatewayTlsMatcher::new();

        // Certificate ref without explicit namespace should use Gateway's namespace
        let tls_config = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "cert-without-ns".to_string(),
                namespace: None, // No namespace specified
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };

        let listener = create_test_listener("https", Some("test.example.com"), Some(tls_config));
        let gateway = create_test_gateway("test-gw", "my-namespace", vec![listener]);

        matcher.rebuild_from_gateways(&[gateway]);

        let result = matcher.match_sni("test.example.com");
        assert!(result.is_ok());
        let entry = result.unwrap();

        // Verify Gateway namespace is captured for later Secret lookup
        assert_eq!(entry.gateway_namespace, "my-namespace");
        // Certificate ref has no explicit namespace
        assert!(entry.certificate_refs[0].namespace.is_none());
    }

    #[test]
    fn test_empty_matcher_fast_path() {
        let matcher = GatewayTlsMatcher::new();

        // New matcher should be empty
        assert!(matcher.is_empty(), "New matcher should be empty");

        // Match should fail immediately without HashMap lookup
        let result = matcher.match_sni("example.com");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not configured"));

        // Rebuild with empty gateway list
        matcher.rebuild_from_gateways(&[]);
        assert!(matcher.is_empty(), "Matcher should still be empty after empty rebuild");

        // Rebuild with gateway that has no TLS
        let listener = create_test_listener("http", Some("example.com"), None);
        let gateway = create_test_gateway("no-tls-gw", "default", vec![listener]);
        matcher.rebuild_from_gateways(&[gateway]);
        assert!(matcher.is_empty(), "Matcher should be empty when gateway has no TLS");

        // Match should still fail fast
        let result = matcher.match_sni("example.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_has_entries_flag_transitions() {
        let matcher = GatewayTlsMatcher::new();

        // Initially empty
        assert!(matcher.is_empty());

        // Add TLS config
        let tls_config = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "test-cert".to_string(),
                namespace: Some("default".to_string()),
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };
        let listener = create_test_listener("https", Some("example.com"), Some(tls_config));
        let gateway = create_test_gateway("test-gw", "default", vec![listener]);

        matcher.rebuild_from_gateways(&[gateway]);

        // Now has entries
        assert!(
            !matcher.is_empty(),
            "Matcher should have entries after adding TLS config"
        );

        // Remove all gateways
        matcher.rebuild_from_gateways(&[]);

        // Back to empty
        assert!(
            matcher.is_empty(),
            "Matcher should be empty after removing all gateways"
        );
    }

    #[test]
    fn test_port_dimension_matching() {
        let matcher = GatewayTlsMatcher::new();

        // Same hostname on different ports with different certs
        let tls_config_443 = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "cert-443".to_string(),
                namespace: Some("default".to_string()),
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };

        let tls_config_8443 = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "cert-8443".to_string(),
                namespace: Some("default".to_string()),
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };

        let listener_443 =
            create_test_listener_with_port("https-443", 443, Some("api.example.com"), Some(tls_config_443));
        let listener_8443 =
            create_test_listener_with_port("https-8443", 8443, Some("api.example.com"), Some(tls_config_8443));

        let gateway = create_test_gateway("multi-port-gw", "default", vec![listener_443, listener_8443]);

        matcher.rebuild_from_gateways(&[gateway]);

        // Test port 443
        let result = matcher.match_sni_with_port(443, "api.example.com");
        assert!(result.is_ok(), "Should find cert for port 443");
        let entry = result.unwrap();
        assert_eq!(entry.port, 443);
        assert_eq!(entry.certificate_refs[0].name, "cert-443");

        // Test port 8443
        let result = matcher.match_sni_with_port(8443, "api.example.com");
        assert!(result.is_ok(), "Should find cert for port 8443");
        let entry = result.unwrap();
        assert_eq!(entry.port, 8443);
        assert_eq!(entry.certificate_refs[0].name, "cert-8443");

        // Test non-existent port
        let result = matcher.match_sni_with_port(9443, "api.example.com");
        assert!(result.is_err(), "Should not find cert for port 9443");

        // Test fallback match_sni (without port) - should find one of them
        let result = matcher.match_sni("api.example.com");
        assert!(result.is_ok(), "Fallback should find at least one cert");
    }

    #[test]
    fn test_multiple_ports_different_hostnames() {
        let matcher = GatewayTlsMatcher::new();

        let tls_config_api = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "api-cert".to_string(),
                namespace: Some("default".to_string()),
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };

        let tls_config_admin = GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: "admin-cert".to_string(),
                namespace: Some("default".to_string()),
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        };

        // Port 443: api.example.com
        let listener_api =
            create_test_listener_with_port("api-https", 443, Some("api.example.com"), Some(tls_config_api));
        // Port 8443: admin.example.com
        let listener_admin =
            create_test_listener_with_port("admin-https", 8443, Some("admin.example.com"), Some(tls_config_admin));

        let gateway = create_test_gateway("mixed-gw", "default", vec![listener_api, listener_admin]);

        matcher.rebuild_from_gateways(&[gateway]);

        // api.example.com on port 443
        let result = matcher.match_sni_with_port(443, "api.example.com");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().certificate_refs[0].name, "api-cert");

        // admin.example.com on port 8443
        let result = matcher.match_sni_with_port(8443, "admin.example.com");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().certificate_refs[0].name, "admin-cert");

        // api.example.com on port 8443 should fail (wrong port)
        let result = matcher.match_sni_with_port(8443, "api.example.com");
        assert!(result.is_err());

        // admin.example.com on port 443 should fail (wrong port)
        let result = matcher.match_sni_with_port(443, "admin.example.com");
        assert!(result.is_err());
    }
}
