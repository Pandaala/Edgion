//! Gateway information for route matching context
//!
//! `GatewayInfo` is a lightweight struct identifying a specific Gateway+Listener
//! combination. It is used throughout route matching, request context, and the
//! port-based GatewayInfo store.

use crate::observe::metrics::TestType;
use crate::runtime::store::listener_gateway_info::format_gateway_key;
use edgion_common::dns::resolver_select::ResolverCandidate;
use std::fmt;

/// Gateway information for route matching context
///
/// Used to pass gateway context during route matching to support
/// both sectionName-based and hostname-based lookup strategies.
///
/// Each Gateway/listener generation creates this struct once and shares it with
/// matched requests through `Arc`; request paths must not deep-clone it.
///
/// Note: Listener configuration (hostname, allowedRoutes) is resolved through
/// `ListenerGatewayInfoStore::GatewayInfoIndex`, which is rebuilt on every
/// Gateway change to support hot-reload.
#[derive(Default)]
pub struct GatewayInfo {
    /// Gateway namespace (None for cluster-scoped or default namespace)
    namespace: Option<String>,
    /// Gateway name
    name: String,
    /// Canonical identity key, resolved once for this Gateway/listener generation.
    gateway_key: String,
    /// Current listener name (required for listener-specific config lookup)
    pub listener_name: Option<String>,

    // ========== Test metrics fields (from Gateway annotations) ==========
    /// Test key for metrics filtering (from edgion.io/metrics-test-key annotation)
    pub metrics_test_key: Option<String>,
    /// Test type for metrics collection (from edgion.io/metrics-test-type annotation)
    pub metrics_test_type: Option<TestType>,

    /// Per-Gateway opt-in for 421 Misdirected Request detection
    /// (from edgion.io/require-sni-host-match annotation).
    /// Only used to enable when global config is off; cannot disable when global is on.
    pub require_sni_host_match: Option<bool>,

    /// Resolved "namespace/name" store keys of EdgionPlugins attached to this Gateway via
    /// the `edgion.io/edgion-plugins` annotation. Run for every request on this Gateway,
    /// after GatewayClass-wide global plugins and before route-level plugins.
    pub plugin_store_keys: Vec<String>,

    /// Gateway-layer DNS resolver candidate, precomputed from the Gateway's
    /// `edgion.io/dns-resolver` / `edgion.io/dns-resolver-ref` annotations. Used
    /// as the middle layer of the data-plane resolver precedence
    /// (Service → Gateway → EGC → system) when self-resolving an `ExternalName`
    /// backend. `None` when the Gateway sets neither annotation.
    pub dns_resolver_candidate: Option<ResolverCandidate>,
}

impl GatewayInfo {
    /// Create a new GatewayInfo
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        namespace: Option<String>,
        name: String,
        listener_name: Option<String>,
        metrics_test_key: Option<String>,
        metrics_test_type: Option<TestType>,
        require_sni_host_match: Option<bool>,
        plugin_store_keys: Vec<String>,
        dns_resolver_candidate: Option<ResolverCandidate>,
    ) -> Self {
        let gateway_key = format_gateway_key(namespace.as_deref().unwrap_or(""), &name);
        Self {
            namespace,
            name,
            gateway_key,
            listener_name,
            metrics_test_key,
            metrics_test_type,
            require_sni_host_match,
            plugin_store_keys,
            dns_resolver_candidate,
        }
    }

    /// Borrow the precomputed Gateway key without allocating on the request path.
    pub fn gateway_key(&self) -> &str {
        &self.gateway_key
    }

    /// Borrow the raw namespace, preserving an absent versus explicitly empty value.
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// Get gateway namespace for metrics (returns empty string if None)
    #[inline]
    pub fn gateway_namespace(&self) -> &str {
        self.namespace.as_deref().unwrap_or("")
    }

    /// Get gateway name for metrics
    #[inline]
    pub fn gateway_name(&self) -> &str {
        &self.name
    }
}

impl fmt::Debug for GatewayInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayInfo")
            .field("namespace", &self.namespace)
            .field("name", &self.name)
            .field("listener_name", &self.listener_name)
            .field("metrics_test_key", &self.metrics_test_key)
            .field("metrics_test_type", &self.metrics_test_type)
            .field("require_sni_host_match", &self.require_sni_host_match)
            .field("plugin_store_keys", &self.plugin_store_keys)
            .field("dns_resolver_candidate", &self.dns_resolver_candidate)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn identity_keys_preserve_raw_namespace_and_canonical_format() {
        for (namespace, name, expected) in [
            (None, "gateway", "gateway"),
            (Some(""), "gateway", "gateway"),
            (Some("first"), "gateway", "first/gateway"),
            (Some("second"), "gateway", "second/gateway"),
            (None, "", ""),
            (Some(""), "", ""),
            (Some("first"), "", "first/"),
        ] {
            let info = GatewayInfo::new(
                namespace.map(str::to_owned),
                name.to_owned(),
                None,
                None,
                None,
                None,
                Vec::new(),
                None,
            );
            assert_eq!(info.namespace(), namespace);
            assert_eq!(info.gateway_namespace(), namespace.unwrap_or(""));
            assert_eq!(info.gateway_name(), name);
            assert_eq!(info.gateway_key(), expected);
            assert_eq!(info.gateway_key(), format_gateway_key(namespace.unwrap_or(""), name));
        }
    }

    #[test]
    fn default_identity_and_debug_remain_unchanged() {
        let info = GatewayInfo::default();
        assert_eq!(info.namespace(), None);
        assert_eq!(info.gateway_namespace(), "");
        assert_eq!(info.gateway_name(), "");
        assert_eq!(info.gateway_key(), "");
        assert_eq!(format!("{info:?}"), "GatewayInfo { namespace: None, name: \"\", listener_name: None, metrics_test_key: None, metrics_test_type: None, require_sni_host_match: None, plugin_store_keys: [], dns_resolver_candidate: None }");
    }

    #[test]
    fn shared_generation_borrows_the_same_key_storage() {
        let info = Arc::new(GatewayInfo::new(
            Some("team".to_owned()),
            "gateway".to_owned(),
            Some("http".to_owned()),
            Some("test-key".to_owned()),
            None,
            Some(true),
            vec!["team/plugin".to_owned()],
            None,
        ));
        let shared = Arc::clone(&info);
        assert_eq!(info.gateway_key().as_ptr(), info.gateway_key().as_ptr());
        assert_eq!(info.gateway_key().as_ptr(), shared.gateway_key().as_ptr());
        assert_eq!(format!("{info:?}"), "GatewayInfo { namespace: Some(\"team\"), name: \"gateway\", listener_name: Some(\"http\"), metrics_test_key: Some(\"test-key\"), metrics_test_type: None, require_sni_host_match: Some(true), plugin_store_keys: [\"team/plugin\"], dns_resolver_candidate: None }");
    }
}
