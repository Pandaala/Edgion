//! Gateway information for route matching context
//!
//! `GatewayInfo` is a lightweight struct identifying a specific Gateway+Listener
//! combination. It is used throughout route matching, request context, and the
//! port-based GatewayInfo store.

use crate::observe::metrics::TestType;

/// Gateway information for route matching context
///
/// Used to pass gateway context during route matching to support
/// both sectionName-based and hostname-based lookup strategies.
///
/// This struct should be created once when the HTTP proxy listener is constructed,
/// not per-request, to avoid allocation overhead.
///
/// Note: Listener configuration (hostname, allowedRoutes) is resolved through
/// `ListenerGatewayInfoStore::GatewayInfoIndex`, which is rebuilt on every
/// Gateway change to support hot-reload.
#[derive(Clone, Debug, Default)]
pub struct GatewayInfo {
    /// Gateway namespace (None for cluster-scoped or default namespace)
    pub namespace: Option<String>,
    /// Gateway name
    pub name: String,
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
}

impl GatewayInfo {
    /// Create a new GatewayInfo
    pub fn new(
        namespace: Option<String>,
        name: String,
        listener_name: Option<String>,
        metrics_test_key: Option<String>,
        metrics_test_type: Option<TestType>,
        require_sni_host_match: Option<bool>,
    ) -> Self {
        Self {
            namespace,
            name,
            listener_name,
            metrics_test_key,
            metrics_test_type,
            require_sni_host_match,
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
