//! Gateway information for route matching context
//!
//! `GatewayInfo` is a lightweight struct identifying a specific Gateway+Listener
//! combination. It is used throughout route matching, request context, and the
//! port-based GatewayInfo store.

use crate::observe::metrics::TestType;
use crate::runtime::store::listener_gateway_info::format_gateway_key;
use edgion_common::dns::resolver_select::ResolverCandidate;
use edgion_common::dns::DnsLookupFamily;
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
    /// after GatewayClass-wide global plugins and before route-level plugins. An invalid
    /// slot is kept as an empty fail-closed sentinel, matching the publication keys.
    pub plugin_store_keys: Vec<String>,

    /// Gateway-layer DNS resolver candidate, precomputed from the Gateway's
    /// `edgion.io/dns-resolver` / `edgion.io/dns-resolver-ref` annotations. Used
    /// as the first layer (Gateway → EGC → system) for AI external hostnames and
    /// external jumps; `ExternalName` Services do not use it. `None` when the
    /// Gateway sets neither annotation.
    pub dns_resolver_candidate: Option<ResolverCandidate>,
    /// Cold-parsed override: absence inherits; invalid values fail DNS resolution.
    pub dns_lookup_family: Option<Result<DnsLookupFamily, &'static str>>,
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
        dns_lookup_family: Option<Result<DnsLookupFamily, &'static str>>,
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
            dns_lookup_family,
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
#[path = "../../tests/unit/runtime/gateway_info_test.rs"]
mod tests;
