/// Backend service kind — pure classifier, no runtime state.
///
/// Matches a `kind` string from a BackendRef into one of the known
/// service variants. Gateway-side code re-exports this from types so
/// all consumers share a single definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceBackendKind {
    /// Standard Kubernetes Service (default behavior, uses EndpointSlice)
    Service,
    /// Service with ClusterIP (direct connection, no LB)
    ServiceClusterIp,
    /// ServiceImport for multi-cluster
    ServiceImport,
    /// Service with ExternalName
    ServiceExternalName,
    /// Force use EndpointSlice resource for backend discovery
    ServiceEndpointSlice,
    /// Unsupported backend kind (Gateway API conformance: must return 500)
    Unsupported(String),
}

impl ServiceBackendKind {
    /// Parse service type from kind string
    pub fn from_kind(kind: Option<&String>) -> Self {
        match kind {
            None => ServiceBackendKind::Service,
            Some(k) if k.is_empty() => ServiceBackendKind::Service,
            Some(k) => match k.as_str() {
                "Service" => ServiceBackendKind::Service,
                "ServiceClusterIp" => ServiceBackendKind::ServiceClusterIp,
                "ServiceImport" => ServiceBackendKind::ServiceImport,
                "ServiceExternalName" => ServiceBackendKind::ServiceExternalName,
                "ServiceEndpointSlice" | "EndpointSlice" => ServiceBackendKind::ServiceEndpointSlice,
                _ => ServiceBackendKind::Unsupported(k.clone()),
            },
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/service_backend_kind_test.rs"]
mod tests;
