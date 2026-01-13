//! Gateway TLS Matcher
//!
//! Provides fallback TLS certificate lookup based on Gateway Listener configurations.
//! This matcher is used when EdgionTls lookup fails.
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
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Hostname pattern (e.g., "*.example.com")
    pub hostname: String,
    /// References to Secrets containing certificates
    pub certificate_refs: Vec<SecretObjectReference>,
    /// Resolved Secret data (filled by Controller)
    pub secrets: Option<Vec<Secret>>,
}

/// Gateway TLS Matcher for hostname-based certificate lookup
///
/// Uses ArcSwap for lock-free reads during TLS handshake.
/// An AtomicBool flag provides fast-path for the common case where
/// no Gateway TLS is configured.
pub struct GatewayTlsMatcher {
    /// Fast-path flag: true if there are any TLS configurations
    /// Relaxed ordering is sufficient since we only use this for optimization
    has_entries: AtomicBool,
    /// The actual hostname -> TLS entry matcher
    matcher: ArcSwap<HashHost<Vec<GatewayTlsEntry>>>,
}

impl GatewayTlsMatcher {
    pub fn new() -> Self {
        Self {
            has_entries: AtomicBool::new(false),
            matcher: ArcSwap::from_pointee(HashHost::new()),
        }
    }

    /// Check if there are any Gateway TLS configurations
    ///
    /// This is a fast O(1) check using atomic load.
    #[inline]
    pub fn is_empty(&self) -> bool {
        !self.has_entries.load(Ordering::Relaxed)
    }

    /// Set the entire Gateway TLS matcher
    ///
    /// # Warning
    /// Do not call this method frequently. Maintain at least 100ms interval between calls.
    pub fn set(&self, matcher: HashHost<Vec<GatewayTlsEntry>>, has_entries: bool) {
        self.matcher.store(Arc::new(matcher));
        self.has_entries.store(has_entries, Ordering::Relaxed);
    }

    /// Match SNI against Gateway Listener hostnames
    ///
    /// Returns the first matching GatewayTlsEntry or an error if not found.
    ///
    /// ## Performance
    /// - Fast path: If no Gateway TLS configured, returns immediately (atomic bool check)
    /// - Slow path: HashMap lookup with O(1) complexity for both exact and wildcard matches
    #[inline]
    pub fn match_sni(&self, sni: &str) -> Result<GatewayTlsEntry, EdError> {
        // Fast path: skip if no Gateway TLS configured (most common case)
        if self.is_empty() {
            return Err(EdError::SniNotMatch("Gateway TLS not configured".to_string()));
        }

        // Slow path: actual HashMap lookup
        let snapshot = self.matcher.load();
        let entries = snapshot.get(sni).cloned().unwrap_or_default();

        entries
            .first()
            .cloned()
            .ok_or_else(|| EdError::SniNotMatch(format!("Gateway TLS: {}", sni)))
    }

    /// Rebuild matcher from Gateway list
    ///
    /// Extracts TLS configurations from all Gateway Listeners and builds
    /// a hostname-based matcher for certificate lookup.
    pub fn rebuild_from_gateways(&self, gateways: &[Gateway]) {
        let mut hash_host: HashHost<Vec<GatewayTlsEntry>> = HashHost::new();
        let mut entry_count = 0usize;

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

                let entry = GatewayTlsEntry {
                    gateway_namespace: gateway_namespace.clone(),
                    gateway_name: gateway_name.clone(),
                    listener_name: listener.name.clone(),
                    hostname: hostname.clone(),
                    certificate_refs: cert_refs,
                    secrets: tls_config.secrets.clone(),
                };

                // Add entry to hash_host
                // Use get_mut to check if entry exists, otherwise insert new
                if let Some(existing) = hash_host.get_mut(&hostname) {
                    existing.push(entry);
                } else {
                    hash_host.insert(&hostname, vec![entry]);
                }
                entry_count += 1;
            }
        }

        let has_entries = entry_count > 0;

        tracing::info!(
            component = "gateway_tls_matcher",
            gateways = gateways.len(),
            entries = entry_count,
            has_entries = has_entries,
            "Rebuilt Gateway TLS matcher"
        );

        self.set(hash_host, has_entries);
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

/// Match SNI against Gateway TLS configurations
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
        Listener {
            name: name.to_string(),
            hostname: hostname.map(|s| s.to_string()),
            port: 443,
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
}
