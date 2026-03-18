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
//! Most users don't configure TLS in Gateway, so the inner `Option` provides a
//! fast path: when no Gateway TLS is configured, lookups return immediately
//! without touching any HashMap.

use crate::core::common::matcher::HashHost;
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

/// Combined matcher data atomically swapped via `ArcSwap`.
struct TlsMatcherData {
    /// Hostname-based matcher: port -> (hostname/SNI -> entries)
    port_map: HashMap<u16, HashHost<Vec<GatewayTlsEntry>>>,
}

/// Gateway TLS Matcher for port and hostname-based certificate lookup
///
/// Uses ArcSwap for lock-free reads during TLS handshake.
/// The inner Option provides fast-path for the common case where
/// no Gateway TLS is configured (avoids HashMap allocation and lookup).
///
/// ## Lookup priority (per port)
/// 1. Exact hostname match (via HashHost)
/// 2. Wildcard hostname match (via HashHost)
pub struct GatewayTlsMatcher {
    data: ArcSwap<Option<TlsMatcherData>>,
}

impl GatewayTlsMatcher {
    pub fn new() -> Self {
        Self {
            data: ArcSwap::from_pointee(None),
        }
    }

    /// Check if there are any Gateway TLS configurations
    ///
    /// This is a fast O(1) check.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.load().is_none()
    }

    /// Match SNI against Gateway Listener hostnames with port dimension
    ///
    /// Returns the first matching GatewayTlsEntry when found.
    ///
    /// ## Parameters
    /// - `port`: The listening port (from TCP connection)
    /// - `sni`: Server Name Indication from TLS Client Hello
    ///
    #[inline]
    pub fn match_sni_with_port(&self, port: u16, sni: &str) -> Option<GatewayTlsEntry> {
        let data_guard = self.data.load();
        let data = data_guard.as_ref().as_ref()?;

        // 1) Hostname-based lookup (exact > wildcard)
        if let Some(host_matcher) = data.port_map.get(&port) {
            if let Some(entries) = host_matcher.get(sni) {
                if let Some(entry) = entries.first() {
                    return Some(entry.clone());
                }
            }
        }

        None
    }

    /// Rebuild matcher from Gateway list with port dimension
    ///
    /// Extracts TLS configurations from all Gateway Listeners and builds
    /// a port -> hostname-based matcher for certificate lookup.
    pub fn rebuild_from_gateways(&self, gateways: &[Gateway]) {
        let mut port_map: HashMap<u16, HashHost<Vec<GatewayTlsEntry>>> = HashMap::new();
        let mut hostname_entry_count = 0usize;
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

                // For listeners using EdgionTls cert-provider, certificate_refs may be
                // absent because certs are loaded dynamically via the EdgionTls CRD.
                // We still need to register these listeners so that TLS termination
                // and route matching work on the correct port.
                let has_edgion_tls_provider = tls_config
                    .options
                    .as_ref()
                    .and_then(|opts| opts.get("edgion.io/cert-provider"))
                    .map_or(false, |v| v.as_str() == Some("EdgionTls"));

                let cert_refs = match &tls_config.certificate_refs {
                    Some(refs) if !refs.is_empty() => refs.clone(),
                    _ if has_edgion_tls_provider => Vec::new(),
                    _ => continue,
                };

                let port = listener.port as u16;
                match &listener.hostname {
                    Some(hostname) => {
                        let entry = GatewayTlsEntry {
                            gateway_namespace: gateway_namespace.clone(),
                            gateway_name: gateway_name.clone(),
                            listener_name: listener.name.clone(),
                            port,
                            hostname: hostname.clone(),
                            certificate_refs: cert_refs,
                            secrets: tls_config.secrets.clone(),
                        };

                        let host_matcher = port_map.entry(port).or_insert_with(|| {
                            port_count += 1;
                            HashHost::new()
                        });

                        if let Some(existing) = host_matcher.get_mut(hostname) {
                            existing.push(entry);
                        } else {
                            host_matcher.insert(hostname, vec![entry]);
                        }
                        hostname_entry_count += 1;
                    }
                    None => {}
                }
            }
        }

        let total_entries = hostname_entry_count;

        tracing::info!(
            component = "gateway_tls_matcher",
            gateways = gateways.len(),
            ports = port_count,
            hostname_entries = hostname_entry_count,
            "Rebuilt Gateway TLS matcher"
        );

        if total_entries > 0 {
            self.data.store(Arc::new(Some(TlsMatcherData { port_map })));
        } else {
            self.data.store(Arc::new(None));
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GatewayTlsMatcherStats {
    pub port_count: usize,
    pub is_empty: bool,
}

impl GatewayTlsMatcher {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> GatewayTlsMatcherStats {
        let data = self.data.load();
        match data.as_ref() {
            Some(d) => GatewayTlsMatcherStats {
                port_count: d.port_map.len(),
                is_empty: false,
            },
            None => GatewayTlsMatcherStats {
                port_count: 0,
                is_empty: true,
            },
        }
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
pub fn match_gateway_tls_with_port(port: u16, sni: &str) -> Option<GatewayTlsEntry> {
    get_gateway_tls_matcher().match_sni_with_port(port, sni)
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

    fn make_tls_config(cert_name: &str, cert_ns: Option<&str>) -> GatewayTLSConfig {
        GatewayTLSConfig {
            mode: Some("Terminate".to_string()),
            certificate_refs: Some(vec![SecretObjectReference {
                name: cert_name.to_string(),
                namespace: cert_ns.map(|s| s.to_string()),
                group: None,
                kind: None,
            }]),
            options: None,
            secrets: None,
        }
    }

    #[test]
    fn test_rebuild_from_gateways() {
        let matcher = GatewayTlsMatcher::new();

        let listener = create_test_listener(
            "https",
            Some("example.com"),
            Some(make_tls_config("test-cert", Some("default"))),
        );
        let gateway = create_test_gateway("test-gw", "default", vec![listener]);

        matcher.rebuild_from_gateways(&[gateway]);

        // Test exact match
        let result = matcher.match_sni_with_port(443, "example.com");
        assert!(result.is_some());
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
        let result = matcher.match_sni_with_port(443, "example.com");
        assert!(result.is_none());
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

        // Hostname-less listeners are ignored for TLS certificate matching.
        let result = matcher.match_sni_with_port(443, "example.com");
        assert!(result.is_none());
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
        let result = matcher.match_sni_with_port(443, "api.example.com");
        assert!(result.is_some(), "api.example.com should match *.example.com");
        let entry = result.unwrap();
        assert_eq!(entry.gateway_name, "wildcard-gw");
        assert_eq!(entry.certificate_refs[0].name, "wildcard-cert");

        // Another subdomain should also match
        let result = matcher.match_sni_with_port(443, "www.example.com");
        assert!(result.is_some(), "www.example.com should match *.example.com");

        // Root domain should NOT match wildcard
        let result = matcher.match_sni_with_port(443, "example.com");
        assert!(result.is_none(), "example.com should NOT match *.example.com");
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
        let result = matcher.match_sni_with_port(443, "api.example.com");
        assert!(result.is_some());
        let entry = result.unwrap();
        assert_eq!(entry.gateway_name, "api-gateway");
        assert_eq!(entry.certificate_refs[0].name, "api-cert");

        // Test www.example.com
        let result = matcher.match_sni_with_port(443, "www.example.com");
        assert!(result.is_some());
        let entry = result.unwrap();
        assert_eq!(entry.gateway_name, "web-gateway");
        assert_eq!(entry.listener_name, "www-https");
        assert_eq!(entry.certificate_refs[0].name, "www-cert");

        // Test admin.example.com
        let result = matcher.match_sni_with_port(443, "admin.example.com");
        assert!(result.is_some());
        let entry = result.unwrap();
        assert_eq!(entry.gateway_name, "web-gateway");
        assert_eq!(entry.listener_name, "admin-https");
        assert_eq!(entry.certificate_refs[0].name, "admin-cert");

        // Test unknown domain
        let result = matcher.match_sni_with_port(443, "unknown.example.com");
        assert!(result.is_none());
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

        let result = matcher.match_sni_with_port(443, "test.example.com");
        assert!(result.is_some());
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
        let result = matcher.match_sni_with_port(443, "example.com");
        assert!(result.is_none());

        // Rebuild with empty gateway list
        matcher.rebuild_from_gateways(&[]);
        assert!(matcher.is_empty(), "Matcher should still be empty after empty rebuild");

        // Rebuild with gateway that has no TLS
        let listener = create_test_listener("http", Some("example.com"), None);
        let gateway = create_test_gateway("no-tls-gw", "default", vec![listener]);
        matcher.rebuild_from_gateways(&[gateway]);
        assert!(matcher.is_empty(), "Matcher should be empty when gateway has no TLS");

        // Match should still fail fast
        let result = matcher.match_sni_with_port(443, "example.com");
        assert!(result.is_none());
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
        assert!(result.is_some(), "Should find cert for port 443");
        let entry = result.unwrap();
        assert_eq!(entry.port, 443);
        assert_eq!(entry.certificate_refs[0].name, "cert-443");

        // Test port 8443
        let result = matcher.match_sni_with_port(8443, "api.example.com");
        assert!(result.is_some(), "Should find cert for port 8443");
        let entry = result.unwrap();
        assert_eq!(entry.port, 8443);
        assert_eq!(entry.certificate_refs[0].name, "cert-8443");

        // Test non-existent port
        let result = matcher.match_sni_with_port(9443, "api.example.com");
        assert!(result.is_none(), "Should not find cert for port 9443");
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
        assert!(result.is_some());
        assert_eq!(result.unwrap().certificate_refs[0].name, "api-cert");

        // admin.example.com on port 8443
        let result = matcher.match_sni_with_port(8443, "admin.example.com");
        assert!(result.is_some());
        assert_eq!(result.unwrap().certificate_refs[0].name, "admin-cert");

        // api.example.com on port 8443 should fail (wrong port)
        let result = matcher.match_sni_with_port(8443, "api.example.com");
        assert!(result.is_none());

        // admin.example.com on port 443 should fail (wrong port)
        let result = matcher.match_sni_with_port(443, "admin.example.com");
        assert!(result.is_none());
    }
}
