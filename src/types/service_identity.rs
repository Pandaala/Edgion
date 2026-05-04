//! Canonical service identity (namespace + name + cached `service_key`).
//!
//! Encapsulates two rules currently duplicated across the codebase:
//! 1. `service_key` formatting: `"{namespace}/{name}"`
//! 2. Gateway API namespace inheritance: `backend_ref.namespace` if present,
//!    otherwise the route's namespace.

use crate::types::ctx::Rsc;
use crate::types::resources::grpc_route::GRPCBackendRef;
use crate::types::resources::http_route::HTTPBackendRef;
use crate::types::resources::tcp_route::TCPBackendRef;
use crate::types::resources::tls_route::TLSBackendRef;
use crate::types::resources::udp_route::UDPBackendRef;

/// Identifies a backing Service for a request: `(namespace, name)` plus the
/// canonical `"{namespace}/{name}"` key used by all service-scoped stores.
///
/// Construct via the `from_*_backend_ref` methods to apply Gateway API namespace
/// inheritance, or [`Self::new`] when both fields are already known.
///
/// Fields are private to protect the invariant that `key` always matches
/// `format!("{}/{}", namespace, name)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceIdentity {
    name: String,
    namespace: String,
    key: String,
}

impl ServiceIdentity {
    /// Build from explicit `(name, namespace)` strings.
    pub fn new(name: impl Into<String>, namespace: impl Into<String>) -> Self {
        let name = name.into();
        let namespace = namespace.into();
        let key = format!("{}/{}", namespace, name);
        Self { name, namespace, key }
    }

    /// Build from an `HTTPBackendRef` with namespace inheritance from the route.
    ///
    /// `route_ns` must be non-empty (caller's contract — K8s guarantees this for
    /// any matched route).
    pub fn from_http_backend_ref(br: &HTTPBackendRef, route_ns: &str) -> Self {
        let namespace = br.namespace.clone().unwrap_or_else(|| route_ns.to_string());
        Self::new(br.name.clone(), namespace)
    }

    /// Build from a `GRPCBackendRef` with namespace inheritance from the route.
    pub fn from_grpc_backend_ref(br: &GRPCBackendRef, route_ns: &str) -> Self {
        let namespace = br.namespace.clone().unwrap_or_else(|| route_ns.to_string());
        Self::new(br.name.clone(), namespace)
    }

    /// Build from a `TCPBackendRef` with namespace inheritance from the route.
    pub fn from_tcp_backend_ref(br: &TCPBackendRef, route_ns: &str) -> Self {
        let namespace = br.namespace.clone().unwrap_or_else(|| route_ns.to_string());
        Self::new(br.name.clone(), namespace)
    }

    /// Build from a `UDPBackendRef` with namespace inheritance from the route.
    pub fn from_udp_backend_ref(br: &UDPBackendRef, route_ns: &str) -> Self {
        let namespace = br.namespace.clone().unwrap_or_else(|| route_ns.to_string());
        Self::new(br.name.clone(), namespace)
    }

    /// Build from a `TLSBackendRef` with namespace inheritance from the route.
    pub fn from_tls_backend_ref(br: &TLSBackendRef, route_ns: &str) -> Self {
        let namespace = br.namespace.clone().unwrap_or_else(|| route_ns.to_string());
        Self::new(br.name.clone(), namespace)
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The canonical `"{namespace}/{name}"` key consumed by service-scoped stores.
    #[inline]
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl From<&ServiceIdentity> for Rsc {
    fn from(id: &ServiceIdentity) -> Rsc {
        Rsc {
            kind: "Service".to_string(),
            ns: id.namespace.clone(),
            name: id.name.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::core::gateway::plugins::runtime::PluginRuntime;
    use crate::types::resources::grpc_route::GRPCBackendRef;
    use crate::types::resources::http_route::HTTPBackendRef;
    use crate::types::resources::http_route_preparse::BackendExtensionInfo;

    fn http_br(name: &str, namespace: Option<&str>) -> HTTPBackendRef {
        HTTPBackendRef {
            name: name.to_string(),
            namespace: namespace.map(|s| s.to_string()),
            port: None,
            weight: None,
            group: None,
            kind: None,
            filters: None,
            extension_info: BackendExtensionInfo::default(),
            plugin_runtime: Arc::new(PluginRuntime::new()),
            backend_tls_policy: None,
            ref_denied: None,
        }
    }

    fn grpc_br(name: &str, namespace: Option<&str>) -> GRPCBackendRef {
        GRPCBackendRef {
            name: name.to_string(),
            namespace: namespace.map(|s| s.to_string()),
            port: None,
            weight: None,
            group: None,
            kind: None,
            filters: None,
            extension_info: BackendExtensionInfo::default(),
            plugin_runtime: Arc::new(PluginRuntime::new()),
            backend_tls_policy: None,
            ref_denied: None,
        }
    }

    #[test]
    fn from_http_uses_route_ns_when_backend_ns_absent() {
        let br = http_br("svc-a", None);
        let id = ServiceIdentity::from_http_backend_ref(&br, "team-x");
        assert_eq!(id.name(), "svc-a");
        assert_eq!(id.namespace(), "team-x");
        assert_eq!(id.key(), "team-x/svc-a");
    }

    #[test]
    fn from_http_keeps_backend_ns_when_present() {
        let br = http_br("svc-a", Some("explicit-ns"));
        let id = ServiceIdentity::from_http_backend_ref(&br, "team-x");
        assert_eq!(id.namespace(), "explicit-ns");
        assert_eq!(id.key(), "explicit-ns/svc-a");
    }

    #[test]
    fn from_grpc_uses_route_ns_when_backend_ns_absent() {
        let br = grpc_br("svc-g", None);
        let id = ServiceIdentity::from_grpc_backend_ref(&br, "team-y");
        assert_eq!(id.name(), "svc-g");
        assert_eq!(id.namespace(), "team-y");
        assert_eq!(id.key(), "team-y/svc-g");
    }

    #[test]
    fn from_grpc_keeps_backend_ns_when_present() {
        let br = grpc_br("svc-g", Some("explicit-ns"));
        let id = ServiceIdentity::from_grpc_backend_ref(&br, "team-y");
        assert_eq!(id.namespace(), "explicit-ns");
    }

    #[test]
    fn key_format() {
        let id = ServiceIdentity::new("name", "ns");
        assert_eq!(id.key(), "ns/name");
    }

    #[test]
    fn rsc_conversion() {
        let id = ServiceIdentity::new("svc", "default");
        let rsc: Rsc = (&id).into();
        assert_eq!(rsc.kind, "Service");
        assert_eq!(rsc.ns, "default");
        assert_eq!(rsc.name, "svc");
    }

    #[test]
    fn equality_and_hash() {
        use std::collections::HashSet;
        let a = ServiceIdentity::new("svc", "default");
        let b = ServiceIdentity::new("svc", "default");
        let c = ServiceIdentity::new("svc", "other");
        assert_eq!(a, b);
        assert_ne!(a, c);

        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
        assert!(!set.contains(&c));
    }
}
